//! tar 目录加速（C# TarAccelService 全套移植；T15 嵌入化：本机 tar 子进程全删，
//! 打包/解包改 tar crate + flate2 gzip 纯 Rust 内嵌实现，远端服务器命令保留）。
//!
//! 触发（C# CanUse 口径）：设置开 && 文件数 ≥ 阈值（且 >0）&& 仅上传/下载整目录
//! （中转/本地复制不走）。判定时机在 worker 领取目录任务时（C# 在 UI 入队时判定，
//! 等价后移；未接管才走 expand 展开逐文件）。
//!
//! - 上传：本地有界计数 → 远端探测（POSIX + gzip）→ `tar::Builder`（复用 T5
//!   有界扫描范式逐条 append，禁 append_dir_all 裸奔；header 保 mtime/权限）+
//!   `GzEncoder` 压缩块经通道桥流式直写远端临时 .tar.gz → 服务端
//!   `mkdir -p tmp && tar -xzf` → sftp rename 原子切换（目标已存在 → 直接解包
//!   覆盖；rename 失败 → 记日志后降级直接解包覆盖）；
//! - 下载：服务端 `tar -czf /tmp/….tar.gz -C <dir> .` → SFTP 单文件下载（复用
//!   download_file：检查点/进度全继承）→ `GzDecoder` + `tar::Archive::unpack`
//!   解到同级临时目录 → 目标不存在 rename 落位 / 已存在合并覆盖（C#
//!   MergeDirectory）。
//!
//! 临时命名：目标父目录下 `.ifree-tar-<guid>.tar.gz` / `.ifree-tar-<guid>`；
//! 看门狗 = 每步 256KB 收/发 60s 无进展判僵死（C# 1s 轮询 60s 无进展的等价口径）；
//! exec 快罩顶 60s（探测/计数/清理/切换）、长罩顶 600s（服务端打包/解包）。
//! 回退链两级：远端 tar+gzip 走不通（非 POSIX / 缺 gzip）→ 带日志回退逐文件；
//! 执行中失败 → 整体失败（reason 前缀「tar 模式为整体传输…」，C# TarFailWrapped，
//! 含 TarStalledException）；finally 清理远端 rm -f/rm -rf 与本地临时件
//! （失败仅记日志）。tar 头按 PAX/UTF-8 写，中文文件名不依赖系统代码页
//! （有意优于 C# 的 bsdtar 代码页风险，MIGRATION.md 有分歧记录）；
//! tar 天然保留 mtime/权限；tar 任务无 verify、无文件级续传（重试整体重来）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use super::{
    CHUNK, Outcome, Profile, SPEED_WINDOW, Shared, cfg_of, checkpoint, download_file, ensure_conn,
    join_remote, log, progress,
};
use crate::i18n::{t, tfmt};
use crate::ssh::{self, SftpConn};

// ── 决策子集（门判据/打包流协议/回退条件/罩顶常量）：official 随闭源 core
//    分发（ifree-core/src/transfer/tar.rs）；源码编译版保留等价钉子，行为不变。
#[cfg(feature = "official")]
use crate::ifree_core::transfer::tar::{EXEC_LONG_SECS, EXEC_QUICK_SECS, STALL_SECS};
#[cfg(not(feature = "official"))]
const STALL_SECS: u64 = 60;
#[cfg(not(feature = "official"))]
const EXEC_QUICK_SECS: u64 = 60;
#[cfg(not(feature = "official"))]
const EXEC_LONG_SECS: u64 = 600;

/// 看门狗：单步（256KB 收/发、通道消息、rename）无进展判僵死。
const STALL: Duration = Duration::from_secs(STALL_SECS);
/// exec 快罩顶（POSIX 探测 / 文件计数 / 临时件清理 / rename 降级命令）。
const EXEC_QUICK: Duration = Duration::from_secs(EXEC_QUICK_SECS);
/// exec 长罩顶（服务端打包 / 服务端解包大目录）。
const EXEC_LONG: Duration = Duration::from_secs(EXEC_LONG_SECS);

/// tar 子流程结局：Fallback = 走不通，静默回退逐文件展开（C# return false）。
enum TarOut {
    Done,
    Failed(String),
    Paused,
    Cancelled,
    Fallback,
}

/// 目录任务领取时尝试 tar 加速；None = 不走 tar（设置关 / 文件数不足 /
/// 前置探测不过），调用方照旧 expand 展开逐文件。
pub(super) async fn maybe_run(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    kind: &super::Kind,
    profile: &Option<Profile>,
) -> Option<Outcome> {
    // CR-4：tar 门与建连超时读该任务行固化的批次快照（与扫描线程预检同一份）。
    let cfg = cfg_of(shared, id);
    let (on, threshold) = (cfg.tar_accel, cfg.tar_threshold);
    if !on || threshold <= 0 {
        return None;
    }
    // 扫描线程 tar 预检旗标（T20-2）：预检已达标回交的行免重复计数——
    // 失败日志与远端计数在扫描线程已出过，此处直接按阈值放行。
    let prechecked = shared
        .st
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .map(|i| i.precheck)
        == Some(true);
    match kind {
        super::Kind::UploadDir { local, remote } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                return Some(Outcome::Failed(e));
            }
            // 远端探测（T15：POSIX + gzip；本地打包是内嵌实现，无需探测本机）。
            // 不过 = 已带日志，回退逐文件。
            if !probe_remote_tar(shared, profile.as_ref(), conn).await {
                return None;
            }
            // 本地有界计数（数到阈值即停）。扫描失败 = 显式 Err（带路径）：
            // 记日志后不走 tar，回退展开逐文件（错误由逐文件路径显式报）。
            let count = if prechecked {
                Ok(threshold as i64)
            } else {
                count_local_files(local, threshold as i64)
            };
            if let Err(e) = &count {
                log(
                    shared,
                    profile.as_ref(),
                    tfmt("[tar 加速] 本地目录扫描失败，回退逐文件：{0}", &[e]),
                );
            }
            // 门判据收口（C# CanUse 口径，gate_open 见下）。
            if !gate_open(on, threshold as i64, true, count.ok()) {
                return None;
            }
            let name = local
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| local.display().to_string());
            log(
                shared,
                profile.as_ref(),
                tfmt(
                    "[tar 加速] {0}（文件数 ≥ {1}，单流打包）",
                    &[&name, &threshold],
                ),
            );
            to_outcome(upload_dir(shared, conn, id, local, remote, profile.as_ref()).await)
        }
        super::Kind::DownloadDir { remote, local } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                return Some(Outcome::Failed(e));
            }
            // 远端探测（T15：POSIX + gzip）。不过 = 已带日志，回退逐文件。
            if !probe_remote_tar(shared, profile.as_ref(), conn).await {
                return None;
            }
            // 远端有界计数（探测已过；超时/失败/解析失败 → -1 不走）。
            let n = if prechecked {
                threshold as i64
            } else {
                match conn.as_ref() {
                    Some(c) => count_remote_files(c, remote, threshold as i64).await,
                    // ensure_conn 已成功，不可达；显式错误替代 unwrap。
                    None => {
                        return Some(Outcome::Failed(t("连接不可用（内部状态异常）").to_owned()));
                    }
                }
            };
            if !gate_open(on, threshold as i64, true, (n >= 0).then_some(n)) {
                return None;
            }
            let base = remote.trim_end_matches('/');
            let name = base
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(remote);
            log(
                shared,
                profile.as_ref(),
                tfmt(
                    "[tar 加速] {0}（文件数 ≥ {1}，单流打包）",
                    &[&name, &threshold],
                ),
            );
            to_outcome(download_dir(shared, conn, id, remote, local, profile.as_ref()).await)
        }
        _ => None,
    }
}

