#[cfg(not(target_os = "macos"))]
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[allow(dead_code)]
enum ClipKind {
    X11,
    Wayland,
    #[cfg(target_os = "macos")]
    MacOS,
    Unavailable(&'static str),
}

pub struct Clipboard {
    kind: ClipKind,
}

impl Clipboard {
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            return Clipboard {
                kind: ClipKind::MacOS,
            };
        }
        #[cfg(not(target_os = "macos"))]
        {
            if env::var("WAYLAND_DISPLAY").is_ok()
                || env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
            {
                return Clipboard {
                    kind: ClipKind::Wayland,
                };
            }
            if env::var("DISPLAY").is_ok() {
                return Clipboard {
                    kind: ClipKind::X11,
                };
            }
            Clipboard {
                kind: ClipKind::Unavailable("未检测到 X11 或 Wayland 显示服务"),
            }
        }
    }

    /// 写入剪贴板。子进程保持运行以持有剪贴板所有权（xclip/wl-copy 的标准用法），
    /// 由操作系统回收，不阻塞 TUI。
    pub fn set(&self, text: &str) -> Result<(), String> {
        let (cmd, args): (&str, Vec<&str>) = match &self.kind {
            ClipKind::X11 => ("xclip", vec!["-selection", "clipboard"]),
            ClipKind::Wayland => ("wl-copy", vec![]),
            #[cfg(target_os = "macos")]
            ClipKind::MacOS => ("pbcopy", vec![]),
            ClipKind::Unavailable(msg) => return Err(msg.to_string()),
        };
        self.run_clipboard_cmd(cmd, args, text)
    }

    /// 复制文件/目录到系统剪贴板，可同时粘贴到文件管理器与聊天软件。
    ///
    /// 各平台写入的格式：
    /// - **X11**：由 `ftree --clip-daemon` 守护进程同时提供
    ///   `text/uri-list`（QQ/微信/Dolphin/浏览器）与 `x-special/gnome-copied-files`
    ///   （Nautilus/Thunar/Nemo）。
    /// - **Wayland**：`wl-copy --type text/uri-list`（wl-copy 只支持单一 MIME 类型）。
    /// - **macOS**：`osascript` 写入 `POSIX file`（Finder/微信 mac 可识别）。
    pub fn set_files(&self, paths: &[PathBuf]) -> Result<(), String> {
        if paths.is_empty() {
            return Err("没有可复制的文件".to_string());
        }
        // file:// URI 必须做百分号编码，否则含空格/中文/#/& 的文件名
        // 在 Chromium(GURL)、Qt(QUrl)、GIO 的严格解析下会被截断。
        let uris: Vec<String> = paths.iter().map(|p| path_to_uri(p)).collect();

        match &self.kind {
            ClipKind::X11 => self.set_files_x11(&uris),
            ClipKind::Wayland => {
                // text/uri-list 标准：每行一个 URI，\r\n 分隔
                let uri_list = uris.join("\r\n");
                self.run_clipboard_cmd("wl-copy", vec!["--type", "text/uri-list"], &uri_list)
            }
            #[cfg(target_os = "macos")]
            ClipKind::MacOS => self.set_files_macos(paths),
            ClipKind::Unavailable(msg) => Err(msg.to_string()),
        }
    }

    /// X11：写入多格式剪贴板。
    ///
    /// xclip 一个进程只能持有一个 target，写 `x-special/gnome-copied-files` 时
    /// QQ/微信(Chromium) 看不到 `text/uri-list`（TARGETS 里没有）→ 粘贴无反应；
    /// 反过来只写 `text/uri-list` 时 Nautilus 又粘不了。
    /// 因此优先启动 ftree 自带的守护进程（`src/clipd.rs`）同时提供两种格式。
    fn set_files_x11(&self, uris: &[String]) -> Result<(), String> {
        if let Some(exe) = daemon_exe() {
            let payload = build_payload("copy", uris);
            let spawned = Command::new(&exe)
                .arg("--clip-daemon")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = spawned {
                let mut ok = false;
                if let Some(mut stdin) = child.stdin.take() {
                    if stdin.write_all(payload.as_bytes()).is_ok() {
                        let _ = stdin.flush();
                        drop(stdin); // EOF → 守护进程读完 payload 后双 fork 脱离
                        // 直接子进程双 fork 后立刻退出，wait 只是回收它（毫秒级），
                        // 避免像 xclip 那样在 ftree 下堆积僵尸进程。
                        // 真正的守护进程是孙进程，已交给 init/systemd，ftree 退出后依然有效。
                        ok = matches!(child.wait(), Ok(status) if status.success());
                    }
                }
                if ok {
                    return Ok(());
                }
                // fork 失败（退出码 2）或写入失败 → 回退到 xclip
            }
        }

        // 回退方案：xclip 只能提供单一格式，选兼容性最广的 text/uri-list
        // （聊天软件/KDE/浏览器可用，代价是 Nautilus 粘贴失效）
        let uri_list = uris.join("\r\n");
        self.run_clipboard_cmd(
            "xclip",
            vec!["-selection", "clipboard", "-t", "text/uri-list"],
            &uri_list,
        )
    }

    /// 从剪贴板读取文件列表，返回已解码的本地路径。
    ///
    /// 兼容来源：ftree 自己、Nautilus、Dolphin、浏览器、QQ/微信复制的文件。
    pub fn get_files(&self) -> Result<Vec<PathBuf>, String> {
        match &self.kind {
            ClipKind::X11 => {
                // 依次尝试 GNOME 私有格式与标准 text/uri-list。
                // 注意 xclip -o 在目标未声明时往往仍会返回所有者给出的数据，
                // 因此两种都试即可覆盖各种来源。
                let gnome = self.read_x11_target("x-special/gnome-copied-files");
                if !gnome.is_empty() {
                    return Ok(gnome);
                }
                let uri_list = self.read_x11_target("text/uri-list");
                if !uri_list.is_empty() {
                    return Ok(uri_list);
                }
                Err("剪贴板中没有文件".to_string())
            }
            ClipKind::Wayland => {
                for args in [
                    vec!["--type", "text/uri-list"],
                    vec!["--type", "x-special/gnome-copied-files"],
                    vec![],
                ] {
                    let paths = self.read_wl_paste(&args);
                    if !paths.is_empty() {
                        return Ok(paths);
                    }
                }
                Err("剪贴板中没有文件".to_string())
            }
            #[cfg(target_os = "macos")]
            ClipKind::MacOS => self.get_files_macos(),
            ClipKind::Unavailable(msg) => Err(msg.to_string()),
        }
    }

    /// 用 xclip 读取指定 target 的内容并解析为路径（失败/超时返回空列表）。
    fn read_x11_target(&self, target: &str) -> Vec<PathBuf> {
        match read_cmd_with_timeout(
            "xclip",
            &["-selection", "clipboard", "-t", target, "-o"],
            CLIP_READ_TIMEOUT,
        ) {
            Some(out) => parse_uri_list(&out),
            None => Vec::new(),
        }
    }

    /// 用 wl-paste 读取剪贴板并解析为路径（失败/超时返回空列表）。
    fn read_wl_paste(&self, extra_args: &[&str]) -> Vec<PathBuf> {
        let mut args = extra_args.to_vec();
        args.push("--no-newline");
        match read_cmd_with_timeout("wl-paste", &args, CLIP_READ_TIMEOUT) {
            Some(out) => parse_uri_list(&out),
            None => Vec::new(),
        }
    }

    /// 内部方法：运行剪贴板命令并写入数据
    fn run_clipboard_cmd(&self, cmd: &str, args: Vec<&str>, data: &str) -> Result<(), String> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("无法启动 {cmd}: {e}"))?;
        let stdin = child.stdin.take().ok_or("无法打开输入管道")?;
        let mut buf = stdin;
        buf.write_all(data.as_bytes())
            .map_err(|e| format!("写入剪贴板失败: {e}"))?;
        let _ = buf.flush();
        drop(buf); // EOF → 数据交给剪贴板进程
        Ok(())
    }

    /// macOS 专用：使用 osascript 将文件设置到剪贴板
    #[cfg(target_os = "macos")]
    fn set_files_macos(&self, paths: &[PathBuf]) -> Result<(), String> {
        // AppleScript 需要原始路径（不做百分号编码）
        let escaped: Vec<String> = paths
            .iter()
            .map(|p| {
                p.to_string_lossy()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            })
            .collect();

        let script = if escaped.len() == 1 {
            format!("set the clipboard to (POSIX file \"{}\")", escaped[0])
        } else {
            let refs: Vec<String> = escaped
                .iter()
                .map(|p| format!("(POSIX file \"{p}\")"))
                .collect();
            format!("set the clipboard to {{{}}}", refs.join(", "))
        };

        Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("无法执行 osascript: {e}"))?;

        Ok(())
    }

    /// macOS 专用：从剪贴板读取文件列表
    #[cfg(target_os = "macos")]
    fn get_files_macos(&self) -> Result<Vec<PathBuf>, String> {
        // osascript 返回的 "file://" + POSIX path 是未编码的原始路径，直接剥离前缀即可
        let script = r#"
            try
                set theClipboard to the clipboard
                set output to ""

                -- 尝试作为文件列表读取
                if class of theClipboard is list then
                    repeat with anItem in theClipboard
                        if class of anItem is «class furl» then
                            set output to output & "file://" & (POSIX path of anItem) & linefeed
                        end if
                    end repeat
                else if class of theClipboard is «class furl» then
                    -- 单个文件
                    set output to "file://" & (POSIX path of theClipboard) & linefeed
                end if

                return output
            on error
                return ""
            end try
        "#;

        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("无法执行 osascript: {e}"))?;

        if !output.status.success() {
            return Err("剪贴板中没有文件".to_string());
        }

        let content = String::from_utf8_lossy(&output.stdout);
        let paths: Vec<PathBuf> = content
            .lines()
            .filter(|line| line.starts_with("file://"))
            .map(|line| PathBuf::from(line.trim_start_matches("file://")))
            .collect();

        if paths.is_empty() {
            return Err("剪贴板中没有文件".to_string());
        }

        Ok(paths)
    }
}

