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

available_signing_identities="$(security find-identity -v -p codesigning 2>/dev/null || true)"
sign_identity_source="explicit"
if [[ -n "${NFIDB_CODESIGN_IDENTITY:-}" ]]; then
  sign_identity="$NFIDB_CODESIGN_IDENTITY"
else
  sign_identity="-"
  sign_identity_source="ad-hoc fallback"

  installed_app="$HOME/Applications/NFiDB.app"
  installed_authority=""
  if [[ -d "$installed_app" ]]; then
    installed_authority="$(
      codesign -d --verbose=2 "$installed_app" 2>&1 \
        | awk -F= '/^Authority=(Developer ID Application:|Apple Development:)/ { print substr($0, index($0, "=") + 1); exit }' \
        || true
    )"
  fi
  if [[ -n "$installed_authority" ]]; then
    installed_identity="$(
      awk -v authority="$installed_authority" 'index($0, "\"" authority "\"") { print $2; exit }' \
        <<<"$available_signing_identities"
    )"
    if [[ -n "$installed_identity" ]]; then
      sign_identity="$installed_identity"
      sign_identity_source="installed NFiDB identity"
    fi
  fi

  if [[ "$sign_identity" == "-" ]]; then
    developer_id_identity="$(
      awk 'index($0, "\"Developer ID Application:") { print $2; exit }' \
        <<<"$available_signing_identities"
    )"
    apple_development_identity="$(
      awk 'index($0, "\"Apple Development:") { print $2; exit }' \
        <<<"$available_signing_identities"
    )"
    if [[ -n "$developer_id_identity" ]]; then
      sign_identity="$developer_id_identity"
      sign_identity_source="detected Developer ID Application identity"
    elif [[ -n "$apple_development_identity" ]]; then
      sign_identity="$apple_development_identity"
      sign_identity_source="detected Apple Development identity"
    fi
  fi
fi

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
icon_source="apps/windows-host/assets/nfidb.png"
icon_width="$(sips -g pixelWidth "$icon_source" | awk '/pixelWidth:/ { print $2 }')"
icon_alpha="$(sips -g hasAlpha "$icon_source" | awk '/hasAlpha:/ { print $2 }')"
if [[ -z "$icon_width" || "$icon_width" -lt 512 || "$icon_alpha" != "yes" ]]; then
  echo "NFiDB source icon must be at least 512 px with transparency (found ${icon_width:-unknown} px, alpha ${icon_alpha:-unknown})." >&2
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
if [[ "$(plutil -extract CFBundleIconFile raw "$contents/Info.plist")" != "NFiDB.icns" ]]; then
  echo "macOS bundle does not declare the NFiDB icon." >&2
  exit 1
fi
if [[ ! -s "$contents/Resources/NFiDB.icns" ]]; then
  echo "macOS NFiDB icon asset is missing or empty." >&2
  exit 1
fi

codesign --force --deep --sign "$sign_identity" --timestamp=none "$app"
codesign --verify --deep --strict "$app"

archive="$package_root/NFiDB-macos-arm64.zip"
rm -f "$archive" "$archive.sha256"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"
shasum -a 256 "$archive" | awk '{print $1}' > "$archive.sha256"

echo "Created $archive"
if [[ "$sign_identity" == "-" ]]; then
  echo "Code signing: ad-hoc (set NFIDB_CODESIGN_IDENTITY for a stable Apple signing identity)"
else
  echo "Code signing: $sign_identity ($sign_identity_source)"
fi
echo "SHA-256 $(cat "$archive.sha256")"
