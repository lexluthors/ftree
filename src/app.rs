#[cfg(not(target_os = "macos"))]
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::clipboard::Clipboard;
use crate::config::Template;
use crate::templates;
use crate::tree::Tree;
use crate::ui::BTN_W;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Browse,
    Picker,
    GitMenu,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TermKind {
    Gnome,
    Konsole,
    Alacritty,
    Kitty,
    Xterm,
    #[cfg(target_os = "macos")]
    MacTerminal,
    Custom,
}

#[derive(Clone)]
pub struct TermCmd {
    pub kind: TermKind,
    pub name: String,
}

pub struct App {
    pub tree: Tree,
    pub mode: Mode,
    pub selected: Vec<PathBuf>,
    pub clipboard: Clipboard,
    pub templates: Vec<Template>,
    pub toast: Option<(String, Instant)>,
    pub picker_index: usize,
    pub git_menu_index: usize,
    pub hover: Option<(u16, u16)>,
    /// 最后一帧渲染时的终端尺寸（鼠标命中判断用）
    pub screen: (u16, u16),
    pub root_display: String,
    pub terminal: Option<TermCmd>,
}

impl App {
    pub fn new(root: PathBuf, show_hidden: bool) -> Self {
        let root_display = root.to_string_lossy().into_owned();
        App {
            tree: Tree::new(root, show_hidden),
            mode: Mode::Browse,
            selected: Vec::new(),
            clipboard: Clipboard::detect(),
            templates: crate::config::load_templates(),
            toast: None,
            picker_index: 0,
            git_menu_index: 0,
            hover: None,
            screen: (0, 0),
            root_display,
            terminal: detect_terminal(),
        }
    }