// ---------------------------------------------------------------------------
// 剪贴板守护进程 payload 协议（写入方：本文件；读取方：src/clipd.rs）
//   第 1 行：模式（copy / cut）
//   其余每行：一个已百分号编码的 file:// URI
// ---------------------------------------------------------------------------

/// 读取剪贴板的超时上限。
const CLIP_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// 剪贴板守护进程的可执行文件（就是 ftree 自身，走隐藏入口 `--clip-daemon`）。
///
/// `FTREE_CLIP_DAEMON_EXE` 可覆盖：单元测试里 `current_exe()` 指向的是测试二进制
/// （不认识 `--clip-daemon`），需要显式指向真正的 ftree 才能测到守护进程路径。
fn daemon_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FTREE_CLIP_DAEMON_EXE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    std::env::current_exe().ok()
}

/// 运行命令并读取其 stdout，最多等待 `timeout`；超时返回 None 并杀掉子进程。
///
/// **为什么必须带超时**：剪贴板所有权交接的瞬间（旧 owner 已退出、新 owner 还没接管），
/// `xclip -o` / `wl-paste` 会永久阻塞在等不到的 SelectionNotify 上 —— 这是 X11 的已知问题。
/// 直接用 `.output()` 会让「粘贴文件」把整个 TUI 冻死，因此统一走这里兜底。
pub(crate) fn read_cmd_with_timeout(
    cmd: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Option<String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    // 读取放到独立线程：管道读满/阻塞时主线程仍能超时返回
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(timeout) {
        Ok(out) => {
            let _ = child.wait();
            Some(out)
        }
        Err(_) => {
            let _ = child.kill(); // 管道随之关闭，读取线程也会退出
            let _ = child.wait(); // 回收，避免僵尸进程
            None
        }
    }
}

