#!/usr/bin/env bash
# Полная сборка RustDesk (с адаптациями SkyDesktop) под Windows/MSVC.
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/env-msvc.sh"

cd /c/projects/SkyDesktop/vendor/rustdesk

echo "=== cargo build (rustdesk) ==="
cargo +stable-x86_64-pc-windows-msvc build "$@"
