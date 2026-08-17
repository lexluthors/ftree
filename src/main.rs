mod app;
mod clipboard;
mod config;
mod git;
mod git_commit;
mod templates;
mod tree;
mod ui;

use std::env;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;

/// 无论正常退出还是 panic，都恢复终端状态。
struct TuiGuard;

impl Drop for TuiGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

fn print_usage() {
    println!("ftree — 终端交互式文件树工具");
    println!();
    println!("用法: ftree [选项] [目录]");
    println!();
    println!("选项:");
    println!("  --no-hidden    默认隐藏隐藏文件（默认显示）");
    println!("  --git-commit   启动交互式 git commit 文件选择器（子命令）");
    println!("  -h, --help     显示本帮助");
    println!();
    println!("操作（键盘 / 鼠标）:");
    println!("  ↑ ↓ / j k          移动光标          点击目录行    展开/收缩");
    println!("  Enter / → / ←      展开 / 收缩        点击文件行    选中/取消");
    println!("  空格               选中/取消          每行右侧按钮  [复制] 复制路径  [cd] 复制cd命令  [终端] 打开终端  [打开] 系统文件管理器  [yolo] Claude Code yolo模式  [Git] git操作");
    println!("  c                  复制路径（有选中项则复制全部选中）");
    println!("  C                  拼接命令（模板选择）");
    println!("  d                  复制 cd <当前文件夹> 命令（粘贴后按 Enter 执行）");
    println!("  o                  在当前文件夹打开系统终端");
    println!("  y                  在当前文件夹启动 Claude Code yolo 模式");
    println!("  t                  显示/隐藏隐藏文件（默认显示）");
    println!("  r                  刷新（重新读取已展开目录）");
    println!("  q / Esc            退出");
    println!();
    println!("模板配置: ~/.config/ftree/templates.toml（首次运行自动生成）");
    println!("占位符: {{files}} {{files_quoted}} {{names}} {{dir}} {{n}}");
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return;
    }

    // 检查 --git-commit 子命令
    if args.iter().any(|a| a == "--git-commit") {
        let dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        if git::is_git_repo(&dir) {
            if let Err(e) = git_commit::run(&dir) {
                eprintln!("git commit 错误: {}", e);
            }
        } else {
            eprintln!("错误: 当前目录不是 git 仓库");
            std::process::exit(1);
        }
        return;
    }

    let show_hidden = !args.iter().any(|a| a == "--no-hidden");

    let mut start = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    for a in &args {
        if !a.starts_with('-') {
            start = PathBuf::from(a);
            break;
        }
    }
    if !start.is_dir() {
        eprintln!("错误: 目录不存在: {}", start.display());
        std::process::exit(1);
    }
    if !io::stdout().is_terminal() {
        eprintln!("错误: 请直接在终端中运行 ftree");
        std::process::exit(1);
    }

    enable_raw_mode().expect("无法进入 raw mode");
    let _guard = TuiGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).expect("无法切换备用屏幕");
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("无法初始化终端");

    let mut app = App::new(start, show_hidden);
    let _ = app.run(&mut terminal);
    drop(terminal);
}