/// 构造守护进程 payload。
pub(crate) fn build_payload(mode: &str, uris: &[String]) -> String {
    let mut out = String::with_capacity(mode.len() + uris.iter().map(|u| u.len() + 1).sum::<usize>());
    out.push_str(mode);
    out.push('\n');
    for u in uris {
        out.push_str(u);
        out.push('\n');
    }
    out
}

/// 解析守护进程 payload，返回 (模式, URI 列表)。非法模式一律按 copy 处理。
#[allow(dead_code)] // 仅 Linux 的 clipd 使用；测试中也会用到
pub(crate) fn parse_payload(payload: &str) -> (String, Vec<String>) {
    let mut lines = payload.lines().map(str::trim).filter(|l| !l.is_empty());
    let mode = match lines.next() {
        Some("cut") => "cut".to_string(),
        _ => "copy".to_string(),
    };
    let uris = lines
        .filter(|l| l.starts_with("file://"))
        .map(str::to_string)
        .collect();
    (mode, uris)
}

// ---------------------------------------------------------------------------
// file:// URI 编解码（跨平台共用）
// ---------------------------------------------------------------------------

/// 本地路径 → `file://` URI（百分号编码，字节安全）。
///
/// 只保留 RFC 3986 的 unreserved 字符与 `/`，其余（空格、中文、`#`、`&`、`%` 等）
/// 一律编码为 `%XX`，保证 Chromium / Qt / GIO 都能正确解析。
pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for &b in path_bytes(path).iter() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `file://` URI → 本地路径（百分号解码）。
///
/// 只接受绝对路径；带主机名的 `file://host/path` 与远程 URL 一律返回 None。
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .or_else(|| uri.strip_prefix("file:"))?;
    let bytes = percent_decode(rest.as_bytes());
    if bytes.first() != Some(&b'/') {
        return None;
    }
    Some(bytes_to_path(bytes))
}

/// 解析剪贴板中的 URI 列表文本为路径列表。
///
/// 跳过空行、`#` 注释行（text/uri-list 规范）以及 GNOME 格式的首行 `copy`/`cut`。
pub fn parse_uri_list(content: &str) -> Vec<PathBuf> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && *l != "copy" && *l != "cut")
        .filter_map(uri_to_path)
        .collect()
}

