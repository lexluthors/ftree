# ftree - 终端交互式文件树工具

一个在终端中运行的交互式文件浏览器，支持键盘和鼠标操作，可以快速复制路径、拼接命令行、打开终端和文件管理器。

## 特性

- 📁 **目录树浏览** - 懒加载，支持大目录
- 🖱️ **完整鼠标支持** - 点击展开/收缩、选中、行右侧按钮
- ⌨️ **丰富键盘操作** - vim 风格快捷键
- 📋 **多选复制** - 选中多个文件后一次性复制路径
- 🔧 **命令拼接** - 使用模板生成 ffmpeg/cat/tar/scp 等命令
- 🖥️ **快速打开** - 一键打开终端或系统文件管理器
- ⚙️ **可配置模板** - 自定义命令模板（~/.config/ftree/templates.toml）

## 安装

已安装到 `~/.local/bin/ftree`，同时有快捷命令 `ff`：

```bash
# 两种方式都可以启动
ff [目录]
ftree [目录]
```

## 使用方法

### 启动

```bash
ff              # 当前目录
ff ~/projects   # 指定目录
ff --hidden     # 显示隐藏文件
```

### 界面布局

```
~/lexWorkSpace/                        ← 顶部状态栏
▼ lexWorkSpace/                [复制] [cd] [终端] [打开]
  ├─ docs/                     [复制] [cd] [终端] [打开]
  ├─ video/a.mp4               [复制] [cd] [终端] [打开]
  └─ install.sh                [复制] [cd] [终端] [打开]
──────────────────────────────────────────────────────
↑/↓ 移动  [空格]选中  [c]复制  [C]拼接命令  [t]隐藏  [q]退出
```

### 键盘操作

| 按键 | 功能 |
|------|------|
| `↑` `↓` / `j` `k` | 移动光标 |
| `Enter` / `→` | 展开目录 |
| `←` / `Backspace` | 收缩目录或返回上级 |
| `空格` | 选中/取消选中文件 |
| `c` | 复制路径（多选时复制所有选中项，否则复制当前项） |
| `d` | 复制 `cd <当前目录>` 命令 |
| `C` | 打开模板选择器，拼接命令 |
| `t` | 显示/隐藏隐藏文件 |
| `o` | 在当前目录打开系统终端 |
| `g` / `G` | 跳到顶部/底部 |
| `q` / `Esc` | 退出 |

### 鼠标操作

| 操作 | 功能 |
|------|------|
| 点击目录行 | 展开/收缩 |
| 点击文件行 | 选中/取消选中 |
| 滚轮 | 上下滚动 |
| 点击 `[复制]` 按钮 | 复制该行路径 |
| 点击 `[cd]` 按钮 | 复制 `cd <该行所在目录>` 命令 |
| 点击 `[终端]` 按钮 | 在该目录打开系统终端 |
| 点击 `[打开]` 按钮 | 在系统文件管理器中打开该目录 |

## 功能详解

### 复制路径

- **键盘 `c`**：复制当前行路径（多选时复制所有选中项，每行一个）
- **鼠标 `[复制]`**：复制当前行路径

### 复制 cd 命令

- **键盘 `d`** 或 **鼠标 `[cd]`**：复制 `cd <路径>` 命令
- 路径包含空格时自动添加引号
- 示例：`cd /home/lex/projects/my-app`

### 打开终端

- **键盘 `o`** 或 **鼠标 `[终端]`**：在当前目录打开系统终端
- 自动检测可用终端：gnome-terminal、konsole、alacritty、kitty、xterm

### 打开文件管理器

- **鼠标 `[打开]`**：在系统文件管理器中打开该目录
- 使用 `xdg-open` 调用系统默认文件管理器

### 命令拼接

按 `C` 打开模板选择器，选择模板后自动生成命令：

```bash
# 选中多个视频文件后选择 "ffmpeg 合并" 模板
# 生成：
ffmpeg -i /path/video1.mp4 /path/video2.mp4 -c copy /path/output.mp4

# 选中多个文件后选择 "tar 打包" 模板
# 生成：
tar -czf archive.tar.gz file1 file2 file3
```

#### 模板占位符

- `{files}` - 完整路径，空格分隔
- `{files_quoted}` - 带引号的路径（推荐）
- `{names}` - 仅文件名
- `{dir}` - 选中文件的公共父目录
- `{n}` - 文件数量

## 配置

模板配置文件位于 `~/.config/ftree/templates.toml`，首次运行自动生成：

```toml
[[templates]]
name = "ffmpeg 转码 h264"
command = "ffmpeg -i {files_quoted} -c:v libx264 -c:a aac {dir}/out.mp4"

[[templates]]
name = "cat 合并"
command = "cat {files} > {dir}/merged.bin"

[[templates]]
name = "tar 打包"
command = "tar -czf {dir}/archive.tar.gz {files}"

[[templates]]
name = "scp 上传"
command = "scp {files} user@host:/remote/path/"

[[templates]]
name = "git add"
command = "git add {files}"
```

可以自定义任意模板，格式同上。

## 调试

设置环境变量 `FTREE_EVENT_LOG=1` 可以查看按键和鼠标事件日志：

```bash
FTREE_EVENT_LOG=1 ff
```

## 技术细节

- **语言**：Rust
- **TUI 框架**：ratatui 0.29 + crossterm 0.28
- **剪贴板**：X11 使用 xclip，Wayland 使用 wl-copy
- **性能**：懒加载目录，支持数千个文件的目录

### 已知问题与解决

- **crossterm 0.28 事件丢失**：快速批量按键时可能丢失事件，已通过在事件排空循环中使用 `poll(1ms)` 修复
- **0×0 终端渲染**：极小终端窗口可能导致渲染 panic，已添加防御性检查

## 开发

```bash
# 编译
cargo build --release

# 运行测试
cargo test

# 开发运行
cargo run
```

## 许可证

MIT
