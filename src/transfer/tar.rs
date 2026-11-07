//! tar 目录加速（C# TarAccelService 全套移植）。
//!
//! 触发（C# CanUse 口径）：设置开 && 文件数 ≥ 阈值（且 >0）&& 仅上传/下载整目录
//! （中转/本地复制不走）。判定时机在 worker 领取目录任务时（C# 在 UI 入队时判定，
//! 等价后移；未接管才走 expand 展开逐文件）。
//!
//! - 上传：本地有界计数 → 本机 tar 可用 → POSIX 探测 → 本机 `tar -cf - -C <dir> .`
//!   stdout 经 SFTP 流式直写远端临时 .tar → 服务端 `mkdir -p tmp && tar -xf` →
//!   sftp rename 原子切换（目标已存在 → 直接解包覆盖；rename 失败 → 记日志后
//!   降级直接解包覆盖）；
//! - 下载：服务端 `tar -cf /tmp/….tar -C <dir> .` → SFTP 单文件下载（复用
//!   download_file：检查点/进度全继承）→ 本机解包到同级临时目录 → 目标不存在
//!   rename 落位 / 已存在合并覆盖（C# MergeDirectory）。
//!
//! 临时命名：目标父目录下 `.ifree-tar-<guid>.tar` / `.ifree-tar-<guid>`；
//! 看门狗 = 每步 256KB 收/发 60s 无进展判僵死（C# 1s 轮询 60s 无进展的等价口径）；
//! 本机 tar 退出等待 30s 超时杀；exec 快罩顶 60s（探测/计数/清理/切换）、
//! 长罩顶 600s（服务端打包/解包）。失败语义：走不通（无本机 tar / 非 POSIX /
//! 本地盘根）→ Fallback 静默回退逐文件；执行中失败 → 整体失败（reason 前缀
//! 「tar 模式为整体传输…」，C# TarFailWrapped，含 TarStalledException）；
//! finally 清理远端 rm -f/rm -rf 与本地临时件（失败仅记日志）。
//! tar 天然保留 mtime/权限；tar 任务无 verify、无文件级续传（重试整体重来）。

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use super::{
    CHUNK, Outcome, Profile, SPEED_WINDOW, Shared, checkpoint, download_file, ensure_conn,
    join_remote, log, progress,
};
use crate::i18n::{t, tfmt};
use crate::ssh::{self, SftpConn};

/// 看门狗：单步（256KB 收/发、通道消息、rename）60s 无进展判僵死。
const STALL: Duration = Duration::from_secs(60);
/// exec 快罩顶（POSIX 探测 / 文件计数 / 临时件清理 / rename 降级命令）。
const EXEC_QUICK: Duration = Duration::from_secs(60);
/// exec 长罩顶（服务端打包 / 服务端解包大目录）。
const EXEC_LONG: Duration = Duration::from_secs(600);
/// 本机 tar 进程退出等待秒数（超时杀，C# WaitForExit(30s) 同口径）。
const TAR_EXIT_WAIT: u64 = 30;

/// tar 子流程结局：Fallback = 走不通，静默回退逐文件展开（C# return false）。
enum TarOut {
    Done,
    Failed(String),
    Paused,
    Cancelled,
    Fallback,
}

