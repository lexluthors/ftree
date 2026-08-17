//! Git 操作模块：检测仓库、获取状态、SSH/token 检测、构建终端命令

use std::path::{Path, PathBuf};
use std::process::Command;

/// 检查目录是否是 git 仓库
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "rev-parse", "--git-dir"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 获取 git 仓库的默认分支名（master 或 main）
pub fn get_default_branch(dir: &Path) -> String {
    // 先尝试获取当前分支
    if let Ok(output) = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "branch", "--show-current"])
        .output()
    {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return branch;
        }
    }
    // 如果没有当前分支（空仓库），检查远程默认分支
    if let Ok(output) = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "remote", "show", "origin"])
        .output()
    {
        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            if line.contains("HEAD branch:") {
                if let Some(branch) = line.split(':').nth(1) {
                    let branch = branch.trim().to_string();
                    if !branch.is_empty() && branch != "(unknown)" {
                        return branch;
                    }
                }
            }
        }
    }
    // 默认使用 master
    "master".to_string()
}

/// Git 文件状态
#[derive(Debug, Clone)]
pub struct GitFileStatus {
    pub path: PathBuf,
    pub status: FileStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

impl GitFileStatus {
    pub fn is_tracked(&self) -> bool {
        self.status != FileStatus::Untracked
    }

    pub fn status_symbol(&self) -> &str {
        match self.status {
            FileStatus::Modified => "M",
            FileStatus::Added => "A",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Untracked => "?",
        }
    }
}

/// 获取 git status --porcelain 的文件列表
pub fn get_git_status(dir: &Path) -> Vec<GitFileStatus> {
    let mut files = Vec::new();

    let output = match Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "status", "--porcelain"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return files,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status_chars = &line[0..2];
        let path = line[3..].to_string();

        let status = match status_chars {
            " M" | "M " => FileStatus::Modified,
            "A " | " A" => FileStatus::Added,
            "D " | " D" => FileStatus::Deleted,
            "R " => FileStatus::Renamed,
            "??" => FileStatus::Untracked,
            _ => continue,
        };

        files.push(GitFileStatus {
            path: PathBuf::from(path),
            status,
        });
    }

    files
}

/// 检测 SSH 密钥是否存在
pub fn has_ssh_key() -> bool {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return false,
    };

    let ssh_dir = PathBuf::from(&home).join(".ssh");
    if !ssh_dir.exists() {
        return false;
    }

    // 检查常见的 SSH 密钥文件
    let key_files = ["id_ed25519", "id_rsa", "id_ecdsa"];
    for key in &key_files {
        if ssh_dir.join(key).exists() {
            return true;
        }
    }

    false
}

/// 检测 git credential helper 是否配置
pub fn has_git_credential_helper() -> bool {
    Command::new("git")
        .args(["config", "--get", "credential.helper"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 检查是否有未提交的更改
pub fn has_uncommitted_changes(dir: &Path) -> bool {
    !get_git_status(dir).is_empty()
}

/// 检查是否有未 push 的 commit
pub fn has_unpushed_commits(dir: &Path) -> bool {
    let output = match Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "status", "-sb"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return false,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    // 如果输出包含 "ahead" 则表示有未 push 的 commit
    stdout.contains("ahead")
}

/// 获取 remote origin 的 URL
pub fn get_remote_url(dir: &Path) -> Option<String> {
    Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ftree-git-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_is_git_repo_false_for_new_dir() {
        let d = temp_dir();
        assert!(!is_git_repo(&d));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn test_is_git_repo_true_after_init() {
        let d = temp_dir();
        Command::new("git")
            .args(["-C", &d.to_string_lossy(), "init"])
            .status()
            .unwrap();
        assert!(is_git_repo(&d));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn test_get_git_status_empty() {
        let d = temp_dir();
        Command::new("git")
            .args(["-C", &d.to_string_lossy(), "init"])
            .status()
            .unwrap();
        let status = get_git_status(&d);
        assert!(status.is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn test_get_git_status_untracked() {
        let d = temp_dir();
        Command::new("git")
            .args(["-C", &d.to_string_lossy(), "init"])
            .status()
            .unwrap();
        fs::write(d.join("test.txt"), "hello").unwrap();
        let status = get_git_status(&d);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].status, FileStatus::Untracked);
        let _ = fs::remove_dir_all(&d);
    }
}
