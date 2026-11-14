# iFreeSftp

English | [简体中文](README.zh-CN.md)

A dual-pane, multi-tab SFTP client for Windows & Linux — fast, secure, and completely free. Native Rust + egui, single-file binary, zero bundled runtime.

![Version](https://img.shields.io/badge/version-0.9.0-4C8BF5) ![License](https://img.shields.io/badge/license-AGPL--3.0-4C8BF5) ![Platform](https://img.shields.io/badge/platform-Windows_%7C_Linux-4C8BF5)

Website <https://ifrees.top> · Releases <https://github.com/iFreeshine/iFreeSftp/releases> · Get a registration code <https://ko-fi.com/ifrees/commissions?commissionAlias=61b90e1115>

> **Free forever**: every feature is free for all users, always. The supporter badge is a purely voluntary honor mark — registration never unlocks or restricts anything.
>
> **License**: released under the [GNU AGPL-3.0](https://www.gnu.org/licenses/agpl-3.0.html). This repository is a **published source snapshot** — registration/update and other private module code is intentionally excluded. Building from this source works out of the box (activation entry absent; see Build below). It is published for showcase and forking and is **not guaranteed to match the released application**.

---

## Screenshots

| | |
|---|---|
| ![Dual panes, dark theme](docs/screenshots/1.png) | ![Dual panes, light theme](docs/screenshots/2.png) |
| ![Session import](docs/screenshots/3.png) | ![Checksum](docs/screenshots/4.png) |

## Why iFreeSftp

- **Blazing multi-folder transfers** — directories above the threshold are tar-packed into a single stream on the server and unpacked on arrival: a 10,000-file tree moves in one round trip instead of per-file chatter; uploads, downloads and same-host remote→remote all get the same treatment, with automatic per-file fallback, byte-offset resume, and optional post-transfer hashing (SHA-256/512/SM3, computed locally and on the server) to catch corruption early
- **Per-file conflict decisions** — the overwrite dialog is a checklist: tick exactly which files to overwrite and untick the rest, with select-all for the batch; items whose target is newer than the source start unticked, so one changed file never forces an all-or-nothing choice
- **Dual panes, IDE-style multi-tab** — both folder windows host independent tabs (local or remote), so you can work on several hosts at once without juggling windows
- **One-click host import** — switching from WinSCP, FileZilla, PuTTY or OpenSSH config? Your hosts come with you: consent-first local scan, duplicate-aware preview, tick what to import; only connection metadata moves, passwords are never read
- **Secure by default** — host fingerprints are confirmed on first connect (SHA256) and pinned afterwards; a changed server key is rejected outright to stop man-in-the-middle attacks; passwords are stored encrypted; every remote-issued link (announcements, update, purchase) passes a strict http(s)-only launcher guard before it is ever opened
- **Built-in editor with remote edit & save-back** — download a temp copy, edit, save, auto-upload; mtime conflict detection warns you when the file changed underneath you
- **Real file-manager ergonomics** — drag & drop transfer, rubber-band multi-select, sync-browse file comparison, transfer queue with per-file progress and retry
- **Server tooling on tap** — chmod grid with live fetch, server-side checksums, host command console with full session log; server-side `rm -rf` for fast directory deletion with SFTP recursion fallback
- **Thoughtful connection handling** — connect/operation timeouts, retry with delay, keep-alive; errors surface in the status bar and log panel instead of hanging forever
- **Single file, no install** — one portable file, no runtime to install: a Windows exe whose import table contains only system DLLs, or a Linux tar.gz; dark/light themes, English & Chinese UI switchable at runtime

## How it compares

How iFreeSftp stacks up against the mainstream SFTP clients on Windows (compiled from the vendors' public documentation, October 2026 — corrections welcome via issue):

| | iFreeSftp | WinSCP | FileZilla | MobaXterm |
|---|---|---|---|---|
| License | AGPL-3.0, free | GPL, free | GPL, free | Freemium (Home edition limited, Pro paid) |
| Dual panes, independent tabs on **both** sides | ✓ | sessions tabbed, panes fixed | panes fixed, no tabs | terminal tabs + sidebar SFTP browser |
| Server-side direct remote→remote | △ planned — same-host copies currently relay through your machine as one packed stream (tar) | re-transfers file-by-file through your PC | ✗ not supported | ✗ not supported |
| Remote edit with auto upload-back | ✓ | ✓ | △ prompts on save | △ via the SFTP browser |
| Sync-browse comparison of two folders | ✓ | ✓ (synchronize) | ✗ | ✗ |
| Server-side checksums (SHA-256/512, SM3) | ✓ (SM3 via server-side openssl only) | ✓ | ✗ | ✗ |
| Portable single exe, no install | ✓ | ✓ | △ | ✓ |

## Download

Grab the latest `iFreeSftp-vX.Y.Z-win-x64.zip` from the [GitHub Releases](https://github.com/iFreeshine/iFreeSftp/releases) page or [ifrees.top](https://ifrees.top):

```
ifreeSftp.exe           # single exe — that's all (no runtime, no DLLs to bundle)
```

Unzip anywhere and run `ifreeSftp.exe`. Or build from source (below).

> Please download only from the links above; repackaged copies from third parties carry no integrity guarantee.

## Screenshots

<!-- TODO: Rust UI screenshots pending — see MIGRATION.md §7. Do not reuse the C#/WinUI 3 screenshots. -->

## Build from source

Requirements: Windows and [Rust](https://rustup.rs) (channel `stable-x86_64-pc-windows-gnu`, pinned by `rust-toolchain.toml`).

```powershell
.\build.ps1 build      # Debug build (compiles out of the box)
.\build.ps1 test       # cargo test
.\build.ps1 run        # Build and run
.\build.ps1 release    # Release source-verification build -> release\ifreeSftp-<ver>-win64.exe (runs version checks first; NOT a release artifact - release builds come from the private repo)
.\build.ps1 clean      # Clean
```

Note: offline registration verification requires a private closed-source module and is **not** available in this repository; a build compiled from this source simply has no activation entry. **The released application (Windows 64-bit, Microsoft Store, Linux) is built from the private development repository and always includes registration** — this showcase snapshot is not guaranteed to match it.

## Repository layout

```
ifreeSftp/                     # Rust workspace (src/ + build.rs + Cargo.toml + toolchain pins)
tools/
└── check-version.ps1          # Version consistency gate (Cargo.toml <-> README badge <-> CHANGELOG)
.github/workflows/build.yml    # CI: source build + test on push/PR
docs/screenshots/              # Screenshots
build.ps1                      # Build script
MIGRATION.md                   # Update & release rules for this repository (read before changing)
```

## Features

| Module | Highlights |
|---|---|
| Main window | **Dual panes + multi-tab**: each side hosts independent tabs (IDE-style tab strip); per-tab local browsing or remote connection; transfer queue, host command/log panel |
| Connection | russh (ring backend) connections, password or private-key auth (OpenSSH/PEM), keep-alive |
| Host manager | Host cards: add/edit/duplicate/delete, click to connect, active highlight |
| Session import | One-click import from WinSCP (INI/registry), FileZilla Site Manager, PuTTY, OpenSSH `~/.ssh/config`, and iFreeSftp's own archive; consent-first wizard with preview and dedup; passwords and login commands are never imported — only connection metadata |
| Connection security | First-connect host-fingerprint (SHA256) confirmation, remembered afterwards; changed server key is rejected outright (MITM protection) |
| Timeouts & retries | Connect timeout, per-operation timeout (errors surfaced in status bar and log panel), max retries, retry delay, keep-alive interval |
| Transfer | Drag & drop and context-menu transfer; directory transfers pack into one tar stream when the file count crosses the threshold (per-file fallback); resumable transfers; transfer queue with waiting/failed tabs and full re-run; directory loop and 128-level depth protection |
| Post-transfer verification | Optional dual-side hashing (SHA-256/512/SM3): always computed locally, plus server-side commands over the exec channel when available; mismatched files are flagged failed; skipped when either side cannot produce a hash (see capability matrix) |
| Settings | Categories (appearance/connection/editor/transfer/registration), live-apply, persisted as JSON under `%LOCALAPPDATA%\iFreeSftp\` |
| Supporter badge | Registration-code activation in the release from ifrees.top (offline ECDsa P-256 verification), gold "Supporter" badge, remote announcements (activation is not included in a build compiled from this repository's source); registration never changes functionality |
| Localization | Chinese / English runtime switching, all strings bilingual |
| Themes | Dark / light |
| File editing | Built-in editor: content-sniffed "Edit" menu (first 8 KB probe, not extension lists), line numbers, find, undo; external editor supported |
| Remote editing | Download temp copy + auto upload on save; mtime conflict detection with confirm dialog; per-host temp cleanup |
| Context menus | Full local menu (transfer to other side, edit, rename, delete, new folder/file, refresh); remote menu adds chmod grid with live fetch and SHA-256/SHA-512/SM3 checksums |
| File comparison | Sync-browse gated; aligns both lists by name, highlights one-side-only (Caution) and differing size/mtime rows (Critical) |
| Rubber-band select | Drag-select multiple entries in both lists, Ctrl to add |

## Architecture

Desktop app, no server backend; single workspace crate:

- **UI**: egui 0.37 / eframe — `src/ui/` (panes, dialogs, host panel, settings, editor window, import wizard)
- **SSH/SFTP**: russh 0.65 (ring backend) + russh-sftp 3.0; a dedicated worker thread runs a current-thread tokio runtime, the UI thread exchanges commands/events over channels and never blocks the frame
- **Transfer**: queue worker with tar-accelerated directory mode, resume, post-transfer verification, explicit-stack directory walk with loop/depth guards
- **Registration**: ECDsa P-256 offline verification lives in a closed-source module that is not part of this repository (see capability note above)
- **Runtime**: Rust `x86_64-pc-windows-gnu`, self-contained CRT linking — import table contains only Windows system DLLs

## Capability matrix (public wording)

Where behaviour differs from the C# edition or naive expectations, this table is authoritative:

| Capability | Status | Notes |
|---|---|---|
| Per-file mtime preservation | **Supported** | Uploads set the remote mtime (SFTP SETSTAT); downloads restore the local mtime; tar transfers carry mtime natively; toggle in Transfer settings ("Preserve modification time after transfer") |
| mtime preservation in tar transfers | Supported | tar archives carry mtime/permissions natively, both directions |
| Transfer conflict decisions | Per-item | The overwrite dialog is a per-file checklist — tick to overwrite, untick to skip, select-all for the batch; items whose target is newer than the source default to unticked; cancel drops the whole pending batch |
| Post-transfer verification structure | Dual-side | The local side is always computed (M-11, including SM3); the server side depends on the exec channel — if either side cannot produce a hash, verification is skipped for that file |
| SM3 | **Supported locally** | Bundled streaming implementation of GB/T 32905-2016 (`src/sm3.rs`, zero new dependencies, pinned to the standard's appendix-A test vectors); server-side SM3 works via `openssl dgst -sm3` when available |
| Server-side hashing on Windows servers | Not supported | Non-POSIX shell; the checksum menu is disabled |
| `.ppk` private keys | Not supported | russh only reads OpenSSH/PEM keys |
| Corrupted hosts.json | Timestamped backup | The unreadable archive is copied verbatim to `hosts.json.corrupt-{timestamp}.bak`, a red notice in the host panel shows where the data went, and the parseable remainder still loads |

## Registration / supporter badge

Settings → Registration (release builds): offline-verified registration code, gold "Supporter" badge, remote announcements. **Registration never changes functionality** — the badge is pure honor. Verification runs fully offline (ECDsa P-256). A build compiled from this repository has no activation entry.

## Sponsor

If iFreeSftp saves you time, a small sponsorship keeps development going — and comes with a gold **Supporter** badge in the app:

- International: [Ko-fi Commissions](https://ko-fi.com/ifrees/commissions?commissionAlias=61b90e1115) — order a registration code directly
- 中国大陆：[爱发电 · registration code item](https://afdian.com/item/3a69f926c4ac11f1878052540025c377) (WeChat / Alipay; full details may require sign-in)

After sponsoring, open a request on Ko-fi Commissions or 爱发电 with the name you want on the badge; the author sends back a signed registration code (see Settings → Registration).

## License

[GNU AGPL-3.0](LICENSE) — Copyright © 2026 [ifrees.top](https://ifrees.top)
