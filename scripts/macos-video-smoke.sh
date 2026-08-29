#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macos-video-smoke.sh must run on macOS." >&2
  exit 1
fi

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
executable="${NFIDB_MACOS_SMOKE_EXECUTABLE:-$repo_root/target/release/nfidb}"
port="${NFIDB_MACOS_SMOKE_PORT:-49131}"
if [[ ! -x "$executable" ]]; then
  echo "NFiDB executable is missing: $executable" >&2
  exit 1
fi

stamp="$(date -u +%Y%m%d-%H%M%S)"
report_root="$repo_root/build/macos-video-smoke/$stamp"
mkdir -p "$report_root"
session_path="$report_root/session.json"
metrics_path="$report_root/host-metrics.json"
browser_path="$report_root/browser.json"
host_log="$report_root/host.log"
host_pid=""

cleanup() {
  if [[ -n "$host_pid" ]] && kill -0 "$host_pid" 2>/dev/null; then
    kill "$host_pid" 2>/dev/null || true
    wait "$host_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

"$executable" \
  --headless \
  --capture test-pattern \
  --input-sink log \
  --no-mdns \
  --port "$port" \
  --run-seconds 90 \
  --session-info "$session_path" \
  --metrics-output "$metrics_path" \
  >"$host_log" 2>&1 &
host_pid=$!

for _ in {1..120}; do
  if [[ -f "$session_path" ]]; then
    break
  fi
  if ! kill -0 "$host_pid" 2>/dev/null; then
    echo "NFiDB exited before writing session metadata. See $host_log" >&2
    exit 1
  fi
  sleep 0.25
done
if [[ ! -f "$session_path" ]]; then
  echo "NFiDB did not write session metadata within 30 seconds. See $host_log" >&2
  exit 1
fi

session_url="$(node -e 'const value=JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(value.url)' "$session_path")"
session_pin="$(node -e 'const value=JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(value.pin)' "$session_path")"

export NFIDB_E2E_URL="$session_url"
export NFIDB_E2E_PIN="$session_pin"
export NFIDB_E2E_CHANNEL="${NFIDB_E2E_CHANNEL:-chrome}"
export NFIDB_E2E_REPORT="$browser_path"
export NFIDB_E2E_EXPECT_WIDTH="1280"
export NFIDB_E2E_EXPECT_HEIGHT="720"

test_status=0
(
  cd apps/ipad-web
  npx playwright test e2e/real-monitor-startup.spec.ts
) || test_status=$?

if [[ $test_status -ne 0 ]]; then
  echo "macOS video/DataChannel smoke failed. Host log: $host_log" >&2
  echo "Browser report (when startup progressed far enough): $browser_path" >&2
  exit "$test_status"
fi

echo "macOS video/DataChannel smoke passed."
echo "Report: $browser_path"
