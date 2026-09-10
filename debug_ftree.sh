#!/bin/bash
# ftree 启动调试脚本

echo "=== ftree 启动调试 ==="
echo "时间: $(date)"
echo "终端类型: $TERM"
echo "当前目录: $(pwd)"
echo "ftree 路径: $(which ftree)"
echo ""

# 检查是否在真实终端
if [ ! -t 1 ]; then
    echo "⚠️  警告: stdout 不是终端"
    echo "请在真实终端（如 Tilix、GNOME Terminal）中运行此脚本"
    exit 1
fi

echo "✓ stdout 是终端"
echo ""

# 测试 1: 直接运行 ftree
echo "=== 测试 1: 直接运行 ftree ==="
echo "启动 ftree，按 Ctrl+C 退出..."
echo "如果卡住，请等待 5 秒后按 Ctrl+C"
echo ""

timeout 5 strace -e trace=write,read,ioctl -f ftree 2>&1 | head -100 &
STRACE_PID=$!

sleep 2

# 检查进程状态
if ps -p $STRACE_PID > /dev/null 2>&1; then
    echo ""
    echo "⚠️  ftree 可能在运行，检查进程..."
    ps aux | grep -E "(ftree|ff)" | grep -v grep
fi

wait $STRACE_PID 2>/dev/null

echo ""
echo "=== 调试完成 ==="
