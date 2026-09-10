#!/bin/bash
# ftree 启动诊断脚本
# 用法：在真实终端中运行 ./diagnose_ftree.sh

set -e

echo "=== ftree 启动诊断 ==="
echo "时间: $(date)"
echo "当前目录: $(pwd)"
echo "终端类型: $TERM"
echo "ftree 路径: $(which ff)"
echo ""

# 检查是否已安装
if ! command -v ff &> /dev/null; then
    echo "❌ ftree 未安装"
    exit 1
fi

echo "✓ ftree 已安装"
echo ""

# 测试 1: 检查 stdout 是否是终端
echo "=== 测试 1: 检查终端环境 ==="
if [ -t 1 ]; then
    echo "✓ stdout 是终端"
else
    echo "❌ stdout 不是终端，请在真实终端中运行"
    exit 1
fi
echo ""

# 测试 2: 检查基本命令
echo "=== 测试 2: 检查依赖命令 ==="
for cmd in xclip xdotool tilix; do
    if command -v $cmd &> /dev/null; then
        echo "✓ $cmd: $(which $cmd)"
    else
        echo "⚠️  $cmd: 未安装"
    fi
done
echo ""

# 测试 3: 在 /tmp 目录测试（避免大量文件）
echo "=== 测试 3: 在 /tmp 目录测试 ==="
cd /tmp
echo "切换到 /tmp 目录"

# 使用 strace 跟踪启动过程
echo "使用 strace 跟踪启动（5秒后超时）..."
timeout 5 strace -e trace=openat,read,write,ioctl,clone -f ff 2>&1 | \
    grep -E "(卡住|退出|错误|enable_raw|EnterAlternate|Terminal::new|App::new)" | \
    tail -20 || true

echo ""
echo "=== 诊断完成 ==="
echo ""
echo "如果程序卡住，请按 Ctrl+C 退出"
echo "如果看到 'enable_raw_mode' 或 'EnterAlternateScreen' 相关的错误，"
echo "说明是终端兼容性问题。"
