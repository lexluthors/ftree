use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use crate::app::{App, Mode};
use crate::tree::NodeKind;

/// 右侧按钮区总宽（逻辑列）：[复制]6 + 间隔1 + [cd]4 + 间隔1 + [终端]6 + 间隔1 + [打开]6 + 间隔1 + [yolo]6 + 间隔1 + [Git]5 = 38
pub const BTN_W: u16 = 38;
pub const BTN0_LEN: u16 = 6;
pub const BTN1_LEN: u16 = 4;
pub const BTN2_LEN: u16 = 6;
pub const BTN3_LEN: u16 = 6;
pub const BTN4_LEN: u16 = 6;
pub const BTN5_LEN: u16 = 5;

const BTN0: &str = "[复制]";
const BTN1: &str = "[cd]";
const BTN2: &str = "[终端]";
const BTN3: &str = "[打开]";
const BTN4: &str = "[yolo]";
const BTN5: &str = "[Git]";

const TOAST_SECS: u64 = 4;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // 终端过小（含 0×0，如 script/管道 pty）：什么都不渲染。
    // 此时缓冲区可能没有可用单元，任何写入都会越界 panic。
    if area.width < BTN_W + 10 || area.height < 4 {
        return;
    }
    app.screen = (area.width, area.height);
    let buf = f.buffer_mut();
    let bottom_y = area.y + area.height - 1;

    // ---- 顶部状态栏 ----
    let sel_n = app.selected.len();
    let status = format!(
        " ftree — {}    已选: {}    隐藏文件: {} ",
        app.root_display,
        sel_n,
        if app.tree.show_hidden { "显示" } else { "隐藏" }
    );
    buf.set_stringn(
        0,
        area.y,
        &status,
        area.width as usize,
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    );

    // ---- 树区 ----
    let view_h = (area.height - 2) as usize;
    app.tree.ensure_visible(view_h);
    let rows = app.tree.visible.len();
    let btn_x = area.width.saturating_sub(BTN_W);
    let text_w = btn_x.saturating_sub(1) as usize; // 树文本与按钮之间留 1 格间隔

    for vi in 0..view_h {
        let row = app.tree.scroll + vi;
        if row >= rows {
            break;
        }
        let y = area.y + 1 + vi as u16;
        if y >= bottom_y {
            break;
        }
        render_row(buf, app, row, y, text_w, btn_x);
    }

    // ---- 底部状态栏（toast 或快捷键提示） ----
    match &app.toast {
        Some((msg, ts)) if ts.elapsed().as_secs() < TOAST_SECS => {
            buf.set_stringn(
                0,
                bottom_y,
                &format!(" {msg}"),
                area.width as usize,
                Style::default().fg(Color::Yellow),
            );
        }
        _ => {
            buf.set_stringn(
                0,
                bottom_y,
                " ↑/↓ 移动  空格 选中  Enter 展开  r 刷新  c 复制  C 模板  d cd  o 终端  t 切换隐藏  q 退出 ",
                area.width as usize,
                Style::default().fg(Color::DarkGray),
            );
        }
    }

    // ---- 模板选择弹层 ----
    if app.mode == Mode::Picker {
        draw_picker(buf, app, area);
    }

    // ---- Git 操作菜单弹层 ----
    if app.mode == Mode::GitMenu {
        draw_git_menu(buf, app, area);
    }
}