/// tar 门判据（C# CanUse 口径）：设置开 && 阈值 >0 && tar_ok && 计数达标。
/// T15 起 tar_ok = 远端探测通过（POSIX shell + gzip 可用，remote_tar_ok）；
/// count=None = 扫描失败 / 远端计数 -1（不可走）；达标含等于阈值（有界计数
/// 数到阈值即停）。maybe_run 前置段已把探测与回退日志分开处理，此处为唯一
/// 判定点；测试覆盖含 on/tar_ok 的全判据。
pub(super) fn gate_open(on: bool, threshold: i64, tar_ok: bool, count: Option<i64>) -> bool {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::transfer::tar::gate_open(on, threshold, tar_ok, count)
    }
    #[cfg(not(feature = "official"))]
    {
        on && threshold > 0 && tar_ok && matches!(count, Some(n) if n >= threshold)
    }
}

/// 远端 tar+gzip 探测门（T15 两级回退链第一级）：POSIX + gzip 有一项不过 =
/// 记回退日志后返回 false（调用方回退逐文件；文案在 i18n 表，守卫绿）。
async fn probe_remote_tar(
    shared: &Shared,
    prof: Option<&Profile>,
    conn: &Option<SftpConn>,
) -> bool {
    let Some(c) = conn.as_ref() else {
        // maybe_run 已 ensure_conn，不可达；按不过处理（静默，避免假日志）。
        return false;
    };
    if remote_tar_ok(c).await {
        return true;
    }
    log(
        shared,
        prof,
        t("[tar 加速] 远端 tar+gzip 不可用，回退逐文件传输").to_owned(),
    );
    false
}

/// TarOut → 队列结局；Fallback = None（调用方回退展开逐文件，不落失败）。
fn to_outcome(o: TarOut) -> Option<Outcome> {
    match o {
        TarOut::Done => Some(Outcome::Done),
        // C# TarFailWrapped：整体传输失败语义提示前缀。
        TarOut::Failed(m) => Some(Outcome::Failed(format!(
            "{} {}",
            t("tar 模式为整体传输，失败只能整体重试（无文件级续传）："),
            m
        ))),
        TarOut::Paused => Some(Outcome::Paused),
        TarOut::Cancelled => Some(Outcome::Cancelled),
        TarOut::Fallback => None,
    }
}

// ── 上传：内嵌打包 → 流式直写远端临时 tar.gz → 服务端解包 → 原子切换 ──

async fn upload_dir(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local: &Path,
    remote: &str,
    prof: Option<&Profile>,
) -> TarOut {
    // POSIX + gzip 探测已在 maybe_run（probe_remote_tar）完成，此处直走。
    let guid = guid();
    let parent = remote_parent(remote);
    let tar_path = join_remote(&parent, &format!(".ifree-tar-{guid}.tar.gz"));
    let tmp_dir = join_remote(&parent, &format!(".ifree-tar-{guid}"));
    let dest_exists = match conn.as_ref() {
        Some(c) => c.sftp.metadata(remote).await.is_ok(),
        None => return TarOut::Failed(t("连接不可用（内部状态异常）").to_owned()),
    };

    let out = upload_body(
        shared,
        conn,
        id,
        local,
        remote,
        &tar_path,
        &tmp_dir,
        dest_exists,
        prof,
    )
    .await;

    // finally：清理远端临时件（失败仅记日志，不影响传输结论）。
    cleanup_remote(shared, conn, &tar_path, Some(&tmp_dir), prof).await;
    out
}

