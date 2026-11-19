use crate::settings;

pub fn is_en() -> bool {
    settings::with(|s| s.language == "en-US")
}

pub fn t(zh: &'static str) -> &'static str {
    t_for(zh, is_en())
}

pub fn t_for(zh: &'static str, en: bool) -> &'static str {
    if !en {
        return zh;
    }
    match zh {

        "本平台不保存密码" => "Passwords are not saved on this platform",
        "请改用密钥文件，或清空密码后在连接时输入；未清空前本平台不会写入存档" => {
            "Use a key file instead, or clear the password and enter it when connecting. The host list is not written to disk while a password is set"
        }
        "凭据需重新录入" => "Credential needs to be re-entered",
        "部分主机的密码无法在本平台解密，请在密码框重新输入；本次输入仅保存在内存中，不会写入存档" => {
            "Some saved passwords cannot be decrypted on this platform. Re-enter them in the password field; the value stays in memory only and is never written to disk"
        }
        "请先在密码框输入本次连接密码；该密码仅保存在内存中" => {
            "Enter the connection password in the password field first; it is kept in memory only"
        }
        "\"{0}\" 已回传到远端" => "\"{0}\" uploaded back to remote",

        "写锁已释放，继续回传" => "Write lock released, resuming upload",
        "写锁未释放，已暂缓回传：{0}（{1}s）" => {
            "Write lock not released, upload deferred: {0} ({1}s)"
        }
        "写锁未释放，已暂缓回传：{0}（{1}s）——不会上传半写文件" => {
            "Write lock not released, upload deferred: {0} ({1}s) — half-written files are never uploaded"
        }
        "等待编辑器完成保存…" => "Waiting for the editor to finish saving…",
        "文件 {0} 的写锁仍未释放，回传已暂缓。本地副本完好，不会上传半写文件。" => {
            "The write lock on {0} is still held; the upload is deferred. The local copy is intact — half-written files are never uploaded."
        }
        "立即重试探测" => "Retry probe now",
        "取消本次回传" => "Cancel this upload",
        "已取消回传：{0}（本地副本保留）" => {
            "Upload cancelled: {0} (local copy kept)"
        }
        "\"{0}\" 的文件缓存已经改变，是否保存编辑结果？" => {
            "The cached copy of \"{0}\" has changed. Save the edits to the server?"
        }
        "All files (*.*)" => "All files (*.*)",
        "Copyright © 2026 ifrees.top" => "Copyright © 2026 ifrees.top",
        "English" => "English",
        "外部编辑器启动失败：{0}" => "Failed to launch external editor: {0}",
        "FileZilla 站点管理器" => "FileZilla Site Manager",
        "Get License (Ko-fi)" => "Get License (Ko-fi)",
        "IP 地址" => "IP address",
        "OpenSSH (~/.ssh/config)" => "OpenSSH (~/.ssh/config)",
        "PDF 文件" => "PDF File",
        "程序 (*.exe)" => "Programs (*.exe)",
        "PuTTY 会话" => "PuTTY sessions",
        "选择外部编辑器" => "Select external editor",
        "SHA256" => "SHA256",
        "SHA512" => "SHA512",
        "SM3" => "SM3",
        "UNC 密钥路径，已回退密码" => "UNC key path, fell back to password",
        "WinSCP (INI 文件)" => "WinSCP (INI file)",
        "WinSCP (注册表)" => "WinSCP (registry)",
        "[mtime] 保留修改时间失败（不影响传输结果）：{0}（{1}）" => {
            "[mtime] Failed to preserve timestamp (transfer result unaffected): {0} ({1})"
        }
        "[tar 加速] {0}（文件数 ≥ {1}，单流打包）" => {
            "[tar accel] {0} (file count ≥ {1}, single packed stream)"
        }
        "[tar 加速] 临时文件清理失败（不影响传输结果）：{0}（{1}）" => {
            "[tar accel] Temp file cleanup failed (transfer itself unaffected): {0} ({1})"
        }
        "[tar 加速] 临时目录切换失败，改为直接解包覆盖：{0}（{1}）" => {
            "[tar accel] Temp dir switch failed, unpacking directly instead: {0} ({1})"
        }
        "[tar 加速] 传输 {0} 秒无进展，判定链路僵死已中止" => {
            "[tar accel] No progress for {0} s, aborted as a stalled connection"
        }
        "[tar 加速] 服务端命令失败（退出码 {1}）：{0} → {2}" => {
            "[tar accel] Server command failed (exit {1}): {0} -> {2}"
        }
        "[tar 加速] 远端 tar+gzip 不可用，回退逐文件传输" => {
            "[tar accel] Remote tar+gzip unavailable, falling back to per-file transfer"
        }
        "[tar 加速] 本地打包失败：{0}" => "[tar accel] Local packing failed: {0}",
        "[命令通道] Windows 服务器, 暂不支持服务端哈希, 校验码菜单已禁用" => {
            "[cmd] Windows server, server-side hashing unsupported, checksum menu disabled"
        }
        "[命令通道] 不可用, 校验跳过（本机回退未实现）: {0}" => {
            "[cmd] unavailable, checksum skipped (no local fallback): {0}"
        }
        "[命令通道] 不可用, 校验跳过（本机回退未实现）: 解析异常输出 {0}" => {
            "[cmd] unavailable, checksum skipped (no local fallback): unexpected output {0}"
        }
        "[命令通道] 服务端哈希探测: sha256sum={0} sha512sum={1} SM3(openssl)={2}" => {
            "[cmd] server hash probe: sha256sum={0} sha512sum={1} SM3(openssl)={2}"
        }
        "[校验] 校验不匹配：{0}（本地 {1} / 远端 {2}）" => {
            "[verify] Mismatch: {0} (local {1} / remote {2})"
        }
        "[校验] 校验跳过：{0}" => "[verify] Skipped: {0}",
        "[校验] 校验通过：{0}（{1}）" => "[verify] Matched: {0} ({1})",
        "[续传] 目标大于源（内容可能已变），全量重传：{0}（目标 {1} 字节 / 源 {2} 字节）" => {
            "[resume] Destination is larger than source (content may have changed), re-sending in full: {0} (dest {1} bytes / source {2} bytes)"
        }
        "[续传] 追加失败，回退全量传输：{0}（{1}）" => {
            "[resume] Append failed, falling back to full transfer: {0} ({1})"
        }
        "hosts.json 读取失败（{1}），原文件已备份到 {0}" => {
            "hosts.json read failed ({1}), original file backed up to {0}"
        }
        "iFreeSftp" => "iFreeSftp",
        "iFreeSftp 存档 (JSON)" => "iFreeSftp archive (JSON)",
        "如果它对你有帮助，欢迎注册成为支持者——这是对开发的支持，不与任何功能绑定。" => {
            "If you find it useful, please consider registering as a supporter — a purely voluntary way to support development, not a feature unlock."
        }
        "keep-alive" => "keep-alive",
        "keep-alive 连接" => "Keep alive",
        "keep-alive 间隔" => "Keep alive interval",
        "tar 加速" => "tar accel",
        "tar 打包加速" => "Tar packing acceleration",
        "tar 文件数阈值" => "Tar file-count threshold",
        "tar 模式为整体传输，失败只能整体重试（无文件级续传）：" => {
            "Tar mode transfers the directory as a whole; a failure restarts everything (no per-file resume):"
        }
        "{0} KB" => "{0} KB",
        "{0} 个" => "{0} task(s)",
        "{0} 个文件" => "{0} files",
        "{0} 个目录" => "{0} folders",
        "{0} 文件" => "{0} file",
        "{0} 次" => "{0} times",
        "{0} 秒" => "{0} s",
        "{0} 项" => "{0} items",
        "上一级" => "Up one level",
        "上传" => "Upload",
        "上传队列" => "Upload queue",
        "下载" => "Download",
        "通过 SFTP 协议逐条递归删除，兼容所有服务器；大目录耗时较长。" => {
            "Deletes item by item over the SFTP protocol; works with every server but is slower for large directories."
        }
        "不关了" => "Keep tab",
        "不可用" => "unavailable",
        "两侧目录不一致，无法建立同步浏览" => {
            "The two sides are out of sync; sync browse cannot start"
        }
        "两次重试之间的等待时间（秒）" => "Wait time between retries (seconds)",

        "中文" => "中文",
        "主机" => "Host",
        "主机命令窗" => "Host console",
        "主机密码解密失败（{0}），已保留原密文；换回原 Windows 账户可恢复，或重新输入密码" => {
            "Failed to decrypt a host password ({0}). The original ciphertext is kept — switch back to the original Windows account to recover it, or re-enter the password."
        }
        "主机密钥已变更" => "Host key has changed",
        "主机密钥校验未通过：存档 {0}，本次 {1}（服务器密钥变更时请重新信任）" => {
            "Host key verification failed: stored {0}, presented {1} (re-trust if the server key changed)"
        }
        "主机未设置同步浏览目录，或一侧未建立同步基准" => {
            "Host has no sync-browse directories, or one side has no sync base"
        }
        "主机设置" => "Host Settings",
        "主机配置读取失败" => "Failed to load host profiles",
        "主题" => "Theme",
        "仅在本机读取您所选来源中的连接元数据（名称、主机、端口、用户名、认证方式、密钥路径）；口令与登录后命令一律不导入。扫描结果只在本弹窗内展示，关闭即丢弃。单文件不超过 8MB，单次最多 5000 条。" => {
            "Only connection metadata (name, host, port, username, auth method, key path) is read locally from the sources you select. Passwords and post-login commands are never imported. Scan results are shown only in this dialog and discarded on close. Files are limited to 8 MB and up to 5,000 sessions per scan."
        }
        "会话数量超过 5000 条上限，已截断 {0} 条。" => {
            "Exceeded the 5,000-session limit, {0} truncated."
        }
        "传输" => "Transfer",
        "传输中" => "Transferring",
        "传输到 {0}" => "Transfer to {0}",
        "传输到另一侧" => "Transfer to other pane",
        "传输后保留修改时间" => "Preserve timestamps after transfer",
        "传输成功时把源文件的修改时间同步到目标侧（tar 路径经 tar 头天然保留）；仅同步文件，目录不做；失败只记日志。默认开启。" => {
            "Sync the source file's last-modified time to the destination after each transfer (the tar path preserves it via tar headers); files only, not directories. Failures are logged only. On by default."
        }
        "传输后校验" => "Verify after transfer",
        "传输失败" => "Failed",
        "传输完成" => "Completed",
        "完成后经 exec 通道比对本地与远端文件杂凑，不一致的任务标记失败；命令通道不可用时跳过校验，不影响传输结果。默认关闭。" => {
            "After each transfer, file hashes are compared over the exec channel and mismatched tasks are flagged as failed. Skips verification when the command channel is unavailable; never affects the transfer result. Off by default."
        }
        "传输完成：成功 {0} 项，跳过 {1} 项，失败 {2} 项" => {
            "Transfer finished: {0} succeeded, {1} skipped, {2} failed"
        }
        "传输已停止" => "Transfer stopped",
        "传输已取消" => "Transfer cancelled",
        "传输已暂停" => "Transfer paused",
        "传输确认" => "Transfer Confirmation",
        "传输继续" => "Transfer resumed",
        "你拒绝了本次主机指纹，连接中止" => {
            "Host fingerprint rejected by user, connection aborted"
        }
        "主机指纹校验未通过，连接中止" => {
            "Host fingerprint verification failed, connection aborted"
        }
        "保存" => "Save",
        "保存成功" => "Saved successfully",
        "保存编辑结果" => "Save to server",
        "信任存档指纹 {0}" => "Trusting archived fingerprint {0}",
        "信任并继续" => "Trust & Continue",
        "修改时间" => "Modified",
        "停止" => "Stop",
        "先选择一个主机" => "Select a host first",
        "免费使用 · 注册后显示支持者勋章" => {
            "Free to use · register to earn the supporter badge"
        }
        "免费承诺" => "Free for everyone",
        "全部功能对所有用户永久免费开放。注册勋章纯属自愿赞助，不解锁、不限制任何功能。" => {
            "All features are free to use, forever. The supporter badge is entirely voluntary — it unlocks nothing and restricts nothing."
        }
        "公告" => "Notice",
        "共享库" => "Shared Library",
        "共找到 {0} 条会话，其中 {1} 条已存在（不可重复导入）。" => {
            "{0} sessions found, {1} of them already exist (cannot be imported twice)."
        }
        "关于" => "About",
        "关于 iFreeSftp" => "About iFreeSftp",
        "关闭" => "Close",
        "关闭标签" => "Close tab",
        "关闭标签页" => "Close tab",
        "其他" => "Others",
        "内置编辑器" => "Built-in editor",
        "写" => "Write",
        "切换主题" => "Toggle theme",
        "切换树形目录" => "Toggle folder tree",
        "删除" => "Delete",
        "删除失败：{0}（{1}）" => "Delete failed: {0} ({1})",
        "优先用服务端命令 rm -rf 删除远程目录（海量文件秒级）；命令通道不可用或未能删净时回退 SFTP 递归。" => {
            "Deletes remote directories with the server-side command (rm -rf) first — huge directories finish in seconds; falls back to SFTP recursion when the command channel is unavailable or the removal is incomplete."
        }
        "刷新" => "Refresh",
        "副本" => "Copy",
        "单次 SFTP 请求等待响应的上限（秒），超时按错误处理不再挂起" => {
            "Per-request response timeout (seconds); expires as an error instead of hanging"
        }
        "压缩文件" => "Archive File",
        "原因" => "Reason",
        "原文件已备份到 {0}，主机列表已重置为空。可从备份文件手工恢复数据。" => {
            "The original file was backed up to {0}. The host list has been reset — you can restore data from the backup manually."
        }
        "发布渠道" => "Release channels",
        "发现新版本 v{0}，可从官网下载" => {
            "New version v{0} available — download from the website"
        }
        "取消" => "Cancel",
        "取消队列并关闭" => "Cancel queue and close",
        "可用" => "available",
        "传输一律使用独立连接：1 = 单连接串行，大于 1 时多连接并发，多文件传输更快；部分服务器可能限制连接数。" => {
            "Transfers always use their own connections: 1 = one connection, serial; above 1, several connections run in parallel, which speeds up multi-file transfers. Some servers may limit connections."
        }
        "同步" => "Sync",
        "同步浏览" => "Sync browse",
        "同步浏览: 两侧目录不一致, 已关闭联动" => {
            "Sync browse: directories out of sync, linkage disabled"
        }
        "名称" => "Name",
        "后台继续" => "Continue in background",
        "周期发送心跳包，防止长连接被路由器或防火墙断开" => {
            "Send periodic heartbeats so routers or firewalls keep the connection open"
        }
        "图片文件" => "Image File",
        "在哪个标签页打开 {0} 的文件列表？" => {
            "Which tab should open the file list of {0}?"
        }
        "复制" => "Duplicate",
        "复制到粘贴板" => "Copy to clipboard",
        "复制选中" => "Copy selection",
        "剪切" => "Cut",
        "粘贴" => "Paste",
        "全选" => "Select all",
        "本地侧未打开具体目录，无法建立同步" => {
            "Open a specific local folder to establish sync browse"
        }
        "外观" => "Appearance",
        "外部编辑器" => "External editor",
        "大小" => "Size",
        "完成" => "Done",
        "密码" => "Password",
        "密钥文件" => "Key file",
        "密钥文件不存在" => "Key file not found",
        "密钥文件路径" => "Key file path",
        "密钥登录方式需要先选择密钥文件" => {
            "Select a key file for key authentication"
        }
        "对侧未就绪：请先在对侧进入本地目录" => {
            "Other pane not ready: open a local folder there first"
        }
        "导入 ({0})" => "Import ({0})",
        "导入主机" => "Import hosts",
        "导入会话" => "Import Sessions",
        "将永久删除 {0} 项。\n此操作无法恢复。" => {
            "Permanently delete {0} item(s).\nThis cannot be undone."
        }
        "将永久删除：{0}\n此操作无法恢复。" => {
            "Permanently delete: {0}\nThis cannot be undone."
        }
        "小于该值的文件一律全量重传（重传代价可忽略，阈值宜取大不取小）" => {
            "Files smaller than this are always re-sent in full (cost is negligible; prefer a larger threshold)"
        }
        "已从本机来源导入 {0} 条主机" => "Imported {0} hosts from local sources",
        "已传输 {0} 项，跳过 {1} 项" => "Transferred {0} item(s), skipped {1}",
        "已传输 {0} 项，跳过 {1} 项，失败 {2} 项" => {
            "Transferred {0} item(s), skipped {1}, failed {2}"
        }
        "已删除 {0} 项" => "Deleted {0} item(s)",
        "已取消" => "Cancelled",
        "已回传到远端" => "Uploaded back to remote",
        "已复制" => "Copied",
        "已复制 {0} 项到另一侧" => "Copied {0} item(s) to other pane",
        "已存在" => "Already exists",
        "本次扫描中重复" => "Duplicate in this scan",
        "已导入 {0} 条会话" => "Imported {0} sessions",
        "已拦截远程通道下发的链接（非 http/https）" => {
            "Blocked a link from remote channel (not http/https)"
        }
        "已断开" => "Disconnected",
        "已是最新版本" => "You're up to date",
        "已暂停" => "Paused",
        "已更新主机指纹存档，下次连接自动校验" => {
            "Host fingerprint archive updated, will be verified automatically next time"
        }
        "已有传输正在进行，请稍候" => "A transfer is already in progress",
        "已校验" => "Verified",
        "已注册：{0}（{1}）" => "Registered: {0} ({1})",
        "已移入回收站：{0}" => "Moved to Recycle Bin: {0}",
        "已记住主机指纹，下次连接自动校验" => {
            "Host fingerprint saved, will be verified automatically next time"
        }
        "已连接 {0}" => "Connected to {0}",
        "已选 {0} 项" => "{0} selected",
        "已重新连接：{0}" => "Reconnected: {0}",
        "常规" => "General",
        "并发传输数" => "Concurrent transfers",
        "应用" => "Apply",
        "应用到本批所有冲突项" => "Apply to all remaining conflicts in this batch",
        "应用程序" => "Application",
        "建立 SSH 连接的等待上限（秒），超时后按重试设置自动重连" => {
            "Max wait while establishing the SSH connection (seconds); auto-retries on timeout"
        }
        "开启同步" => "Enable sync",
        "中断后从断点继续：目标小于源时追加，否则全量重传；同名旧文件先弹窗确认，不会误追加。默认开启。" => {
            "Resumes from the breakpoint after an interruption: append when the destination is smaller than the source, otherwise re-send in full. A same-name file is confirmed via a dialog first, so nothing is appended by mistake. On by default."
        }
        "开启比较" => "Enable comparison",
        "开始传输 {0} 项 → {1}" => "Transferring {0} item(s) → {1}",
        "开始扫描" => "Start Scan",
        "当前标签页" => "Current tab",
        "心跳包发送周期（秒），需开启 keep-alive 连接" => {
            "Heartbeat period (seconds); requires keep alive enabled"
        }
        "快捷方式" => "Shortcut",
        "总是当前标签页" => "Always current tab",
        "总是执行此操作，不再询问" => "Always do this and don't ask again",
        "总是新标签页" => "Always new tab",
        "总计 {0} 字节" => "total {0} bytes",
        "我已知晓并同意按上述方式仅在本机读取所选来源" => {
            "I understand and agree to read the selected sources locally only"
        }
        "所有者" => "Owner",
        "所选内容将移入回收站，可在回收站还原。" => {
            "The selected items will be moved to the Recycle Bin and can be restored."
        }
        "所选内容将被永久删除，此操作无法恢复。" => {
            "The selected items will be permanently deleted. This cannot be undone."
        }
        "打开" => "Open",
        "打开主机文件" => "Open host files",
        "打开目录" => "Open folder",
        "打开目标" => "Open target",
        "执行" => "Execute",
        "扫描中" => "Scanning",
        "批量删除失败（{0} 项，首项 {1}）：{2}" => {
            "Batch delete failed ({0} items, first: {1}): {2}"
        }
        "操作超时（秒）" => "Operation timeout (s)",
        "操作超时（{0} 秒）" => "Operation timed out ({0} s)",
        "支持开发" => "Support the project",
        "支持者" => "Supporter",
        "文件" => "File",
        "文件夹" => "File folder",
        "文件比对" => "Compare files",
        "文件编辑" => "File editing",
        "文件读取失败" => "Failed to read file",
        "文件超过 10 MB，暂不支持远程编辑" => {
            "File exceeds 10 MB, remote editing is not supported"
        }
        "文件超过 8MB，已跳过" => "File exceeds 8 MB, skipped",
        "文本文档" => "Text Document",
        "文档文件" => "Document File",
        "断开" => "Disconnect",
        "断点续传" => "Resume transfer",
        "新主机" => "New host",
        "新增主机" => "Add host",
        "新建文件" => "New file",
        "新建文件夹" => "New folder",
        "新建文本文档.txt" => "New Text Document.txt",
        "新建标签" => "New tab",
        "新条目" => "New",
        "新标签页" => "New tab",
        "新版本 v{0}" => "New version v{0}",
        "无法连接更新服务，请稍后再试" => {
            "Couldn't reach the update server, try again later"
        }
        "时间" => "Time",
        "显示/隐藏隐藏文件" => "Show/hide hidden files",
        "暂停" => "Pause",
        "暂无主机，点 + 新增" => "No hosts yet. Click + to add one",
        "更新并信任" => "Update and trust",
        "最大重试次数" => "Max retries",
        "最新版本与发布动态见官网 / GitHub 仓库" => {
            "Latest releases and updates — see the website / GitHub repository"
        }
        "有效期至 {0}" => "valid until {0}",
        "有新版本时提醒我" => "Notify me on new versions",
        "服务器 {0} 出示的主机密钥与此前信任的不一致。\n\n此前信任：{1}\n本次出示：{2}\n\n常见原因：服务器重装/更换机器，或主机卡片曾修改过 IP。\n若你知晓此变更，可选择更新信任；否则请拒绝，以防中间人攻击。" => {
            "The host key presented by {0} does not match the previously trusted one.\n\nPreviously trusted: {1}\nPresented now: {2}\n\nCommon causes: server reinstalled/replaced, or the host card's IP was edited.\nUpdate and trust only if you are aware of this change; otherwise reject to prevent man-in-the-middle attacks."
        }
        "服务器在超时时间内无响应（可能连接数已达上限或服务器繁忙）。可稍后重试或断开重连。" => {
            "Server did not respond within the timeout (connection limit reached or server busy). Retry later or reconnect."
        }
        "服务端命令删除失败，已回退 SFTP 递归删除" => {
            "Server-side delete failed, fell back to SFTP recursive delete"
        }
        "服务端复制失败（服务器不支持 cp 或路径无效）" => {
            "Server-side copy failed (server does not support cp or invalid path)"
        }
        "服务端打包中" => "Packing on server",
        "未找到可导入的新会话" => "No new sessions found to import",
        "未返回错误详情" => "no error details returned",
        "本地" => "Local",
        "本地同步路径" => "Local sync path",
        "本地文件系统" => "Local file system",
        "本地站点" => "Local site",
        "本应用以 GNU AGPL-3.0 许可证开源发布。ifrees.top 发布版额外集成了独立分发的增值组件，这些组件不包含在本仓库源码。" => {
            "The app is released as open source under the GNU AGPL-3.0 license. The release from ifrees.top additionally bundles separately distributed value-added components that are not part of this repository's source."
        }
        "权限" => "Permissions",
        "权限已更新为 {0}" => "Permissions updated to {0}",
        "校验不匹配" => "Mismatch",
        "校验不匹配（本地 {0} / 远端 {1}）" => {
            "Checksum mismatch (local {0}, remote {1})"
        }
        "校验中" => "Verifying",
        "校验码" => "Checksum",
        "校验算法" => "Hash algorithm",
        "校验跳过" => "Verify skipped",
        "检查更新" => "Check for updates",
        "正在传输 {0}…" => "Transferring {0}…",
        "正在准备传输（探测目标/统计文件）…" => {
            "Preparing transfer (checking target/counting files)…"
        }
        "已落位 {0} 项已开始传输，本次确认只处理剩余 {1} 项（其中冲突 {2} 项）" => {
            "{0} items already queued and transferring; this confirmation only covers the remaining {1} ({2} conflicts)"
        }
        "预检超时：{0} 项未完成探测，已按远端不存在放行（传输时按续传决策兜底）" => {
            "Precheck timeout: {0} items were not probed and queued as if absent (resume policy decides at transfer time)"
        }

        "已跳过 {0} 个冲突项，无剩余任务入队" => {
            "Skipped {0} conflicting items; no tasks left to queue"
        }
        "已取消传输确认：{0} 项未入队" => {
            "Transfer confirmation canceled: {0} items not queued"
        }
        "已放弃上一批待确认的 {0} 项（未入队）" => {
            "Abandoned {0} unconfirmed items from the previous batch (not queued)"
        }

        "已暂存 {0} 项" => "{0} items staged",
        "入队" => "Queue",
        "放弃暂存" => "Discard stash",
        "已暂存 {0} 项（Shift 拖入）——点击面板底部「入队」开始传输" => {
            "Staged {0} items (Shift-drop) — click \"Queue\" on the pane status bar to start"
        }
        "已追加 {0} 项到暂存（共 {1} 项）——点击面板底部「入队」开始传输" => {
            "Added {0} items to the stash ({1} total) — click \"Queue\" on the pane status bar to start"
        }
        "已放弃上一批暂存的 {0} 项（未入队）" => {
            "Abandoned {0} staged items from the previous batch (not queued)"
        }
        "已放弃暂存的 {0} 项（未入队）" => "Discarded {0} staged items (not queued)",
        "暂存 {0} 项已入队（探测目标/统计文件）" => {
            "Queued {0} staged items (checking target/counting files)"
        }
        "目标远程会话已断开，暂存 {0} 项已放弃" => {
            "Target session disconnected; discarded {0} staged items"
        }
        "外部拖入已忽略：{0}" => "External drop ignored: {0}",

        "需两侧为本地+远程目录配对" => {
            "Needs a local + remote directory pair across the two panes"
        }
        "需先开启同步浏览" => "Needs sync browse enabled first",
        "目标面板不在远程目录" => "target pane is not in a remote directory",
        "目标远程会话未连接" => "target remote session is not connected",
        "无可上传的条目" => "nothing uploadable in the drop",

        "勾选的冲突项覆盖上传，未勾选的跳过" => {
            "Checked conflicts are overwritten and uploaded; unchecked ones are skipped"
        }
        "正在扫描…" => "Scanning…",
        "正在检查…" => "Checking…",
        "正在连接 {0}:{1} …" => "Connecting to {0}:{1} …",
        "正在重连…" => "Reconnecting…",
        "此标签页仍有 {0} 个进行中的传输任务，如何处理？" => {
            "This tab still has {0} active transfer tasks. What would you like to do?"
        }
        "此电脑" => "This PC",
        "每次询问" => "Ask every time",
        "永久有效" => "permanent",
        "注册" => "Registration",
        "注册已被吊销，请重新激活" => "Registration revoked, please activate again",
        "注册状态" => "Registration status",
        "注册码" => "Registration code",
        "注册码已过期" => "Registration code has expired",
        "注册码校验失败" => "Registration code verification failed",
        "注册码格式无效" => "Invalid registration code format",
        "注册表读取失败" => "Failed to read registry",
        "浅色" => "Light",
        "测试连接" => "Test connection",
        "浏览" => "Browse",
        "浏览…" => "Browse…",
        "浏览本地文件" => "Browse local files",
        "深色" => "Dark",
        "清除全部" => "Clear all",
        "清除已完成" => "Clear completed",
        "清除所有" => "Clear all",
        "源" => "Source",
        "源代码文件" => "Source Code File",
        "激活" => "Activate",
        "激活成功，感谢支持！" => "Activated. Thank you for your support!",
        "激活组件未随本仓库源码分发，请使用 ifrees.top 发布版" => {
            "Activation is not included in this repository's source; use the release from ifrees.top"
        }
        "点击主机卡片连接时，右侧窗口的打开方式" => {
            "How the right pane opens when connecting from a host card"
        }
        "版本 {0} · 构建于 {1}" => "Version {0} · built {1}",
        "版权" => "Copyright",
        "状态" => "Status",
        "用户名" => "Username",
        "界面显示语言" => "Language used across the app",
        "界面配色跟随系统，或手动指定深浅" => {
            "Follow the system theme, or pick dark or light manually"
        }
        "跟随系统" => "Follow system",
        "登录方式" => "Authentication",
        "登录重试延迟（秒）" => "Retry delay (s)",
        "目录不存在: {0}" => "Directory not found: {0}",
        "目录文件数达到阈值时用单流打包替代逐文件往返（海量小文件数量级提速）；远端 tar+gzip 不可用或本地扫描失败时回退逐文件传输，开始传输后失败只能整体重试。" => {
            "For directories with many files, a single packed stream replaces per-file round trips (orders of magnitude faster for huge numbers of small files). Falls back to per-file transfer when remote tar+gzip is unavailable or the local scan fails; once started, a failure restarts the whole transfer."
        }
        "目录内文件数达到该值才改用单流打包，少量文件逐文件直传更快" => {
            "Switch to single-stream packing only when a directory contains at least this many files; per-file transfer is faster for small batches."
        }
        "目录展开失败，已跳过：{0}（{1}）" => {
            "Failed to enumerate directory, skipped: {0} ({1})"
        }
        "目标" => "Destination",
        "目标位置已存在同名条目：“{0}”。是否覆盖？" => {
            "\"{0}\" already exists at the destination. Overwrite?"
        }
        "知道了" => "OK",
        "确认主机指纹" => "Confirm host fingerprint",
        "移入回收站失败（文件可能被占用或已不存在）。" => {
            "Failed to move to the Recycle Bin (file may be in use or no longer exists)."
        }
        "移除" => "Remove",
        "端口" => "Port",
        "等 {0} 项" => "and {0} more item(s)",
        "等待中" => "Waiting",
        "等待主机指纹确认 …" => "Waiting for host fingerprint confirmation …",
        "算法" => "Algorithm",
        "类型" => "Type",
        "粘贴收到的注册码，点击激活" => "Paste the code you received, then activate",
        "终端" => "Terminal",
        "继续传输" => "Resume transfer",
        "续传中" => "Resuming",
        "续传阈值" => "Resume threshold",
        "编辑" => "Edit",
        "编辑主机" => "Edit host",
        "编辑器路径" => "Editor path",
        "编辑方式" => "Editor",
        "群组" => "Group",
        "脚本文件" => "Script File",
        "自动" => "Auto",
        "自动：按可用性顺位选用（SHA256 → SHA512 → SM3）；固定算法不可用时校验跳过" => {
            "Auto: pick the first available algorithm in order (SHA256 → SHA512 → SM3). A fixed algorithm that is unavailable skips verification."
        }
        "覆盖" => "Overwrite",
        "视频文件" => "Video File",
        "解包中" => "Unpacking",
        "解析失败" => "Failed to parse",
        "计算中…" => "Computing…",
        "认证" => "Auth",
        "许可证" => "License",
        "设置" => "Settings",
        "访问官网" => "Visit website",
        "语言" => "Language",
        "请先填写 IP 地址和登录名" => "Enter IP address and username first",
        "读" => "Read",
        "购买注册码 (爱发电)" => "Buy a license (Afdian)",
        "路径" => "Path",
        "跳过" => "Skip",
        "返回" => "Back",
        "远程" => "Remote",
        "远程删除方式" => "Remote deletion",
        "远程删除：{0}" => "Remote delete: {0}",
        "远程同步路径" => "Remote sync path",
        "远程站点" => "Remote site",
        "远端文件已被修改，仍要覆盖吗？" => {
            "The remote file has been modified. Overwrite anyway?"
        }
        "连接" => "Connection",
        "连接中…" => "Connecting…",
        "连接失败" => "Connection failed",
        "连接失败后的自动重试次数" => {
            "Automatic retry attempts after a failed connection"
        }
        "连接失败：{0}" => "Connection failed: {0}",
        "连接失败：{0}，{1} 秒后进行第 {2}/{3} 次重试 …" => {
            "Connection failed: {0}. Retrying (attempt {2}/{3}) in {1} s …"
        }
        "连接成功" => "Connection successful",
        "连接已断开" => "Disconnected",
        "连接打开方式" => "Open connections in",
        "连接超时（秒）" => "Connect timeout (s)",
        "选择主机" => "Choose host",
        "外部编辑器可执行文件（如 Sublime Text、VS Code）；留空使用内置编辑器。" => {
            "Executable of the external editor (e.g. Sublime Text, VS Code); leave empty to use the built-in editor."
        }
        "选择打开文件使用的编辑器" => "Which editor opens when editing files",
        "选择要扫描的来源" => "Choose sources to scan",
        "选择要浏览的本地或远端文件系统" => {
            "Choose a local or remote file system to browse"
        }
        "速度" => "Speed",
        "重命名" => "Rename",
        "重置并重新请求所有" => "Reset and re-request all",
        "重置并重新请求所有：{0} 项" => "Reset and re-request all: {0} items",
        "重试" => "Retry",
        "重连" => "Reconnect",
        "重连失败：{0}" => "Reconnect failed: {0}",
        "间隔秒数" => "Interval (s)",
        "队列中" => "Queued",
        "非文本文件，已取消打开" => "Not a text file, opening cancelled",
        "音频文件" => "Audio File",
        "项目名称" => "Name",
        "首次连接 {0}，无法验证主机身份。\n\n主机密钥指纹 (SHA256)：\n{1}\n\n信任该主机并继续连接？" => {
            "Connecting to {0} for the first time. The host identity cannot be verified.\n\nHost key fingerprint (SHA256):\n{1}\n\nTrust this host and continue?"
        }

        "关闭 {0}" => "Close {0}",
        "[远程]" => "[Remote]",
        "主机尚未保存进存档，指纹仅本会话生效" => {
            "Host not saved to profiles yet; the fingerprint only lasts for this session"
        }
        "传输连接超时" => "Transfer connection timed out",
        "内部错误：目录任务未展开" => "Internal error: directory task was not expanded",
        "内部错误：非目录任务" => "Internal error: not a directory task",
        "加载中…" => "Loading…",
        "正在删除…" => "Deleting…",
        "目录读取失败" => "Failed to read directory",
        "同步浏览: 两侧目录仍不一致" => "Sync browse: directories still out of sync",
        "命令无输出" => "Command produced no output",
        "命令超时（10 秒）" => "Command timed out (10 s)",
        "密钥文件 (*.ppk;*.pem;*.key)" => "Key files (*.ppk;*.pem;*.key)",
        "已关闭同步联动。可重新定位两侧目录后手动建立。" => {
            "Sync linkage disabled. Re-align the two sides to establish it again."
        }
        "平台提示" => "Platform notice",
        "当前平台暂不支持凭据加密存储，注册码仅本次会话有效，重启后需重新激活。" => {
            "This platform does not yet support encrypted credential storage; the license is valid for this session only and must be re-activated after a restart."
        }
        "所有文件 (*.*)" => "All files (*.*)",
        "指纹确认超时，连接中止" => {
            "Fingerprint confirmation timed out, connection aborted"
        }
        "无远程连接档案" => "No remote connection profile",
        "未连接远程主机" => "Not connected to a remote host",
        "正在建立 SSH 连接" => "Establishing SSH connection",
        "正在连接…" => "Connecting…",
        "注册码格式错误" => "Invalid license key format",
        "源不是文件" => "Source is not a file",
        "激活组件未随本仓库源码分发，请从 ifrees.top 下载发布版。" => {
            "Activation is not included in this repository's source; download the release from ifrees.top."
        }

        "版本 {0}（Rust 迁移版）" => "Version {0} (Rust migration)",
        "认证失败" => "Authentication failed",
        "认证失败：用户名或密码错误" => {
            "Authentication failed: wrong username or password"
        }
        "认证失败：私钥被服务器拒绝" => {
            "Authentication failed: the server rejected the private key"
        }
        "连接不可用（内部状态异常）" => {
            "Connection unavailable (unexpected internal state)"
        }
        "选择密钥文件" => "Select key file",
        "选择左侧主机查看与编辑" => "Select a host on the left to view and edit",
        "重新建立" => "Re-establish",
        "{0} 条会话无效已跳过（缺少主机名或端口无效）。" => {
            "{0} invalid sessions were skipped (missing hostname or invalid port)."
        }

        "[tar 加速] 本地目录扫描失败，回退逐文件：{0}" => {
            "[tar accel] Local directory scan failed, falling back to per-file: {0}"
        }
        "[{0}] 上传编辑副本…" => "[{0}] Uploading edited copy…",

        "[{0}] 回传未完成：{1}" => "[{0}] Upload incomplete: {1}",
        "回传未完成：{0}" => "Upload incomplete: {0}",
        "未找到主机会话" => "Host session not found",
        "正在回传…" => "Uploading…",
        "已回传 {0} 字节 · {1}" => "Uploaded {0} bytes · {1}",
        "{0} 已回传 {1} 字节，远端 mtime {2}：{3}" => {
            "{0} uploaded {1} bytes, remote mtime {2}: {3}"
        }
        "[中转] 阶段 1/2 下载：{0} → 本地中转" => {
            "[relay] Stage 1/2 download: {0} -> local relay"
        }
        "[中转] 阶段 2/2 上传：本地中转 → {0}" => {
            "[relay] Stage 2/2 upload: local relay -> {0}"
        }
        "[传输] 失败：{0} → {1}：{2}" => "[transfer] Failed: {0} -> {1}: {2}",
        "[本地] 批量删除失败 {0} 项：{1}" => {
            "[local] Batch delete failed for {0} item(s): {1}"
        }
        "[本地] 删除失败 {0}：{1}" => "[local] Delete failed {0}: {1}",
        "[本地] 已删除 {0}" => "[local] Deleted {0}",
        "[续传] 从断点续传：{0}（{1}/{2} 字节）" => {
            "[resume] Resuming from offset: {0} ({1}/{2} bytes)"
        }
        "{0} 上传 {1} 失败：{2}" => "{0} upload of {1} failed: {2}",
        "{0} 下载 {1} 失败：{2}" => "{0} download of {1} failed: {2}",
        "{0} 已上传编辑副本 {1}" => "{0} uploaded edited copy {1}",
        "{0} 已下载 {1}（{2} 字节）到临时副本" => {
            "{0} downloaded {1} ({2} bytes) to a temp copy"
        }
        "下载 {0}…" => "Downloading {0}…",
        "中转件转正失败：{0}" => "Failed to finalize relay file: {0}",
        "传输连接失败：{0}" => "Transfer connection failed: {0}",
        "保存失败：{0}（存档未改动，修复后重试）" => {
            "Save failed: {0} (archive unchanged; fix the problem and retry)"
        }
        "写入后取元数据失败：{0}" => "Failed to fetch metadata after write: {0}",
        "写入失败：{0}" => "Write failed: {0}",
        "写入目标失败：{0}" => "Failed to write destination: {0}",
        "写入远端失败：{0}" => "Failed to write to remote: {0}",
        "创建中转目录失败：{0}" => "Failed to create relay directory: {0}",
        "创建临时目录失败：{0}" => "Failed to create temp directory: {0}",
        "合并目录失败：{0}" => "Failed to merge directory: {0}",
        "命令退出码 {0}" => "Command exit code {0}",
        "复制落盘失败：{0}（副本已加入列表，保存可重试）" => {
            "Failed to copy to disk: {0} (the entry was still added; use Save to retry)"
        }
        "导入落盘失败 {0} 条（已加入列表，主机面板「保存」可重试）" => {
            "Failed to persist {0} imported entries (they are in the list; use Save in the host panel to retry)"
        }
        "打开源失败：{0}" => "Failed to open source: {0}",
        "打开目标失败：{0}" => "Failed to open destination: {0}",
        "打开远端临时文件失败：{0}" => "Failed to open remote temp file: {0}",
        "指纹已在本会话生效，但写入存档失败：{0}" => {
            "The fingerprint is active for this session, but saving it to profiles failed: {0}"
        }
        "新建文件失败：{0}（{1}）" => "Failed to create file: {0} ({1})",
        "新建文件夹失败：{0}（{1}）" => "Failed to create folder: {0} ({1})",
        "远程重命名：{0} → {1}" => "Remote rename: {0} → {1}",
        "重命名失败：{0}（{1}）" => "Rename failed: {0} ({1})",
        "保留修改时间" => "Preserve modified time",
        "服务端命令超时：{0}" => "Server command timed out: {0}",
        "本机解包任务失败：{0}" => "Local unpack task failed: {0}",
        "枚举源目录失败：{0}" => "Failed to enumerate source directory: {0}",
        "源不可读：{0}" => "Source is not readable: {0}",
        "激活未完成：{0}（注册码未能保存，请重试）" => {
            "Activation incomplete: {0} (the license key was not saved; please retry)"
        }
        "目录落位失败：{0}" => "Failed to move directory into place: {0}",
        "目标已存在 {0} 个同名项目：" => {
            "The destination already contains {0} item(s) with the same name:"
        }
        "私钥读取失败：{0}" => "Failed to read private key: {0}",
        "端口无效：{0}（需 1-65535），连接未发起" => {
            "Invalid port: {0} (must be 1-65535); connection not attempted"
        }
        "计算失败：{0}" => "Hash computation failed: {0}",
        "认证超时（{0} 秒）" => "Authentication timed out ({0} s)",
        "设置保存失败：{0}" => "Failed to save settings: {0}",

        "设置未保存，正在自动重试" => "Settings not saved - retrying automatically",
        "读取失败：{0}" => "Read failed: {0}",
        "读取源失败：{0}" => "Failed to read source: {0}",
        "读取源文件失败：{0}" => "Failed to read source file: {0}",
        "远程新建文件夹：{0}" => "New remote folder: {0}",
        "远程新建文件：{0}" => "New remote file: {0}",
        "连接超时（{0} 秒）" => "Connection timed out ({0} s)",

        "行 {0}, 列 {1}" => "Row {0}, Column {1}",
        "此源码编译版不含服务端直达复制组件；请分两步传输（先下载到本机，再上传到目标）" => {
            "This source build does not include the server-to-server direct copy component; transfer in two steps (download to local, then upload to the target)"
        }
        _ => zh,
    }
}

pub fn tfmt(zh: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::from(t(zh));
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), &a.to_string());
    }
    out
}