fn render_row(buf: &mut Buffer, app: &mut App, row: usize, y: u16, text_w: usize, btn_x: u16) {
    let chain = app.tree.visible[row].clone();
    let node = app.tree.node_at(&chain);
    let depth = chain.len(); // 根 = 0
    let is_dir = node.is_dir();

    // 行前缀：缩进 + 展开符号
    let symbol = if is_dir {
        if node.expanded { "▼ " } else { "► " }
    } else {
        "  "
    };
    let prefix = format!("{}{}", "  ".repeat(depth), symbol);

    let name_display = if depth == 0 {
        app.root_display.clone() // 根行显示完整路径
    } else {
        node.name.clone()
    };
    let sel = if app.selected.iter().any(|p| p == &node.path) {
        " [✓]"
    } else {
        ""
    };
    let mut text = format!("{}{}{}{}", prefix, name_display, if is_dir { "/" } else { "" }, sel);
    if unicode_width(&text) > text_w {
        text = truncate_mid(&text, text_w);
    }

    // 行样式
    let mut style = Style::default();
    let is_cursor = row == app.tree.cursor;
    let hovered = app
        .hover
        .map(|(c, r)| r == y && c < btn_x)
        .unwrap_or(false);
    if is_cursor {
        style = style.bg(Color::Blue).fg(Color::White);
    } else if hovered {
        style = style.add_modifier(Modifier::BOLD);
    }
    if !is_cursor {
        if is_dir {
            style = style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
        } else if node.name.starts_with('.') {
            style = style.fg(Color::DarkGray);
        }
    }

    buf.set_stringn(0, y, &text, text_w, style);

    // 行右侧按钮
    let btn_style = if is_cursor || hovered {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    buf.set_stringn(btn_x, y, BTN0, BTN0_LEN as usize, btn_style);
    buf.set_stringn(btn_x + BTN0_LEN + 1, y, BTN1, BTN1_LEN as usize, btn_style);
    buf.set_stringn(
        btn_x + BTN0_LEN + 1 + BTN1_LEN + 1,
        y,
        BTN2,
        BTN2_LEN as usize,
        btn_style,
    );
    buf.set_stringn(
        btn_x + BTN0_LEN + 1 + BTN1_LEN + 1 + BTN2_LEN + 1,
        y,
        BTN3,
        BTN3_LEN as usize,
        btn_style,
    );
    buf.set_stringn(
        btn_x + BTN0_LEN + 1 + BTN1_LEN + 1 + BTN2_LEN + 1 + BTN3_LEN + 1,
        y,
        BTN4,
        BTN4_LEN as usize,
        btn_style,
    );
    buf.set_stringn(
        btn_x + BTN0_LEN + 1 + BTN1_LEN + 1 + BTN2_LEN + 1 + BTN3_LEN + 1 + BTN4_LEN + 1,
        y,
        BTN5,
        BTN5_LEN as usize,
        btn_style,
    );
}

fn draw_picker(buf: &mut Buffer, app: &App, area: Rect) {
    let w = (area.width * 2 / 3).max(50).min(area.width - 2);
    let h = ((app.templates.len() + 5) as u16).min(area.height.saturating_sub(4));
    let x = area.x + area.width / 2 - w / 2;
    let y = area.y + area.height / 2 - h / 2;

    // 清空弹层区域
    for yy in y..y + h {
        for xx in x..x + w {
            buf[(xx, yy)].reset();
        }
    }

    // 边框
    let hline = "─".repeat((w - 2) as usize);
    let style = Style::default().fg(Color::Cyan);
    buf.set_string(x, y, &format!("┌{hline}┐"), style);
    buf.set_string(x, y + h - 1, &format!("└{hline}┘"), style);
    for yy in (y + 1)..(y + h - 1) {
        let cell = buf.cell_mut((x, yy)).unwrap();
        cell.set_symbol("│").set_style(style);
        let cell = buf.cell_mut((x + w - 1, yy)).unwrap();
        cell.set_symbol("│").set_style(style);
    }

    // 标题
    let title = format!(
        " 拼接模板 — 已选 {} 个文件 ",
        app.selected.len()
    );
    let tx = x + ((w as usize).saturating_sub(title_width(&title)) / 2) as u16;
    buf.set_stringn(tx, y, &title, w as usize, Style::default().add_modifier(Modifier::BOLD));

    // 模板列表
    for (i, t) in app.templates.iter().enumerate() {
        if i as u16 >= h - 4 {
            break;
        }
        let (sym, is_cur) = if i == app.picker_index {
            ("▶ ", true)
        } else {
            ("  ", false)
        };
        let text = format!("{}{}", sym, t.name);
        let mut st = Style::default();
        if is_cur {
            st = st.bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(x + 1, y + 1 + i as u16, &text, (w - 2) as usize, st);
    }

    // 命令预览
    let preview = app.picker_preview();
    let py = y + (app.templates.len() as u16).min(h - 4) + 1;
    if (app.templates.len() as u16) < h.saturating_sub(4) {
        buf.set_stringn(
            x + 1,
            py,
            "预览:",
            (w - 2) as usize,
            Style::default().fg(Color::DarkGray),
        );
        let mut pv = preview;
        if unicode_width(&pv) > w as usize - 2 {
            pv = truncate_mid(&pv, w as usize - 2);
        }
        buf.set_stringn(
            x + 1,
            py + 1,
            &pv,
            (w - 2) as usize,
            Style::default().fg(Color::Yellow),
        );
    }

    // 底部提示
    buf.set_stringn(
        x + 1,
        y + h - 2,
        "↑/↓ 选择  Enter 复制并关闭  Esc 返回",
        (w - 2) as usize,
        Style::default().fg(Color::DarkGray),
    );
}

fn draw_git_menu(buf: &mut Buffer, app: &App, area: Rect) {
    let w = 40u16.min(area.width - 2);
    let h = 10u16.min(area.height.saturating_sub(4));
    let x = area.x + area.width / 2 - w / 2;
    let y = area.y + area.height / 2 - h / 2;

    // 清空弹层区域
    for yy in y..y + h {
        for xx in x..x + w {
            buf[(xx, yy)].reset();
        }
    }

    // 边框
    let hline = "─".repeat((w - 2) as usize);
    let style = Style::default().fg(Color::Cyan);
    buf.set_string(x, y, &format!("┌{hline}┐"), style);
    buf.set_string(x, y + h - 1, &format!("└{hline}┘"), style);
    for yy in (y + 1)..(y + h - 1) {
        let cell = buf.cell_mut((x, yy)).unwrap();
        cell.set_symbol("│").set_style(style);
        let cell = buf.cell_mut((x + w - 1, yy)).unwrap();
        cell.set_symbol("│").set_style(style);
    }

    // 标题
    let title = " Git 操作 ";
    let tx = x + ((w as usize).saturating_sub(title.len() * 2) / 2) as u16;
    buf.set_stringn(tx, y, title, w as usize, Style::default().add_modifier(Modifier::BOLD));

    // 菜单选项
    let options = [
        "1. Share on GitHub",
        "2. git pull",
        "3. git commit",
        "4. git push",
    ];

    for (i, opt) in options.iter().enumerate() {
        if i as u16 >= h - 4 {
            break;
        }
        let (sym, is_cur) = if i == app.git_menu_index {
            ("▶ ", true)
        } else {
            ("  ", false)
        };
        let text = format!("{}{}", sym, opt);
        let mut st = Style::default();
        if is_cur {
            st = st.bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(x + 1, y + 1 + i as u16, &text, (w - 2) as usize, st);
    }

    // 底部提示
    buf.set_stringn(
        x + 1,
        y + h - 2,
        "↑/↓ 选择  Enter 执行  Esc 返回",
        (w - 2) as usize,
        Style::default().fg(Color::DarkGray),
    );
}

fn title_width(s: &str) -> usize {
    unicode_width(s)
}

/// 简易 unicode 显示宽度：ASCII 1 格，其余按 2 格。
fn unicode_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// 中间截断：保留前/后半段，中间用省略号。
fn truncate_mid(s: &str, max_w: usize) -> String {
    let half = (max_w / 2).saturating_sub(1);
    let mut front = String::new();
    let mut fl = 0;
    for ch in s.chars().take(80) {
        let w = if ch.is_ascii() { 1 } else { 2 };
        if fl + w > half {
            break;
        }
        front.push(ch);
        fl += w;
    }
    let mut back = String::new();
    let mut bl = 0;
    for ch in s.chars().rev().take(80) {
        let w = if ch.is_ascii() { 1 } else { 2 };
        if bl + w > half {
            break;
        }
        back.push(ch);
        bl += w;
    }
    format!("{front}…{back}")
}

#[allow(dead_code)]
fn _nodekind_import_hint(_: NodeKind) {}