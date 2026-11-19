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

#[cfg(feature = "official")]
use crate::ifree_core::transfer::tar::{EXEC_LONG_SECS, EXEC_QUICK_SECS, STALL_SECS};
#[cfg(not(feature = "official"))]
const STALL_SECS: u64 = 60;
#[cfg(not(feature = "official"))]
const EXEC_QUICK_SECS: u64 = 60;
#[cfg(not(feature = "official"))]
const EXEC_LONG_SECS: u64 = 600;

const STALL: Duration = Duration::from_secs(STALL_SECS);

const EXEC_QUICK: Duration = Duration::from_secs(EXEC_QUICK_SECS);

const EXEC_LONG: Duration = Duration::from_secs(EXEC_LONG_SECS);

enum TarOut {
    Done,
    Failed(String),
    Paused,
    Cancelled,
    Fallback,
}

pub(super) async fn maybe_run(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    kind: &super::Kind,
    profile: &Option<Profile>,
) -> Option<Outcome> {

    let cfg = cfg_of(shared, id);
    let (on, threshold) = (cfg.tar_accel, cfg.tar_threshold);
    if !on || threshold <= 0 {
        return None;
    }

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

            if !probe_remote_tar(shared, profile.as_ref(), conn).await {
                return None;
            }

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

            if !probe_remote_tar(shared, profile.as_ref(), conn).await {
                return None;
            }

            let n = if prechecked {
                threshold as i64
            } else {
                match conn.as_ref() {
                    Some(c) => count_remote_files(c, remote, threshold as i64).await,

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

async fn probe_remote_tar(
    shared: &Shared,
    prof: Option<&Profile>,
    conn: &Option<SftpConn>,
) -> bool {
    let Some(c) = conn.as_ref() else {

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

fn to_outcome(o: TarOut) -> Option<Outcome> {
    match o {
        TarOut::Done => Some(Outcome::Done),

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

async fn upload_dir(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local: &Path,
    remote: &str,
    prof: Option<&Profile>,
) -> TarOut {

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

    progress(shared, id, 0, 0);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let root = local.to_path_buf();
    let packer = tokio::task::spawn_blocking(move || pack_dir(&root, PackerW(tx)));

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

        if let Some(o) = checkpoint(shared, id) {
            return match o {
                Outcome::Paused => TarOut::Paused,
                _ => TarOut::Cancelled,
            };
        }

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

        match tokio::time::timeout(STALL, rx.recv()).await {
            Err(_) => return TarOut::Failed(stalled()),
            Ok(None) => break,
            Ok(Some(chunk)) => out_buf.extend_from_slice(&chunk),
        }
    }

    if !out_buf.is_empty()
        && let Err(e) = flush_to_remote(&mut rf, &mut out_buf, &mut transferred).await
    {
        return TarOut::Failed(e);
    }
    drop(rf);

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

    let cmd = unpack_cmd(tar_path, remote);
    match exec_cmd(conn, &cmd, EXEC_LONG).await {
        Ok(()) => TarOut::Done,
        Err(e) => TarOut::Failed(e),
    }
}

async fn download_dir(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    remote: &str,
    local: &Path,
    prof: Option<&Profile>,
) -> TarOut {

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

    let cmd = pack_remote_cmd(remote_tar, remote);
    if let Err(e) = exec_cmd(conn, &cmd, EXEC_LONG).await {
        return TarOut::Failed(e);
    }

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

    let lt = local_tar.to_path_buf();
    let ltd = local_tmp.to_path_buf();
    let joined = tokio::task::spawn_blocking(move || unpack_gz_tar(&lt, &ltd)).await;
    match joined {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return TarOut::Failed(tfmt("本机解包任务失败：{0}", &[&e])),
        Err(e) => return TarOut::Failed(tfmt("本机解包任务失败：{0}", &[&e.to_string()])),
    }

    if local.exists() {
        if let Err(e) = merge_dir(local_tmp, local) {
            return TarOut::Failed(tfmt("合并目录失败：{0}", &[&e]));
        }
    } else if let Err(e) = std::fs::rename(local_tmp, local) {
        return TarOut::Failed(tfmt("目录落位失败：{0}", &[&e]));
    }
    TarOut::Done
}

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

pub(super) async fn count_remote_files(c: &SftpConn, remote: &str, limit: i64) -> i64 {
    match tokio::time::timeout(EXEC_QUICK, c.exec(&count_cmd(remote, limit))).await {
        Ok(Ok((0, out, _))) => out.trim().parse::<i64>().unwrap_or(-1),
        _ => -1,
    }
}

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

pub(super) fn count_local_files(root: &Path, limit: i64) -> Result<i64, String> {
    use std::path::PathBuf;
    const MAX_DEPTH: usize = 128;
    let mut n = 0i64;
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));

    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for e in std::fs::read_dir(&dir).map_err(err_at(&dir))? {
            let e = e.map_err(err_at(&dir))?;
            let meta = std::fs::symlink_metadata(e.path()).map_err(err_at(&e.path()))?;

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

fn err_io_at(p: &Path) -> impl Fn(std::io::Error) -> std::io::Error + '_ {
    move |e| std::io::Error::other(format!("{}：{e}", p.display()))
}

fn err_at(p: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}：{e}", p.display())
}

fn merge_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::path::PathBuf;
    const MAX_DEPTH: usize = 128;
    enum Frame {

        Copy { from: PathBuf, to: PathBuf },

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

fn pack_dir<W: std::io::Write>(root: &Path, w: W) -> Result<(), String> {
    use flate2::{Compression, write::GzEncoder};
    let gz = GzEncoder::new(w, Compression::new(6));
    let mut b = tar::Builder::new(gz);
    const MAX_DEPTH: usize = 128;
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));

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

                b.append_path_with_name(&p, Path::new(&rel))
                    .map_err(err_at(&p))?;
            }
        }
    }
    let gz = b.into_inner().map_err(err_at(root))?;
    gz.finish().map_err(err_at(root))?;
    Ok(())
}

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

