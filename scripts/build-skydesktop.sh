#!/usr/bin/env bash
# Сборка SkyDesktop: оконное приложение с ядром внутри.
#
# Итог: core/target/release/SkyDesktop.exe — одно самодостаточное приложение.
# Интерфейс рисуется нативно, поэтому ничего, кроме Rust, собирать не нужно:
# шрифты (Roboto) и значки (Bootstrap Icons) лежат в assets и вшиваются в exe.
#
# Адрес каталога нод тоже вшивается в сборку:
#   SKYDESKTOP_DIRECTORY=https://nodes.example.com/nodes scripts/build-skydesktop.sh
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/env-msvc.sh"

PROFILE="${1:-release}"

# Без адреса каталога приложение собирается молча, но работает урезанно:
# не забирает список серверов и не получает постоянный адрес машины.
export SKYDESKTOP_DIRECTORY="${SKYDESKTOP_DIRECTORY:-http://lab-rnd.ru:8787/nodes}"

echo "=== Приложение (core/skydesktop-app) ==="
cd "$ROOT/core"
if [ "$PROFILE" = "debug" ]; then
  cargo build -p skydesktop-app
  EXE="$ROOT/core/target/debug/SkyDesktop.exe"
else
  cargo build --release -p skydesktop-app
  EXE="$ROOT/core/target/release/SkyDesktop.exe"
fi

echo
echo "Готово: $EXE"
echo "Каталог нод: $SKYDESKTOP_DIRECTORY"
