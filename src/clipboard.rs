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
        let mut child = Command::new(cmd)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("无法启动 {cmd}: {e}"))?;
        let stdin = child.stdin.take().ok_or("无法打开输入管道")?;
        let mut buf = stdin;
        buf.write_all(text.as_bytes())
            .map_err(|e| format!("写入剪贴板失败: {e}"))?;
        let _ = buf.flush();
        drop(buf); // EOF → 数据交给剪贴板进程
        Ok(())
    }
}