#[allow(clippy::too_many_arguments)]
async fn upload_body(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local: &Path,
    remote: &str,
    tar_path: &str,
    tmp_dir: &str,
    dest_exists: bool,
    prof: Option<&Profile>,
) -> TarOut {
    // 打包线程（T15 内嵌化：tar::Builder + GzEncoder 纯 Rust，删除本机 tar
    // 子进程/管理线程桥/stderr 收集/30s 超时杀整段）：有界扫描逐条 append，
    // 压缩块经通道桥进 async 侧。接收端断开（检查点暂停/取消/失败结算或写
    // 远端出错）→ blocking_send 报错 → 打包线程自行收尾退出。
    // 注意：上传打包在本地进行（phase=1 专指下载方向的服务端打包，此处不置，
    // 免得「服务端打包中」文案在本地打包/流式上传期间误导用户）。
    progress(shared, id, 0, 0);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let root = local.to_path_buf();
    let packer = tokio::task::spawn_blocking(move || pack_dir(&root, PackerW(tx)));

    // 流式直写远端临时 .tar.gz：256KB 聚包后落 SFTP（与旧子进程 256KB 块同档，
    // 防 gzip 输出的小碎块拖慢 SFTP 大包）；进度按压缩流实际字节数（口径不变）。
    let sftp = &match conn.as_ref() {
        Some(c) => &c.sftp,
        None => return TarOut::Failed(t("连接不可用（内部状态异常）").to_owned()),
    };
    use russh_sftp::protocol::OpenFlags;
    let mut rf = match sftp
        .open_with_flags(
            tar_path,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
    {
        Ok(f) => f,
        Err(e) => return TarOut::Failed(tfmt("打开远端临时文件失败：{0}", &[&e])),
    };
    let mut transferred = 0u64;
    let mut win_at = Instant::now();
    let mut win_bytes = 0u64;
    let mut cur_speed = 0u64;
    let mut out_buf: Vec<u8> = Vec::with_capacity(CHUNK);
    loop {
        // 检查点：tar 整体任务暂停/取消即断（重试整体重来，无文件级续传）；
        // 返回即 drop 接收端 → 打包线程 send 报错自行退出。
        if let Some(o) = checkpoint(shared, id) {
            return match o {
                Outcome::Paused => TarOut::Paused,
                _ => TarOut::Cancelled,
            };
        }
        // 聚包满 → 落 SFTP（含速度窗与进度上报）。
        if out_buf.len() >= CHUNK {
            if let Err(e) = flush_to_remote(&mut rf, &mut out_buf, &mut transferred).await {
                return TarOut::Failed(e);
            }
            let now = Instant::now();
            if now.duration_since(win_at) >= SPEED_WINDOW {
                let dt = now.duration_since(win_at).as_secs_f64();
                cur_speed = ((transferred - win_bytes) as f64 / dt) as u64;
                win_at = now;
                win_bytes = transferred;
            }
            progress(shared, id, transferred, cur_speed);
        }
        // 取压缩块（看门狗：打包线程 60s 无产出判僵死）。
        match tokio::time::timeout(STALL, rx.recv()).await {
            Err(_) => return TarOut::Failed(stalled()),
            Ok(None) => break, // 通道关闭 = 打包完成
            Ok(Some(chunk)) => out_buf.extend_from_slice(&chunk),
        }
    }
    // 收尾落包尾（含 GzEncoder finish 冲出的剩余压缩块）。
    if !out_buf.is_empty()
        && let Err(e) = flush_to_remote(&mut rf, &mut out_buf, &mut transferred).await
    {
        return TarOut::Failed(e);
    }
    drop(rf); // 发送 close：临时 .tar.gz 闭合，服务端解包前必须完整落盘

    // 打包线程结果：Err = 本地扫描/打包失败（带路径）或线程崩溃。
    let packed = match packer.await {
        Ok(r) => r,
        Err(je) => Err(je.to_string()),
    };
    if let Err(e) = packed {
        return TarOut::Failed(tfmt("[tar 加速] 本地打包失败：{0}", &[&e]));
    }

    set_phase(shared, id, 2);
    progress(shared, id, transferred, 0);

    if !dest_exists {
        // 原子切换：解包到同级临时目录 → sftp rename（C# RENAME 语义）。
        let cmd = unpack_cmd(tar_path, tmp_dir);
        if let Err(e) = exec_cmd(conn, &cmd, EXEC_LONG).await {
            return TarOut::Failed(e);
        }
        let renamed = match conn.as_ref() {
            Some(c) => tokio::time::timeout(STALL, c.sftp.rename(tmp_dir, remote)).await,
            None => return TarOut::Failed(t("连接不可用（内部状态异常）").to_owned()),
        };
        match renamed {
            Ok(Ok(())) => return TarOut::Done,
            Ok(Err(e)) => {
                // rename 失败（目标竞现/服务器不支持等）：记日志后降级直接解包
                // 覆盖（C# 同降级路径）。
                log(
                    shared,
                    prof,
                    tfmt(
                        "[tar 加速] 临时目录切换失败，改为直接解包覆盖：{0}（{1}）",
                        &[&remote, &e],
                    ),
                );
            }
            Err(_) => return TarOut::Failed(stalled()),
        }
    }
    // 直接解包覆盖（目标已存在，或 rename 降级）。
    let cmd = unpack_cmd(tar_path, remote);
    match exec_cmd(conn, &cmd, EXEC_LONG).await {
        Ok(()) => TarOut::Done,
        Err(e) => TarOut::Failed(e),
    }
}

// ── 下载：服务端打包 → SFTP 单文件下载 → 本机解包 → 落位/合并 ──

async fn download_dir(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    remote: &str,
    local: &Path,
    prof: Option<&Profile>,
) -> TarOut {
    // 本地盘根没有父目录可放临时件（C# N9）。
    let Some(parent) = local.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return TarOut::Fallback;
    };
    let guid = guid();
    let remote_tar = format!("/tmp/.ifree-tar-{guid}.tar.gz");
    let local_tar = parent.join(format!(".ifree-tar-{guid}.tar.gz"));
    let local_tmp = parent.join(format!(".ifree-tar-{guid}"));

    let out = download_body(
        shared,
        conn,
        id,
        remote,
        local,
        &remote_tar,
        &local_tar,
        &local_tmp,
        prof,
    )
    .await;

    // finally：清理远端 tar 与本地临时件（幂等；失败忽略/仅日志）。
    cleanup_remote(shared, conn, &remote_tar, None, prof).await;
    let _ = std::fs::remove_file(&local_tar);
    let _ = std::fs::remove_dir_all(&local_tmp);
    out
}

