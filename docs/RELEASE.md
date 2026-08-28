# Release process

## Versioning

The workspace and browser package share one version. Release tags use `vMAJOR.MINOR.PATCH`. Until the physical iPad/app matrix and performance target pass, releases are marked prerelease.

## Local packages

From a clean Windows checkout with the documented toolchain:

```powershell
.\scripts\test.ps1
.\scripts\build-release.ps1
.\scripts\gui-smoke.ps1 -ArchivePath .\build\packages\NFiDB-windows-x64.zip
.\scripts\portable-smoke.ps1
.\scripts\file-transfer-smoke.ps1 -SkipBuild -ExecutablePath .\target\release\nfidb.exe
```

For unattended user-run validation and a resumable result, the equivalent supported handoff is:

```powershell
.\scripts\validate-for-codex.ps1
```

The script records the commit, dirty-tree fingerprint, step timings/errors, release checksum, and transcript under `build\user-validation`, with the latest result always at `build\user-validation\latest.json`.

When compilation has already succeeded and only packaging or a smoke check needs to be resumed, use:

```powershell
.\scripts\validate-for-codex.ps1 -ResumeAfterBuild
```

This reuses `target\release`, rebuilds the portable archive from a unique staging directory, and runs the native, GUI-startup, portable, and file-transfer smoke checks without invoking npm or Cargo. Unique staging also lets an older portable copy remain open while a new ZIP is produced.

When source validation is complete but the optimized executable has not been built, use `validate-for-codex.ps1 -ResumeAfterSourceValidation`. It skips repeated source/frontend validation, performs the release build, then runs every package and smoke gate.

The build script creates `build\packages\NFiDB-windows-x64.zip` and a sibling `.sha256` file. It contains the GUI host, pointer sink, README, changelog, licenses, third-party notice, and operating documentation. An installer is explicitly deferred; the ZIP is portable and unsigned.

From a clean Apple-silicon Mac checkout with Xcode Command Line Tools, Rust, and Node.js installed:

```bash
./scripts/build-macos.sh
codesign --verify --deep --strict build/packages/.verification/NFiDB.app # after extracting to any temporary directory
```

`build-macos.sh` runs the browser build and host/core/protocol/transport tests, creates an icon-bearing `NFiDB.app`, ad-hoc signs it, verifies that signature, and writes `build/packages/NFiDB-macos-arm64.zip` plus its `.sha256`. The app requires macOS 13 or newer. It is not Developer ID signed or notarized, so the release notes must retain the control-click **Open** instruction.

## GitHub automation

- `ci.yml` runs frontend install/typecheck/unit/build plus Rust fmt/check/Clippy/test/release-build. Windows also runs its GUI-window/status smoke; macOS verifies the app-bundle signature, ZIP, and executable version in a native arm64 runner.
- `pages.yml` publishes the static `site/` download/documentation landing page to GitHub Pages.
- `release.yml` runs for `v*` tags, builds both Windows x64 and macOS arm64 packages, validates each platform artifact, generates SHA-256 files, and only then attaches them to the GitHub Release.

The repository's Pages source must be set to **GitHub Actions**. Workflows need read contents permission; release needs write contents; Pages needs its generated Pages permissions.

## Prerelease checklist

- Clean checkout and all CI checks pass.
- `pointer-sink --self-test` passes on the release machine.
- The release executable creates its GUI window and serves `/api/status` under `scripts/gui-smoke.ps1`.
- No secret, developer IP, local path, or test PIN is tracked.
- Dependency/license table matches direct shipped dependencies.
- Private-network firewall language and HTTP LAN limitation are visible.
- Portable ZIP opens and runs on a separate Windows 11 machine.
- The exact macOS release app opens on an Apple-silicon Mac, obtains Screen Recording and Accessibility permission, captures a real monitor, and controls it from physical iPad Safari.
- SHA-256 matches after downloading the Release asset.
- `KNOWN_ISSUES.md` and `TEST_MATRIX.md` are current.

## Stable checklist

In addition to prerelease: physical iPad/Apple Pencil, Krita, Rebelle, reconnect, WAN-offline, and 30-minute stability rows must pass; physical Safari codec results and release-mode performance must be published on more than one GPU vendor; GPU-side preprocessing should be evaluated against the current measured CPU-copy path.
