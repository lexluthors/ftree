#[cfg(not(target_os = "macos"))]
use std::env;
use std::io::Write;
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

    /// 复制文件 URI 列表到剪贴板（text/uri-list 格式）。
    /// 用于粘贴到文件管理器（Nautilus/Dolphin）或聊天工具（微信/QQ）。
    /// uri_list 格式：每行一个 file:// URI，使用 \r\n 分隔（标准格式）。
    ///
    /// 兼容性说明：
    /// - GNOME 文件管理器（Nautilus）使用 x-special/gnome-copied-files 格式
    /// - KDE 文件管理器（Dolphin）和聊天工具使用 text/uri-list 格式
    /// - 这里优先使用 GNOME 格式，因为大多数现代文件管理器都支持
    pub fn set_uri_list(&self, uri_list: &str) -> Result<(), String> {
        match &self.kind {
            ClipKind::X11 => {
                // GNOME 格式：第一行是操作类型（copy/cut），后面是 URI 列表
                // 注意：GNOME 格式使用 \n 分隔，不是 \r\n
                // 需要将输入的 \r\n 转换为 \n
                let normalized_uris = uri_list.replace("\r\n", "\n");
                let gnome_format = format!("copy\n{}", normalized_uris);
                self.run_clipboard_cmd(
                    "xclip",
                    vec!["-selection", "clipboard", "-t", "x-special/gnome-copied-files"],
                    &gnome_format,
                )
            }
            ClipKind::Wayland => {
                // Wayland: wl-copy 支持 --type 参数
                // 使用标准 text/uri-list 格式（Wayland 文件管理器通常支持）
                self.run_clipboard_cmd("wl-copy", vec!["--type", "text/uri-list"], uri_list)
            }
            #[cfg(target_os = "macos")]
            ClipKind::MacOS => {
                // macOS: pbcopy 不支持 text/uri-list，使用 osascript 设置 POSIX file
                self.set_files_macos(uri_list)
            }
            ClipKind::Unavailable(msg) => Err(msg.to_string()),
        }
    }

    /// 从剪贴板读取文件 URI 列表。
    /// 返回 Vec<String>，每个元素是 file:// URI。
    pub fn get_uri_list(&self) -> Result<Vec<String>, String> {
        let (cmd, args): (&str, Vec<&str>) = match &self.kind {
            ClipKind::X11 => {
                // 尝试读取 GNOME 格式（x-special/gnome-copied-files）
                // 格式：第一行是 "copy" 或 "cut"，后面是 URI 列表
                ("xclip", vec!["-selection", "clipboard", "-t", "x-special/gnome-copied-files", "-o"])
            }
            ClipKind::Wayland => {
                ("wl-paste", vec!["--type", "text/uri-list"])
            }
            #[cfg(target_os = "macos")]
            ClipKind::MacOS => return self.get_files_macos(),
            ClipKind::Unavailable(msg) => return Err(msg.to_string()),
        };

        let output = Command::new(cmd)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("无法读取剪贴板: {e}"))?;

        if !output.status.success() {
            return Err("剪贴板读取失败".to_string());
        }

        let content = String::from_utf8_lossy(&output.stdout);

        // 解析 URI 列表
        let uris: Vec<String> = content
            .lines()
            .filter(|line| {
                let line = line.trim();
                // 跳过空行和操作类型行（copy/cut）
                !line.is_empty() && line != "copy" && line != "cut" && line.starts_with("file://")
            })
            .map(|line| line.trim().to_string())
            .collect();

        Ok(uris)
    }

    /// 内部方法：运行剪贴板命令并写入数据
    fn run_clipboard_cmd(&self, cmd: &str, args: Vec<&str>, data: &str) -> Result<(), String> {
        let mut child = Command::new(cmd)
            .args(&args)
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
    fn set_files_macos(&self, uri_list: &str) -> Result<(), String> {
        // 解析 file:// URI，提取路径
        let paths: Vec<&str> = uri_list
            .lines()
            .filter(|l| l.starts_with("file://"))
            .map(|l| &l[7..]) // 去掉 "file://" 前缀
            .collect();

        if paths.is_empty() {
            return Err("没有有效的文件路径".to_string());
        }

        // 构建 AppleScript
        // 注意：macOS 剪贴板设置多个文件需要使用正确的语法
        let script = if paths.len() == 1 {
            // 单个文件
            format!(
                "set the clipboard to (POSIX file \"{}\")",
                paths[0].replace('\\', "\\\\").replace('"', "\\\"")
            )
        } else {
            // 多个文件：使用列表语法
            let file_refs: Vec<String> = paths
                .iter()
                .map(|p| format!("(POSIX file \"{}\")", p.replace('\\', "\\\\").replace('"', "\\\"")))
                .collect();
            format!("set the clipboard to {{{}}}", file_refs.join(", "))
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
    fn get_files_macos(&self) -> Result<Vec<String>, String> {
        // 使用 osascript 读取剪贴板中的文件
        // 注意：需要处理单个文件和多个文件的情况
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
        let uris: Vec<String> = content
            .lines()
            .filter(|line| line.starts_with("file://"))
            .map(|line| line.trim().to_string())
            .collect();

        if uris.is_empty() {
            return Err("剪贴板中没有文件".to_string());
        }

        Ok(uris)
    }
}