fn unpack_gz_tar(tar_path: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(tar_path).map_err(err_at(tar_path))?;
    let dec = flate2::read::GzDecoder::new(f);
    tar::Archive::new(dec).unpack(dest).map_err(err_at(dest))?;
    Ok(())
}

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

fn guid() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let v = (nanos << 32) ^ SEQ.fetch_add(1, Ordering::Relaxed) ^ (std::process::id() as u64);
    format!("{v:016x}")
}

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

fn set_phase(shared: &Shared, id: u64, phase: u8) {
    let mut st = shared.st.lock().unwrap();
    if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
        i.phase = phase;
    }
}

fn stalled() -> String {
    tfmt(
        "[tar 加速] 传输 {0} 秒无进展，判定链路僵死已中止",
        &[&STALL.as_secs()],
    )
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn remote_probe_ok_and_count_reaches_threshold() {
        assert!(gate_open(true, 50, true, Some(60)));

        assert!(gate_open(true, 50, true, Some(50)));

        assert!(!gate_open(true, 50, true, Some(49)));
    }

    #[test]
    fn remote_probe_failed_or_disabled_never_takes_tar() {
        assert!(!gate_open(true, 50, false, Some(999_999)));
        assert!(!gate_open(false, 50, true, Some(999_999)));
        assert!(!gate_open(true, 0, true, Some(999_999)));
        assert!(!gate_open(true, 50, true, None));
    }

    #[test]
    fn non_posix_probe_output_is_fallback() {
        assert!(!is_posix_output("Windows_NT"));
        assert!(!is_posix_output("OS=$env:OS"));
        assert!(is_posix_output("Linux"));
    }

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

    #[test]
    fn count_bounded_scan_and_explicit_error() {
        let td = std::env::temp_dir().join(format!("ifree-tar-gate-{}", std::process::id()));
        let sub = td.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        for i in 0..4 {
            std::fs::write(td.join(format!("f{i}.txt")), b"x").unwrap();
            std::fs::write(sub.join(format!("g{i}.txt")), b"x").unwrap();
        }

        assert_eq!(count_local_files(&td, 5), Ok(5));

        assert_eq!(count_local_files(&td, 100), Ok(8));

        let missing = td.join("nope");
        let err = count_local_files(&missing, 5).unwrap_err();
        assert!(err.contains("nope"), "错误须带路径：{err}");
        let _ = std::fs::remove_dir_all(&td);
    }

    #[test]
    fn merge_overwrites_and_reports_explicit_error() {
        let base = std::env::temp_dir().join(format!("ifree-tar-merge-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(src.join("d")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        std::fs::write(src.join("d").join("b.txt"), b"nested").unwrap();

        std::fs::write(dst.join("a.txt"), b"old").unwrap();
        merge_dir(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"new");
        assert_eq!(
            std::fs::read(dst.join("d").join("b.txt")).unwrap(),
            b"nested"
        );

        let err = merge_dir(&base.join("gone"), &base.join("dst2")).unwrap_err();
        assert!(err.to_string().contains("gone"), "错误须带路径：{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cycle_detected_by_all_three_walks() {
        use super::super::walk_local;
        let base = std::env::temp_dir().join(format!("ifree-cycle-{}", std::process::id()));
        let dir = base.join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), b"x").unwrap();

        make_self_loop(&dir);

        let msg = walk_local(&dir).unwrap_err().to_string();
        assert!(msg.contains("检测到目录环"), "walk_local：{msg}");
        assert!(msg.contains("loop"), "walk_local 错误须带环路径：{msg}");

        let err = count_local_files(&dir, 100).unwrap_err();
        assert!(
            err.contains("检测到目录环") && err.contains("loop"),
            "count：{err}"
        );

        let msg = merge_dir(&dir, &base.join("dst")).unwrap_err().to_string();
        assert!(
            msg.contains("检测到目录环") && msg.contains("loop"),
            "merge：{msg}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn depth_cap_at_128_levels() {
        use super::super::walk_local;
        let base = std::env::temp_dir().join(format!("ifree-deep-{}", std::process::id()));
        let mut p = base.clone();
        for i in 0..200 {
            p = p.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&p).unwrap();

        let msg = walk_local(&base).unwrap_err().to_string();
        assert!(msg.contains("目录嵌套超过 128 层上限"), "walk_local：{msg}");
        assert!(msg.contains("d1"), "walk_local 错误须带深层路径：{msg}");

        let err = count_local_files(&base, 10_000).unwrap_err();
        assert!(
            err.contains("目录嵌套超过 128 层上限") && err.contains("d1"),
            "count：{err}"
        );

        let dst = std::env::temp_dir().join(format!("ifree-deep-dst-{}", std::process::id()));
        let msg = merge_dir(&base, &dst).unwrap_err().to_string();
        assert!(
            msg.contains("目录嵌套超过 128 层上限") && msg.contains("d1"),
            "merge：{msg}"
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&dst);
    }

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

    #[test]
    fn gz_tar_roundtrip_preserves_names_content_and_mtime() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let base = std::env::temp_dir().join(format!("ifree-tar-rt-{}", std::process::id()));
        let src = base.join("源目录");
        let sub = src.join("子目录");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(src.join("报告.txt"), "中文内容——第一版".as_bytes()).unwrap();

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

        let mut buf: Vec<u8> = Vec::new();
        pack_dir(&src, &mut buf).expect("打包成功");

        let out = base.join("还原目录");
        std::fs::create_dir_all(&out).unwrap();
        let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(buf));
        tar::Archive::new(dec).unpack(&out).unwrap();

        assert_eq!(
            std::fs::read(out.join("报告.txt")).unwrap(),
            "中文内容——第一版".as_bytes()
        );
        assert_eq!(
            std::fs::read(out.join("子目录").join("data.bin")).unwrap(),
            [0u8, 1, 2, 250, 255]
        );

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

    #[test]
    fn remote_command_construction_quotes_and_shape() {

        assert_eq!(
            pack_remote_cmd("/tmp/a b.tar.gz", "/src/dir"),
            "tar -czf '/tmp/a b.tar.gz' -C '/src/dir' ."
        );

        assert_eq!(
            unpack_cmd("/tmp/a.tar.gz", "/dst dir"),
            "mkdir -p '/dst dir' && tar -xzf '/tmp/a.tar.gz' -C '/dst dir'"
        );

        assert_eq!(
            count_cmd("/some dir", 50),
            "find '/some dir' -type f | head -n 50 | wc -l"
        );
    }
}