#[allow(clippy::too_many_arguments)]
async fn download_body(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    remote: &str,
    local: &Path,
    remote_tar: &str,
    local_tar: &Path,
    local_tmp: &Path,
    prof: Option<&Profile>,
) -> TarOut {
    set_phase(shared, id, 1);
    progress(shared, id, 0, 0);
    // 服务端打包（gzip 统一压缩，T15 改 -czf；C# 同结构 tar -c）。
    let cmd = pack_remote_cmd(remote_tar, remote);
    if let Err(e) = exec_cmd(conn, &cmd, EXEC_LONG).await {
        return TarOut::Failed(e);
    }

    // SFTP 单文件下载（复用逐文件下载：检查点/进度/速度窗全继承；
    // 本地临时 tar.gz 名每次新 guid，无续传干扰）。
    match download_file(shared, conn, id, remote_tar, local_tar, 0, prof).await {
        Outcome::Done => {}
        Outcome::Paused => return TarOut::Paused,
        Outcome::Cancelled => return TarOut::Cancelled,
        Outcome::Failed(m) => return TarOut::Failed(m),
    }

    set_phase(shared, id, 2);
    if let Err(e) = std::fs::create_dir_all(local_tmp) {
        return TarOut::Failed(tfmt("创建临时目录失败：{0}", &[&e]));
    }
    // 本地解包（T15 内嵌化：GzDecoder + tar::Archive::unpack 纯 Rust，删除本机
    // tar 子进程；spawn_blocking 保留，阻塞点仍在。unpack 内建路径逃逸守卫，
    // 默认保留 mtime 与 Unix 权限位；PAX/UTF-8 读头，中文文件名不依赖代码页）。
    let lt = local_tar.to_path_buf();
    let ltd = local_tmp.to_path_buf();
    let joined = tokio::task::spawn_blocking(move || unpack_gz_tar(&lt, &ltd)).await;
    match joined {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return TarOut::Failed(tfmt("本机解包任务失败：{0}", &[&e])),
        Err(e) => return TarOut::Failed(tfmt("本机解包任务失败：{0}", &[&e.to_string()])),
    }

    // 落位：目标已存在 → 合并覆盖（C# MergeDirectory）；否则 rename 原子落位。
    if local.exists() {
        if let Err(e) = merge_dir(local_tmp, local) {
            return TarOut::Failed(tfmt("合并目录失败：{0}", &[&e]));
        }
    } else if let Err(e) = std::fs::rename(local_tmp, local) {
        return TarOut::Failed(tfmt("目录落位失败：{0}", &[&e]));
    }
    TarOut::Done
}

// ── 辅助 ──

/// exec 罩顶封装：超时/失败归一 Err（i18n 键对齐 C#）。
async fn exec_cmd(conn: &mut Option<SftpConn>, cmd: &str, limit: Duration) -> Result<(), String> {
    let c = conn
        .as_ref()
        .ok_or_else(|| t("连接不可用（内部状态异常）").to_owned())?;
    match tokio::time::timeout(limit, c.exec(cmd)).await {
        Err(_) => Err(tfmt("服务端命令超时：{0}", &[&cmd])),
        Ok(Err(e)) => Err(e),
        Ok(Ok((code, _, _))) if code == 0 => Ok(()),
        Ok(Ok((code, out, err))) => {
            let detail = err.trim();
            let detail = if detail.is_empty() {
                out.trim()
            } else {
                detail
            };
            Err(tfmt(
                "[tar 加速] 服务端命令失败（退出码 {1}）：{0} → {2}",
                &[&cmd, &code, &detail],
            ))
        }
    }
}

/// 清理远端临时件（finally；rm -f / rm -rf 分开两条快罩顶，各自失败仅记日志）。
async fn cleanup_remote(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    tar: &str,
    dir: Option<&str>,
    prof: Option<&Profile>,
) {
    let cmd = format!("rm -f -- {}", ssh::shell_quote(tar));
    if let Err(e) = exec_cmd(conn, &cmd, EXEC_QUICK).await {
        log(
            shared,
            prof,
            tfmt(
                "[tar 加速] 临时文件清理失败（不影响传输结果）：{0}（{1}）",
                &[&tar, &e],
            ),
        );
    }
    if let Some(d) = dir {
        let cmd = format!("rm -rf -- {}", ssh::shell_quote(d));
        if let Err(e) = exec_cmd(conn, &cmd, EXEC_QUICK).await {
            log(
                shared,
                prof,
                tfmt(
                    "[tar 加速] 临时文件清理失败（不影响传输结果）：{0}（{1}）",
                    &[&d, &e],
                ),
            );
        }
    }
}

/// 远端有界文件计数：`find <dir> -type f | head -n <limit> | wc -l`；
/// 超时/失败/解析失败 → -1（不走 tar，C# CountRemoteFilesAsync 同口径）。
/// POSIX + gzip 探测已上收至 probe_remote_tar/remote_tar_ok（T15：gate_open
/// 的 tar_ok 语义改为远端探测，计数不再内含探测）。
pub(super) async fn count_remote_files(c: &SftpConn, remote: &str, limit: i64) -> i64 {
    match tokio::time::timeout(EXEC_QUICK, c.exec(&count_cmd(remote, limit))).await {
        Ok(Ok((0, out, _))) => out.trim().parse::<i64>().unwrap_or(-1),
        _ => -1,
    }
}

/// 远端 tar 能力探测（T15 新语义：gate_open 的 tar_ok）= POSIX shell
/// （is_posix_output，沿用现 exec 探测与判定口径）+ gzip 可用（`command -v
/// gzip` 退出码 0）。每任务一次，与旧 POSIX 探测同节奏。
async fn remote_tar_ok(c: &SftpConn) -> bool {
    let posix = match tokio::time::timeout(EXEC_QUICK, c.exec("echo $env:OS")).await {
        Ok(Ok((_, out, _))) => is_posix_output(out.trim()),
        _ => false,
    };
    if !posix {
        return false;
    }
    matches!(
        tokio::time::timeout(EXEC_QUICK, c.exec("command -v gzip")).await,
        Ok(Ok((0, _, _)))
    )
}

