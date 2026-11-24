# MCP 服务器

简体中文 | [English](mcp.md)

iFreeSftp 发布版内置 **MCP（Model Context Protocol）服务器**，支持 MCP 的客户端（Claude Desktop、Cursor 等）可经由你既有的 SFTP 主机浏览远程目录与收发文件。

服务器以同一 exe 的 stdio 子命令形态运行——没有独立二进制、端口或后台服务。MCP 客户端自行拉起 `ifreeSftp.exe mcp`，走 **stdio 行分隔 JSON-RPC** 通信。

> 从本仓库自行编译的源码构建不含 MCP 服务器。

## 在 MCP 客户端中启用

让客户端以 `mcp` 参数指向本程序。通用 `claude_desktop_config.json` 写法：

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

路径按实际安装位置调整。应用内也可打开 **设置 → MCP**，页面上有下文配置文件的入口。

## 安全模型

服务器只暴露**既有主机与会话**——不能新建主机，也不能绕过 GUI 的主机指纹确认（指纹未确认的主机无法经 MCP 连接；请先在 GUI 连接一次完成确认）。

路径类工具由 `mcp_daemon.json` 中的**目录白名单**守卫：

- 文件位置：`%LOCALAPPDATA%\iFreeSftp\mcp_daemon.json`（Windows）。应用内 **设置 → MCP** 页显示确切路径，并可一键创建/打开该文件。
- **`allow_roots` 为空或缺失时默认全拒**：未显式声明前，所有路径类工具一律拒绝。
- 白名单作用于已连接服务器上解析出的远程路径；范围外路径一律拒绝。

示例：

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

所有字段可选；缺省值：`max_read_bytes` 65536（64 KB）、`rate_per_min` 120、`delete_max_items` 20、`delete_max_ratio_pct` 10、`confirm_ttl_secs` 300。

其他内置加固：

- `read_file` 单次最多返回 64 KB（更大文件报真实总长，需改用 `download`）。
- `write_file` 必须**乐观锁**：`expected_hash`（当前远端内容的 SHA-256；新文件/空文件用空串哈希 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`），单次写入上限 64 KB。
- 删除为**两段式**：先 `delete` 带 `dry_run: true` 拿清单与一次性 `confirm_token`；再以完全相同的参数加该令牌重发执行。任一参数不一致即令牌作废，需重新确认。令牌 `confirm_ttl_secs` 秒后过期。
- 每次工具调用写入审计日志（应用数据目录 `audit/mcp-audit.jsonl`），敏感值脱敏。

## 工具清单

会话与主机：

| 工具 | 说明 |
|---|---|
| `list_sessions` | 在线会话清单（别名 `s1`、`s2`…；不含凭据） |
| `connect` | 按 `host_id` 连接主机（指纹未经确认的主机将拒绝连接） |
| `list_hosts` | 已配置主机列表的脱敏摘要（不含口令/私钥） |
| `describe_host` | 单主机脱敏摘要 |

远程文件（路径必须在 `allow_roots` 内）：

| 工具 | 说明 |
|---|---|
| `list_dir` | 列远程目录 |
| `stat_path` | 查看远程路径的属性（类型/大小/时间等） |
| `read_file` | 读取远程文件（≤ 64 KB） |
| `upload` | 本地文件上传远端 |
| `download` | 远程文件下载本地 |
| `write_file` | 覆写远程文件（≤ 64 KB，`expected_hash` 乐观锁） |

传输：

| 工具 | 说明 |
|---|---|
| `enqueue` | 入队传输任务 |
| `transfer_status` | 任务/队列状态 |
| `pause` / `resume` / `cancel` / `retry` | 任务控制 |

其他：

| 工具 | 说明 |
|---|---|
| `tail_log` | 查看会话命令窗日志的末尾内容 |
| `delete` | 高危远程删除（两段式，见上） |

各工具语义经 JSON schema 自描述——客户端内 `tools/list` 即权威参考。
