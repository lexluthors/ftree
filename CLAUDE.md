# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

ftree is a terminal interactive file tree tool written in Rust. It provides a TUI for browsing files with mouse/keyboard support, multi-select, path copying, and command template concatenation.

## Commands

```bash
# Build
cargo build --release

# Run tests
cargo test

# Development run
cargo run

# Run with debug event logging
FTREE_EVENT_LOG=1 cargo run
```

Binary installs to `~/.local/bin/ftree` with alias `ff`.

## Architecture

The application follows a classic TUI event loop pattern:

### Core Components

- **main.rs**: Entry point, terminal setup (crossterm/ratatui), argument parsing. Uses `TuiGuard` to ensure terminal state restoration on exit/panic.

- **app.rs**: Central state machine with two modes:
  - `Mode::Browse`: Normal tree navigation
  - `Mode::Picker`: Template selection modal
  
  Handles keyboard/mouse events, clipboard operations, and external terminal/file manager launching. The event loop drains crossterm's queue with 1ms poll to work around an event loss bug in crossterm 0.28.

- **tree.rs**: Directory tree with lazy loading. Key design:
  - `Node`: Tree node with lazy `loaded` flag and `children` vector
  - `Tree`: Maintains `visible` vector of index chains (paths from root) for efficient rendering
  - `rebuild()`: Flattens expanded tree into visible rows after any structural change
  - Directories sorted before files, both alphabetically

- **watcher.rs**: File system watcher for automatic tree refresh.
  - Uses `notify` crate with OS-native backends: **FSEvents** (macOS) / **inotify** (Linux)
  - Runs in background thread, sends events via `mpsc::channel`
  - Only monitors Create/Remove/Modify events (ignores metadata-only changes)
  - Debounces events (300ms quiet period) to avoid excessive refreshes
  - `Tree::refresh()` only reloads expanded directories — efficient for large trees

- **ui.rs**: Rendering with ratatui. Layout:
  - Top: Status bar (root path, selection count, hidden file toggle)
  - Middle: Tree rows with right-side button hotzones `[复制] [cd] [终端] [打开]` (total width `BTN_W = 25`)
  - Bottom: Toast notifications or keybinding hints
  - Modal: Picker overlay for template selection

- **templates.rs**: Command template rendering with placeholders:
  - `{files}` / `{files_quoted}` / `{names}` / `{dir}` / `{n}`
  - `common_parent()`: Finds longest common parent directory for selected files
  - `quote()`: POSIX shell quoting with single-quote escaping

- **config.rs**: Loads templates from `~/.config/ftree/templates.toml`, writes defaults on first run.

- **clipboard.rs**: Detects X11 (`xclip`), Wayland (`wl-copy`), or macOS (`pbcopy`), spawns clipboard process and writes to stdin. Process stays alive to own the clipboard selection.

### Platform Support

- **Linux**: xclip (X11), wl-copy (Wayland), xdg-open, gnome-terminal/konsole/alacritty/kitty/xterm
- **macOS**: pbcopy, open (Finder), Terminal.app, iTerm2 (auto-detected)

### Key Design Decisions

1. **Lazy loading**: Directories only read when expanded, enabling handling of large directory trees
2. **Visible index chains**: `Tree.visible` stores `Vec<Vec<usize>>` paths rather than node pointers, allowing efficient rebuilds without tree restructuring
3. **Mouse hit zones**: Button areas calculated from right edge (`BTN_W` constant), with 1-column gaps between buttons
4. **Toast auto-dismiss**: 4-second timeout checked in `tick()` called when no events pending
5. **Terminal detection**: Probes `$TERMINAL` env var first, then tries common terminals with `--version`
6. **Auto-refresh with OS events**: Uses `notify` crate (FSEvents/inotify) for zero-overhead file watching. Events are debounced (300ms) and only trigger refresh of expanded directories. Idle CPU usage: ~0%

## Testing

Tests create unique temp directories per test (using atomic counters or nanosecond timestamps) to avoid conflicts when run in parallel.

Clipboard tests require `xclip` and use a `CLIP_LOCK` mutex since X11 clipboard is a shared resource. Tests poll clipboard content with timeout since it's updated asynchronously.

## Configuration

User-editable config at `~/.config/ftree/templates.toml`:

```toml
[[templates]]
name = "ffmpeg 转码 h264"
command = "ffmpeg -i {files_quoted} -c:v libx264 -c:a aac {dir}/out.mp4"
```

Defaults are written on first run if the file doesn't exist.

## Dependencies

- **ratatui 0.29**: TUI framework
- **crossterm 0.28**: Terminal manipulation and event handling
- **serde + toml**: Configuration parsing

## Platform Notes

使用 `#[cfg(target_os = "macos")]` 条件编译处理跨平台差异：
- **剪贴板**：macOS 用 `pbcopy`/`pbpaste`，Linux 用 `xclip`/`wl-copy`
- **文件管理器**：macOS 用 `open`，Linux 用 `xdg-open`
- **终端检测**：macOS 用 `osascript` 检测 iTerm2/Terminal.app，Linux 检测各终端命令
- 测试中的剪贴板读取函数也按平台分别实现

## 开发规范（强制）

### 跨平台支持
所有新增功能必须同时支持 **Linux** 和 **macOS** 两个平台。

### 终端优先级
优先使用系统自带的终端：
- **macOS**: Terminal.app（优先） > iTerm2
- **Linux**: 检测 `$TERMINAL` 环境变量，若未设置则按优先级尝试：gnome-terminal > konsole > alacritty > kitty > xterm

实现时必须使用条件编译 `#[cfg(target_os = "...")]` 处理平台差异。
