# Changelog

English | [简体中文](CHANGELOG.zh-CN.md)

All notable, user-visible changes will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to Semantic Versioning (`MAJOR.MINOR.PATCH`).

> This is the Rust edition's changelog, starting at 0.9.0 (full native rewrite of the C#/WinUI 3 edition). The C# edition's history lives in its own repository and is not merged here.

## [0.9.2] - 2026-11-24

### Added

- Built-in MCP server (`ifreesftp mcp`, stdio): MCP clients can browse remote directories and transfer files via existing hosts; path-based tools are guarded by the `mcp_daemon.json` allow-list (blank list rejects everything by default), deletes are two-phase confirmed and writes use an optimistic lock — see [docs/mcp.md](docs/mcp.md)
- New "MCP" category in Settings: overview, configuration file access, and docs link
- Various settings logic refinements

### Changed

- Dark/light theme polish

### Fixed

- UI fixes

## [0.9.1] - 2026-11-18

### Added

- Dragging files from Explorer into a remote pane uploads immediately; Shift+drop only stages the batch for review, queue it manually
- Two more rubber-band modes: Shift+drag from a row, and right-button drag banding

### Changed

- Connecting to a host is now tab-first, without the initial empty-state cover
- Minor UI polish

### Fixed

- Minor UI fixes

## [0.9.0] - 2026-11-07

First release of the native Rust edition: a single-exe, zero-bundled-runtime rewrite of the C#/WinUI 3 client (eframe 0.37 / egui, russh 0.65).

### Added
- Dual-pane, multi-tab main window: independent tabs on both sides (local or remote), host card panel, per-tab sessions.
- Real SSH/SFTP connections (russh, ring backend): password and private-key auth (OpenSSH/PEM), keep-alive, connect/operation timeouts, retries with delay.
- Connection security: first-connect host fingerprint confirmation (SHA256) with pinned archives, silent fail-closed gate for transfer and exec connections, outright rejection of changed server keys.
- Transfer queue: drag & drop and context-menu transfers, per-file progress, waiting/failed tabs, batch re-run; tar-accelerated directory transfers above a file-count threshold (server-side packing, single stream) with per-file fallback; resumable transfers from byte offsets.
- Post-transfer verification (off by default): server-side hashing over the exec channel (SHA-256/512; SM3 via server-side openssl), mismatched files flagged failed.
- Server tooling: chmod grid with live fetch, SHA-256/SHA-512/SM3 checksum dialog, host command console with full session log, server-side `rm -rf` directory deletion with SFTP recursion fallback.
- Built-in editor with remote edit & save-back: temp-copy download, mtime conflict detection, per-host temp cleanup; external editor support.
- Session import wizard: consent-first scan of WinSCP (INI/registry), FileZilla Site Manager, PuTTY, OpenSSH `~/.ssh/config` and iFreeSftp archives; duplicate-aware preview; passwords and post-login commands never imported.
- Sync-browse file comparison, rubber-band multi-select, dark/light themes, Chinese/English runtime switching.
- Registration (release builds only): offline ECDsa P-256 registration-code verification, supporter badge, remote announcements; builds compiled from this repository have no activation entry.
- Safety hardening: directory loop detection and 128-level depth limit in local walks, fingerprint confirmation timeout, idle queue with no busy loop.

### Notes
- See the README capability matrix for deliberate differences from the C# edition (per-file mtime preservation, batch-level conflict decisions, no local verification fallback, SM3 server-side only).
- Server-side direct remote→remote transfer (a single server-side `cp`) is planned, not yet shipped; same-host copies currently relay through the local machine.
