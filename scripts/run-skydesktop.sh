#!/usr/bin/env bash
# Запуск собранного SkyDesktop.
#
# Приложение самодостаточно: интерфейс вшит в exe, ядро и мост поднимаются
# внутри того же процесса. Отдельный dev-сервер нужен только для `npm run dev`
# (см. scripts/run-skydesktop-dev.sh).
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

EXE="$ROOT/core/target/release/SkyDesktop.exe"
[ -f "$EXE" ] || EXE="$ROOT/core/target/debug/SkyDesktop.exe"
[ -f "$EXE" ] || { echo "Приложение не собрано: запустите scripts/build-skydesktop.sh"; exit 1; }

echo "Запуск: $EXE"
exec "$EXE" "$@"