    pub fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    pub fn tick(&mut self) {
        if let Some((_, t)) = &self.toast {
            if t.elapsed() > Duration::from_secs(4) {
                self.toast = None;
            }
        }
    }

    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        loop {
            terminal.draw(|f| crate::ui::draw(f, self))?;
            if crossterm::event::poll(Duration::from_millis(250))? {
                // 排空 crossterm 事件队列，防止批量按键遗漏。
                // 注意：不能用 poll(Duration::ZERO)——crossterm 0.28 对 0 超时
                // 会直接跳过解析器内部缓冲检查（try_read 的 while 条件不成立），
                // 导致已解析但未取出的事件被永久丢弃。用 1ms 微超时兜住内部缓冲。
                loop {
                    let had = crossterm::event::poll(Duration::from_millis(1))?;
                    if !had {
                        break;
                    }
                    match crossterm::event::read()? {
                        crossterm::event::Event::Key(k) => {
                            if self.handle_key(k) {
                                return Ok(());
                            }
                        }
                        crossterm::event::Event::Mouse(m) => self.handle_mouse(m),
                        _ => {}
                    }
                }
            } else {
                self.tick();
            }
        }
    }

    // ---------- 键盘 ----------

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if std::env::var("FTREE_EVENT_LOG").is_ok() {
            eprintln!("KEY: {:?}", key);
        }
        use KeyCode::*;
        match self.mode {
            Mode::GitMenu => match key.code {
                Up | Char('k') => {
                    self.git_menu_index = (self.git_menu_index + 3) % 4
                }
                Down | Char('j') => self.git_menu_index = (self.git_menu_index + 1) % 4,
                Esc | Char('q') => self.mode = Mode::Browse,
                Enter => self.git_menu_apply(),
                _ => {}
            },
            Mode::Picker => match key.code {
                Up | Char('k') => {
                    self.picker_index =
                        (self.picker_index + self.templates.len() - 1) % self.templates.len()
                }
                Down | Char('j') => self.picker_index = (self.picker_index + 1) % self.templates.len(),
                Esc | Char('q') => self.mode = Mode::Browse,
                Enter => self.picker_apply(),
                _ => {}
            },
            Mode::Browse => match key.code {
                Char('q') | Esc => return true,
                Char('t') => self.tree.flip_hidden(),
                Char('r') => {
                    self.tree.refresh();
                    self.set_toast("已刷新");
                }
                Char('c') => self.copy_paths(),
                Char('d') => self.copy_cd(),
                Char('o') => self.open_terminal(),
                Char('y') => self.open_yolo(),
                Char('C') => self.open_picker(),
                Char(' ') => self.toggle_selected(),
                Char('g') | Home => self.tree.move_cursor(-(self.tree.cursor as isize)),
                Char('G') | End => self.tree.move_cursor(self.tree.visible.len() as isize),
                Up | Char('k') => self.tree.move_cursor(-1),
                Down | Char('j') => self.tree.move_cursor(1),
                PageUp => self.tree.move_cursor(-10),
                PageDown => self.tree.move_cursor(10),
                Enter | Right | Char('l') => self.tree.toggle_cursor(),
                Left | Backspace | Char('h') => self.tree.collapse_up(),
                _ => {}
            },
        }
        false
    }

    // ---------- 鼠标 ----------

    pub fn handle_mouse(&mut self, m: MouseEvent) {
        if std::env::var("FTREE_EVENT_LOG").is_ok() {
            eprintln!("MOUSE: col={} row={} kind={:?}", m.column, m.row, m.kind);
        }
        let (col, row) = (m.column, m.row);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.hover = Some((col, row));
                if self.mode == Mode::Picker {
                    return; // 弹层用键盘操作
                }
                if self.mode == Mode::GitMenu {
                    self.git_menu_mouse_click(col, row);
                    return;
                }
                self.mouse_click(col, row);
            }
            MouseEventKind::ScrollUp => {
                if self.mode == Mode::Browse {
                    let view_h = self.screen.1.saturating_sub(2) as usize;
                    self.tree.scroll_by(-3, view_h);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.mode == Mode::Browse {
                    let view_h = self.screen.1.saturating_sub(2) as usize;
                    self.tree.scroll_by(3, view_h);
                }
            }
            MouseEventKind::Moved => self.hover = Some((col, row)),
            _ => {}
        }
    }

    fn git_menu_mouse_click(&mut self, col: u16, row: u16) {
        let (w, h) = self.screen;
        if w == 0 || h == 0 {
            return;
        }

        // Git 菜单的尺寸和位置（与 draw_git_menu 保持一致）
        let menu_w = 40u16.min(w.saturating_sub(2));
        let menu_h = 10u16.min(h.saturating_sub(4));
        let menu_x = w / 2 - menu_w / 2;
        let menu_y = h / 2 - menu_h / 2;

        // 检查点击是否在菜单区域内
        if col >= menu_x && col < menu_x + menu_w && row >= menu_y && row < menu_y + menu_h {
            // 计算点击的是哪个菜单项（菜单项从 menu_y + 1 开始）
            let item_row = row.saturating_sub(menu_y + 1);
            if item_row < 4 {
                // 有 4 个菜单项
                self.git_menu_index = item_row as usize;
                self.git_menu_apply();
            }
        } else {
            // 点击菜单外部，关闭菜单
            self.mode = Mode::Browse;
        }
    }

    fn mouse_click(&mut self, col: u16, row: u16) {
        let (w, h) = self.screen;
        if w == 0 || h == 0 || row == 0 || row >= h.saturating_sub(1) {
            return; // 状态栏/底部栏点击忽略
        }
        let btn_zone_start = w.saturating_sub(BTN_W);
        if col >= btn_zone_start && col < w {
            let off = col - btn_zone_start;
            let vis_row = row as usize - 1 + self.tree.scroll;
            if vis_row < self.tree.visible.len() {
                self.tree.cursor = vis_row;
                // 按钮热区：[复制]6 / 间隔1 / [cd]4 / 间隔1 / [终端]6 / 间隔1 / [打开]6 / 间隔1 / [yolo]6 / 间隔1 / [Git]5
                let btn = match off {
                    0..=5 => 0,     // [复制]
                    7..=10 => 1,    // [cd]
                    12..=17 => 2,   // [终端]
                    19..=24 => 3,   // [打开]
                    26..=31 => 4,   // [yolo]
                    33..=37 => 5,   // [Git]
                    _ => return,    // 间隔区点击忽略
                };
                self.do_row_button(btn);
            }
            return;
        }
        // 树区点击：目录展开/收缩，文件选中
        let vis_row = row as usize - 1 + self.tree.scroll;
        if vis_row >= self.tree.visible.len() {
            return;
        }
        self.tree.cursor = vis_row;
        if self.tree.cursor_node().is_dir() {
            self.tree.toggle_cursor();
        } else {
            self.toggle_selected();
        }
    }

    fn do_row_button(&mut self, btn: usize) {
        match btn {
            0 => {
                let p = self.tree.cursor_node().path.clone();
                match self.clipboard.set(&p.to_string_lossy()) {
                    Ok(()) => self.set_toast(format!("已复制路径: {}", p.to_string_lossy())),
                    Err(e) => self.set_toast(format!("复制失败: {e}")),
                }
            }
            1 => self.copy_cd(),
            2 => self.open_terminal(),
            3 => self.open_file_manager(),
            4 => self.open_yolo(),
            5 => self.open_git_menu(),
            _ => {}
        }
    }

    // ---------- 功能 ----------

    fn open_picker(&mut self) {
        if self.selected.is_empty() {
            self.set_toast("请先按空格选中文件");
            return;
        }
        self.picker_index = 0;
        self.mode = Mode::Picker;
    }

    fn picker_apply(&mut self) {
        let tpl = match self.templates.get(self.picker_index) {
            Some(t) => t.clone(),
            None => return,
        };
        let cmd = templates::render(&tpl, &self.selected);
        match self.clipboard.set(&cmd) {
            Ok(()) => self.set_toast(format!("已复制命令（{} 字符）", cmd.chars().count())),
            Err(e) => self.set_toast(format!("复制失败: {e}")),
        }
        self.mode = Mode::Browse;
    }

    pub fn picker_preview(&self) -> String {
        self.templates
            .get(self.picker_index)
            .map(|t| templates::render(t, &self.selected))
            .unwrap_or_default()
    }

    fn toggle_selected(&mut self) {
        let p = self.tree.cursor_node().path.clone();
        if let Some(pos) = self.selected.iter().position(|x| x == &p) {
            self.selected.remove(pos);
        } else {
            self.selected.push(p);
        }
    }

    fn copy_paths(&mut self) {
        let list = self.copy_targets();
        let text = list
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        match self.clipboard.set(&text) {
            Ok(()) => self.set_toast(format!("已复制 {} 个路径", list.len())),
            Err(e) => self.set_toast(format!("复制失败: {e}")),
        }
    }

    /// 有选中项则复制全部选中路径，否则复制当前行。
    fn copy_targets(&self) -> Vec<PathBuf> {
        if !self.selected.is_empty() {
            self.selected.clone()
        } else {
            vec![self.tree.cursor_node().path.clone()]
        }
    }

    fn copy_cd(&mut self) {
        let dir = self.tree.cursor_dir();
        let cmd = format!("cd {}", shell_quote_path(&dir));
        match self.clipboard.set(&cmd) {
            Ok(()) => self.set_toast(format!("已复制: {}（粘贴后按 Enter）", cmd)),
            Err(e) => self.set_toast(format!("复制失败: {e}")),
        }
    }

    fn open_terminal(&mut self) {
        let dir = self.tree.cursor_dir();
        let Some(term) = &self.terminal else {
            self.set_toast("未检测到可用终端");
            return;
        };
        let mut cmd = Command::new(&term.name);
        match term.kind {
            TermKind::Gnome | TermKind::Alacritty => {
                cmd.arg("--working-directory").arg(&dir);
            }
            TermKind::Konsole => {
                cmd.arg("--workdir").arg(&dir);
            }
            TermKind::Kitty => {
                cmd.arg("-d").arg(&dir);
            }
            #[cfg(target_os = "macos")]
            TermKind::MacTerminal => {
                // 使用 osascript 在 Terminal.app 中打开指定目录
                // 先创建新窗口（使用默认 profile），再发送命令
                let script = format!("cd {}", dir.to_string_lossy().replace('"', "\\\""));
                cmd.arg("-e")
                   .arg("tell application \"Terminal\"")
                   .arg("-e")
                   .arg("activate")
                   .arg("-e")
                   .arg("set newWindow to (do script \"\")")
                   .arg("-e")
                   .arg("delay 0.3")
                   .arg("-e")
                   .arg(format!("do script \"{}\" in newWindow", script.replace('"', "\\\"")))
                   .arg("-e")
                   .arg("end tell");
            }
            TermKind::Xterm | TermKind::Custom => {
                cmd.current_dir(&dir); // 以该目录为工作目录启动
            }
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已在 {} 打开终端", dir.to_string_lossy())),
            Err(e) => self.set_toast(format!("打开终端失败: {e}")),
        }
    }

    /// 在系统文件管理器中打开当前文件夹
    #[cfg(target_os = "macos")]
    fn open_file_manager(&mut self) {
        let dir = self.tree.cursor_dir();
        let mut cmd = Command::new("open");
        cmd.arg(&dir);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已在文件管理器中打开 {}", dir.to_string_lossy())),
            Err(e) => self.set_toast(format!("打开文件管理器失败: {e}")),
        }
    }

    /// 在系统文件管理器中打开当前文件夹（用 xdg-open 调起桌面默认应用）
    #[cfg(not(target_os = "macos"))]
    fn open_file_manager(&mut self) {
        let dir = self.tree.cursor_dir();
        let mut cmd = Command::new("xdg-open");
        cmd.arg(&dir);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已在文件管理器中打开 {}", dir.to_string_lossy())),
            Err(e) => self.set_toast(format!("打开文件管理器失败: {e}")),
        }
    }

    /// 在终端中启动 Claude Code yolo 模式（--dangerously-skip-permissions）
    fn open_yolo(&mut self) {
        let dir = self.tree.cursor_dir();
        let Some(term) = &self.terminal else {
            self.set_toast("未检测到可用终端");
            return;
        };
        let mut cmd = Command::new(&term.name);
        match term.kind {
            TermKind::Gnome | TermKind::Alacritty => {
                cmd.arg("--working-directory").arg(&dir);
                cmd.arg("-e").arg("claude --dangerously-skip-permissions");
            }
            TermKind::Konsole => {
                cmd.arg("--workdir").arg(&dir);
                cmd.arg("-e").arg("claude --dangerously-skip-permissions");
            }
            TermKind::Kitty => {
                cmd.arg("-d").arg(&dir);
                cmd.arg("claude --dangerously-skip-permissions");
            }
            #[cfg(target_os = "macos")]
            TermKind::MacTerminal => {
                // 先打开新窗口（使用默认 profile），等窗口完成初始化后再发送命令，
                // 避免 Terminal.app GPU 合成器在窗口未就绪时渲染大量输出导致画面残影。
                let script = format!("cd {} && claude --dangerously-skip-permissions", dir.to_string_lossy().replace('"', "\\\""));
                cmd.arg("-e")
                   .arg("tell application \"Terminal\"")
                   .arg("-e")
                   .arg("activate")
                   .arg("-e")
                   .arg("set newWindow to (do script \"\")")
                   .arg("-e")
                   .arg("delay 0.3")
                   .arg("-e")
                   .arg(format!("do script \"{}\" in newWindow", script.replace('"', "\\\"")))
                   .arg("-e")
                   .arg("end tell");
            }
            TermKind::Xterm | TermKind::Custom => {
                cmd.current_dir(&dir);
                cmd.arg("-e").arg("claude --dangerously-skip-permissions");
            }
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已在 {} 启动 Claude Code yolo 模式", dir.to_string_lossy())),
            Err(e) => self.set_toast(format!("启动 Claude Code 失败: {e}")),
        }
    }

    // ---------- Git 操作 ----------

    fn open_git_menu(&mut self) {
        self.git_menu_index = 0;
        self.mode = Mode::GitMenu;
    }

    fn git_menu_apply(&mut self) {
        let dir = self.tree.cursor_dir();
        let is_repo = crate::git::is_git_repo(&dir);

        match self.git_menu_index {
            0 => self.git_share(dir, is_repo),
            1 => self.git_pull(dir, is_repo),
            2 => self.git_commit(dir, is_repo),
            3 => self.git_push(dir, is_repo),
            _ => {}
        }
        self.mode = Mode::Browse;
    }

    fn git_share(&mut self, dir: std::path::PathBuf, is_repo: bool) {
        if is_repo {
            self.set_toast("已经是 git 仓库，无需 share");
            return;
        }
        let Some(term) = self.terminal.clone() else {
            self.set_toast("未检测到可用终端");
            return;
        };

        // 构建 share 脚本
        let script = r#"
echo "=== Git Share on GitHub ==="
echo ""

# 检查是否已安装 git
if ! command -v git &> /dev/null; then
    echo "错误: 未安装 git，请先安装 git"
    echo "按任意键退出..."
    read -n1
    exit 1
fi

# 初始化 git
git init
echo "✓ 已初始化 git 仓库"

# 检测默认分支名
BRANCH=$(git symbolic-ref HEAD 2>/dev/null | sed 's|refs/heads/||')
if [ -z "$BRANCH" ]; then
    BRANCH="master"
fi
echo "  默认分支: $BRANCH"

# 提示输入 GitHub 仓库 URL
echo ""
echo "请输入 GitHub 仓库 URL (例如: git@github.com:user/repo.git):"
echo "  或直接按 Enter 跳过 remote 配置"
read -r REPO_URL

if [ -n "$REPO_URL" ]; then
    git remote add origin "$REPO_URL"
    echo "✓ 已添加 remote origin: $REPO_URL"
else
    echo "跳过 remote 配置"
fi

# 首次 commit
echo ""
echo "请输入 commit 描述 (直接按 Enter 使用默认 'first commit'):"
read -r MSG
if [ -z "$MSG" ]; then
    MSG="first commit"
fi

git add .
git commit -m "$MSG"
echo "✓ 已提交: $MSG"

# Push
if [ -n "$REPO_URL" ]; then
    echo ""
    echo "正在 push 到 $REPO_URL ..."
    git push -u origin "$BRANCH"
    if [ $? -eq 0 ]; then
        echo "✓ push 成功"
    else
        echo "✗ push 失败，请检查网络和认证"
    fi
fi

echo ""
echo "完成！按任意键退出..."
read -n1
"#;

        self.run_in_terminal(&term, &dir, script, "Git Share");
    }

    fn git_pull(&mut self, dir: std::path::PathBuf, is_repo: bool) {
        if !is_repo {
            self.set_toast("当前目录不是 git 仓库");
            return;
        }
        let Some(term) = self.terminal.clone() else {
            self.set_toast("未检测到可用终端");
            return;
        };

        let script = r#"
echo "=== Git Pull ==="
echo ""
git pull
echo ""
echo "完成！按任意键退出..."
read -n1
"#;

        self.run_in_terminal(&term, &dir, script, "Git Pull");
    }

    fn git_commit(&mut self, dir: std::path::PathBuf, is_repo: bool) {
        if !is_repo {
            self.set_toast("当前目录不是 git 仓库");
            return;
        }
        let Some(term) = self.terminal.clone() else {
            self.set_toast("未检测到可用终端");
            return;
        };

        // 获取当前可执行文件路径
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "ftree".to_string());

        let script = format!(r#"
echo "启动交互式 git commit..."
"{}" --git-commit
"#, exe);

        self.run_in_terminal(&term, &dir, &script, "Git Commit");
    }

    fn git_push(&mut self, dir: std::path::PathBuf, is_repo: bool) {
        if !is_repo {
            self.set_toast("当前目录不是 git 仓库");
            return;
        }
        let Some(term) = self.terminal.clone() else {
            self.set_toast("未检测到可用终端");
            return;
        };

        let script = r#"
echo "=== Git Push ==="
echo ""
git push
echo ""
echo "完成！按任意键退出..."
read -n1
"#;

        self.run_in_terminal(&term, &dir, script, "Git Push");
    }

    /// 在系统终端中执行脚本
    fn run_in_terminal(&mut self, term: &TermCmd, dir: &std::path::Path, script: &str, label: &str) {
        let mut cmd = Command::new(&term.name);
        match term.kind {
            TermKind::Gnome => {
                cmd.arg("--working-directory").arg(dir);
                cmd.arg("--").arg("bash").arg("-c").arg(script);
            }
            TermKind::Alacritty => {
                cmd.arg("--working-directory").arg(dir);
                cmd.arg("-e").arg("bash").arg("-c").arg(script);
            }
            TermKind::Konsole => {
                cmd.arg("--workdir").arg(dir);
                cmd.arg("-e").arg("bash").arg("-c").arg(script);
            }
            TermKind::Kitty => {
                cmd.arg("-d").arg(dir);
                cmd.arg("bash").arg("-c").arg(script);
            }
            #[cfg(target_os = "macos")]
            TermKind::MacTerminal => {
                let cd_script = format!("cd {}", dir.to_string_lossy().replace('"', "\\\""));
                let full_script = format!("{} && {}", cd_script, script.replace('"', "\\\""));
                cmd.arg("-e")
                   .arg("tell application \"Terminal\"")
                   .arg("-e")
                   .arg("activate")
                   .arg("-e")
                   .arg("set newWindow to (do script \"\")")
                   .arg("-e")
                   .arg("delay 0.3")
                   .arg("-e")
                   .arg(format!("do script \"{}\" in newWindow", full_script))
                   .arg("-e")
                   .arg("end tell");
            }
            TermKind::Xterm | TermKind::Custom => {
                cmd.current_dir(dir);
                cmd.arg("-e").arg("bash").arg("-c").arg(script);
            }
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已在终端打开 {}", label)),
            Err(e) => self.set_toast(format!("打开终端失败: {e}")),
        }
    }
}

