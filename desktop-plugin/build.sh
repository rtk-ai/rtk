#!/bin/sh
set -e
ORIG=$(pwd)
cd "$(dirname "$0")"
VERSION=""; RTK=""; OUT="dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --rtk) RTK="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done
[ -n "$VERSION" ] || { echo "usage: build.sh --version <version> --rtk <linux musl rtk> [--out <dir>]" >&2; exit 2; }
case "$RTK" in "") RTK=".rtk-musl/rtk" ;; /*) ;; *) RTK="$ORIG/$RTK" ;; esac
case "$OUT" in /*) ;; *) OUT="$ORIG/$OUT" ;; esac
[ -f "$RTK" ] || { echo "no rtk binary at $RTK: pass --rtk with a Linux x86_64 musl build (see README.md)" >&2; exit 2; }
command -v node >/dev/null || { echo "node is needed to check the service" >&2; exit 2; }
command -v zip >/dev/null || { echo "zip is needed" >&2; exit 2; }

node --check desktop-src/server/index.mjs
RTK=$(cd "$(dirname "$RTK")" && pwd)/$(basename "$RTK")
rm -rf "$OUT"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE="$OUT/rtk"
BUNDLE="$OUT/rtk-desktop"

cp -R plugin "$STAGE"
cp -R desktop-src "$BUNDLE"
find "$STAGE" "$BUNDLE" \( -name __pycache__ -o -name .DS_Store -o -name test \) -prune -exec rm -rf {} + 2>/dev/null || true
python3 - "$VERSION" "$STAGE/.claude-plugin/plugin.json" "$BUNDLE/manifest.json" <<'EOF'
import json, sys
for p in sys.argv[2:]:
    m = json.load(open(p))
    m["version"] = sys.argv[1]
    with open(p, "w") as fh:
        json.dump(m, fh, indent=2)
        fh.write("\n")
EOF
mkdir -p "$STAGE/desktop" "$STAGE/runtime"
(cd "$BUNDLE" && zip -qr "$STAGE/desktop/rtk-desktop.mcpb" manifest.json icon.png server laya)
rm -rf "$BUNDLE"
cp "$RTK" "$STAGE/runtime/rtk"
chmod 755 "$STAGE/runtime/rtk"
cp ../LICENSE "$STAGE/LICENSE"
cp ../engine/LICENSE "$STAGE/LICENSE-ENGINE"

ZIP="rtk-desktop-plugin-$VERSION.zip"
(cd "$STAGE" && zip -qr "$OUT/$ZIP" .)
if command -v sha256sum >/dev/null; then (cd "$OUT" && sha256sum "$ZIP" > "$ZIP.sha256"); else (cd "$OUT" && shasum -a 256 "$ZIP" > "$ZIP.sha256"); fi
echo "built $OUT/$ZIP ($(wc -c < "$OUT/$ZIP" | tr -d ' ') bytes), sha256 $(cut -d' ' -f1 "$OUT/$ZIP.sha256")"
