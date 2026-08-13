#!/usr/bin/env bash
# minstall 开发启动脚本：先清理残留进程（避免 Vite 端口 1420 冲突导致白屏），再启动。
# 用法: ./start.sh
set -euo pipefail
cd "$(dirname "$0")"

echo "==> 清理残留进程..."
pkill -f "tauri dev" 2>/dev/null || true
pkill -f "target/debug/minstall" 2>/dev/null || true
pkill -f "vite" 2>/dev/null || true
sleep 1

# 确认 1420 端口已释放（Vite devUrl，冲突会导致窗口白屏）
if ss -tlnp 2>/dev/null | grep -q ":1420 "; then
  echo "!! 端口 1420 仍被占用："
  ss -tlnp 2>/dev/null | grep ":1420 "
  echo "请手动释放后重试（fuser -k 1420/tcp）"
  exit 1
fi

echo "==> 启动 tauri dev (Vite :1420)..."
npm run tauri dev
