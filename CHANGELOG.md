# Changelog

English | [简体中文](CHANGELOG.zh-CN.md)

All notable, user-visible changes will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to Semantic Versioning (`MAJOR.MINOR.PATCH`).

## [Unreleased]

## [0.8.1] - 2026-10-12

Security hardening and maintainability release.

### Added
- Update check: a "New version" badge in the title bar (click to open the download page); the reminder can be toggled in Settings → About with a manual check button. Releasing a new version only requires updating `UPDATE_VERSION` in the Worker.

### Security
- Host key fingerprint enforcement everywhere: test-connection now shows the same fingerprint confirmation dialog and writes the result back to the store; exec/transfer channels reject any connection without a fingerprint; connection paths without a fingerprint callback fail closed, eliminating every bypass.
- Fingerprint pre-fill is consolidated into a single entry point with audit logging, so "why was this host silently trusted this time" is traceable.
- Remote-delivered link allowlist: announcement/purchase/update links open only over http/https (entry filtering + consumption-point interception as double protection); announcement text is truncated to 2000 characters.
- Host passwords encrypted at rest: passwords in `hosts.json` are stored as DPAPI (per-user) ciphertext; legacy plaintext is upgraded automatically.
- Statistics API hardening (Worker): the token now travels via the Authorization header with constant-time comparison and no longer appears in access logs. Re-deploying the Worker is required.

### Fixed
- Remote config (announcements, badge toggle, purchase link) used to stay stale until the 24h cache expired after restart; now restored on cache hit.
- A corrupted hosts.json is automatically backed up with a timestamp, and the host panel shows the backup path for manual recovery.
- Host password DPAPI decryption failure (switched Windows account / new machine / profile reset) no longer silently wipes the password: the original ciphertext is kept on disk with a log hint, recoverable after switching back to the original account; encryption is now idempotent, eliminating the "undecryptable → save empty → permanent loss" overwrite chain.
- Transfer-queue pause is now zero-CPU (signal gate) instead of 100ms polling; suspended tasks are woken immediately on stop and recorded as stopped.
- Delete-confirm/rename dialogs: language-change listeners are now paired with the dialog lifetime (unsubscribed on unload); re-opening while a previous dialog is still pending terminates the earlier wait instead of hanging.
- Settings writes are debounced into 500ms batches (slider drags no longer hammer the disk), and pending changes are flushed on app exit.
- Cleaned up 5 groups of duplicate keys in resource files (later definitions silently overriding earlier values) and a "file/folder" display typo.

### Improved
- Login method is now an enum: the store format is unchanged (compatible with old hosts.json; invalid values fall back to password login).
- Machine fingerprint hash switched from MD5 to SHA-256 (statistics only; metrics transition naturally).
- SM3 (Chinese national cipher) gained a GB/T 32905-2016 Appendix A standard-vector startup self-check (Debug builds), so a tampered implementation fails loudly at startup.
- New tests/ project with pure-function unit tests: version comparison, SM3 standard vectors, permission strings, path joining, URL allowlist, file-size formatting, host password encryption (55 cases, runs standalone via `dotnet test`).
- The system icon cache gained an LRU cap (512 bitmaps / 1024 type names), so long sessions no longer grow without bound.
- Icon-only buttons now expose screen-reader names (AutomationProperties.Name), so screen readers can announce toolbar button purposes.
- README gained a "Security boundaries & build variants" section (obfuscation is not a security boundary; download only from official channels).

## [0.8.0] - 2026-10-11

First public release.

### Added
- Server-side command channel: same-machine remote→remote `cp` direct transfer, instant bulk `rm -rf` deletion, with automatic fallback to per-item SFTP
- File checksums: SHA-256 / SHA-512 / SM3 (Chinese national cipher), server-side commands preferred for instant large-file hashing
- Built-in AvalonEdit editor: edit remote text files, auto-upload on save, conflict detection

### Improved
- Language dictionaries moved to an external locales folder; user-created language files are discovered automatically
- Synchronized browsing, file comparison, rubber-band selection, cross-pane drag-and-drop transfer

[Unreleased]: https://github.com/iFreeshine/iFreeSftp/compare/v0.8.1...HEAD
[0.8.1]: https://github.com/iFreeshine/iFreeSftp/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/iFreeshine/iFreeSftp/releases/tag/v0.8.0
