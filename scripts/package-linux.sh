#!/usr/bin/env bash
# Build the release tarball from an existing release binary. CI runs this after
# `cargo build --release` (or `npm run build`). Official frontend assets are
# already embedded in the binary by tauri::generate_context!.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

VERSION="$(node -p "require('./package.json').version")"
ARCH="$(uname -m)"
NAME="wuwa-tappo-linux-${VERSION}-${ARCH}"
OUT="$DIR/dist/$NAME"

BIN="$DIR/bin/wuwatappo-linux"
if [[ ! -f "$BIN" ]]; then
  BIN="$DIR/src-tauri/target/release/wuwatappo-linux"
fi
if [[ ! -x "$BIN" ]]; then
  echo "没有可打包的 release 二进制。请先运行 'cargo build --release --manifest-path src-tauri/Cargo.toml'。" >&2
  exit 1
fi
ENTRY="$(grep -o 'assets/index-[^"]*\.js' "$DIR/build/frontend/index.html" | head -1 || true)"
if [[ -z "$ENTRY" || ! -f "$DIR/build/frontend/$ENTRY" ]]; then
  echo "官方前端尚未准备。请先运行 'npm run prepare:frontend'，否则打出的包只有占位页面。" >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/assets"

cp "$BIN" "$OUT/bin/wuwatappo-linux"
chmod 755 "$OUT/bin/wuwatappo-linux"
cp run.sh install.sh LICENSE README.md upstream-cn.json "$OUT/"
cp -r docs "$OUT/"
chmod 755 "$OUT/run.sh" "$OUT/install.sh"

for candidate in src-tauri/icons/128x128.png frontend/app-icon.png; do
  if [[ -f "$candidate" ]]; then cp "$candidate" "$OUT/assets/icon.png"; break; fi
done

tar --sort=name --owner=0 --group=0 --numeric-owner \
  -C "$DIR/dist" -czf "$DIR/dist/$NAME.tar.gz" "$NAME"
( cd "$DIR/dist" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256" )
rm -rf "$OUT"
echo "已生成 dist/$NAME.tar.gz"
echo "校验和：$(cat "$DIR/dist/$NAME.tar.gz.sha256")"
