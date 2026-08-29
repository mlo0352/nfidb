#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macos-real-monitor-smoke.sh must run on a Mac with Screen Recording and Accessibility approved." >&2
  exit 1
fi

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
app="${NFIDB_MACOS_REAL_MONITOR_APP:-$HOME/Applications/NFiDB.app}"
executable="$app/Contents/MacOS/nfidb"
port="${NFIDB_MACOS_REAL_MONITOR_PORT:-49132}"
if [[ ! -x "$executable" ]]; then
  echo "NFiDB app executable is missing: $executable" >&2
  exit 1
fi

stamp="$(date -u +%Y%m%d-%H%M%S)"
report_root="$repo_root/build/macos-real-monitor-smoke/$stamp"
mkdir -p "$report_root"
session_path="$report_root/session.json"
browser_path="$report_root/browser.json"
local_diagnostics_path="$report_root/local-diagnostics.json"
host_log="$report_root/host.log"
host_pid=""

cleanup() {
  if [[ -n "$host_pid" ]] && kill -0 "$host_pid" 2>/dev/null; then
    running_command="$(ps -p "$host_pid" -o command= 2>/dev/null || true)"
    if [[ "$running_command" == "$executable"* ]]; then
      kill "$host_pid" 2>/dev/null || true
    fi
  fi
}
trap cleanup EXIT

rm -f "$session_path"
open -n "$app" --args \
  --headless \
  --capture monitor \
  --input-sink inject \
  --no-mdns \
  --port "$port" \
  --run-seconds 120 \
  --session-info "$session_path" \
  >"$host_log" 2>&1

for _ in {1..160}; do
  if [[ -f "$session_path" ]]; then
    break
  fi
  sleep 0.25
done
if [[ ! -f "$session_path" ]]; then
  echo "The signed app did not start real-monitor capture within 40 seconds. See $host_log" >&2
  exit 1
fi

host_pid="$(node -e 'const value=JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(String(value.pid))' "$session_path")"
session_url="$(node -e 'const value=JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(value.url)' "$session_path")"
session_pin="$(node -e 'const value=JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(value.pin)' "$session_path")"
capture_source="$(node -e 'const value=JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(value.capture)' "$session_path")"
if [[ "$capture_source" != Display* ]]; then
  echo "The smoke did not start ScreenCaptureKit monitor capture: $capture_source" >&2
  exit 1
fi

export NFIDB_E2E_URL="$session_url"
export NFIDB_E2E_PIN="$session_pin"
export NFIDB_E2E_CHANNEL="${NFIDB_E2E_CHANNEL:-chrome}"
export NFIDB_E2E_REPORT="$browser_path"
export NFIDB_E2E_DRIVE_POINTER="1"

test_status=0
(
  cd apps/ipad-web
  npx playwright test e2e/real-monitor-startup.spec.ts
) || test_status=$?

curl -fsS "http://127.0.0.1:$port/api/local/diagnostics" >"$local_diagnostics_path" || true
if [[ $test_status -ne 0 ]]; then
  echo "macOS real-monitor/Safari-path smoke failed. Host log: $host_log" >&2
  echo "Browser report: $browser_path" >&2
  exit "$test_status"
fi

echo "macOS real-monitor capture/video/DataChannel smoke passed."
echo "Browser report: $browser_path"
echo "Host diagnostics: $local_diagnostics_path"
