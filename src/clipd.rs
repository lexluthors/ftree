//! X11 剪贴板守护进程（仅 Linux/X11，由 `ftree --clip-daemon` 隐藏入口启动）。
//!
//! ## 为什么需要它
//!
//! X11 剪贴板是「目标协商」机制：客户端先读 `TARGETS` 列表，再按自己认识的格式取数据。
//! 而 `xclip` 一个进程只能持有**一个** target：
//!
//! | 写入格式 | Nautilus | Dolphin / QQ / 微信（Qt/Chromium） |
//! |---|---|---|
//! | `x-special/gnome-copied-files` | ✅ | ❌ TARGETS 里没有 uri-list，粘贴无反应 |
//! | `text/uri-list` | ❌ GNOME 私有约定不认 | ✅ |
//!
//! 连开两个 xclip 也没用 —— 后者会抢走 CLIPBOARD 所有权，前者收到 SelectionClear 直接退出。
//! 所以必须由一个进程同时提供多种格式，本模块就是这个进程。
//!
//! ## 进程模型
//!
//! `ftree --clip-daemon` 从 stdin 读 payload → 双 fork 脱离控制终端 → 拥有 CLIPBOARD →
//! 阻塞响应 `SelectionRequest`，直到被其他程序抢占（`SelectionClear`）或 X 连接断开。
//! 与 xclip 的后台模式行为一致：**ftree 退出后剪贴板内容依然可用**。
//!
//! ## 提供的格式
//!
//! - `text/uri-list`：百分号编码 URI，`\r\n` 分隔 → QQ / 微信 / Dolphin / 浏览器
//! - `x-special/gnome-copied-files`：首行 `copy`，随后 `\n` 分隔的 URI → Nautilus / Thunar / Nemo
//! - `TARGETS` / `TIMESTAMP`：协议要求
//!
//! 刻意**不提供** `UTF8_STRING` 等纯文本格式：否则聊天软件可能把路径当普通文本发出去，
//! 而不是当作文件附件。需要粘路径请用 ftree 的 `c`（复制路径）功能。

use std::io::Read;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateWindowAux, EventMask, PropMode, SelectionNotifyEvent,
    SelectionRequestEvent, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::clipboard::parse_payload;

/// 剪贴板数据：各 target 的字节内容在启动时一次性构造好，之后只做只读响应。
struct ClipData {
    /// `text/uri-list` 内容
    uri_list: Vec<u8>,
    /// `x-special/gnome-copied-files` 内容
    gnome: Vec<u8>,
}

/// 守护进程入口。返回进程退出码（由 main 直接 `process::exit`）。
pub fn run() -> i32 {
    // 必须在任何线程启动前执行（main 的最前面），fork 才是安全的
    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return 1;
    }
    let (mode, uris) = parse_payload(&payload);
    if uris.is_empty() {
        return 1;
    }

    let data = ClipData {
        // text/uri-list 标准：\r\n 分隔，结尾带一个换行
        uri_list: format!("{}\r\n", uris.join("\r\n")).into_bytes(),
        // GNOME 格式：首行操作类型，\n 分隔
        gnome: format!("{mode}\n{}\n", uris.join("\n")).into_bytes(),
    };

    // 双 fork 脱离控制终端：ftree 退出后剪贴板内容依然有效，
    // 且直接子进程立即退出、由 init 收养，不会在 ftree 下长期堆积。
    unsafe { daemonize() };

    match serve(data) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// 双 fork + setsid 脱离终端（此时进程内只有主线程，fork 安全）。
///
/// 直接子进程（ftree spawn 的那个）会在 fork 后**立即退出**，因此 ftree 可以
/// `wait()` 回收它、不产生僵尸进程；真正的守护进程是孙进程，由 init/systemd 收养。
/// 退出码约定：0 = 已脱离并接管剪贴板；2 = fork 失败（调用方回退到 xclip）。
unsafe fn daemonize() {
    match libc::fork() {
        -1 => std::process::exit(2), // fork 失败：让 ftree 回退到 xclip
        0 => {}                      // 子进程继续
        _ => std::process::exit(0),  // 父进程立即退出，子进程交给 init 收养
    }
    libc::setsid();
    match libc::fork() {
        // 第二次 fork：确保永远不会重新获得控制终端。
        // 失败不致命 —— 当前进程直接当守护进程用（只是没完全脱离）。
        -1 | 0 => {}
        _ => std::process::exit(0),
    }
    // payload 已读完，关闭继承自 ftree 的管道
    libc::close(0);
}

