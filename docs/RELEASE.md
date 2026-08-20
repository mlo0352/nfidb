# Release process

## Versioning

The workspace and browser package share one version. Release tags use `vMAJOR.MINOR.PATCH`. Until the physical iPad/app matrix and performance target pass, releases are marked prerelease.

## Local package

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

The build script creates `build\packages\NFiDB-windows-x64.zip` and a sibling `.sha256` file. It contains the GUI host, pointer sink, README, changelog, licenses, third-party notice, and operating documentation. An installer is explicitly deferred; the ZIP is portable and unsigned.

## GitHub automation

- `ci.yml` runs frontend install/typecheck/unit/build plus Rust fmt/check/Clippy/test/release-build and a real GUI-window/status smoke on Windows before uploading the executables.
- `pages.yml` publishes the static `site/` download/documentation landing page to GitHub Pages.
- `release.yml` runs for `v*` tags, rebuilds on Windows, packages the portable ZIP, requires the GUI startup smoke, generates SHA-256, and only then attaches both to the GitHub Release.

The repository's Pages source must be set to **GitHub Actions**. Workflows need read contents permission; release needs write contents; Pages needs its generated Pages permissions.

## Prerelease checklist

- Clean checkout and all CI checks pass.
- `pointer-sink --self-test` passes on the release machine.
- The release executable creates its GUI window and serves `/api/status` under `scripts/gui-smoke.ps1`.
- No secret, developer IP, local path, or test PIN is tracked.
- Dependency/license table matches direct shipped dependencies.
- Private-network firewall language and HTTP LAN limitation are visible.
- Portable ZIP opens and runs on a separate Windows 11 machine.
- SHA-256 matches after downloading the Release asset.
- `KNOWN_ISSUES.md` and `TEST_MATRIX.md` are current.

## Stable checklist

In addition to prerelease: physical iPad/Apple Pencil, Krita, Rebelle, reconnect, WAN-offline, and 30-minute stability rows must pass; physical Safari codec results and release-mode performance must be published on more than one GPU vendor; GPU-side preprocessing should be evaluated against the current measured CPU-copy path.