/// POSIX 探测输出判定（C# IsPosixShellAsync 口径）：输出含 Windows_NT/$env
/// 特征即非 POSIX；探测超时/执行失败由调用点按 false（非 POSIX → Fallback）。
fn is_posix_output(out: &str) -> bool {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::transfer::tar::is_posix_output(out)
    }
    #[cfg(not(feature = "official"))]
    {
        !(out.contains("Windows_NT") || out.contains("$env"))
    }
}

/// 本地有界文件计数（数到 limit 即停）。显式栈迭代（禁递归防爆栈）：
/// 深度上限 128、symlink_metadata 不跟链接（符号链接一律不计）、规范化路径
/// visited 集合防环（目录联接/循环符号链接）。扫描失败 = 显式 Err（带路径），
/// 绝不静默跳过 —— 调用方记日志后回退逐文件传输。
pub(super) fn count_local_files(root: &Path, limit: i64) -> Result<i64, String> {
    use std::path::PathBuf;
    const MAX_DEPTH: usize = 128;
    let mut n = 0i64;
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));
    // 栈元素：(待扫目录, 深度)。目录计数无顺序要求，简单 LIFO 即可。
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for e in std::fs::read_dir(&dir).map_err(err_at(&dir))? {
            let e = e.map_err(err_at(&dir))?;
            let meta = std::fs::symlink_metadata(e.path()).map_err(err_at(&e.path()))?;
            // 链接/目录联接不盲跟：目标是目录才按目录进入，环由 canonicalize
            // visited 集合拦截（自环即显式 Err，不会死循环）。
            let is_dir = if meta.file_type().is_symlink() {
                std::fs::metadata(e.path())
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            } else {
                meta.is_dir()
            };
            if is_dir {
                if depth + 1 >= MAX_DEPTH {
                    return Err(format!(
                        "目录嵌套超过 {MAX_DEPTH} 层上限：{}",
                        e.path().display()
                    ));
                }
                let canon = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
                if !visited.insert(canon) {
                    return Err(format!(
                        "检测到目录环（符号链接/联接循环）：{}",
                        e.path().display()
                    ));
                }
                stack.push((e.path(), depth + 1));
            } else if meta.is_file() {
                n += 1;
                if n >= limit {
                    return Ok(n);
                }
            }
        }
    }
    Ok(n)
}

/// 带路径的扫描错误包装：io::Error 版（merge_dir 的 io::Result 通道用）。
fn err_io_at(p: &Path) -> impl Fn(std::io::Error) -> std::io::Error + '_ {
    move |e| std::io::Error::other(format!("{}：{e}", p.display()))
}

/// 带路径的扫描错误包装：String 版（count_local_files 的显式 Err 通道用）。
fn err_at(p: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}：{e}", p.display())
}

/// 目录合并（C# MergeDirectory）：目标已存在时建目录 + 逐项覆盖复制。
/// 显式栈迭代（禁递归防爆栈）：深度上限 128、symlink_metadata 不跟链接、
/// 规范化路径 visited 集合防环；失败 = 显式 Err（带路径），绝不静默跳过。
fn merge_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::path::PathBuf;
    const MAX_DEPTH: usize = 128;
    enum Frame {
        /// 叶子动作：复制文件（路径已带上下文）。
        Copy { from: PathBuf, to: PathBuf },
        /// 待展开目录：展开时先建目标目录再压子项。
        Dir {
            from: PathBuf,
            to: PathBuf,
            depth: usize,
        },
    }
    std::fs::create_dir_all(dst)
        .map_err(|e| std::io::Error::other(format!("创建目标目录 {} 失败：{e}", dst.display())))?;
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf()));
    let mut stack: Vec<Frame> = vec![Frame::Dir {
        from: src.to_path_buf(),
        to: dst.to_path_buf(),
        depth: 0,
    }];
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Copy { from, to } => {
                std::fs::copy(&from, &to).map_err(|e| {
                    std::io::Error::other(format!(
                        "复制 {} → {} 失败：{e}",
                        from.display(),
                        to.display()
                    ))
                })?;
            }
            Frame::Dir { from, to, depth } => {
                std::fs::create_dir_all(&to).map_err(|e| {
                    std::io::Error::other(format!("创建目录 {} 失败：{e}", to.display()))
                })?;
                if depth + 1 >= MAX_DEPTH {
                    return Err(std::io::Error::other(format!(
                        "目录嵌套超过 {MAX_DEPTH} 层上限：{}",
                        from.display()
                    )));
                }
                for e in std::fs::read_dir(&from).map_err(err_io_at(&from))? {
                    let e = e.map_err(err_io_at(&from))?;
                    let meta = std::fs::symlink_metadata(e.path()).map_err(err_io_at(&e.path()))?;
                    // 链接/目录联接不盲跟：目标是目录才按目录进入（环由 visited
                    // 拦截）；目标是文件则复制其内容。
                    let is_dir = if meta.file_type().is_symlink() {
                        std::fs::metadata(e.path())
                            .map(|m| m.is_dir())
                            .unwrap_or(false)
                    } else {
                        meta.is_dir()
                    };
                    let to_path = to.join(e.file_name());
                    if is_dir {
                        let canon = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
                        if !visited.insert(canon) {
                            return Err(std::io::Error::other(format!(
                                "检测到目录环（符号链接/联接循环）：{}",
                                e.path().display()
                            )));
                        }
                        stack.push(Frame::Dir {
                            from: e.path(),
                            to: to_path,
                            depth: depth + 1,
                        });
                    } else {
                        stack.push(Frame::Copy {
                            from: e.path(),
                            to: to_path,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// 本机打包（T15 内嵌化）：tar::Builder + GzEncoder（级别固定 6，无设置项）
/// 纯 Rust 实现。复用 T5 有界扫描范式：显式栈迭代（禁递归防爆栈）、深度上限
/// 128、symlink 不盲跟（一律跳过，与计数口径一致）、规范化路径 visited 集合
/// 防环；逐条 append（禁 append_dir_all 裸奔），header 保 mtime/权限
/// （append_path_with_data 读文件元数据写入 tar 头）。压缩块经 W 流出——
/// 上传侧传通道桥（PackerW）实现流式直写远端；测试侧传内存/文件。
fn pack_dir<W: std::io::Write>(root: &Path, w: W) -> Result<(), String> {
    use flate2::{Compression, write::GzEncoder};
    let gz = GzEncoder::new(w, Compression::new(6));
    let mut b = tar::Builder::new(gz);
    const MAX_DEPTH: usize = 128;
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));
    // 栈元素：(待扫目录绝对路径, 归档内相对前缀, 深度)。LIFO + 每层按名排序
    // = 确定性顺序；根自身不进档（内容落在档根，与 `tar -C <dir> .` 同布局）。
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&dir)
            .map_err(err_at(&dir))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err_at(&dir))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p).map_err(err_at(&p))?;
            // 链接/目录联接不盲跟：一律跳过（与 count_local_files 计数口径
            // 一致；环守卫因此天然不触及链接）。
            if meta.file_type().is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if meta.is_dir() {
                if depth + 1 >= MAX_DEPTH {
                    return Err(format!("目录嵌套超过 {MAX_DEPTH} 层上限：{}", p.display()));
                }
                let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
                if !visited.insert(canon) {
                    return Err(format!(
                        "检测到目录环（符号链接/联接循环）：{}",
                        p.display()
                    ));
                }
                b.append_dir(Path::new(&rel), &p).map_err(err_at(&p))?;
                stack.push((p, rel, depth + 1));
            } else if meta.is_file() {
                // mtime/权限随文件元数据入 tar 头（T15：中文文件名按 PAX/UTF-8
                // 写头，不依赖系统代码页——有意优于 C# 的 bsdtar 代码页风险）。
                b.append_path_with_name(&p, Path::new(&rel))
                    .map_err(err_at(&p))?;
            }
        }
    }
    let gz = b.into_inner().map_err(err_at(root))?;
    gz.finish().map_err(err_at(root))?;
    Ok(())
}