/// `%XX` 解码为原始字节（非 UTF-8 文件名也能安全还原）。
fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(hi), Some(lo)) = (hex_val(input[i + 1]), hex_val(input[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 路径的原始字节表示（Unix 下文件名不要求是合法 UTF-8）。
#[cfg(unix)]
fn path_bytes(path: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

/// 原始字节 → PathBuf。
#[cfg(unix)]
fn bytes_to_path(bytes: Vec<u8>) -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    #[test]
    fn path_to_uri_encodes_special_chars() {
        let uri = path_to_uri(Path::new("/tmp/my file#1&2.png"));
        assert_eq!(uri, "file:///tmp/my%20file%231%262.png");
    }

    #[test]
    fn path_to_uri_keeps_unreserved_chars() {
        let uri = path_to_uri(Path::new("/home/lex/a-b_c.d~e/f"));
        assert_eq!(uri, "file:///home/lex/a-b_c.d~e/f");
    }

    #[test]
    fn uri_to_path_decodes_percent() {
        assert_eq!(
            uri_to_path("file:///tmp/my%20file%231%262.png").unwrap(),
            PathBuf::from("/tmp/my file#1&2.png")
        );
        assert_eq!(
            uri_to_path("file:///tmp/%E4%B8%AD%E6%96%87.txt").unwrap(),
            PathBuf::from("/tmp/中文.txt")
        );
        // 小写十六进制同样支持（浏览器常用）
        assert_eq!(
            uri_to_path("file:///tmp/a%20b").unwrap(),
            PathBuf::from("/tmp/a b")
        );
    }

    #[test]
    fn uri_roundtrip_preserves_tricky_names() {
        for name in [
            "/tmp/普通文件.txt",
            "/tmp/with space & #hash%.mp4",
            "/tmp/'quote'\"dq\".log",
            "/tmp/emoji 🎉.png",
            "/tmp/100%_done",
        ] {
            let p = PathBuf::from(name);
            assert_eq!(uri_to_path(&path_to_uri(&p)).as_deref(), Some(p.as_path()), "name={name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn uri_roundtrip_non_utf8_filename() {
        // 非法 UTF-8 文件名：必须按字节编解码，不能 lossy
        let raw = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff, 0xfe, b'.', b'b']);
        let p = PathBuf::from(raw.clone());
        let uri = path_to_uri(&p);
        assert!(uri.contains("%FF%FE"), "uri={uri}");
        assert_eq!(uri_to_path(&uri).unwrap().as_os_str().as_bytes(), p.as_os_str().as_bytes());
    }

    #[test]
    fn uri_to_path_rejects_remote_and_relative() {
        assert!(uri_to_path("https://example.com/a.png").is_none());
        assert!(uri_to_path("file://host/share/a.png").is_none());
        assert!(uri_to_path("file://relative/a.png").is_none());
        assert!(uri_to_path("/tmp/plain/path").is_none());
    }

    #[test]
    fn parse_uri_list_skips_header_comments_and_blank_lines() {
        let content = "copy\r\n# comment line\r\nfile:///tmp/a.txt\r\n\r\nfile:///tmp/b%20c.txt\r\n";
        assert_eq!(
            parse_uri_list(content),
            vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b c.txt")]
        );
    }

    #[test]
    fn parse_uri_list_handles_cut_mode_and_lf() {
        let content = "cut\nfile:///tmp/x\n";
        assert_eq!(parse_uri_list(content), vec![PathBuf::from("/tmp/x")]);
    }

    #[test]
    fn parse_uri_list_ignores_non_file_uris() {
        let content = "https://example.com\nfile:///tmp/ok\n";
        assert_eq!(parse_uri_list(content), vec![PathBuf::from("/tmp/ok")]);
    }

    #[test]
    fn payload_roundtrip() {
        let uris = vec![
            "file:///tmp/a.txt".to_string(),
            "file:///tmp/%E4%B8%AD%20b.png".to_string(),
        ];
        let payload = build_payload("copy", &uris);
        assert_eq!(payload, "copy\nfile:///tmp/a.txt\nfile:///tmp/%E4%B8%AD%20b.png\n");

        let (mode, parsed) = parse_payload(&payload);
        assert_eq!(mode, "copy");
        assert_eq!(parsed, uris);
    }

    #[test]
    fn payload_defaults_to_copy_on_bad_mode() {
        let (mode, uris) = parse_payload("weird\nfile:///tmp/a\n");
        assert_eq!(mode, "copy");
        assert_eq!(uris, vec!["file:///tmp/a".to_string()]);

        let (mode, uris) = parse_payload("cut\nfile:///tmp/a\n");
        assert_eq!(mode, "cut");
        assert_eq!(uris.len(), 1);

        // 空 payload 不应 panic
        let (mode, uris) = parse_payload("");
        assert_eq!(mode, "copy");
        assert!(uris.is_empty());
    }
}
