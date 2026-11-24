# MCP Server

[简体中文](mcp.zh-CN.md) | English

iFreeSftp release builds ship a built-in **MCP (Model Context Protocol) server**, so MCP-capable clients (Claude Desktop, Cursor, etc.) can browse remote directories and transfer files through your existing SFTP hosts.

The server runs as a stdio subcommand of the same executable — there is no separate binary, port, or background service. MCP clients launch `ifreeSftp.exe mcp` themselves and talk **line-delimited JSON-RPC over stdio**.

> Source builds compiled from this repository do not include the MCP server.

## Enable it in your MCP client

Point your client at the executable with the `mcp` argument. Generic `claude_desktop_config.json` style:

```json
{
  "mcpServers": {
    "ifreesftp": {
      "command": "C:\\Program Files\\iFreeSftp\\ifreeSftp.exe",
      "args": ["mcp"]
    }
  }
}
```

Adjust the path to your install location. In the app you can also open **Settings → MCP** for a pointer to the configuration file described below.

## Security model

The server exposes your **existing hosts and sessions only** — it cannot create hosts or bypass the GUI's host-fingerprint confirmation (a host whose fingerprint is not yet confirmed cannot be connected from MCP; connect once in the GUI first).

Path-based tools are guarded by an **allow-list** declared in `mcp_daemon.json`:

- File location: `%LOCALAPPDATA%\iFreeSftp\mcp_daemon.json` (Windows). The **Settings → MCP** page in the app shows the exact path and can create/open the file for you.
- **An empty or missing `allow_roots` rejects everything by default**: every path-based tool is rejected until you explicitly declare roots.
- Allowed roots apply to remote paths resolved on the connected server; anything outside them is rejected.

Example:

```json
{
  "allow_roots": ["C:/Users/me/work", "/home/me/projects"],
  "max_read_bytes": 65536,
  "rate_per_min": 120,
  "delete_max_items": 20,
  "delete_max_ratio_pct": 10,
  "confirm_ttl_secs": 300
}
```

All fields are optional; defaults: `max_read_bytes` 65536 (64 KB), `rate_per_min` 120, `delete_max_items` 20, `delete_max_ratio_pct` 10, `confirm_ttl_secs` 300.

Additional built-in safeguards:

- `read_file` returns at most 64 KB per call (larger files report their total size and must be fetched with `download`).
- `write_file` requires an **optimistic lock**: `expected_hash` (SHA-256 of the current remote content; for a new or empty file use the SHA-256 of the empty string, `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`), writes capped at 64 KB.
- Deletes are **two-phase**: call `delete` with `dry_run: true` to get the item list plus a one-time `confirm_token`; repeat the call with the exact same parameters plus that token to execute. Any parameter mismatch invalidates the token and a new confirmation is required. Tokens expire after `confirm_ttl_secs`.
- Every tool call is written to an audit log (`audit/mcp-audit.jsonl` in the app data directory) with sensitive values redacted.

## Tools

Sessions & hosts:

| Tool | Description |
|---|---|
| `list_sessions` | Online sessions (aliases `s1`, `s2`, …; no credentials) |
| `connect` | Connect a host by `host_id` (hosts with unconfirmed fingerprints are refused) |
| `list_hosts` | Sanitized summary of your configured host list (no passwords/keys) |
| `describe_host` | Sanitized summary of a single host |

Remote files (paths must fall under `allow_roots`):

| Tool | Description |
|---|---|
| `list_dir` | List a remote directory |
| `stat_path` | Get attributes (type, size, timestamps, etc.) of a remote path |
| `read_file` | Read up to 64 KB of a remote file |
| `upload` | Upload a local file to the remote |
| `download` | Download a remote file to local |
| `write_file` | Overwrite a remote file (≤ 64 KB, `expected_hash` optimistic lock) |

Transfers:

| Tool | Description |
|---|---|
| `enqueue` | Queue a transfer task |
| `transfer_status` | Task/queue status |
| `pause` / `resume` / `cancel` / `retry` | Task control |

Misc:

| Tool | Description |
|---|---|
| `tail_log` | View the end of a session's command-window log |
| `delete` | Destructive remote delete (two-phase, see above) |

Tool semantics are self-describing through each tool's JSON schema — `tools/list` in your client is the authoritative reference.
