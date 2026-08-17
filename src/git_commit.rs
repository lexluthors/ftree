//! 交互式 git commit TUI（子命令入口）
//! 在系统终端中运行，支持多选文件、输入 commit 描述、选择 commit 方式

use crossterm::event::{self, Event, KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, cursor};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use crate::git::{self, GitFileStatus};

/// 文件选择状态
struct FileItem {
    status: GitFileStatus,
    selected: bool,
}

/// 交互模式
#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusGroup {
    Tracked,
    Untracked,
}

/// Commit 方式选择
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommitAction {
    CommitOnly,
    CommitAndPush,
}

/// 当前阶段
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    SelectFiles,
    InputMessage,
    SelectAction,
    Executing,
    Done,
}

pub fn run(dir: &Path) -> io::Result<()> {
    // 进入 raw mode
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

    let result = run_inner(dir, &mut stdout);

    // 恢复终端
    execute!(stdout, cursor::Show, LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    result
}

fn run_inner(dir: &Path, stdout: &mut io::Stdout) -> io::Result<()> {
    // 获取 git status
    let all_files = git::get_git_status(dir);
    if all_files.is_empty() {
        show_message(stdout, "没有需要提交的更改")?;
        wait_for_key(stdout)?;
        return Ok(());
    }

    // 初始化文件列表
    let mut files: Vec<FileItem> = all_files
        .into_iter()
        .map(|s| FileItem {
            status: s,
            selected: false,
        })
        .collect();

    let mut focus = FocusGroup::Tracked;
    let mut tracked_cursor: usize = 0;
    let mut untracked_cursor: usize = 0;
    let mut stage = Stage::SelectFiles;
    let mut commit_message = String::new();
    let mut action = CommitAction::CommitAndPush;

    loop {
        // 渲染
        render(stdout, &files, focus, tracked_cursor, untracked_cursor, stage, &commit_message, action)?;

        // 处理事件
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    match stage {
                        Stage::SelectFiles => {
                            if handle_select_keys(key, &mut files, &mut focus, &mut tracked_cursor, &mut untracked_cursor, &mut stage) {
                                break;
                            }
                        }
                        Stage::InputMessage => {
                            if handle_message_keys(key, &mut commit_message, &mut stage) {
                                break;
                            }
                        }
                        Stage::SelectAction => {
                            if handle_action_keys(key, &mut action, &mut stage) {
                                break;
                            }
                        }
                        Stage::Executing | Stage::Done => {
                            // 按任意键退出
                            break;
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    if stage == Stage::SelectFiles {
                        handle_mouse(mouse, &mut files, &mut focus, &mut tracked_cursor, &mut untracked_cursor);
                    }
                }
                _ => {}
            }
        }
    }

    // 如果选择了文件并确认了操作
    if stage == Stage::Executing {
        let selected: Vec<&Path> = files
            .iter()
            .filter(|f| f.selected)
            .map(|f| f.status.path.as_path())
            .collect();

        if !selected.is_empty() {
            execute_git_commit(dir, &selected, &commit_message, action)?;
        }
    }

    Ok(())
}