pub fn t_ctx(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { zh }
}

#[cfg(test)]
mod tests {

    const LOG_ONLY_WHITELIST: &[&str] = &[

    ];

    fn scan_call_literals(text: &str, out: &mut Vec<String>) {
        let bytes = text.as_bytes();
        let n = bytes.len();
        let mut i = 0;
        let mut block_comment = 0usize;
        let mut in_line_comment = false;
        let mut in_string = false;
        while i < n {
            let b = bytes[i];
            if in_line_comment {
                if b == b'\n' {
                    in_line_comment = false;
                }
                i += 1;
                continue;
            }
            if block_comment > 0 {
                if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    block_comment += 1;
                    i += 2;
                } else if b == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    block_comment -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if in_string {
                match b {
                    b'\\' => i += 2,
                    b'"' => {
                        in_string = false;
                        i += 1;
                    }
                    _ => i += 1,
                }
                continue;
            }
            match b {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    in_line_comment = true;
                    i += 2;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    block_comment = 1;
                    i += 2;
                }
                b'"' => {
                    in_string = true;
                    i += 1;
                }

                b'\'' => {
                    let mut j = i + 1;
                    let mut closed = None;
                    while j < n && bytes[j] != b'\n' && j - i <= 16 {
                        match bytes[j] {
                            b'\\' => j += 2,
                            b'\'' => {
                                closed = Some(j);
                                break;
                            }
                            _ => j += 1,
                        }
                    }
                    i = match closed {
                        Some(e) => e + 1,
                        None => i + 1,
                    };
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    let start = i;
                    while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                    let token = &text[start..i];

                    if (token == "r" || token == "br")
                        && i < n
                        && (bytes[i] == b'"' || bytes[i] == b'#')
                    {
                        let mut hashes = 0usize;
                        let mut j = i;
                        while j < n && bytes[j] == b'#' {
                            hashes += 1;
                            j += 1;
                        }
                        if j < n && bytes[j] == b'"' {
                            let mut end = None;
                            let mut k = j + 1;
                            while k + 1 + hashes <= n {
                                if bytes[k] == b'"'
                                    && bytes[k + 1..k + 1 + hashes].iter().all(|&c| c == b'#')
                                {
                                    end = Some(k + 1 + hashes);
                                    break;
                                }
                                k += 1;
                            }
                            i = end.unwrap_or(n);
                            continue;
                        }
                    }
                    if (token == "t" || token == "tfmt") && i < n && bytes[i] == b'(' {
                        let mut j = i + 1;
                        while j < n && matches!(bytes[j], b' ' | b'\t' | b'\r' | b'\n' | 0x0C) {
                            j += 1;
                        }
                        if j < n
                            && bytes[j] == b'"'
                            && let Some(s) = extract_quoted(&text[j..])
                        {
                            out.push(s);
                        }
                    }
                }
                _ => i += 1,
            }
        }
    }

    fn extract_quoted(s: &str) -> Option<String> {
        let bytes = s.as_bytes();
        let mut out = String::new();
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => {
                    let n = bytes.get(j + 1).copied()?;
                    out.push(match n {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        other => other as char,
                    });
                    j += 2;
                }
                b'"' => return Some(out),
                _ => {
                    let l = utf8_len(bytes[j]);
                    out.push_str(s.get(j..j + l)?);
                    j += l;
                }
            }
        }
        None
    }

    fn utf8_len(b: u8) -> usize {
        match b {
            0x00..=0x7F => 1,
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            _ => 4,
        }
    }

    fn extract_first_literal(line: &str) -> Option<String> {
        let bytes = line.as_bytes();
        let start = line.find('"')?;
        let mut s = String::new();
        let mut j = start + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => {
                    let n = bytes.get(j + 1).copied().unwrap_or(b'"');
                    s.push(match n {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        other => other as char,
                    });
                    j += 2;
                }
                b'"' => return Some(s),
                _ => {
                    let ch_len = utf8_len(bytes[j]);
                    s.push_str(line.get(j..j + ch_len)?);
                    j += ch_len;
                }
            }
        }
        None
    }

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read src dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                collect_rs_files(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }

    #[test]
    fn t_literals_all_have_match_arms() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let src = std::path::Path::new(manifest).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);

        let mut used: Vec<String> = Vec::new();
        for f in &files {
            if f.file_name().is_some_and(|n| n == "i18n.rs") {
                continue;
            }
            let body = std::fs::read_to_string(f).expect("read source");
            scan_call_literals(&body, &mut used);
        }

        used.retain(|u| !LOG_ONLY_WHITELIST.contains(&u.as_str()));

        assert!(
            !used.is_empty(),
            "守卫哨兵：全仓没扫到任何 t()/tfmt() 调用，扫描器已失灵"
        );
        assert!(
            used.len() > 300,
            "守卫哨兵：扫到 {} 条调用，远低于历史水位，扫描器疑似漏扫",
            used.len()
        );

        let table = std::fs::read_to_string(std::path::Path::new(manifest).join("src/i18n.rs"))
            .expect("read i18n.rs");
        let lines: Vec<&str> = table.lines().collect();
        let mut keys: Vec<String> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let l = line.trim_start();
            if l.starts_with("//") || !l.starts_with('"') {
                continue;
            }
            let next_is_arrow = lines
                .get(i + 1)
                .is_some_and(|n| n.trim_start().starts_with("=>"));
            if l.contains("\" =>") || next_is_arrow {
                if let Some(s) = extract_first_literal(l) {
                    keys.push(s);
                }
            }
        }

        assert!(
            keys.len() > 400,
            "守卫哨兵：只解析出 {}/411+ 表键，match 臂解析已失灵",
            keys.len()
        );

        let mut missing: Vec<&String> = used.iter().filter(|u| !keys.contains(u)).collect();
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "i18n 缺英文分支 {} 条（英文界面会回退中文）：\n{}",
            missing.len(),
            missing
                .iter()
                .map(|m| format!("  {m:?}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn extractor_sees_tfmt_and_multiline_not_format() {
        let mut got = Vec::new();

        scan_call_literals("let s = tfmt(\"X\", &[1]);", &mut got);
        assert_eq!(got, vec!["X"]);

        got.clear();
        scan_call_literals("foo(\n    t(\n        \"Y\"\n    ),\n);", &mut got);
        assert_eq!(got, vec!["Y"]);

        got.clear();
        scan_call_literals(
            "let a = format!(\"Z\"); let b: Result<u8,_> = Ok(1); let _ = b.expect(\"W\");",
            &mut got,
        );
        assert!(got.is_empty(), "format!/expect( 被误扫: {got:?}");

        got.clear();
        scan_call_literals(
            "let hint = \"call t(\\\"S\\\") to translate\"; // t(\"C1\")\n/* t(\"C2\") */ let _ = t(\"T\");",
            &mut got,
        );
        assert_eq!(got, vec!["T"]);

        got.clear();
        scan_call_literals(
            "let x = Stat(\"A\"); let y = 1; let z = v.split(\"B\"); tfmt(\"C\", &[]);",
            &mut got,
        );
        assert_eq!(got, vec!["C"]);
    }

    #[test]
    fn extractor_survives_char_literals_and_raw_strings() {
        let mut got = Vec::new();

        scan_call_literals(
            "if c == b'\"' { let _ = t(\"T\"); }\n    t(\"关闭\");",
            &mut got,
        );
        assert_eq!(got, vec!["T", "关闭"]);

        got.clear();
        scan_call_literals(
            "let q: &'static str = t(\"L\"); let e = b'\\''; let _ = t(\"M\");",
            &mut got,
        );
        assert_eq!(got, vec!["L", "M"]);

        got.clear();
        scan_call_literals("let s = r#\"t(\"Z\")\"#; let _ = t(\"W\");", &mut got);
        assert_eq!(got, vec!["W"]);
    }
}
