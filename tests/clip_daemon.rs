//! 集成测试：`ftree --clip-daemon` 必须把多种格式**同时**放上 X11 剪贴板。
//!
//! 回归背景：以前只用 `xclip -t x-special/gnome-copied-files` 写入，
//! `TARGETS` 里没有 `text/uri-list`，导致 QQ/微信（Chromium）、Dolphin（Qt）
//! 认为剪贴板是空的 —— "右键-复制文件"粘不进聊天软件（Nautilus 却正常）。
//!
//! 注意：本测试会占用系统剪贴板，结束后清空以释放守护进程。

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 仅在 X11 会话下运行（Wayland 走 wl-copy，不启动守护进程）
fn x11_available() -> bool {
    std::env::var("DISPLAY").is_ok()
        && std::env::var("WAYLAND_DISPLAY").is_err()
        && Command::new("xclip")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
}

fn read_target(target: &str) -> String {
    let out = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", target, "-o"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

#[test]
fn clip_daemon_serves_uri_list_and_gnome_targets() {
    if !x11_available() {
        eprintln!("跳过：无 X11 显示服务或 xclip");
        return;
    }

    // 含空格 / # / & / 中文的路径，URI 必须已百分号编码
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("ftree-clipd-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("my file #1 & 中文.png");
    std::fs::write(&file, b"probe").unwrap();

    let uri = format!(
        "file://{dir}/my%20file%20%231%20%26%20%E4%B8%AD%E6%96%87.png",
        dir = dir.display()
    );

    // 与 clipboard.rs::build_payload 相同的协议：首行模式，其余每行一个 URI
    let payload = format!("copy\n{uri}\n");

    let mut child = Command::new(env!("CARGO_BIN_EXE_ftree"))
        .arg("--clip-daemon")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("无法启动 ftree --clip-daemon");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    // 直接子进程双 fork 后立即退出，回收它避免留下僵尸进程
    let status = child.wait().expect("等待守护进程派生失败");
    assert!(status.success(), "守护进程派生失败: {status}");

    // 守护进程需要一点时间连接 X 并取得所有权，轮询等待
    let deadline = Instant::now() + Duration::from_secs(5);
    let (targets, uri_list, gnome) = loop {
        let targets = read_target("TARGETS");
        let uri_list = read_target("text/uri-list");
        let gnome = read_target("x-special/gnome-copied-files");
        let ready = targets.contains("text/uri-list")
            && targets.contains("x-special/gnome-copied-files")
            && uri_list.contains(&uri)
            && gnome.starts_with("copy")
            && gnome.contains(&uri);
        if ready {
            break (targets, uri_list, gnome);
        }
        assert!(
            Instant::now() < deadline,
            "剪贴板守护进程未提供预期格式\nTARGETS={targets:?}\nuri-list={uri_list:?}\ngnome={gnome:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(!targets.is_empty(), "TARGETS 为空");

    // text/uri-list 用 \r\n 分隔（QQ/微信/Dolphin 依赖该格式）
    assert!(uri_list.contains(&uri), "text/uri-list 缺少 URI");
    // GNOME 格式首行是操作类型，换行分隔（Nautilus 依赖）
    assert_eq!(gnome.lines().next().unwrap(), "copy");
    assert_eq!(gnome.lines().nth(1).unwrap(), uri);

    // 清理：清空剪贴板 → 守护进程收到 SelectionClear 后自动退出
    let mut clear = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    clear.stdin.take().unwrap().write_all(b"").unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
