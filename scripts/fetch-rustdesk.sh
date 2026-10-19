#!/usr/bin/env bash
# Готовит ядро: RustDesk той версии, на которой построен SkyDesktop, и наши
# правки к нему из vendor/patches.
#
# Итог: vendor/rustdesk — то, что ждёт core/skydesktop-app/Cargo.toml.
#   scripts/fetch-rustdesk.sh            # скачать и наложить правки
#   scripts/fetch-rustdesk.sh <папка>    # то же, но в другую папку (проверка)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PATCHES="$ROOT/vendor/patches"
DEST="${1:-$ROOT/vendor/rustdesk}"

# Версии, к которым написаны патчи. Меняются только вместе с патчами.
RUSTDESK_COMMIT=3f207e91f6061b637f704f94074ee487b030625f
HBB_COMMON_COMMIT=b2b1ac453d1d694046f63be20d792d608dac1c93

if [ -e "$DEST" ]; then
  echo "$DEST уже существует — удалите папку, чтобы скачать ядро заново" >&2
  exit 1
fi

# Переводы строк — как в репозитории RustDesk: иначе патчи не лягут.
fetch() { # <url> <commit> <папка>
  git -c init.defaultBranch=main init -q "$3"
  git -C "$3" config core.autocrlf false
  git -C "$3" remote add origin "$1"
  git -C "$3" fetch -q --depth 1 origin "$2"
  git -C "$3" checkout -q FETCH_HEAD
}

echo "=== RustDesk $RUSTDESK_COMMIT ==="
fetch https://github.com/rustdesk/rustdesk.git "$RUSTDESK_COMMIT" "$DEST"
rmdir "$DEST/libs/hbb_common" 2>/dev/null || true
echo "=== hbb_common $HBB_COMMON_COMMIT ==="
fetch https://github.com/rustdesk/hbb_common "$HBB_COMMON_COMMIT" "$DEST/libs/hbb_common"

echo "=== правки SkyDesktop ==="
git -C "$DEST" apply --whitespace=nowarn "$PATCHES/rustdesk.patch"
git -C "$DEST/libs/hbb_common" apply --whitespace=nowarn "$PATCHES/hbb_common.patch"

echo "Готово: $DEST"