/// 通道桥写端（std::io::Write → tokio mpsc）：打包线程（spawn_blocking，非
/// async 上下文）用 blocking_send 推压缩块；接收端断开（检查点暂停/取消/
/// 失败结算或写远端出错）即报错，打包线程据此自行收尾退出。
struct PackerW(tokio::sync::mpsc::Sender<Vec<u8>>);

impl std::io::Write for PackerW {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.blocking_send(buf.to_vec()).is_err() {
            return Err(std::io::Error::other(
                "打包接收端已断开（任务已暂停/取消/失败）",
            ));
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 压缩块落 SFTP：256KB 聚包一次 write_all（与旧子进程 256KB 块同档，防
/// gzip 输出小碎块拖慢 SFTP 大包）；成功后清空缓冲并累加已传字节（压缩流
/// 实际字节数，进度口径不变）。
async fn flush_to_remote<W: tokio::io::AsyncWrite + Unpin>(
    rf: &mut W,
    out_buf: &mut Vec<u8>,
    transferred: &mut u64,
) -> Result<(), String> {
    match tokio::time::timeout(STALL, rf.write_all(out_buf.as_slice())).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(tfmt("写入远端失败：{0}", &[&e])),
        Err(_) => return Err(stalled()),
    }
    *transferred += out_buf.len() as u64;
    out_buf.clear();
    Ok(())
}

/// 本地解包（T15 内嵌化）：GzDecoder + tar::Archive::unpack 纯 Rust。
/// unpack 默认保留 mtime（preserve_mtime 默认开）与 Unix 基本权限位；
/// 内建路径逃逸守卫（拒绝 .. 与绝对路径条目）；PAX/UTF-8 读头，中文文件名
/// 不依赖系统代码页。删除原本机 tar -xf 子进程段。
fn unpack_gz_tar(tar_path: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(tar_path).map_err(err_at(tar_path))?;
    let dec = flate2::read::GzDecoder::new(f);
    tar::Archive::new(dec).unpack(dest).map_err(err_at(dest))?;
    Ok(())
}

/// 远端打包命令（纯函数化供单测）：gzip 统一压缩（T15 由 -cf 改 -czf）。
fn pack_remote_cmd(tar_path: &str, dir: &str) -> String {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::transfer::tar::pack_remote_cmd(tar_path, dir)
    }
    #[cfg(not(feature = "official"))]
    {
        format!(
            "tar -czf {} -C {} .",
            ssh::shell_quote(tar_path),
            ssh::shell_quote(dir)
        )
    }
}

/// 远端解包命令（纯函数化供单测）：-z 对应统一 gzip 压缩流（T15 由 -xf 改 -xzf）。
fn unpack_cmd(tar_path: &str, dir: &str) -> String {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::transfer::tar::unpack_cmd(tar_path, dir)
    }
    #[cfg(not(feature = "official"))]
    {
        format!(
            "mkdir -p {} && tar -xzf {} -C {}",
            ssh::shell_quote(dir),
            ssh::shell_quote(tar_path),
            ssh::shell_quote(dir)
        )
    }
}

/// 远端有界计数命令（纯函数化供单测）。
fn count_cmd(dir: &str, limit: i64) -> String {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::transfer::tar::count_cmd(dir, limit)
    }
    #[cfg(not(feature = "official"))]
    {
        format!(
            "find {} -type f | head -n {} | wc -l",
            ssh::shell_quote(dir),
            limit
        )
    }
}

/// 临时件名 guid（16hex：纳秒时钟 + 进程内序号混合；C# Guid 等价用途）。
fn guid() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let v = (nanos << 32) ^ SEQ.fetch_add(1, Ordering::Relaxed) ^ (std::process::id() as u64);
    format!("{v:016x}")
}

/// 远端父目录（临时件落点；C# GetRemoteParentDirectory：末段去掉；
/// 无 `/` 或仅根 → 根目录）。
fn remote_parent(p: &str) -> String {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::transfer::tar::remote_parent(p)
    }
    #[cfg(not(feature = "official"))]
    {
        let t = p.trim_end_matches('/');
        match t.rfind('/') {
            Some(0) | None => "/".to_owned(),
            Some(i) => t[..i].to_owned(),
        }
    }
}