fn handle_select_keys(
    key: KeyEvent,
    files: &mut [FileItem],
    focus: &mut FocusGroup,
    tracked_cursor: &mut usize,
    untracked_cursor: &mut usize,
    stage: &mut Stage,
) -> bool {
    let tracked_count = files.iter().filter(|f| f.status.is_tracked()).count();
    let untracked_count = files.iter().filter(|f| !f.status.is_tracked()).count();

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return true,
        KeyCode::Char('j') | KeyCode::Down => {
            match focus {
                FocusGroup::Tracked => {
                    if tracked_count > 0 {
                        *tracked_cursor = (*tracked_cursor + 1).min(tracked_count - 1);
                    }
                }
                FocusGroup::Untracked => {
                    if untracked_count > 0 {
                        *untracked_cursor = (*untracked_cursor + 1).min(untracked_count - 1);
                    }
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            match focus {
                FocusGroup::Tracked => {
                    if tracked_count > 0 {
                        *tracked_cursor = tracked_cursor.saturating_sub(1);
                    }
                }
                FocusGroup::Untracked => {
                    if untracked_count > 0 {
                        *untracked_cursor = untracked_cursor.saturating_sub(1);
                    }
                }
            }
        }
        KeyCode::Tab => {
            *focus = match focus {
                FocusGroup::Tracked => FocusGroup::Untracked,
                FocusGroup::Untracked => FocusGroup::Tracked,
            };
        }
        KeyCode::Char(' ') => {
            // 切换当前项的选中状态
            let mut idx = 0;
            for f in files.iter_mut() {
                let is_in_group = match focus {
                    FocusGroup::Tracked => f.status.is_tracked(),
                    FocusGroup::Untracked => !f.status.is_tracked(),
                };
                if is_in_group {
                    let cursor = match focus {
                        FocusGroup::Tracked => *tracked_cursor,
                        FocusGroup::Untracked => *untracked_cursor,
                    };
                    if idx == cursor {
                        f.selected = !f.selected;
                        break;
                    }
                    idx += 1;
                }
            }
        }
        KeyCode::Char('a') => {
            // 全选当前组
            for f in files.iter_mut() {
                let is_in_group = match focus {
                    FocusGroup::Tracked => f.status.is_tracked(),
                    FocusGroup::Untracked => !f.status.is_tracked(),
                };
                if is_in_group {
                    f.selected = true;
                }
            }
        }
        KeyCode::Char('A') => {
            // 取消全选当前组
            for f in files.iter_mut() {
                let is_in_group = match focus {
                    FocusGroup::Tracked => f.status.is_tracked(),
                    FocusGroup::Untracked => !f.status.is_tracked(),
                };
                if is_in_group {
                    f.selected = false;
                }
            }
        }
        KeyCode::Enter => {
            let has_selected = files.iter().any(|f| f.selected);
            if has_selected {
                *stage = Stage::InputMessage;
            }
        }
        _ => {}
    }
    false
}

fn handle_message_keys(key: KeyEvent, message: &mut String, stage: &mut Stage) -> bool {
    match key.code {
        KeyCode::Esc => return true,
        KeyCode::Enter => {
            if !message.is_empty() {
                *stage = Stage::SelectAction;
            }
        }
        KeyCode::Backspace => {
            message.pop();
        }
        KeyCode::Char(c) => {
            message.push(c);
        }
        _ => {}
    }
    false
}

fn handle_action_keys(key: KeyEvent, action: &mut CommitAction, stage: &mut Stage) -> bool {
    match key.code {
        KeyCode::Esc => {
            *stage = Stage::InputMessage;
        }
        KeyCode::Char('1') => {
            *action = CommitAction::CommitOnly;
        }
        KeyCode::Char('2') => {
            *action = CommitAction::CommitAndPush;
        }
        KeyCode::Enter => {
            // 使用当前选择的 action
            *stage = Stage::Executing;
        }
        _ => {}
    }
    false
}

fn handle_mouse(
    mouse: MouseEvent,
    files: &mut [FileItem],
    focus: &mut FocusGroup,
    tracked_cursor: &mut usize,
    untracked_cursor: &mut usize,
) {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }

    let row = mouse.row as usize;
    let col = mouse.column as usize;

    // 计算各组的位置（与渲染对应）
    let tracked_start = 3; // 标题后第一行
    let tracked_count = files.iter().filter(|f| f.status.is_tracked()).count();
    let untracked_start = tracked_start + tracked_count + 3; // 标题 + 文件 + 空行 + 标题

    // 点击已跟踪组
    if row >= tracked_start && row < tracked_start + tracked_count {
        *focus = FocusGroup::Tracked;
        let idx = row - tracked_start;
        *tracked_cursor = idx;
        // 切换选中
        let mut count = 0;
        for f in files.iter_mut() {
            if f.status.is_tracked() {
                if count == idx {
                    f.selected = !f.selected;
                    break;
                }
                count += 1;
            }
        }
    }

    // 点击未跟踪组
    let untracked_count = files.iter().filter(|f| !f.status.is_tracked()).count();
    if row >= untracked_start && row < untracked_start + untracked_count {
        *focus = FocusGroup::Untracked;
        let idx = row - untracked_start;
        *untracked_cursor = idx;
        // 切换选中
        let mut count = 0;
        for f in files.iter_mut() {
            if !f.status.is_tracked() {
                if count == idx {
                    f.selected = !f.selected;
                    break;
                }
                count += 1;
            }
        }
    }

    // 点击按钮区域
    if col < 20 && row > 20 {
        // 底部按钮区域 - 这里简化处理
    }
}