/// 本机 tar 管理线程 → async 侧消息（stdout 数据 / EOF / 进程退出）。
enum TarMsg {
    Chunk(Vec<u8>),
    Eof,
    /// Ok = 退出码；Err = 超时杀 / wait 失败（C# KilledProcess 同位）。
    Exited(Result<i32, ()>),
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
    let (on, threshold) = crate::settings::with(|s| (s.tar_accel, s.tar_threshold));
    if !on || threshold <= 0 {
        return None;
    }
    // 本机 tar 可用性两方向共用（进程内缓存一次探测）。
    if !local_tar_available() {
        log(
            shared,
            profile.as_ref(),
            t("[tar 加速] 本机没有 tar 命令，回退逐文件传输").to_owned(),
        );
        return None;
    }
    match kind {
        super::Kind::UploadDir { local, remote } => {
            // 本地有界计数（数到阈值即停）。扫描失败 = 显式 Err（带路径）：
            // 记日志后不走 tar，回退展开逐文件（错误由逐文件路径显式报）。
            let count = count_local_files(local, threshold as i64);
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
            if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                return Some(Outcome::Failed(e));
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
            if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                return Some(Outcome::Failed(e));
            }
            // 远端有界计数（内含 POSIX 探测；超时/失败/非 POSIX → -1 不走）。
            let n = match conn.as_ref() {
                Some(c) => count_remote_files(c, remote, threshold as i64).await,
                // ensure_conn 已成功，不可达；显式错误替代 unwrap。
                None => return Some(Outcome::Failed(t("连接不可用（内部状态异常）").to_owned())),
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

/// tar 门判据（C# CanUse 口径）：设置开 && 阈值 >0 && 本机 tar 可用 && 计数达标。
/// count=None = 扫描失败 / 远端计数 -1（不可走）；达标含等于阈值（有界计数
/// 数到阈值即停）。maybe_run 前置段已把 on/tar_ok 的日志语义分开处理，此处
/// 为唯一判定点；测试覆盖含 on/tar_ok 的全判据。
fn gate_open(on: bool, threshold: i64, tar_ok: bool, count: Option<i64>) -> bool {
    on && threshold > 0 && tar_ok && matches!(count, Some(n) if n >= threshold)
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

// ── 上传：本机打包 → 流式直写远端临时 tar → 服务端解包 → 原子切换 ──

async fn upload_dir(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local: &Path,
    remote: &str,
    prof: Option<&Profile>,
) -> TarOut {
    // POSIX 探测（每任务一次；C# IsPosixShellAsync：输出含 Windows_NT/$env
    // 特征按非 POSIX，探测超时/失败同按非 POSIX，Windows 服务器静默回退）。
    let posix = {
        let Some(c) = conn.as_ref() else {
            return TarOut::Failed(t("连接不可用（内部状态异常）").to_owned());
        };
        match tokio::time::timeout(EXEC_QUICK, c.exec("echo $env:OS")).await {
            Ok(Ok((_, out, _))) => is_posix_output(out.trim()),
            _ => false,
        }
    };
    if !posix {
        return TarOut::Fallback;
    }
    let guid = guid();
    let parent = remote_parent(remote);
    let tar_path = join_remote(&parent, &format!(".ifree-tar-{guid}.tar"));
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
    set_phase(shared, id, 1);
    progress(shared, id, 0, 0);

    // 本机 tar 打包到 stdout（tokio 无 process 特性：std 子进程 + 管理线程桥进
    // async；C# Process tar -cf - 同命令）。
    let mut child = match std::process::Command::new("tar")
        .args(["-cf", "-", "-C"])
        .arg(local)
        .arg(".")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return TarOut::Failed(tfmt("启动本机 tar 失败：{0}", &[&e])),
    };
    // stderr 收集（tar 报错文本留给退出码非 0 的失败详情）。
    let Some(mut stderr) = child.stderr.take() else {
        return TarOut::Failed(t("本机 tar 进程无 stderr 管道（内部异常）").to_owned());
    };
    let err_buf = Arc::new(Mutex::new(String::new()));
    let err_sink = err_buf.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        // 锁中毒（持有方 panic）只丢报错详情，不致命——显式退出而非 unwrap。
        let Ok(mut tail) = err_sink.lock() else {
            return;
        };
        loop {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.push_str(&String::from_utf8_lossy(&buf[..n]));
                    // 只留尾部（退出详情够用，防超大输出占内存）。
                    if tail.len() > 8192 {
                        let cut = tail.len() - 4096;
                        tail.drain(..cut);
                    }
                }
            }
        }
    });

    // 管理线程：stdout 分块转发；接收端断开（暂停/取消/失败）即杀进程收尾；
    // EOF 后等退出 30s，超时杀（C# WaitForExit 30s 同口径）。
    let Some(mut stdout) = child.stdout.take() else {
        return TarOut::Failed(t("本机 tar 进程无 stdout 管道（内部异常）").to_owned());
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<TarMsg>(2);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.blocking_send(TarMsg::Chunk(buf[..n].to_vec())).is_err() {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }
            }
        }
        let _ = tx.blocking_send(TarMsg::Eof);
        let deadline = Instant::now() + Duration::from_secs(TAR_EXIT_WAIT);
        let code = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Ok(st.code().unwrap_or(-1)),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => break Err(()),
            }
        };
        let _ = tx.blocking_send(TarMsg::Exited(code));
    });

    // 流式直写远端临时 .tar（检查点/看门狗/速度窗与逐文件同口径）。
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
    loop {
        // 检查点：tar 整体任务暂停/取消即断（重试整体重来，无文件级续传）。
        if let Some(o) = checkpoint(shared, id) {
            return match o {
                Outcome::Paused => TarOut::Paused,
                _ => TarOut::Cancelled,
            };
        }
        let msg = match tokio::time::timeout(STALL, rx.recv()).await {
            Err(_) => return TarOut::Failed(stalled()),
            // 管理线程异常消失（防御）。
            Ok(None) => return TarOut::Failed(t("本机 tar 进程异常退出").into()),
            Ok(Some(m)) => m,
        };
        match msg {
            TarMsg::Chunk(buf) => {
                match tokio::time::timeout(STALL, rf.write_all(&buf)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => return TarOut::Failed(tfmt("写入远端失败：{0}", &[&e])),
                    Err(_) => return TarOut::Failed(stalled()),
                }
                transferred += buf.len() as u64;
                let now = Instant::now();
                if now.duration_since(win_at) >= SPEED_WINDOW {
                    let dt = now.duration_since(win_at).as_secs_f64();
                    cur_speed = ((transferred - win_bytes) as f64 / dt) as u64;
                    win_at = now;
                    win_bytes = transferred;
                }
                progress(shared, id, transferred, cur_speed);
            }
            TarMsg::Eof => {}
            TarMsg::Exited(res) => match res {
                Ok(0) => break,
                Ok(code) => {
                    // 锁中毒（stderr 收集线程 panic）只丢详情文本，不影响失败结算。
                    let detail = err_buf
                        .lock()
                        .map(|b| b.trim().to_owned())
                        .unwrap_or_default();
                    return TarOut::Failed(tfmt(
                        "[tar 加速] 本机 tar 失败（退出码 {0}）：{1}",
                        &[&code, &detail],
                    ));
                }
                Err(()) => return TarOut::Failed(hang_killed()),
            },
        }
    }
    drop(rf); // 发送 close：临时 .tar 闭合，服务端解包前必须完整落盘

    set_phase(shared, id, 2);
    progress(shared, id, transferred, 0);

    if !dest_exists {
        // 原子切换：解包到同级临时目录 → sftp rename（C# RENAME 语义）。
        let cmd = format!(
            "mkdir -p {} && tar -xf {} -C {}",
            ssh::shell_quote(tmp_dir),
            ssh::shell_quote(tar_path),
            ssh::shell_quote(tmp_dir)
        );
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
    let cmd = format!(
        "mkdir -p {} && tar -xf {} -C {}",
        ssh::shell_quote(remote),
        ssh::shell_quote(tar_path),
        ssh::shell_quote(remote)
    );
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
    let remote_tar = format!("/tmp/.ifree-tar-{guid}.tar");
    let local_tar = parent.join(format!(".ifree-tar-{guid}.tar"));
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
    // 服务端打包（C# 同命令 tar -cf /tmp/… -C <dir> .）。
    let cmd = format!(
        "tar -cf {} -C {} .",
        ssh::shell_quote(remote_tar),
        ssh::shell_quote(remote)
    );
    if let Err(e) = exec_cmd(conn, &cmd, EXEC_LONG).await {
        return TarOut::Failed(e);
    }

    // SFTP 单文件下载（复用逐文件下载：检查点/进度/速度窗全继承；
    // 本地临时 tar 名每次新 guid，无续传干扰）。
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
    // 本机解包（阻塞走 spawn_blocking；C# Process tar -xf 同语义）。
    let lt = local_tar.to_path_buf();
    let ltd = local_tmp.to_path_buf();
    let joined = tokio::task::spawn_blocking(move || {
        std::process::Command::new("tar")
            .arg("-xf")
            .arg(lt)
            .arg("-C")
            .arg(ltd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| e.to_string())
    })
    .await;
    let res = match joined {
        Ok(r) => r,
        Err(e) => return TarOut::Failed(tfmt("本机解包任务失败：{0}", &[&e])),
    };
    match res {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            let code = out.status.code().unwrap_or(-1);
            return TarOut::Failed(tfmt(
                "[tar 加速] 本机 tar 失败（退出码 {0}）：{1}",
                &[&code, &detail],
            ));
        }
        Err(e) => return TarOut::Failed(tfmt("启动本机 tar 失败：{0}", &[&e])),
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

/// 远端有界文件计数：POSIX 探测 + `find <dir> -type f | head -n <limit> | wc -l`；
/// 超时/失败/非 POSIX/解析失败 → -1（不走 tar，C# CountRemoteFilesAsync 同口径）。
async fn count_remote_files(c: &SftpConn, remote: &str, limit: i64) -> i64 {
    let posix = match tokio::time::timeout(EXEC_QUICK, c.exec("echo $env:OS")).await {
        Ok(Ok((_, out, _))) => {
            let out = out.trim();
            !(out.contains("Windows_NT") || out.contains("$env"))
        }
        _ => false,
    };
    if !posix {
        return -1;
    }
    let cmd = format!(
        "find {} -type f | head -n {} | wc -l",
        ssh::shell_quote(remote),
        limit
    );
    match tokio::time::timeout(EXEC_QUICK, c.exec(&cmd)).await {
        Ok(Ok((0, out, _))) => out.trim().parse::<i64>().unwrap_or(-1),
        _ => -1,
    }
}

/// POSIX 探测输出判定（C# IsPosixShellAsync 口径）：输出含 Windows_NT/$env
/// 特征即非 POSIX；探测超时/执行失败由调用点按 false（非 POSIX → Fallback）。
fn is_posix_output(out: &str) -> bool {
    !(out.contains("Windows_NT") || out.contains("$env"))
}

/// 本地有界文件计数（数到 limit 即停）。显式栈迭代（禁递归防爆栈）：
/// 深度上限 128、symlink_metadata 不跟链接（符号链接一律不计）、规范化路径
/// visited 集合防环（目录联接/循环符号链接）。扫描失败 = 显式 Err（带路径），
/// 绝不静默跳过 —— 调用方记日志后回退逐文件传输。
fn count_local_files(root: &Path, limit: i64) -> Result<i64, String> {
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

/// 本机 tar 可用性（进程内缓存一次探测；`tar --version` 可启动即成功）。
fn local_tar_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("tar")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
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
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(i) => t[..i].to_owned(),
    }
}

/// tar 阶段旗标（队列第四列文案用；0 无 / 1 服务端打包中 / 2 解包中）。
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

/// 本机 tar 超时杀文案（i18n 键「本地 tar 进程」）。
fn hang_killed() -> String {
    tfmt(
        "[tar 加速] 本地 tar 进程 {0} 秒未退出，已终止",
        &[&TAR_EXIT_WAIT],
    )
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// 门判据·本机 tar 可用：设置开 + 阈值正 + 计数达标（含恰等阈值）→ 走 tar。
    #[test]
    fn tar_available_and_count_reaches_threshold() {
        assert!(gate_open(true, 50, true, Some(60)));
        // 有界计数数到阈值即停，恰等阈值同样达标（C# >= 口径）。
        assert!(gate_open(true, 50, true, Some(50)));
        // 计数不足 → 不走。
        assert!(!gate_open(true, 50, true, Some(49)));
    }

    /// 门判据·本机 tar 不可用：任何计数下整门不过（回退逐文件，绝不走 tar）；
    /// 设置关 / 阈值非正同样整门不过。
    #[test]
    fn tar_unavailable_or_disabled_never_takes_tar() {
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
}