/// tar 阶段旗标（队列第四列文案用；0 无 / 1 服务端打包中（仅下载方向）/ 2 解包中）。
fn set_phase(shared: &Shared, id: u64, phase: u8) {
    let mut st = shared.st.lock().unwrap();
    if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
        i.phase = phase;
    }
}

/// 僵死判定文案（C# TarStalledException 消息）。
fn stalled() -> String {
    tfmt(
        "[tar 加速] 传输 {0} 秒无进展，判定链路僵死已中止",
        &[&STALL.as_secs()],
    )
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// 门判据·远端探测通过（T15 新语义：tar_ok = POSIX + gzip 远端探测通过）：
    /// 设置开 + 阈值正 + 计数达标（含恰等阈值）→ 走 tar。
    #[test]
    fn remote_probe_ok_and_count_reaches_threshold() {
        assert!(gate_open(true, 50, true, Some(60)));
        // 有界计数数到阈值即停，恰等阈值同样达标（C# >= 口径）。
        assert!(gate_open(true, 50, true, Some(50)));
        // 计数不足 → 不走。
        assert!(!gate_open(true, 50, true, Some(49)));
    }

    /// 门判据·远端探测不过（T15 新语义：非 POSIX / 缺 gzip）：任何计数下整门
    /// 不过（回退逐文件，绝不走 tar）；设置关 / 阈值非正同样整门不过。
    #[test]
    fn remote_probe_failed_or_disabled_never_takes_tar() {
        assert!(!gate_open(true, 50, false, Some(999_999)));
        assert!(!gate_open(false, 50, true, Some(999_999)));
        assert!(!gate_open(true, 0, true, Some(999_999)));
        assert!(!gate_open(true, 50, true, None));
    }

    /// 服务端非 POSIX：探测输出含 Windows_NT/$env 特征 → 非 POSIX（Fallback）；
    /// POSIX 输出通过。探测超时/执行失败由调用点按 false，不经本判定。
    #[test]
    fn non_posix_probe_output_is_fallback() {
        assert!(!is_posix_output("Windows_NT"));
        assert!(!is_posix_output("OS=$env:OS"));
        assert!(is_posix_output("Linux"));
    }

    /// Fallback 对拍：Fallback = None（调用方回退展开逐文件，不落失败结局）；
    /// Failed 落整体失败结局（C# TarFailWrapped 前缀在 to_outcome 包裹）；
    /// Done/Paused/Cancelled 一一对应。
    #[test]
    fn fallback_maps_to_none_not_failure() {
        assert!(to_outcome(TarOut::Fallback).is_none());
        assert!(matches!(
            to_outcome(TarOut::Failed("x".to_owned())),
            Some(Outcome::Failed(_))
        ));
        assert!(matches!(to_outcome(TarOut::Done), Some(Outcome::Done)));
        assert!(matches!(to_outcome(TarOut::Paused), Some(Outcome::Paused)));
        assert!(matches!(
            to_outcome(TarOut::Cancelled),
            Some(Outcome::Cancelled)
        ));
    }

    /// count_local_files：有界即停 + 子目录递归 + 显式 Err 带路径（不静默）。
    #[test]
    fn count_bounded_scan_and_explicit_error() {
        let td = std::env::temp_dir().join(format!("ifree-tar-gate-{}", std::process::id()));
        let sub = td.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        for i in 0..4 {
            std::fs::write(td.join(format!("f{i}.txt")), b"x").unwrap();
            std::fs::write(sub.join(format!("g{i}.txt")), b"x").unwrap();
        }
        // 数到 limit 即停（8 个文件，limit=5 → 5）。
        assert_eq!(count_local_files(&td, 5), Ok(5));
        // limit 大于实际 → 全数（8，含子目录）。
        assert_eq!(count_local_files(&td, 100), Ok(8));
        // 扫描失败 = 显式 Err 且带路径（根不存在）。
        let missing = td.join("nope");
        let err = count_local_files(&missing, 5).unwrap_err();
        assert!(err.contains("nope"), "错误须带路径：{err}");
        let _ = std::fs::remove_dir_all(&td);
    }

    /// merge_dir：显式栈合并覆盖语义（新建 + 已存在覆盖）+ 失败显式 Err 带路径。
    #[test]
    fn merge_overwrites_and_reports_explicit_error() {
        let base = std::env::temp_dir().join(format!("ifree-tar-merge-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(src.join("d")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        std::fs::write(src.join("d").join("b.txt"), b"nested").unwrap();
        // 目标已存在同名文件 → 覆盖。
        std::fs::write(dst.join("a.txt"), b"old").unwrap();
        merge_dir(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"new");
        assert_eq!(
            std::fs::read(dst.join("d").join("b.txt")).unwrap(),
            b"nested"
        );
        // 源不存在 → 显式 Err（read_dir 错误带源路径）。
        let err = merge_dir(&base.join("gone"), &base.join("dst2")).unwrap_err();
        assert!(err.to_string().contains("gone"), "错误须带路径：{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 目录环：junction/符号链接自环 → 三处扫描函数同口径显式 Err（含
    /// 「检测到目录环」+ 环路径），进程不 panic。Windows 用 mklink /J
    /// （目录联接，免管理员权限），非 Windows 用符号链接。
    #[test]
    fn cycle_detected_by_all_three_walks() {
        use super::super::walk_local; // transfer.rs 私有函数，子模块可见
        let base = std::env::temp_dir().join(format!("ifree-cycle-{}", std::process::id()));
        let dir = base.join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), b"x").unwrap();
        // 自环：<base>/dir/loop → <base>/dir
        make_self_loop(&dir);
        // walk_local：Err 含「检测到目录环」+ 环路径。
        let msg = walk_local(&dir).unwrap_err().to_string();
        assert!(msg.contains("检测到目录环"), "walk_local：{msg}");
        assert!(msg.contains("loop"), "walk_local 错误须带环路径：{msg}");
        // count_local_files：同口径（显式 Err 带路径，不静默跳过）。
        let err = count_local_files(&dir, 100).unwrap_err();
        assert!(
            err.contains("检测到目录环") && err.contains("loop"),
            "count：{err}"
        );
        // merge_dir：同口径（io::Result Err）。
        let msg = merge_dir(&dir, &base.join("dst")).unwrap_err().to_string();
        assert!(
            msg.contains("检测到目录环") && msg.contains("loop"),
            "merge：{msg}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 深度上限：200 层嵌套 → 三处扫描函数同口径显式 Err（含
    /// 「目录嵌套超过 128 层上限」+ 深层路径），进程不 panic。
    #[test]
    fn depth_cap_at_128_levels() {
        use super::super::walk_local;
        let base = std::env::temp_dir().join(format!("ifree-deep-{}", std::process::id()));
        let mut p = base.clone();
        for i in 0..200 {
            p = p.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&p).unwrap();
        // walk_local：深度上限 Err。
        let msg = walk_local(&base).unwrap_err().to_string();
        assert!(msg.contains("目录嵌套超过 128 层上限"), "walk_local：{msg}");
        assert!(msg.contains("d1"), "walk_local 错误须带深层路径：{msg}");
        // count_local_files：同口径。
        let err = count_local_files(&base, 10_000).unwrap_err();
        assert!(
            err.contains("目录嵌套超过 128 层上限") && err.contains("d1"),
            "count：{err}"
        );
        // merge_dir：同口径。目标放源树外（dst 在 src 内会自我嵌套复制）。
        let dst = std::env::temp_dir().join(format!("ifree-deep-dst-{}", std::process::id()));
        let msg = merge_dir(&base, &dst).unwrap_err().to_string();
        assert!(
            msg.contains("目录嵌套超过 128 层上限") && msg.contains("d1"),
            "merge：{msg}"
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&dst);
    }

    /// 造自环目录（临时目录自建自删，不依赖手工环境）：
    /// Windows 优先相对目标符号链接（内核打开时按链接所在目录解析 —— 部分沙箱/
    /// 虚拟化环境会把 mklink /J 联接的绝对目标改写成不可解析的设备路径，相对
    /// 符号链接不受影响）；无符号链接特权时回退 mklink /J（目录联接，免管理员）。
    /// 非 Windows 用符号链接。
    fn make_self_loop(dir: &Path) {
        #[cfg(windows)]
        {
            let loop_dir = dir.join("loop");
            if std::os::windows::fs::symlink_dir(".", &loop_dir).is_err() {
                let st = std::process::Command::new("cmd")
                    .args(["/C", "mklink", "/J"])
                    .arg(&loop_dir)
                    .arg(dir)
                    .status()
                    .expect("mklink /J 执行失败");
                assert!(st.success(), "mklink /J 返回非零");
            }
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(dir, dir.join("loop")).expect("symlink 失败");
        }
    }

    /// T15 ① round-trip：Builder+GzEncoder 打包 → GzDecoder+unpack 还原；
    /// 覆盖中文文件名（PAX/UTF-8 往返，不依赖系统代码页）、mtime 不同
    /// （tar 头按秒存，还原值与原值逐文件对拍）、子目录嵌套。
    #[test]
    fn gz_tar_roundtrip_preserves_names_content_and_mtime() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let base = std::env::temp_dir().join(format!("ifree-tar-rt-{}", std::process::id()));
        let src = base.join("源目录");
        let sub = src.join("子目录");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(src.join("报告.txt"), "中文内容——第一版".as_bytes()).unwrap();
        // 两文件 mtime 落不同秒（tar 头按秒存，同秒无法验证保留）。
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(sub.join("data.bin"), [0u8, 1, 2, 250, 255]).unwrap();
        let m1 = std::fs::metadata(src.join("报告.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let m2 = std::fs::metadata(sub.join("data.bin"))
            .unwrap()
            .modified()
            .unwrap();
        let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_ne!(secs(m1), secs(m2), "用例前置：两文件 mtime 须不同秒");

        // 打包到内存（压缩级别 6 由 pack_dir 内定，无设置项）。
        let mut buf: Vec<u8> = Vec::new();
        pack_dir(&src, &mut buf).expect("打包成功");

        // 解包还原（与 download_body 同构造：GzDecoder + Archive::unpack）。
        let out = base.join("还原目录");
        std::fs::create_dir_all(&out).unwrap();
        let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(buf));
        tar::Archive::new(dec).unpack(&out).unwrap();

        // 内容对拍（中文名 UTF-8 往返 + 二进制字节对拍）。
        assert_eq!(
            std::fs::read(out.join("报告.txt")).unwrap(),
            "中文内容——第一版".as_bytes()
        );
        assert_eq!(
            std::fs::read(out.join("子目录").join("data.bin")).unwrap(),
            [0u8, 1, 2, 250, 255]
        );
        // mtime 对拍：逐文件还原一致，两文件差异保持。
        let m1o = std::fs::metadata(out.join("报告.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let m2o = std::fs::metadata(out.join("子目录").join("data.bin"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(secs(m1), secs(m1o), "mtime 须还原一致（报告.txt）");
        assert_eq!(secs(m2), secs(m2o), "mtime 须还原一致（data.bin）");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// T15 ③ 远端命令构造纯函数：shell_quote 嵌入与命令骨架（-z 统一 gzip）。
    #[test]
    fn remote_command_construction_quotes_and_shape() {
        // 打包：tar -czf <tar> -C <dir> .
        assert_eq!(
            pack_remote_cmd("/tmp/a b.tar.gz", "/src/dir"),
            "tar -czf '/tmp/a b.tar.gz' -C '/src/dir' ."
        );
        // 解包：mkdir -p <dir> && tar -xzf <tar> -C <dir>
        assert_eq!(
            unpack_cmd("/tmp/a.tar.gz", "/dst dir"),
            "mkdir -p '/dst dir' && tar -xzf '/tmp/a.tar.gz' -C '/dst dir'"
        );
        // 计数：find | head | wc -l
        assert_eq!(
            count_cmd("/some dir", 50),
            "find '/some dir' -type f | head -n 50 | wc -l"
        );
    }
}
