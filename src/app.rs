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
    ActionMenu,
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
    pub action_menu_index: usize,
    pub hover: Option<(u16, u16)>,
    /// 最后一帧渲染时的终端尺寸（鼠标命中判断用）
    pub screen: (u16, u16),
    pub root_display: String,
    pub terminal: Option<TermCmd>,
    /// 上一次左键按下（时间, 列, 行），用于双击检测
    pub last_click: Option<(Instant, u16, u16)>,
    /// 待删除确认：存储 (路径, 显示名, 父目录链)
    pub pending_delete: Option<(PathBuf, String, Vec<usize>)>,
}

/// 双击判定窗口：300ms 内同一单元格第二次按下视为双击
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(300);

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
            action_menu_index: 0,
            hover: None,
            screen: (0, 0),
            root_display,
            terminal: detect_terminal(),
            last_click: None,
            pending_delete: None,
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
            Mode::ActionMenu => {
                let n = Self::action_menu_options().len();
                match key.code {
                    Up | Char('k') => {
                        self.action_menu_index = (self.action_menu_index + n - 1) % n
                    }
                    Down | Char('j') => self.action_menu_index = (self.action_menu_index + 1) % n,
                    Esc | Char('q') => self.mode = Mode::Browse,
                    Enter => self.action_menu_apply(),
                    _ => {}
                }
            }
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
            Mode::Browse => {
                // 删除确认态：只有 y/Enter 确认，其他键一律取消
                if self.pending_delete.is_some() {
                    match key.code {
                        Char('y') | Enter => self.confirm_delete(),
                        _ => {
                            self.pending_delete = None;
                            self.set_toast("已取消删除");
                        }
                    }
                    return false;
                }
                match key.code {
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
            }
            }
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
                if self.mode == Mode::ActionMenu {
                    self.action_menu_mouse_click(col, row);
                    return;
                }
                // 双击检测：300ms 内同一单元格第二次按下。
                // 判定为双击后清空 last_click，三连击会重新从单击开始计数。
                let now = Instant::now();
                let is_double = matches!(self.last_click, Some((t, c, r))
                    if now.duration_since(t) <= DOUBLE_CLICK_WINDOW && c == col && r == row);
                self.last_click = if is_double { None } else { Some((now, col, row)) };
                if is_double {
                    self.double_click(col, row);
                } else {
                    self.mouse_click(col, row);
                }
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
        // 删除确认态下，点击取消确认
        if self.pending_delete.is_some() {
            self.pending_delete = None;
            self.set_toast("已取消删除");
            return;
        }
        let (w, h) = self.screen;
        if w == 0 || h == 0 || row == 0 || row >= h.saturating_sub(1) {
            return; // 状态栏/底部栏点击忽略
        }
        let vis_row = row as usize - 1 + self.tree.scroll;
        if vis_row >= self.tree.visible.len() {
            return;
        }
        let btn_zone_start = w.saturating_sub(BTN_W);
        if col >= btn_zone_start && col < w {
            let off = col - btn_zone_start;
            self.tree.cursor = vis_row;
            // 按钮热区：[操作]5 / 1 / [复制]6 / 1 / [cd]4 / 1 / [终端]6 / 1 / [打开]6 / 1 / [yolo]6 / 1 / [Git]5 = 44
            let btn = match off {
                0..=4 => 0,     // [操作]
                6..=11 => 1,    // [复制]
                13..=16 => 2,   // [cd]
                18..=23 => 3,   // [终端]
                25..=30 => 4,   // [打开]
                32..=37 => 5,   // [yolo]
                39..=43 => 6,   // [Git]
                _ => return,    // 间隔区点击忽略
            };
            self.do_row_button(btn);
            return;
        }
        // 树区点击：目录展开/收缩，文件选中
        self.tree.cursor = vis_row;
        if self.tree.cursor_node().is_dir() {
            self.tree.toggle_cursor();
        } else {
            self.toggle_selected();
        }
    }

    /// 双击处理（仅文件行生效）：
    /// - 文本类文件 → terax 打开（terax 的 single-instance 会自动弹出/置顶窗口）
    /// - 其他文件 → 系统默认应用打开
    /// 第一击已执行过单击动作（移动光标/切换选中），这里对选中做回滚，
    /// 保证"双击打开"不改变选中状态。
    fn double_click(&mut self, col: u16, row: u16) {
        let (w, h) = self.screen;
        if w == 0 || h == 0 || row == 0 || row >= h.saturating_sub(1) {
            return; // 状态栏/底部栏忽略
        }
        if col >= w.saturating_sub(BTN_W) {
            return; // 按钮热区忽略（第一击已触发按钮动作）
        }
        let vis_row = row as usize - 1 + self.tree.scroll;
        if vis_row >= self.tree.visible.len() {
            return;
        }
        self.tree.cursor = vis_row;
        if self.tree.cursor_node().is_dir() {
            return; // 第一击已展开/收缩，无额外动作
        }
        // 回滚第一击的选中切换（toggle 自反）
        self.toggle_selected();
        let path = self.tree.cursor_node().path.clone();
        if is_text_file(&path) {
            self.open_with_terax(&path);
        } else {
            self.open_with_default_app(&path);
        }
    }

    fn do_row_button(&mut self, btn: usize) {
        match btn {
            0 => self.open_action_menu(),
            1 => {
                let p = self.tree.cursor_node().path.clone();
                match self.clipboard.set(&p.to_string_lossy()) {
                    Ok(()) => self.set_toast(format!("已复制路径: {}", p.to_string_lossy())),
                    Err(e) => self.set_toast(format!("复制失败: {e}")),
                }
            }
            2 => self.copy_cd(),
            3 => self.open_terminal(),
            4 => self.open_file_manager(),
            5 => self.open_yolo(),
            6 => self.open_git_menu(),
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
        let cmd = format!("cd {} && cc", shell_quote_path(&dir));
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

    /// 在系统文件管理器中打开并定位到当前文件或文件夹
    #[cfg(target_os = "macos")]
    fn open_file_manager(&mut self) {
        let node = self.tree.cursor_node();
        let path = &node.path;

        // 使用 open -R 在 Finder 中显示并选中文件/文件夹
        let mut cmd = Command::new("open");
        cmd.arg("-R").arg(path);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已在 Finder 中定位 {}", path.to_string_lossy())),
            Err(e) => self.set_toast(format!("打开文件管理器失败: {e}")),
        }
    }

    /// 在系统文件管理器中打开并定位到当前文件或文件夹
    #[cfg(not(target_os = "macos"))]
    fn open_file_manager(&mut self) {
        let node = self.tree.cursor_node();
        let path = &node.path;

        // 尝试使用支持 --select 的文件管理器
        let file_managers = [
            ("nautilus", vec!["--select".to_string(), path.to_string_lossy().to_string()]),
            ("nemo", vec!["--select".to_string(), path.to_string_lossy().to_string()]),
            ("dolphin", vec!["--select".to_string(), path.to_string_lossy().to_string()]),
        ];

        for (manager, args) in &file_managers {
            if Command::new(manager)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
            {
                let mut cmd = Command::new(manager);
                for arg in args {
                    cmd.arg(arg);
                }
                cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                match cmd.spawn() {
                    Ok(_) => {
                        self.set_toast(format!("已在 {} 中定位 {}", manager, path.to_string_lossy()));
                        return;
                    }
                    Err(_) => continue,
                }
            }
        }

        // 回退到 xdg-open（只打开文件夹，无法定位文件）
        let dir = if node.is_dir() {
            path.clone()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

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

    /// 在系统终端中运行当前 .sh 脚本，执行完毕后提示按任意键关闭
    fn run_script(&mut self) {
        let node = self.tree.cursor_node();
        let path = node.path.clone();
        let dir = path.parent().unwrap_or(&path).to_path_buf();
        let Some(term) = self.terminal.clone() else {
            self.set_toast("未检测到可用终端");
            return;
        };
        let escaped = path.to_string_lossy().replace('\'', "'\\''");
        let script = format!(
            "bash '{}'\necho \"\"\necho \"按任意键关闭...\"\nread -n 1",
            escaped
        );
        self.run_in_terminal(&term, &dir, &script, "运行脚本");
    }

    // ---------- 双击打开 ----------

    /// 用 terax 编辑器打开文本文件。
    /// terax 内置 single-instance：已运行时转发文件给现有实例（开新标签并自动
    /// show + set_focus 弹出/置顶窗口）；未运行时直接新建主窗口。
    fn open_with_terax(&mut self, path: &std::path::Path) {
        let Some(bin) = terax_binary() else {
            self.set_toast("未找到 terax，无法打开文件");
            return;
        };
        let mut cmd = Command::new(bin);
        cmd.arg(path);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已用 terax 打开 {}", file_name_display(path))),
            Err(e) => self.set_toast(format!("terax 启动失败: {e}")),
        }
    }

    /// 非文本文件：用系统默认应用打开（Linux: xdg-open）
    #[cfg(not(target_os = "macos"))]
    fn open_with_default_app(&mut self, path: &std::path::Path) {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已用默认应用打开 {}", file_name_display(path))),
            Err(e) => self.set_toast(format!("打开失败: {e}")),
        }
    }

    /// 非文本文件：用系统默认应用打开（macOS: open）
    #[cfg(target_os = "macos")]
    fn open_with_default_app(&mut self, path: &std::path::Path) {
        let mut cmd = Command::new("open");
        cmd.arg(path);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(_) => self.set_toast(format!("已用默认应用打开 {}", file_name_display(path))),
            Err(e) => self.set_toast(format!("打开失败: {e}")),
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

        // 取目录名作为仓库名，清理非法字符（GitHub 仓库名只允许 字母/数字/-/_/.)
        let raw_name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        let repo_name = raw_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches(|c| c == '.' || c == '-')
            .to_string();
        let repo_name = if repo_name.is_empty() {
            "repo".to_string()
        } else {
            repo_name
        };

        // 认证优先级：gh 已认证 → 环境变量 GH_TOKEN → 交互式输入 token
        // push 协议优先级：本地有 SSH key → ssh；否则 → https
        let script = format!(
            r#"echo "=== Git Share on GitHub ==="
echo ""

# 1. 检查 gh CLI
if ! command -v gh &> /dev/null; then
    echo "✗ 未安装 GitHub CLI (gh)"
    echo "  安装: sudo apt install gh"
    echo "  或访问: https://cli.github.com/"
    echo ""
    echo "按 Enter 键退出..."
    read
    exit 1
fi

# 2. 认证检测
if ! gh auth status &> /dev/null; then
    if [ -n "$GH_TOKEN" ]; then
        echo "✓ 使用环境变量 GH_TOKEN 认证"
    else
        echo "gh 未登录，请输入 GitHub Personal Access Token 进行认证"
        echo "  获取 token: https://github.com/settings/tokens/new"
        echo "  需要勾选的 scope: repo, read:org"
        echo ""
        echo -n "请粘贴 token (输入时不回显): "
        read -s TOKEN
        echo ""
        if [ -z "$TOKEN" ]; then
            echo "取消操作"
            echo "按 Enter 键退出..."
            read
            exit 1
        fi
        echo "$TOKEN" | gh auth login --with-token > /dev/null 2>&1
        if [ $? -ne 0 ]; then
            echo "✗ 认证失败，请检查 token 是否正确"
            echo "按 Enter 键退出..."
            read
            exit 1
        fi
        echo "✓ 认证成功"
    fi
else
    echo "✓ gh 已认证"
fi

# 3. 检测 SSH key，决定 push 协议
if ls ~/.ssh/id_* 1> /dev/null 2>&1; then
    gh config set git_protocol ssh > /dev/null 2>&1
    echo "✓ 检测到 SSH key，使用 ssh 协议 push"
else
    gh config set git_protocol https > /dev/null 2>&1
    echo "  未检测到 SSH key，使用 https 协议 push"
fi

# 4. 初始化 git
git init -q
echo "✓ 已初始化 git 仓库"

# 5. 首次 commit
git add .
git commit -q -m "first commit"
echo "✓ 已提交"

# 6. 创建 GitHub 仓库并 push
echo ""
echo "正在创建 GitHub 仓库: {repo_name} (private)..."
gh repo create "{repo_name}" --private --source=. --push

if [ $? -eq 0 ]; then
    REPO_URL=$(gh repo view --json url -q .url 2>/dev/null)
    echo ""
    echo "✓ 创建成功: ${{REPO_URL:-{repo_name}}}"
else
    echo ""
    echo "✗ 创建失败，请检查上方错误信息"
fi

echo ""
echo "按 Enter 键退出..."
read
"#
        );

        self.run_in_terminal(&term, &dir, &script, "Git Share");
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
echo "完成！按 Enter 键退出..."
read
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
echo "完成！按 Enter 键退出..."
read
"#;

        self.run_in_terminal(&term, &dir, script, "Git Push");
    }

    /// 在系统终端中执行脚本
    fn run_in_terminal(&mut self, term: &TermCmd, dir: &std::path::Path, script: &str, label: &str) {
        let mut cmd = Command::new(&term.name);
        match term.kind {
            TermKind::Gnome => {
                cmd.arg("--working-directory").arg(dir);
                // 使用 -e + 单字符串，与 open_yolo 保持一致（-- 多参数方式在部分版本不生效）
                cmd.arg("-e").arg(format!("bash -c '{script}'",
                    script = script.replace('\'', "'\\''")));
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
                // macOS Terminal.app: 将脚本写入临时文件，然后执行
                use std::io::Write;
                let temp_dir = std::env::temp_dir();
                let script_file = temp_dir.join(format!("ftree-git-{}.sh", std::process::id()));

                // 写入脚本到临时文件
                if let Ok(mut file) = std::fs::File::create(&script_file) {
                    let _ = file.write_all(b"#!/bin/bash\n");
                    let _ = file.write_all(format!("cd '{}'\n", dir.to_string_lossy().replace('\'', "'\\''")).as_bytes());
                    let _ = file.write_all(script.as_bytes());
                    drop(file);

                    // 设置执行权限
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755));
                    }

                    let script_path = script_file.to_string_lossy().replace('"', "\\\"");

                    cmd.arg("-e")
                       .arg("tell application \"Terminal\"")
                       .arg("-e")
                       .arg("activate")
                       .arg("-e")
                       .arg(format!("do script \"{}; rm -f '{}'; exit\"", script_path, script_path))
                       .arg("-e")
                       .arg("end tell");
                } else {
                    self.set_toast("无法创建临时脚本文件");
                    return;
                }
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

    // ---------- 操作菜单 ----------

    /// 删除当前光标所指的文件或文件夹：进入二次确认状态。
    fn delete_current(&mut self) {
        let node = self.tree.cursor_node();
        let path = node.path.clone();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        // 不允许删除根目录
        if path == self.tree.root.path {
            self.set_toast("无法删除根目录");
            return;
        }

        // 计算父目录的索引链（当前行的 chain 去掉最后一项）
        let cursor_chain = self.tree.visible.get(self.tree.cursor).cloned().unwrap_or_default();
        let parent_chain = if cursor_chain.is_empty() {
            Vec::new()
        } else {
            cursor_chain[..cursor_chain.len() - 1].to_vec()
        };

        self.pending_delete = Some((path, name, parent_chain));
    }

    /// 确认删除：执行实际删除，局部刷新父目录，保留其他展开/收缩状态。
    fn confirm_delete(&mut self) {
        let (path, name, parent_chain) = match self.pending_delete.take() {
            Some(v) => v,
            None => return,
        };
        let is_dir = path.is_dir();
        let result = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => {
                self.set_toast(format!("已删除: {name}"));
                self.selected.retain(|s| !s.starts_with(&path));
                self.tree.refresh_node(&parent_chain);
                if self.tree.visible.is_empty() {
                    self.tree.cursor = 0;
                } else if self.tree.cursor >= self.tree.visible.len() {
                    self.tree.cursor = self.tree.visible.len() - 1;
                }
            }
            Err(e) => {
                self.set_toast(format!("删除失败: {e}"));
            }
        }
    }

    /// 操作菜单选项列表（后续新增功能直接加到这里）
    pub fn action_menu_options() -> Vec<&'static str> {
        vec![
            "1. 运行脚本",
            "2. 删除",
        ]
    }

    fn open_action_menu(&mut self) {
        self.action_menu_index = 0;
        self.mode = Mode::ActionMenu;
    }

    fn action_menu_apply(&mut self) {
        match self.action_menu_index {
            0 => self.run_script(),
            1 => self.delete_current(),
            _ => {}
        }
        self.mode = Mode::Browse;
    }

    fn action_menu_mouse_click(&mut self, col: u16, row: u16) {
        let (w, h) = self.screen;
        if w == 0 || h == 0 {
            return;
        }

        let options = Self::action_menu_options();
        let menu_w = 40u16.min(w.saturating_sub(2));
        let menu_h = (options.len() as u16 + 4).min(h.saturating_sub(4));
        let menu_x = w / 2 - menu_w / 2;
        let menu_y = h / 2 - menu_h / 2;

        if col >= menu_x && col < menu_x + menu_w && row >= menu_y && row < menu_y + menu_h {
            let item_row = row.saturating_sub(menu_y + 1);
            if (item_row as usize) < options.len() {
                self.action_menu_index = item_row as usize;
                self.action_menu_apply();
            }
        } else {
            self.mode = Mode::Browse;
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

/// 文件名（用于 toast 显示）
fn file_name_display(path: &std::path::Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// 在 PATH 中查找可执行文件
fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// 查找 terax 可执行文件（Linux：PATH）
#[cfg(not(target_os = "macos"))]
fn terax_binary() -> Option<String> {
    find_in_path("terax").map(|p| p.to_string_lossy().into_owned())
}

/// 查找 terax 可执行文件（macOS：PATH → .app 内置二进制）
#[cfg(target_os = "macos")]
fn terax_binary() -> Option<String> {
    if let Some(p) = find_in_path("terax") {
        return Some(p.to_string_lossy().into_owned());
    }
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join("Applications/Terax.app/Contents/MacOS/terax"));
    }
    candidates.push(PathBuf::from("/Applications/Terax.app/Contents/MacOS/terax"));
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

/// 常见二进制后缀：直接判否，不做内容探测
const BINARY_EXTS: &[&str] = &[
    // 视频/音频（注意：不能收录 ts——与 TypeScript 冲突，文本优先）
    "mp4", "mkv", "avi", "mov", "flv", "wmv", "webm", "m4v", "mpg", "mpeg",
    "mp3", "wav", "flac", "aac", "ogg", "m4a", "wma",
    // 图片/字体
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif", "heic", "avif",
    "ttf", "otf", "woff", "woff2",
    // 文档/压缩/可执行
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "zip", "tar", "gz", "bz2", "xz", "zst", "7z", "rar", "deb", "rpm", "dmg", "iso",
    "exe", "dll", "so", "dylib", "bin", "class", "jar", "apk", "aab", "dex", "o", "a",
    "wasm", "pyc", "sqlite", "db",
];

/// 文本类文件白名单（编程语言 + 标记/配置 + 纯文本）
const TEXT_EXTS: &[&str] = &[
    // 编程语言
    "rs", "py", "js", "mjs", "cjs", "jsx", "ts", "tsx", "go", "java", "kt", "kts",
    "c", "h", "cpp", "hpp", "cc", "cxx", "cs", "rb", "php", "swift", "dart",
    "sh", "bash", "zsh", "fish", "ps1", "lua", "pl", "r", "scala", "groovy",
    "vue", "svelte",
    // 标记/配置（含 markdown）
    "md", "markdown", "rst", "html", "htm", "css", "scss", "less", "json", "jsonc",
    "toml", "yaml", "yml", "xml", "ini", "cfg", "conf", "properties", "env",
    "sql", "graphql", "proto", "dockerfile",
    // 纯文本/日志
    "txt", "log", "csv", "tsv", "lock",
];

/// 无扩展名时按文件名识别的常见文本文件（小写比较）
const TEXT_NAMES: &[&str] = &[
    "makefile", "dockerfile", "license", "readme", "changelog", "authors", "notice",
    ".gitignore", ".gitattributes", ".gitmodules", ".env", ".editorconfig",
];

/// 判断是否文本类文件（双击时用 terax 打开）：
/// ① 扩展名白名单（编程语言/标记配置/markdown/纯文本）
/// ② 无扩展名的常见文件名（Makefile、.gitignore 等）
/// ③ 常见二进制后缀直接排除
/// ④ 兜底：读首 8KB，无 NUL 字节视为文本（UTF-16 BOM 特判）
fn is_text_file(path: &std::path::Path) -> bool {
    let fname = path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if TEXT_NAMES.contains(&fname.as_str()) {
        return true;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_l = ext.to_lowercase();
        if TEXT_EXTS.contains(&ext_l.as_str()) {
            return true;
        }
        if BINARY_EXTS.contains(&ext_l.as_str()) {
            return false;
        }
    }
    looks_like_text(path)
}

/// 内容探测：读首 8KB，无 NUL 字节视为文本；空文件视为文本。
fn looks_like_text(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 8192];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    let data = &buf[..n];
    // UTF-16 BOM（内容含大量 NUL，但确实是文本）
    if data.len() >= 2 && (data[..2] == [0xFF, 0xFE] || data[..2] == [0xFE, 0xFF]) {
        return true;
    }
    !data.contains(&0)
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

    /// 模拟一次左键按下
    fn click(app: &mut App, col: u16, row: u16) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
    }

    #[test]
    fn single_click_file_toggles_selection() {
        let (_d, mut app) = fixture();
        app.screen = (80, 24);
        // 可见行: root(0), a.mp4(1), b.mp4(2)；屏幕行 = vis_row + 1
        assert!(app.selected.is_empty());
        click(&mut app, 2, 2); // 单击 a.mp4
        assert_eq!(app.selected.len(), 1);
    }

    #[test]
    fn double_click_dir_keeps_expanded() {
        let (d, mut app) = fixture();
        let sub = d.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"hi").unwrap();
        app.tree.refresh();
        app.screen = (80, 24);
        // 可见行: root, sub, a.mp4, b.mp4
        assert_eq!(app.tree.visible.len(), 4);
        click(&mut app, 2, 2); // 第一击：展开 sub
        assert_eq!(app.tree.visible.len(), 5);
        click(&mut app, 2, 2); // 300ms 内第二击 → 双击：目录行无额外动作
        assert_eq!(app.tree.visible.len(), 5, "双击不应把目录收缩回去");
    }

    #[test]
    fn text_file_detection() {
        let d = std::env::temp_dir().join(format!(
            "ftree-test-text-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("main.rs"), b"fn main() {}").unwrap();
        std::fs::write(d.join("app.ts"), b"export {}").unwrap();
        std::fs::write(d.join("README.md"), b"# hi").unwrap();
        std::fs::write(d.join("Makefile"), b"all:").unwrap();
        std::fs::write(d.join(".gitignore"), b"target").unwrap();
        std::fs::write(d.join("data.unkext"), b"plain text").unwrap();
        std::fs::write(d.join("blob.unkext"), b"\x00\x01\x02\x03").unwrap();

        // 编程语言（含 ts——不得与 MPEG-TS 混淆）/ markdown / 无扩展名文本文件
        assert!(is_text_file(&d.join("main.rs")));
        assert!(is_text_file(&d.join("app.ts")));
        assert!(is_text_file(&d.join("README.md")));
        assert!(is_text_file(&d.join("Makefile")));
        assert!(is_text_file(&d.join(".gitignore")));
        // 未知扩展名走内容探测
        assert!(is_text_file(&d.join("data.unkext")));
        assert!(!is_text_file(&d.join("blob.unkext")), "含 NUL 应判为二进制");
        // 二进制后缀直接排除
        assert!(!is_text_file(&d.join("video.mp4")));
        assert!(!is_text_file(&d.join("photo.png")));
    }
}