/// 拥有 CLIPBOARD 并响应所有取数请求，直到被抢占或连接断开。
fn serve(data: ClipData) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];

    // InputOnly 窗口：仅用作 selection owner，不显示任何内容
    let win = conn.generate_id()?;
    conn.create_window(
        0,
        win,
        screen.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )?;

    let atoms = Atoms {
        targets: intern(&conn, b"TARGETS")?,
        timestamp: intern(&conn, b"TIMESTAMP")?,
        atom_type: intern(&conn, b"ATOM")?,
        integer_type: intern(&conn, b"INTEGER")?,
        uri_list: intern(&conn, b"text/uri-list")?,
        gnome: intern(&conn, b"x-special/gnome-copied-files")?,
    };
    let clipboard = intern(&conn, b"CLIPBOARD")?;

    conn.set_selection_owner(win, clipboard, CURRENT_TIME)?;
    conn.flush()?;

    // 确认真的拿到了所有权（被别的剪贴板管理器拦截时会失败）
    if conn.get_selection_owner(clipboard)?.reply()?.owner != win {
        return Err("未取得 CLIPBOARD 所有权".into());
    }

    loop {
        match conn.wait_for_event()? {
            XEvent::SelectionRequest(req) => answer(&conn, &atoms, &data, req)?,
            // 其他程序接管了剪贴板 → 使命结束。
            // 但必须先把队列里已收到的请求答完：同一连接上 SelectionRequest 一定排在
            // SelectionClear 之前，若直接退出，请求方（xclip -o / 聊天软件）会永远
            // 等不到 SelectionNotify 而挂死（X 协议不会替死掉的 owner 补发通知）。
            XEvent::SelectionClear(_) => {
                loop {
                    match conn.poll_for_event()? {
                        Some(XEvent::SelectionRequest(req)) => answer(&conn, &atoms, &data, req)?,
                        Some(_) => continue, // 其他事件（如属性通知）忽略，继续排空
                        None => break,
                    }
                }
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

/// 本次持有的各 target/类型 atom
struct Atoms {
    targets: u32,
    timestamp: u32,
    atom_type: u32,
    integer_type: u32,
    uri_list: u32,
    gnome: u32,
}

/// 响应一次取数请求：写入请求方的属性并回送 SelectionNotify。
fn answer<C: Connection>(
    conn: &C,
    atoms: &Atoms,
    data: &ClipData,
    req: SelectionRequestEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    // ICCCM：property 为 None 时用 target 作为属性名
    let prop = if req.property == NONE { req.target } else { req.property };

    // (类型 atom, 格式位宽, 数据)；None = 拒绝该 target
    let response: Option<(u32, u8, Vec<u8>)> = if req.target == atoms.targets {
        let mut bytes = Vec::with_capacity(4 * 4);
        for atom in [atoms.targets, atoms.timestamp, atoms.uri_list, atoms.gnome] {
            bytes.extend_from_slice(&atom.to_ne_bytes());
        }
        Some((atoms.atom_type, 32, bytes))
    } else if req.target == atoms.timestamp {
        Some((atoms.integer_type, 32, req.time.to_ne_bytes().to_vec()))
    } else if req.target == atoms.uri_list {
        Some((atoms.uri_list, 8, data.uri_list.clone()))
    } else if req.target == atoms.gnome {
        Some((atoms.gnome, 8, data.gnome.clone()))
    } else {
        None
    };

    // 注：数据量很小时才走这条路径（几十个文件 < 4KB），
    // 超过 X 请求上限的场景（INCR 增量传输）暂不支持。
    let reply_prop = match response {
        Some((typ, format, bytes)) => {
            let unit = usize::from(format) / 8;
            let data_len = (bytes.len() / unit.max(1)) as u32;
            conn.change_property(
                PropMode::REPLACE,
                req.requestor,
                prop,
                typ,
                format,
                data_len,
                &bytes,
            )?;
            prop
        }
        None => NONE, // 拒绝：property=None
    };

    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: reply_prop,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
    conn.flush()?;
    Ok(())
}

fn intern<C: Connection>(conn: &C, name: &[u8]) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(conn.intern_atom(false, name)?.reply()?.atom)
}
