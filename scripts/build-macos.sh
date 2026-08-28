#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

skip_tests=0
skip_web=0
for argument in "$@"; do
  case "$argument" in
    --skip-tests) skip_tests=1 ;;
    --skip-web) skip_web=1 ;;
    *) echo "Unknown argument: $argument" >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-macos.sh must run on macOS." >&2
  exit 1
fi

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"

if [[ $skip_web -eq 0 ]]; then
  npm --prefix apps/ipad-web ci
  npm --prefix apps/ipad-web run build
fi

if [[ $skip_tests -eq 0 ]]; then
  cargo test --locked -p nfidb-host-macos
  cargo test --locked -p nfidb-core -p nfidb-protocol -p nfidb-transport
fi

cargo build --release --locked -p nfidb

version="$(awk -F '"' '/^version = "/ { print $2; exit }' Cargo.toml)"
package_root="$repo_root/build/packages"
mkdir -p "$package_root"
stage_root="$(mktemp -d "$package_root/.macos-staging.XXXXXX")"
trap 'rm -rf "$stage_root"' EXIT
app="$stage_root/NFiDB.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources" "$contents/Frameworks"

cp target/release/nfidb "$contents/MacOS/nfidb"
sed "s/__NFIDB_VERSION__/$version/g" apps/windows-host/assets/Info.plist > "$contents/Info.plist"

icon_work="$stage_root/icon-work"
iconset="$icon_work/NFiDB.iconset"
mkdir -p "$iconset"
qlmanage -t -s 1024 -o "$icon_work" apps/windows-host/assets/nfidb.svg >/dev/null 2>&1
icon_source="$icon_work/nfidb.svg.png"
if [[ ! -f "$icon_source" ]]; then
  echo "Quick Look failed to render the NFiDB icon." >&2
  exit 1
fi
for spec in \
  "16 icon_16x16.png" \
  "32 icon_16x16@2x.png" \
  "32 icon_32x32.png" \
  "64 icon_32x32@2x.png" \
  "128 icon_128x128.png" \
  "256 icon_128x128@2x.png" \
  "256 icon_256x256.png" \
  "512 icon_256x256@2x.png" \
  "512 icon_512x512.png" \
  "1024 icon_512x512@2x.png"; do
  size="${spec%% *}"
  name="${spec#* }"
  sips -z "$size" "$size" "$icon_source" --out "$iconset/$name" >/dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/NFiDB.icns"

codesign --force --deep --sign - --timestamp=none "$app"
codesign --verify --deep --strict "$app"

archive="$package_root/NFiDB-macos-arm64.zip"
rm -f "$archive" "$archive.sha256"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"
shasum -a 256 "$archive" | awk '{print $1}' > "$archive.sha256"

echo "Created $archive"
echo "SHA-256 $(cat "$archive.sha256")"
