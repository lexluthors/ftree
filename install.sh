#!/bin/bash
set -e

echo "🔨 编译 ftree (release)..."
cargo build --release

echo "📦 安装到 ~/.local/bin/ftree..."
mkdir -p ~/.local/bin
rm -f ~/.local/bin/ftree
cp target/release/ftree ~/.local/bin/ftree

echo "✅ 安装完成！"
echo "   运行: ff 或 ftree"
