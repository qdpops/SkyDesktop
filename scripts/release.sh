#!/usr/bin/env bash
# Выпуск новой версии: сборка, контрольная сумма и релиз на GitHub.
#
#   1. поднимите version в core/skydesktop-app/Cargo.toml;
#   2. scripts/release.sh ["что нового"]
#
# Релиз получает метку v<версия> и два файла: SkyDesktop.exe и
# SkyDesktop.exe.sha256. По ним установленные копии и находят обновление
# (см. core/skydesktop-app/src/update.rs) — имена менять нельзя.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO=qdpops/SkyDesktop

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/core/skydesktop-app/Cargo.toml" | head -1)
TAG="v$VERSION"
NOTES="${1:-SkyDesktop $VERSION}"

if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "Релиз $TAG уже есть — поднимите версию в core/skydesktop-app/Cargo.toml" >&2
  exit 1
fi
if [ -n "$(git -C "$ROOT" status --porcelain)" ]; then
  echo "Есть незафиксированные изменения: релиз должен собираться из коммита" >&2
  exit 1
fi

"$SCRIPT_DIR/build-skydesktop.sh"

OUT="$ROOT/core/target/release"
(cd "$OUT" && sha256sum SkyDesktop.exe > SkyDesktop.exe.sha256)
cat "$OUT/SkyDesktop.exe.sha256"

git -C "$ROOT" push -q origin HEAD
gh release create "$TAG" \
  --repo "$REPO" \
  --target "$(git -C "$ROOT" rev-parse HEAD)" \
  --title "SkyDesktop $VERSION" \
  --notes "$NOTES" \
  "$OUT/SkyDesktop.exe" \
  "$OUT/SkyDesktop.exe.sha256"