/// 路径含空格或引号时用单引号包裹（内部单引号转义），否则原样输出。
fn shell_quote_path(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
        templates::quote(&s)
    } else {
        s.into_owned()
    }
}

#[cfg(target_os = "macos")]
fn detect_terminal() -> Option<TermCmd> {
    // macOS: 使用系统自带的 Terminal.app
    Some(TermCmd {
        kind: TermKind::MacTerminal,
        name: "osascript".to_string(),
    })
}

#[cfg(not(target_os = "macos"))]
fn detect_terminal() -> Option<TermCmd> {
    if let Ok(t) = env::var("TERMINAL") {
        let name = t.trim().to_string();
        if !name.is_empty() {
            let kind = match name.as_str() {
                "gnome-terminal" => TermKind::Gnome,
                "konsole" => TermKind::Konsole,
                "alacritty" => TermKind::Alacritty,
                "kitty" => TermKind::Kitty,
                "xterm" => TermKind::Xterm,
                _ => TermKind::Custom,
            };
            return Some(TermCmd { kind, name });
        }
    }
    for (name, kind) in [
        ("gnome-terminal", TermKind::Gnome),
        ("konsole", TermKind::Konsole),
        ("alacritty", TermKind::Alacritty),
        ("kitty", TermKind::Kitty),
        ("xterm", TermKind::Xterm),
    ] {
        if Command::new(name)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return Some(TermCmd {
                kind,
                name: name.to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use std::sync::Mutex;

    /// 剪贴板是系统共享资源（X11 全局/系统剪贴板），测试必须串行，否则互相覆盖。
    static CLIP_LOCK: Mutex<()> = Mutex::new(());

    fn fixture() -> (std::path::PathBuf, App) {
        let d = std::env::temp_dir().join(format!(
            "ftree-test-app-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.mp4"), b"").unwrap();
        std::fs::write(d.join("b.mp4"), b"").unwrap();
        let app = App::new(d.clone(), false);
        (d, app)
    }

    /// 读取剪贴板内容（跨平台）
    #[cfg(target_os = "macos")]
    fn read_clipboard() -> String {
        std::process::Command::new("pbpaste")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }

    #[cfg(not(target_os = "macos"))]
    fn read_clipboard() -> String {
        std::process::Command::new("xclip")
            .args(["-o", "-selection", "clipboard"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }

    /// 轮询剪贴板直到与期望一致（剪贴板是进程共享资源，可能被其他测试写入）。
    fn wait_until(expected: &str) -> bool {
        for _ in 0..40 {
            if read_clipboard().trim() == expected {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        read_clipboard().trim() == expected
    }

    #[test]
    fn c_key_copies_cursor_path() {
        let _g = CLIP_LOCK.lock().unwrap();
        let (d, mut app) = fixture();
        app.handle_key(KeyEvent::from(KeyCode::Char('c')));
        let expect = d.to_string_lossy().into_owned();
        assert!(
            wait_until(&expect),
            "剪贴板未匹配:\n  got:    {:?}\n  expect: {:?}",
            read_clipboard(),
            expect
        );
    }

    #[test]
    fn picker_flow_concats_ffmpeg_template() {
        let _g = CLIP_LOCK.lock().unwrap();
        let (d, mut app) = fixture();
        // 选中 a.mp4 b.mp4（行: root, a.mp4, b.mp4）
        app.tree.move_cursor(1);
        app.toggle_selected();
        app.tree.move_cursor(1);
        app.toggle_selected();
        assert_eq!(app.selected.len(), 2);
        // 打开模板面板
        app.handle_key(KeyEvent::from(KeyCode::Char('C')));
        assert_eq!(app.mode, Mode::Picker);
        // 应用模板 0（ffmpeg）
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Browse);
        let expect = format!(
            "ffmpeg -i '{}' '{}' -c:v libx264 -c:a aac {}/out.mp4",
            d.join("a.mp4").to_string_lossy(),
            d.join("b.mp4").to_string_lossy(),
            d.to_string_lossy()
        );
        assert!(
            wait_until(&expect),
            "剪贴板未匹配:\n  got:    {:?}\n  expect: {:?}",
            read_clipboard(),
            expect
        );
    }
}