fn render(
    stdout: &mut io::Stdout,
    files: &[FileItem],
    focus: FocusGroup,
    tracked_cursor: usize,
    untracked_cursor: usize,
    stage: Stage,
    commit_message: &str,
    action: CommitAction,
) -> io::Result<()> {
    let (_cols, _rows) = terminal::size()?;
    execute!(stdout, terminal::Clear(terminal::ClearType::All))?;

    let mut y = 0;

    // 标题
    write!(stdout, "\x1b[1;36m")?; // 粗体青色
    write!(stdout, "\x1b[{};1H", y + 1)?;
    write!(stdout, " Git Commit - 选择要提交的文件 ")?;
    write!(stdout, "\x1b[0m")?;
    y += 2;

    // 已跟踪文件组
    let tracked_files: Vec<&FileItem> = files.iter().filter(|f| f.status.is_tracked()).collect();
    let tracked_title = format!(" 已跟踪文件 ({}) ", tracked_files.len());

    let is_tracked_focused = focus == FocusGroup::Tracked;
    if is_tracked_focused {
        write!(stdout, "\x1b[1;33m")?; // 粗体黄色
    } else {
        write!(stdout, "\x1b[33m")?; // 黄色
    }
    write!(stdout, "\x1b[{};1H", y + 1)?;
    write!(stdout, "┌{}┐", "─".repeat(tracked_title.chars().count().max(40)))?;
    y += 1;
    write!(stdout, "\x1b[{};1H", y + 1)?;
    write!(stdout, "│{}│", tracked_title)?;
    y += 1;

    let mut tracked_idx = 0;
    for f in &tracked_files {
        write!(stdout, "\x1b[{};1H", y + 1)?;
        let is_cursor = is_tracked_focused && tracked_idx == tracked_cursor;
        let checkbox = if f.selected { "[✓]" } else { "[ ]" };
        let status = f.status.status_symbol();
        let path = f.status.path.to_string_lossy();

        if is_cursor {
            write!(stdout, "\x1b[7m")?; // 反转
        }
        write!(stdout, "│ {} {} {} {}", checkbox, status, path, " ".repeat(40usize.saturating_sub(path.len())))?;
        if is_cursor {
            write!(stdout, "\x1b[0m")?;
        }
        write!(stdout, "│")?;
        y += 1;
        tracked_idx += 1;
    }
    write!(stdout, "\x1b[0m")?;
    write!(stdout, "\x1b[{};1H", y + 1)?;
    write!(stdout, "└{}┘", "─".repeat(tracked_title.chars().count().max(40)))?;
    y += 2;

    // 未跟踪文件组
    let untracked_files: Vec<&FileItem> = files.iter().filter(|f| !f.status.is_tracked()).collect();
    let untracked_title = format!(" 未跟踪文件 ({}) ", untracked_files.len());

    let is_untracked_focused = focus == FocusGroup::Untracked;
    if is_untracked_focused {
        write!(stdout, "\x1b[1;32m")?; // 粗体绿色
    } else {
        write!(stdout, "\x1b[32m")?; // 绿色
    }
    write!(stdout, "\x1b[{};1H", y + 1)?;
    write!(stdout, "┌{}┐", "─".repeat(untracked_title.chars().count().max(40)))?;
    y += 1;
    write!(stdout, "\x1b[{};1H", y + 1)?;
    write!(stdout, "│{}│", untracked_title)?;
    y += 1;

    let mut untracked_idx = 0;
    for f in &untracked_files {
        write!(stdout, "\x1b[{};1H", y + 1)?;
        let is_cursor = is_untracked_focused && untracked_idx == untracked_cursor;
        let checkbox = if f.selected { "[✓]" } else { "[ ]" };
        let path = f.status.path.to_string_lossy();

        if is_cursor {
            write!(stdout, "\x1b[7m")?;
        }
        write!(stdout, "│ {} ? {} {}", checkbox, path, " ".repeat(40usize.saturating_sub(path.len())))?;
        if is_cursor {
            write!(stdout, "\x1b[0m")?;
        }
        write!(stdout, "│")?;
        y += 1;
        untracked_idx += 1;
    }
    write!(stdout, "\x1b[0m")?;
    write!(stdout, "\x1b[{};1H", y + 1)?;
    write!(stdout, "└{}┘", "─".repeat(untracked_title.chars().count().max(40)))?;
    y += 2;

    // 操作提示
    match stage {
        Stage::SelectFiles => {
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " 操作: ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "空格")?;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "=选择  ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "a")?;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "=全选当前组  ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "A")?;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "=取消全选  ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "Tab")?;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "=切换组  ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "Enter")?;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "=确认  ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "Esc")?;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "=取消")?;
        }
        Stage::InputMessage => {
            write!(stdout, "\x1b[1;36m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " 请输入 commit 描述: ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "{}", commit_message)?;
            write!(stdout, "\x1b[7m")?;
            write!(stdout, " ")?; // 光标
            write!(stdout, "\x1b[0m")?;
            y += 1;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " Enter=确认  Esc=取消")?;
        }
        Stage::SelectAction => {
            write!(stdout, "\x1b[1;36m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " commit 描述: ")?;
            write!(stdout, "\x1b[33m")?;
            write!(stdout, "{}", commit_message)?;
            y += 1;
            write!(stdout, "\x1b[1;36m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " 选择操作: ")?;
            // 高亮当前选择的选项
            let sel1 = if action == CommitAction::CommitOnly { "\x1b[7m" } else { "\x1b[0m" };
            let sel2 = if action == CommitAction::CommitAndPush { "\x1b[7m" } else { "\x1b[0m" };
            write!(stdout, "{} 1. 仅 commit {}    ", sel1, "\x1b[0m")?;
            write!(stdout, "{} 2. commit and push (默认) {}", sel2, "\x1b[0m")?;
            y += 1;
            write!(stdout, "\x1b[37m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " 按 1/2 切换，Enter=确认，Esc=返回")?;
        }
        Stage::Executing => {
            write!(stdout, "\x1b[1;32m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " 正在执行 git commit...")?;
        }
        Stage::Done => {
            write!(stdout, "\x1b[1;32m")?;
            write!(stdout, "\x1b[{};1H", y + 1)?;
            write!(stdout, " 完成！按任意键退出...")?;
        }
    }

    write!(stdout, "\x1b[0m")?;
    stdout.flush()?;
    Ok(())
}

fn show_message(stdout: &mut io::Stdout, msg: &str) -> io::Result<()> {
    execute!(stdout, terminal::Clear(terminal::ClearType::All))?;
    write!(stdout, "\x1b[2;1H")?;
    write!(stdout, "\x1b[1;33m")?;
    write!(stdout, " {}", msg)?;
    write!(stdout, "\x1b[0m")?;
    stdout.flush()?;
    Ok(())
}

fn wait_for_key(_stdout: &mut io::Stdout) -> io::Result<()> {
    loop {
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(_) = event::read()? {
                break;
            }
        }
    }
    Ok(())
}

fn execute_git_commit(
    dir: &Path,
    files: &[&Path],
    message: &str,
    action: CommitAction,
) -> io::Result<()> {
    // 完全退出 TUI 模式
    terminal::disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, cursor::Show)?;

    // 清空事件队列，防止残留事件
    while event::poll(std::time::Duration::from_millis(10))? {
        let _ = event::read();
    }

    println!("\x1b[1;36m执行 git commit...\x1b[0m\n");

    // git add 选中的文件
    for file in files {
        println!("\x1b[33m  git add {}\x1b[0m", file.to_string_lossy());
        let _ = Command::new("git")
            .args(["-C", &dir.to_string_lossy(), "add", &file.to_string_lossy()])
            .status();
    }

    // git commit
    println!("\n\x1b[33m  git commit -m \"{}\"\x1b[0m", message);
    let commit_result = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "commit", "-m", message])
        .status();

    match commit_result {
        Ok(status) if status.success() => {
            println!("\n\x1b[1;32m✓ commit 成功\x1b[0m");

            // 如果需要 push
            if action == CommitAction::CommitAndPush {
                println!("\n\x1b[33m  git push...\x1b[0m");
                let push_result = Command::new("git")
                    .args(["-C", &dir.to_string_lossy(), "push"])
                    .status();

                match push_result {
                    Ok(status) if status.success() => {
                        println!("\n\x1b[1;32m✓ push 成功\x1b[0m");
                    }
                    _ => {
                        println!("\n\x1b[1;31m✗ push 失败\x1b[0m");
                    }
                }
            }
        }
        _ => {
            println!("\n\x1b[1;31m✗ commit 失败\x1b[0m");
        }
    }

    // 显示提示并等待按键 - 使用标准输入，不依赖 crossterm
    println!("\n\x1b[37m按任意键退出...\x1b[0m");
    io::stdout().flush()?;

    // 使用 crossterm 等待按键（不使用 raw mode）
    loop {
        match event::read() {
            Ok(Event::Key(_)) | Ok(Event::Mouse(_)) => break,
            _ => continue,
        }
    }

    Ok(())
}
