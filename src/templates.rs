use std::path::PathBuf;

use crate::config::Template;

/// 占位符：
///   {files}        完整路径，空格分隔
///   {files_quoted} 带单引号包裹的完整路径（防空格/特殊字符注入）
///   {names}        仅文件名，空格分隔
///   {dir}          所有选中项的最长公共父目录（单文件 = 其所在目录）
///   {n}            文件数量
pub fn render(t: &Template, selected: &[PathBuf]) -> String {
    let files: Vec<String> = selected
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let quoted: Vec<String> = files.iter().map(|f| quote(f)).collect();
    let names: Vec<String> = selected
        .iter()
        .filter_map(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    let dir = common_parent(selected)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut out = t.command.clone();
    out = out.replace("{files}", &files.join(" "));
    out = out.replace("{files_quoted}", &quoted.join(" "));
    out = out.replace("{names}", &names.join(" "));
    out = out.replace("{dir}", &dir);
    out = out.replace("{n}", &selected.len().to_string());
    out
}

/// 单引号包裹 + 内部单引号转义（POSIX shell 合法）。
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub fn common_parent(paths: &[PathBuf]) -> Option<PathBuf> {
    if paths.is_empty() {
        return None;
    }
    let groups: Vec<Vec<_>> = paths
        .iter()
        .map(|p| {
            let mut cs: Vec<_> = p
                .components()
                .map(|c| c.as_os_str().to_os_string())
                .collect();
            if !p.is_dir() {
                cs.pop(); // 文件取父目录
            }
            cs
        })
        .collect();
    let first = &groups[0];
    let mut prefix: Vec<std::ffi::OsString> = Vec::new();
    for (i, c) in first.iter().enumerate() {
        if groups.iter().all(|g| g.get(i) == Some(c)) {
            prefix.push(c.clone());
        } else {
            break;
        }
    }
    if prefix.is_empty() {
        return None;
    }
    let mut pb = PathBuf::new();
    for c in prefix {
        pb.push(c);
    }
    Some(pb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn fixture_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("ftree-test-render-{id}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.mp4"), b"").unwrap();
        std::fs::write(d.join("b.mp4"), b"").unwrap();
        d
    }

    #[test]
    fn common_parent_same_dir() {
        let d = fixture_dir();
        let paths = vec![d.join("a.mp4"), d.join("b.mp4")];
        assert_eq!(common_parent(&paths).unwrap(), d);
    }

    #[test]
    fn common_parent_nested() {
        let base = std::env::temp_dir().join("ftree-test-nested");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("x")).unwrap();
        std::fs::create_dir_all(base.join("y")).unwrap();
        let paths = vec![base.join("x/1.txt"), base.join("y/2.txt")];
        assert_eq!(common_parent(&paths).unwrap(), base);
    }

    #[test]
    fn common_parent_single_file() {
        let d = fixture_dir();
        assert_eq!(common_parent(&[d.join("a.mp4")]).unwrap(), d);
    }

    #[test]
    fn render_replaces_placeholders() {
        let d = fixture_dir();
        let t = Template {
            name: "t".into(),
            command: "cat {files} > {dir}/out-{n}.txt {names} {files_quoted}".into(),
        };
        let out = render(&t, &[d.join("a.mp4"), d.join("b.mp4")]);
        let a = d.join("a.mp4").to_string_lossy().into_owned();
        let b = d.join("b.mp4").to_string_lossy().into_owned();
        let ds = d.to_string_lossy().into_owned();
        assert!(out.contains(&a));
        assert!(out.contains(&b));
        assert!(out.contains(&format!("{ds}/out-2.txt")));
        assert!(out.contains("a.mp4 b.mp4"));
        assert!(out.contains(&format!("'{a}' '{b}'")));
    }

    #[test]
    fn quote_escapes_quote() {
        assert_eq!(quote("/a b/c'd.mp4"), "'/a b/c'\\''d.mp4'");
    }
}