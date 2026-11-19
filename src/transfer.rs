use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::i18n::{t, tfmt};
use crate::local;
use crate::ssh::{self, SftpConn};

pub mod core;
mod tar;

const CHUNK: usize = 256 * 1024;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TransferCfg {
    pub resume: bool,
    pub resume_threshold_kb: i32,
    pub verify: bool,
    pub verify_algo: usize,
    pub preserve_mtime: bool,
    pub tar_accel: bool,
    pub tar_threshold: i32,
    pub connect_timeout: i32,
}

impl From<&crate::settings::AppSettings> for TransferCfg {
    fn from(s: &crate::settings::AppSettings) -> Self {
        Self {
            resume: s.resume,
            resume_threshold_kb: s.resume_threshold_kb,
            verify: s.verify,
            verify_algo: s.verify_algo,
            preserve_mtime: s.preserve_mtime,
            tar_accel: s.tar_accel,
            tar_threshold: s.tar_threshold,
            connect_timeout: s.connect_timeout,
        }
    }
}

impl Default for TransferCfg {

    fn default() -> Self {
        Self::from(&crate::settings::AppSettings::default())
    }
}

fn resume_threshold(cfg: &TransferCfg) -> u64 {
    (cfg.resume_threshold_kb as u64) * 1024
}

fn cfg_of(shared: &Shared, id: u64) -> TransferCfg {
    shared
        .st
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .map(|i| i.cfg.clone())
        .unwrap_or_default()
}

const SPEED_WINDOW: Duration = Duration::from_millis(400);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Profile {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: crate::ssh::Auth,

    pub fingerprint: Option<String>,
}

pub type Probe = Arc<Mutex<(u64, Option<crate::ssh::HashProbe>)>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TaskOwner {
    pub src: (u8, u32),
    pub dst: (u8, u32),
}

impl TaskOwner {

    #[allow(dead_code)]
    pub const ZERO: Self = Self {
        src: (0, 0),
        dst: (0, 0),
    };

    pub fn owned_by(&self, key: (u8, u32)) -> bool {
        self.src == key || self.dst == key
    }
}

pub fn count_owned_active<'a>(
    rows: impl IntoIterator<Item = (&'a TaskOwner, Status)>,
    key: (u8, u32),
) -> usize {
    rows.into_iter()
        .filter(|(o, s)| {
            o.owned_by(key)
                && matches!(s, Status::Waiting | Status::Transferring | Status::Scanning)
        })
        .count()
}

pub fn collect_owner_tabs<'a>(
    rows: impl IntoIterator<Item = (&'a TaskOwner, Status)>,
) -> Vec<(u8, u32)> {
    let mut v: Vec<(u8, u32)> = rows
        .into_iter()
        .filter(|(_, s)| {
            matches!(
                s,
                Status::Waiting | Status::Transferring | Status::Verifying | Status::Scanning
            )
        })
        .flat_map(|(o, _)| [o.src, o.dst])
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

pub const REFRESH_THROTTLE_SECS: f64 = 3.0;

pub fn refresh_due(now: f64, last_refresh: f64, done_delta: u64, same_dir: bool) -> bool {
    (now - last_refresh) >= REFRESH_THROTTLE_SECS && done_delta > 0 && same_dir
}

pub fn same_dest_dir(current: &str, dest: &str, remote: bool) -> bool {
    if remote {
        current == dest
    } else {
        current.eq_ignore_ascii_case(dest)
    }
}

pub fn dest_dir(dest: &str, remote: bool) -> Option<String> {
    if remote {
        let p = dest.trim_end_matches('/');
        let i = p.rfind('/')?;
        Some(if i == 0 {
            "/".to_owned()
        } else {
            p[..i].to_owned()
        })
    } else {
        std::path::Path::new(dest)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().into_owned())
    }
}

pub fn refresh_targets<'a>(
    rows: impl IntoIterator<Item = (&'a TaskOwner, bool, &'a str, Status)>,
) -> Vec<((u8, u32), u64, Vec<String>)> {
    let mut m: std::collections::BTreeMap<(u8, u32), (u64, Vec<String>)> = Default::default();
    for (o, dest_remote, dest, status) in rows {
        let done = matches!(status, Status::Done | Status::Failed);
        if o.src == o.dst {
            let e = m.entry(o.src).or_default();
            if done {
                e.0 += 1;
            }
        } else {
            let se = m.entry(o.src).or_default();
            if done {
                se.0 += 1;
            }
            let de = m.entry(o.dst).or_default();
            if done {
                de.0 += 1;
            }
            if let Some(d) = dest_dir(dest, dest_remote)
                && !de.1.contains(&d)
            {
                de.1.push(d);
            }
        }
    }
    m.into_iter()
        .map(|(k, (done, mut ds))| {
            ds.sort();
            (k, done, ds)
        })
        .collect()
}

pub struct NewTask {
    pub source: String,
    pub dest: String,
    pub size: u64,
    pub force: bool,

    pub probe_incomplete: bool,
    pub kind: Kind,
    pub owner: TaskOwner,
}

#[derive(Clone)]
pub enum Kind {
    UploadFile {
        local: PathBuf,
        remote: String,
    },
    DownloadFile {
        remote: String,
        local: PathBuf,
    },
    CopyLocal {
        from: PathBuf,
        to: PathBuf,
    },

    MakeDir {
        remote: Option<String>,
        local: Option<PathBuf>,
    },

    UploadDir {
        local: PathBuf,
        remote: String,
    },
    DownloadDir {
        remote: String,
        local: PathBuf,
    },
    CopyLocalDir {
        from: PathBuf,
        to: PathBuf,
    },

    #[allow(dead_code)]
    ServerCopy {
        src: String,
        dst: String,
        is_dir: bool,

        merge: bool,
    },

    RelayFile {
        src: Profile,
        dst: Profile,
        remote_src: String,
        remote_dst: String,

        temp: PathBuf,
    },

    RelayDir {
        src: Profile,
        dst: Profile,
        remote_src: String,
        remote_dst: String,
        temp: PathBuf,
    },
}

impl Kind {
    fn is_remote(&self) -> bool {
        matches!(
            self,
            Kind::UploadFile { .. }
                | Kind::DownloadFile { .. }
                | Kind::MakeDir {
                    remote: Some(_),
                    ..
                }
                | Kind::UploadDir { .. }
                | Kind::DownloadDir { .. }
                | Kind::ServerCopy { .. }
                | Kind::RelayFile { .. }
                | Kind::RelayDir { .. }
        )
    }

    fn is_dir_job(&self) -> bool {
        matches!(
            self,
            Kind::UploadDir { .. }
                | Kind::DownloadDir { .. }
                | Kind::CopyLocalDir { .. }
                | Kind::RelayDir { .. }
        )
    }

    fn is_relay(&self) -> bool {
        matches!(self, Kind::RelayFile { .. } | Kind::RelayDir { .. })
    }

    fn dest_is_remote(&self) -> bool {
        matches!(
            self,
            Kind::UploadFile { .. }
                | Kind::UploadDir { .. }
                | Kind::MakeDir {
                    remote: Some(_),
                    ..
                }
                | Kind::ServerCopy { .. }
                | Kind::RelayFile { .. }
                | Kind::RelayDir { .. }
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Waiting,
    Transferring,

    Verifying,

    Scanning,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VerifyKind {

    None,
    Match,
    Mismatch,
    Skipped,
}

#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub source: String,
    pub dest: String,
    pub size: u64,
    pub transferred: u64,
    pub status: Status,

    pub speed: u64,

    pub resumed: bool,

    pub phase: u8,

    pub time: String,

    pub verify_result: VerifyKind,
    pub reason: String,
    pub cancel_req: bool,

    pub owner: TaskOwner,

    batch: u64,
    force: bool,

    probe_incomplete: bool,
    kind: Kind,

    profile: Option<Profile>,

    probe: Probe,

    parent: u64,

    precheck: bool,

    cfg: TransferCfg,
}

impl Item {

    pub fn is_remote(&self) -> bool {
        self.kind.is_remote()
    }

    pub fn dest_is_remote(&self) -> bool {
        self.kind.dest_is_remote()
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        status: Status,
        size: u64,
        transferred: u64,
        speed: u64,
        resumed: bool,
    ) -> Self {
        Self {
            id: 0,
            source: String::new(),
            dest: String::new(),
            size,
            transferred,
            status,
            speed,
            resumed,
            phase: 0,
            time: String::new(),
            verify_result: VerifyKind::None,
            reason: String::new(),
            cancel_req: false,
            owner: TaskOwner::ZERO,
            batch: 0,
            force: false,
            probe_incomplete: false,
            kind: Kind::UploadFile {
                local: PathBuf::new(),
                remote: String::new(),
            },
            profile: None,
            probe: Arc::new(Mutex::new((0, None))),
            parent: 0,
            precheck: false,
            cfg: TransferCfg::default(),
        }
    }
}

struct QueueState {
    items: Vec<Item>,
    next_id: u64,

    batch_no: u64,

    reported: HashSet<u64>,

    skips: HashMap<u64, u64>,
}

struct VerifyJob {
    id: u64,
    local: PathBuf,
    remote: String,

    log_path: String,
    prof: Option<Profile>,
    probe: Probe,

    cfg: TransferCfg,
}

struct ScanJob {
    id: u64,
    kind: Kind,
    profile: Option<Profile>,
    owner: TaskOwner,
}

const SCAN_BATCH: usize = 1000;

struct Shared {
    st: Mutex<QueueState>,
    paused: AtomicBool,
    stopping: AtomicBool,
    log: Mutex<Vec<String>>,

    wake: tokio::sync::Notify,

    verify_tx: tokio::sync::mpsc::UnboundedSender<VerifyJob>,

    scan_tx: tokio::sync::mpsc::UnboundedSender<ScanJob>,

    #[cfg(test)]
    scan_batches_placed: AtomicUsize,
}

pub struct TransferQueue {
    shared: Arc<Shared>,
}

impl TransferQueue {
    pub fn new() -> Self {
        let (verify_tx, verify_rx) = tokio::sync::mpsc::unbounded_channel::<VerifyJob>();
        let (scan_tx, scan_rx) = tokio::sync::mpsc::unbounded_channel::<ScanJob>();
        let shared = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
            verify_tx,
            scan_tx,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("transfer".into())
            .spawn(move || run(worker))
            .expect("spawn transfer worker");

        let verify_worker = shared.clone();
        std::thread::Builder::new()
            .name("verify".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(verify_run(verify_worker, verify_rx));
            })
            .expect("spawn verify worker");

        let scan_worker = shared.clone();
        std::thread::Builder::new()
            .name("scan".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(scan_run(scan_worker, scan_rx));
            })
            .expect("spawn scan worker");
        Self { shared }
    }

    pub fn enqueue(
        &self,
        profile: Option<Profile>,
        probe: Probe,
        tasks: Vec<NewTask>,
        skipped: u64,
    ) -> u64 {
        if tasks.is_empty() {
            return 0;
        }

        let cfg = crate::settings::with(|s| TransferCfg::from(s));

        self.shared.stopping.store(false, Ordering::Relaxed);
        let mut st = self.shared.st.lock().unwrap();
        st.batch_no += 1;
        let batch = st.batch_no;
        if skipped > 0 {
            st.skips.insert(batch, skipped);
        }

        let n = tasks.len().to_string();
        let dest = dest_dir(&tasks[0].dest, tasks[0].kind.dest_is_remote())
            .unwrap_or_else(|| tasks[0].dest.clone());
        let line = tfmt("开始传输 {0} 项 → {1}", &[&n, &dest]);

        let mut jobs: Vec<ScanJob> = Vec::new();
        for t in tasks {
            let id = st.next_id;
            st.next_id += 1;
            let scanning = t.kind.is_dir_job();
            if scanning {
                jobs.push(ScanJob {
                    id,
                    kind: t.kind.clone(),
                    profile: profile.clone(),
                    owner: t.owner,
                });
            }
            st.items.push(Item {
                id,
                source: t.source,
                dest: t.dest,

                size: if t.kind.is_relay() {
                    t.size.saturating_mul(2)
                } else {
                    t.size
                },
                transferred: 0,
                status: if scanning {
                    Status::Scanning
                } else {
                    Status::Waiting
                },
                speed: 0,
                resumed: false,
                phase: 0,
                time: String::new(),
                verify_result: VerifyKind::None,
                reason: String::new(),
                cancel_req: false,
                owner: t.owner,
                batch,
                force: t.force,
                probe_incomplete: t.probe_incomplete,
                kind: t.kind,
                profile: profile.clone(),
                probe: probe.clone(),
                parent: 0,
                precheck: false,
                cfg: cfg.clone(),
            });
        }

        drop(st);
        for j in jobs {
            let _ = self.shared.scan_tx.send(j);
        }
        log(&self.shared, profile.as_ref(), line);
        self.shared.wake.notify_one();
        batch
    }

    pub fn cancel_batches(&self, batches: &[u64]) -> usize {
        let mut st = self.shared.st.lock().unwrap();
        let mut n = 0;
        for i in st.items.iter_mut().filter(|i| batches.contains(&i.batch)) {
            match i.status {
                Status::Waiting | Status::Scanning => {
                    i.status = Status::Cancelled;
                    i.time = local::now_hms();
                    n += 1;
                }
                Status::Transferring => i.cancel_req = true,
                _ => {}
            }
        }
        n
    }

    pub fn cancel(&self, id: u64) {
        let mut st = self.shared.st.lock().unwrap();
        let scanning = st
            .items
            .iter()
            .any(|i| i.id == id && i.status == Status::Scanning);
        if scanning {
            for i in st.items.iter_mut().filter(|i| i.id == id || i.parent == id) {
                match i.status {
                    Status::Scanning | Status::Waiting => {
                        i.status = Status::Cancelled;
                        i.time = local::now_hms();
                    }
                    Status::Transferring => i.cancel_req = true,
                    _ => {}
                }
            }
            return;
        }
        if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
            match i.status {
                Status::Waiting => {
                    i.status = Status::Cancelled;
                    i.time = local::now_hms();
                }
                Status::Transferring => i.cancel_req = true,
                _ => {}
            }
        }
    }

    pub fn cancel_owned(&self, key: (u8, u32)) -> bool {
        let mut st = self.shared.st.lock().unwrap();
        let mut any = false;
        for i in st.items.iter_mut().filter(|i| i.owner.owned_by(key)) {
            match i.status {
                Status::Waiting | Status::Scanning => {
                    i.status = Status::Cancelled;
                    i.time = local::now_hms();
                    any = true;
                }
                Status::Transferring => {
                    i.cancel_req = true;
                    any = true;
                }
                _ => {}
            }
        }
        any
    }

    pub fn retry(&self, id: u64) {
        let mut st = self.shared.st.lock().unwrap();
        let mut requeued = None;
        let mut job = None;
        if let Some(i) = st.items.iter_mut().find(|i| i.id == id)
            && matches!(i.status, Status::Failed | Status::Cancelled)
        {

            let dir_job = i.kind.is_dir_job();
            i.status = if dir_job {
                i.precheck = false;
                Status::Scanning
            } else {
                Status::Waiting
            };
            i.cancel_req = false;
            i.speed = 0;
            i.reason.clear();
            i.time.clear();
            i.transferred = 0;
            i.resumed = false;
            i.verify_result = VerifyKind::None;

            requeued = Some(i.batch);
            if dir_job {
                job = Some(ScanJob {
                    id: i.id,
                    kind: i.kind.clone(),
                    profile: i.profile.clone(),
                    owner: i.owner,
                });
            }
        }
        if let Some(b) = requeued {
            st.reported.remove(&b);
        }
        if let Some(j) = &job {
            st.items.retain(|i| i.parent != j.id);
        }
        drop(st);
        if let Some(j) = job {
            let _ = self.shared.scan_tx.send(j);
        }
    }

    pub fn reset_all(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        let mut st = self.shared.st.lock().unwrap();

        let dir_ids: HashSet<u64> = st
            .items
            .iter()
            .filter(|i| i.kind.is_dir_job())
            .map(|i| i.id)
            .collect();
        st.items
            .retain(|i| i.parent == 0 || !dir_ids.contains(&i.parent));

        let batches: Vec<u64> = st.items.iter().map(|i| i.batch).collect();
        let mut jobs = Vec::new();
        for i in st.items.iter_mut() {
            if i.kind.is_dir_job() {
                i.status = Status::Scanning;
                i.precheck = false;
                jobs.push(ScanJob {
                    id: i.id,
                    kind: i.kind.clone(),
                    profile: i.profile.clone(),
                    owner: i.owner,
                });
            } else {
                i.status = Status::Waiting;
            }
            i.cancel_req = false;
            i.speed = 0;
            i.reason.clear();
            i.time.clear();
            i.transferred = 0;
            i.resumed = false;
            i.verify_result = VerifyKind::None;
        }
        for b in batches {
            st.reported.remove(&b);
        }
        drop(st);
        for j in jobs {
            let _ = self.shared.scan_tx.send(j);
        }

        self.shared.wake.notify_waiters();
    }

    pub fn remove(&self, id: u64) {
        self.shared
            .st
            .lock()
            .unwrap()
            .items
            .retain(|i| i.id != id && i.parent != id);
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);

        self.shared.wake.notify_waiters();
    }

    pub fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    pub fn stop_all(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        self.shared.stopping.store(true, Ordering::Relaxed);

        self.shared.wake.notify_waiters();
        let mut st = self.shared.st.lock().unwrap();
        for i in &mut st.items {
            match i.status {
                Status::Waiting | Status::Scanning => {
                    i.status = Status::Cancelled;
                    i.time = local::now_hms();
                }
                Status::Transferring => i.cancel_req = true,
                _ => {}
            }
        }
    }

    pub fn clear_done(&self) {
        self.shared.st.lock().unwrap().items.retain(|i| {
            matches!(
                i.status,
                Status::Waiting | Status::Transferring | Status::Scanning
            )
        });
    }

    pub fn clear_all(&self) {
        self.shared.st.lock().unwrap().items.clear();
    }

    pub fn drain_batch_reports(&self) -> Vec<String> {
        let mut st = self.shared.st.lock().unwrap();

        let active: HashSet<u64> = st
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i.status,
                    Status::Waiting | Status::Transferring | Status::Verifying | Status::Scanning
                )
            })
            .map(|i| i.batch)
            .collect();

        let mut batches: Vec<u64> = st
            .items
            .iter()
            .map(|i| i.batch)
            .filter(|b| !active.contains(b) && !st.reported.contains(b))
            .collect();
        batches.sort_unstable();
        batches.dedup();
        let mut out = Vec::new();
        for b in batches {
            let mut done = 0u64;
            let mut failed = 0u64;
            let mut prof = None;
            for i in st.items.iter().filter(|i| i.batch == b) {
                match i.status {
                    Status::Done => done += 1,
                    Status::Failed | Status::Cancelled => failed += 1,
                    _ => {}
                }
                if prof.is_none() {
                    prof = i.profile.clone();
                }
            }
            let skipped = st.skips.remove(&b).unwrap_or(0);
            st.reported.insert(b);
            let (d, s, f) = (done.to_string(), skipped.to_string(), failed.to_string());
            let line = tfmt(
                "传输完成：成功 {0} 项，跳过 {1} 项，失败 {2} 项",
                &[&d, &s, &f],
            );
            out.push(format!("{} {line}", log_pfx(prof.as_ref())));
        }
        out
    }

    pub fn with_items<R>(&self, f: impl FnOnce(&[Item]) -> R) -> R {
        f(&self.shared.st.lock().unwrap().items)
    }

    pub fn drain_log(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.log.lock().unwrap())
    }
}

const MAX_LANES: usize = 3;

fn run(shared: Arc<Shared>) {

    let mut handles = Vec::new();
    for lane in 0..MAX_LANES {
        let shared = shared.clone();
        handles.push(
            std::thread::Builder::new()
                .name(format!("transfer{lane}"))
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("tokio runtime");
                    rt.block_on(lane_run(shared, lane));
                })
                .expect("spawn transfer lane"),
        );
    }
    for h in handles {
        let _ = h.join();
    }
}

async fn lane_run(shared: Arc<Shared>, lane: usize) {

    let mut conn: Option<SftpConn> = None;
    let mut conn_of: Option<Profile> = None;
    loop {
        let enabled = lane < crate::settings::with(|s| s.concurrent_transfers.max(1) as usize);
        if !enabled {

            conn = None;
            conn_of = None;
        } else {
            step(&shared, &mut conn, &mut conn_of).await;
        }

        let _ = tokio::time::timeout(Duration::from_millis(100), shared.wake.notified()).await;
    }
}

enum Outcome {
    Done,
    Failed(String),

    Paused,

    Cancelled,
}

enum Gate {
    Idle,
    Held,
    Job(u64, TaskOwner, Kind, Option<Profile>, Probe),
}

fn claim(shared: &Shared) -> Gate {
    let mut st = shared.st.lock().unwrap();
    let idle = !st
        .items
        .iter()
        .any(|i| matches!(i.status, Status::Waiting | Status::Transferring));
    if idle {
        return Gate::Idle;
    }
    if shared.paused.load(Ordering::Relaxed) {
        return Gate::Held;
    }
    st.items
        .iter_mut()
        .find(|i| i.status == Status::Waiting && !i.cancel_req)
        .map(|i| {
            i.status = Status::Transferring;
            Gate::Job(
                i.id,
                i.owner,
                i.kind.clone(),
                i.profile.clone(),
                i.probe.clone(),
            )
        })
        .unwrap_or(Gate::Held)
}

async fn step(shared: &Shared, conn: &mut Option<SftpConn>, conn_of: &mut Option<Profile>) {
    let (id, owner, kind, profile, probe) = match claim(shared) {
        Gate::Idle => {

            shared.stopping.store(false, Ordering::Relaxed);
            *conn = None;
            *conn_of = None;
            return;
        }
        Gate::Held => return,
        Gate::Job(id, owner, kind, profile, probe) => (id, owner, kind, profile, probe),
    };

    if kind.is_dir_job() {
        let tar_out = match &kind {
            Kind::UploadDir { .. } | Kind::DownloadDir { .. } => {
                tar::maybe_run(shared, conn, conn_of, id, &kind, &profile).await
            }
            _ => None,
        };
        if let Some(outcome) = tar_out {
            settle(shared, conn, conn_of, id, kind.is_remote(), outcome);
            return;
        }
        expand(shared, conn, conn_of, id, kind, &profile, &probe, owner).await;
        return;
    }

    let is_remote = kind.is_remote();
    let outcome = run_kind(shared, conn, conn_of, id, kind, &profile).await;
    settle(shared, conn, conn_of, id, is_remote, outcome);
}

fn settle(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    is_remote: bool,
    outcome: Outcome,
) {
    let mut verify_job: Option<VerifyJob> = None;
    let (source, dest, msg, prof) = {
        let mut st = shared.st.lock().unwrap();
        match st.items.iter_mut().find(|i| i.id == id) {
            Some(i) => {

                let do_verify = i.cfg.verify;
                match &outcome {
                    Outcome::Done => {

                        if i.status == Status::Verifying {
                            i.status = Status::Done;
                            i.transferred = i.size;
                            i.time = local::now_hms();
                        } else if let Some((local, remote, log_path)) =
                            verify_target(&i.kind).filter(|_| do_verify)
                        {
                            if i.status == Status::Transferring && !i.cancel_req {
                                verify_job = Some(VerifyJob {
                                    id,
                                    local,
                                    remote,
                                    log_path,
                                    prof: i.profile.clone(),
                                    probe: i.probe.clone(),
                                    cfg: i.cfg.clone(),
                                });
                                i.status = Status::Verifying;
                                i.speed = 0;
                            } else {
                                i.status = Status::Cancelled;
                                i.time = local::now_hms();
                            }
                        } else {
                            i.status = Status::Done;
                            i.transferred = i.size;
                            i.time = local::now_hms();
                        }
                    }
                    Outcome::Failed(m) => {
                        i.status = Status::Failed;
                        i.reason = m.clone();
                        i.time = local::now_hms();
                    }
                    Outcome::Paused => i.status = Status::Waiting,
                    Outcome::Cancelled => {
                        i.status = Status::Cancelled;
                        i.time = local::now_hms();
                    }
                }
                i.speed = 0;
                i.phase = 0;
                (
                    i.source.clone(),
                    i.dest.clone(),
                    i.reason.clone(),
                    i.profile.clone(),
                )
            }
            None => (String::new(), String::new(), String::new(), None),
        }
    };
    if let Some(job) = verify_job {

        let _ = shared.verify_tx.send(job);
    }
    if let Outcome::Failed(m) = &outcome {
        log(
            shared,
            prof.as_ref(),
            tfmt("[传输] 失败：{0} → {1}：{2}", &[&source, &dest, &m]),
        );

        if is_remote {
            *conn = None;
            *conn_of = None;
        }
    }
    let _ = msg;

    shared.wake.notify_one();
}

fn verify_target(kind: &Kind) -> Option<(PathBuf, String, String)> {
    match kind {
        Kind::UploadFile { local, remote } => Some((
            local.clone(),
            remote.clone(),
            local.to_string_lossy().into_owned(),
        )),
        Kind::DownloadFile { remote, local } => {
            Some((local.clone(), remote.clone(), remote.clone()))
        }
        _ => None,
    }
}

fn mkdir_judge(path: &str, create_err: Option<String>, recheck_is_dir: bool) -> Outcome {
    match create_err {
        None => Outcome::Done,

        Some(_e) if recheck_is_dir => Outcome::Done,
        Some(e) => Outcome::Failed(tfmt("新建文件夹失败：{0}（{1}）", &[&path, &e.as_str()])),
    }
}

async fn run_kind(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    kind: Kind,
    profile: &Option<Profile>,
) -> Outcome {
    let cfg = cfg_of(shared, id);
    match kind {
        Kind::MakeDir { remote, local } => {
            if let Some(r) = remote {
                if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                    return Outcome::Failed(e);
                }
                let sftp = &conn.as_ref().unwrap().sftp;
                match sftp.create_dir(&r).await {
                    Ok(()) => {}
                    Err(e) => {

                        let recheck = sftp.metadata(&r).await.map(|m| m.is_dir()).unwrap_or(false);
                        return mkdir_judge(&r, Some(e.to_string()), recheck);
                    }
                }
            }
            if let Some(l) = local {
                match std::fs::create_dir_all(&l) {
                    Ok(()) => {}
                    Err(e) => {

                        let recheck = l.is_dir();
                        return mkdir_judge(&l.to_string_lossy(), Some(e.to_string()), recheck);
                    }
                }
            }
            Outcome::Done
        }
        Kind::CopyLocal { from, to } => copy_local(shared, id, &from, &to).await,
        Kind::UploadFile { local, remote } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                return Outcome::Failed(e);
            }

            upload_file(shared, conn, id, &local, &remote, 0, profile.as_ref()).await
        }
        Kind::DownloadFile { remote, local } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                return Outcome::Failed(e);
            }
            download_file(shared, conn, id, &remote, &local, 0, profile.as_ref()).await
        }
        Kind::ServerCopy {
            src,
            dst,
            is_dir,
            merge,
        } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                return Outcome::Failed(e);
            }

            let core = core::make_transfer_core();
            if !core.is_full() {
                return Outcome::Failed(t("此源码编译版不含服务端直达复制组件；请分两步传输（先下载到本机，再上传到目标）").into());
            }
            let cmd = match core.server_copy_cmd(&src, &dst, is_dir, merge) {
                Ok(c) => c,
                Err(_) => {
                    return Outcome::Failed(t("此源码编译版不含服务端直达复制组件；请分两步传输（先下载到本机，再上传到目标）").into());
                }
            };
            let timeout = core.server_copy_timeout_secs();
            let judged = match tokio::time::timeout(
                Duration::from_secs(timeout),
                conn.as_ref().unwrap().exec(&cmd),
            )
            .await
            {
                Ok(Ok((exit, out, _))) => core.server_copy_ok(exit, &out),
                _ => false,
            };
            if judged {
                Outcome::Done
            } else {
                Outcome::Failed(t("服务端复制失败（服务器不支持 cp 或路径无效）").into())
            }
        }
        Kind::RelayFile {
            src,
            dst,
            remote_src,
            remote_dst,
            temp,
        } => relay_file(shared, id, &src, &dst, &remote_src, &remote_dst, &temp).await,
        _ => Outcome::Failed(t("内部错误：目录任务未展开").into()),
    }
}

async fn connect_profile(p: &Profile, cfg: &TransferCfg) -> Result<SftpConn, String> {
    match tokio::time::timeout(
        Duration::from_secs(cfg.connect_timeout.max(1) as u64),
        ssh::connect_sftp_silent(&p.host, p.port, &p.user, &p.auth, p.fingerprint.clone()),
    )
    .await
    {
        Ok(Ok(c)) => Ok(c),
        Ok(Err(e)) => Err(tfmt("传输连接失败：{0}", &[&String::from(e)])),
        Err(_) => Err(t("传输连接超时").into()),
    }
}

async fn ensure_conn(
    profile: &Option<Profile>,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    cfg: &TransferCfg,
) -> Result<(), String> {
    let Some(p) = profile else {
        return Err(t("无远程连接档案").into());
    };
    if conn.is_some() && conn_of.as_ref().is_some_and(|o| o == p) {
        return Ok(());
    }
    match connect_profile(p, cfg).await {
        Ok(c) => {
            *conn = Some(c);
            *conn_of = Some(p.clone());
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn checkpoint(shared: &Shared, id: u64) -> Option<Outcome> {
    if shared.paused.load(Ordering::Relaxed) {
        return Some(Outcome::Paused);
    }
    if shared.stopping.load(Ordering::Relaxed) {
        return Some(Outcome::Cancelled);
    }
    let st = shared.st.lock().unwrap();
    match st.items.iter().find(|i| i.id == id) {
        None => Some(Outcome::Cancelled),
        Some(i) if i.cancel_req => Some(Outcome::Cancelled),
        _ => None,
    }
}

fn progress(shared: &Shared, id: u64, transferred: u64, speed: u64) {
    let mut st = shared.st.lock().unwrap();
    if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
        i.transferred = transferred;
        i.speed = speed;
    }
}

fn log_pfx(prof: Option<&Profile>) -> String {
    match prof {
        Some(p) => format!("[{}@{}]", p.user, p.host),
        None => format!("[{}]", t("本地")),
    }
}

fn log(shared: &Shared, prof: Option<&Profile>, line: String) {

    let pfx = log_pfx(prof);
    shared.log.lock().unwrap().push(format!("{pfx} {line}"));
}

async fn upload_file(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local_path: &Path,
    remote: &str,
    base: u64,
    prof: Option<&Profile>,
) -> Outcome {
    let cfg = cfg_of(shared, id);
    let meta = match std::fs::metadata(local_path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Outcome::Failed(t("源不是文件").into()),
        Err(e) => return Outcome::Failed(tfmt("读取源文件失败：{0}", &[&e])),
    };
    let src = meta.len();
    let mut file = match std::fs::File::open(local_path) {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(tfmt("读取源文件失败：{0}", &[&e])),
    };

    let sftp = &conn.as_ref().unwrap().sftp;
    let (dest_exists, dest_len) = match sftp.metadata(remote).await {
        Ok(m) => (true, m.len()),
        Err(_) => (false, 0),
    };

    let (force, probe_incomplete) = {
        let st = shared.st.lock().unwrap();
        match st.items.iter().find(|i| i.id == id) {
            Some(i) => (i.force, i.probe_incomplete),
            None => (false, false),
        }
    };
    let decision = resume_decision(
        src,
        dest_len,
        dest_exists,
        cfg.resume,
        force,
        resume_threshold(&cfg),
        probe_incomplete,
    );
    match decision {
        ResumeDecision::Conflict => {
            log(
                shared,
                prof,
                tfmt(
                    "[续传] 目标大于源（内容可能已变），全量重传：{0}（目标 {1} 字节 / 源 {2} 字节）",
                    &[&remote, &dest_len, &src],
                ),
            );
        }
        ResumeDecision::Append => {
            log(
                shared,
                prof,
                tfmt(
                    "[续传] 从断点续传：{0}（{1}/{2} 字节）",
                    &[&remote, &dest_len, &src],
                ),
            );
        }
        ResumeDecision::Overwrite => {}
    }
    let append = decision == ResumeDecision::Append;
    use russh_sftp::protocol::OpenFlags;
    let (mut rf, start) = if append {
        match sftp
            .open_with_flags(remote, OpenFlags::CREATE | OpenFlags::WRITE)
            .await
        {
            Ok(mut f) => match f.seek(std::io::SeekFrom::Start(dest_len)).await {
                Ok(_) => (f, dest_len),

                Err(_) => match sftp
                    .open_with_flags(
                        remote,
                        OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                    )
                    .await
                {
                    Ok(f) => (f, 0),
                    Err(e) => return Outcome::Failed(tfmt("打开目标失败：{0}", &[&e])),
                },
            },

            Err(_) => match sftp
                .open_with_flags(
                    remote,
                    OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                )
                .await
            {
                Ok(f) => (f, 0),
                Err(e) => return Outcome::Failed(tfmt("打开目标失败：{0}", &[&e])),
            },
        }
    } else {
        match sftp
            .open_with_flags(
                remote,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
        {
            Ok(f) => (f, 0),
            Err(e) => return Outcome::Failed(tfmt("打开目标失败：{0}", &[&e])),
        }
    };
    if append {
        let mut st = shared.st.lock().unwrap();
        if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
            i.resumed = true;
        }
    }

    let mut buf = vec![0u8; CHUNK];
    let mut transferred = start;
    let mut speed_base = Instant::now();
    let mut speed_bytes = start;
    let mut cur_speed = 0u64;
    progress(shared, id, base + transferred, 0);
    loop {
        if let Some(o) = checkpoint(shared, id) {
            let _ = rf.flush().await;
            return o;
        }
        use std::io::Read;
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Outcome::Failed(tfmt("读取源文件失败：{0}", &[&e])),
        };
        if let Err(e) = rf.write_all(&buf[..n]).await {
            return Outcome::Failed(tfmt("写入远端失败：{0}", &[&e]));
        }
        transferred += n as u64;
        let now = Instant::now();
        if now.duration_since(speed_base) >= SPEED_WINDOW {
            let dt = now.duration_since(speed_base).as_secs_f64();
            cur_speed = ((transferred - speed_bytes) as f64 / dt) as u64;
            speed_base = now;
            speed_bytes = transferred;
        }
        progress(shared, id, base + transferred, cur_speed);
    }
    let _ = rf.flush().await;
    drop(rf);

    if cfg.preserve_mtime
        && let Ok(m) = std::fs::metadata(local_path)
        && let Ok(modt) = m.modified()
        && let Some(d) = modt.duration_since(std::time::UNIX_EPOCH).ok()
        && d.as_secs() <= u32::MAX as u64
    {
        let attrs = russh_sftp::protocol::FileAttributes {
            mtime: Some(d.as_secs() as u32),
            ..Default::default()
        };
        if let Err(e) = sftp.set_metadata(remote, attrs).await {
            log(
                shared,
                prof,
                tfmt(
                    "[mtime] 保留修改时间失败（不影响传输结果）：{0}（{1}）",
                    &[&remote, &e],
                ),
            );
        }
    }
    Outcome::Done
}

async fn download_file(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    remote: &str,
    local_path: &Path,
    base: u64,
    prof: Option<&Profile>,
) -> Outcome {
    let cfg = cfg_of(shared, id);
    let sftp = &conn.as_ref().unwrap().sftp;
    let (src, src_mtime) = match sftp.metadata(remote).await {
        Ok(m) if !m.is_dir() => (m.len(), m.mtime),
        Ok(_) => return Outcome::Failed(t("源不是文件").into()),
        Err(e) => return Outcome::Failed(tfmt("源不可读：{0}", &[&e])),
    };

    let (dest_exists, dest_len) = match std::fs::metadata(local_path) {
        Ok(m) => (true, m.len()),
        Err(_) => (false, 0),
    };
    let (force, probe_incomplete) = {
        let st = shared.st.lock().unwrap();
        match st.items.iter().find(|i| i.id == id) {
            Some(i) => (i.force, i.probe_incomplete),
            None => (false, false),
        }
    };
    let decision = resume_decision(
        src,
        dest_len,
        dest_exists,
        cfg.resume,
        force,
        resume_threshold(&cfg),
        probe_incomplete,
    );
    match decision {
        ResumeDecision::Conflict => {
            log(
                shared,
                prof,
                tfmt(
                    "[续传] 目标大于源（内容可能已变），全量重传：{0}（目标 {1} 字节 / 源 {2} 字节）",
                    &[&remote, &dest_len, &src],
                ),
            );
        }
        ResumeDecision::Append => {
            log(
                shared,
                prof,
                tfmt(
                    "[续传] 从断点续传：{0}（{1}/{2} 字节）",
                    &[&remote, &dest_len, &src],
                ),
            );
        }
        ResumeDecision::Overwrite => {}
    }
    let append = decision == ResumeDecision::Append;

    let mut lf = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .open(local_path)
    {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(tfmt("打开目标失败：{0}", &[&e])),
    };
    if append {
        use std::io::Seek;
        if lf.seek(std::io::SeekFrom::Start(dest_len)).is_err() {

            let _ = lf.set_len(0);
            let _ = lf.seek(std::io::SeekFrom::Start(0));
        } else {
            let mut st = shared.st.lock().unwrap();
            if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
                i.resumed = true;
            }
        }
    }

    let mut rf = match sftp.open(remote).await {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(tfmt("打开源失败：{0}", &[&e])),
    };
    let mut buf = vec![0u8; CHUNK];
    let mut transferred = if append { dest_len } else { 0 };
    let mut speed_base = Instant::now();
    let mut speed_bytes = transferred;
    let mut cur_speed = 0u64;
    progress(shared, id, base + transferred, 0);
    loop {
        if let Some(o) = checkpoint(shared, id) {
            return o;
        }
        use std::io::Write;
        let n = match rf.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Outcome::Failed(tfmt("读取源失败：{0}", &[&e])),
        };
        if let Err(e) = lf.write_all(&buf[..n]) {
            return Outcome::Failed(tfmt("写入目标失败：{0}", &[&e]));
        }
        transferred += n as u64;
        let now = Instant::now();
        if now.duration_since(speed_base) >= SPEED_WINDOW {
            let dt = now.duration_since(speed_base).as_secs_f64();
            cur_speed = ((transferred - speed_bytes) as f64 / dt) as u64;
            speed_base = now;
            speed_bytes = transferred;
        }
        progress(shared, id, base + transferred, cur_speed);
    }

    if cfg.preserve_mtime
        && let Some(mt) = src_mtime
    {
        let ft = filetime::FileTime::from_unix_time(mt as i64, 0);
        if let Err(e) = filetime::set_file_handle_times(&lf, Some(ft), Some(ft)) {
            log(
                shared,
                prof,
                tfmt(
                    "[mtime] 保留修改时间失败（不影响传输结果）：{0}（{1}）",
                    &[&local_path.display(), &e],
                ),
            );
        }
    }
    Outcome::Done
}

async fn relay_file(
    shared: &Shared,
    id: u64,
    src_prof: &Profile,
    dst_prof: &Profile,
    remote_src: &str,
    remote_dst: &str,
    temp_dir: &Path,
) -> Outcome {
    let cfg = cfg_of(shared, id);
    let name = remote_src.rsplit('/').next().unwrap_or(remote_src);

    let tmp_file = temp_dir.join(name);
    let part_file = temp_dir.join(format!("{name}.part"));

    if let Err(e) = std::fs::create_dir_all(temp_dir) {
        return Outcome::Failed(tfmt("创建中转目录失败：{0}", &[&e]));
    }
    log(
        shared,
        Some(src_prof),
        tfmt("[中转] 阶段 1/2 下载：{0} → 本地中转", &[&remote_src]),
    );
    let mut src_conn = match connect_profile(src_prof, &cfg).await {
        Ok(c) => Some(c),
        Err(e) => return Outcome::Failed(e),
    };
    let dl = download_file(
        shared,
        &mut src_conn,
        id,
        remote_src,
        &part_file,
        0,
        Some(src_prof),
    )
    .await;
    drop(src_conn);
    if !matches!(dl, Outcome::Done) {
        return dl;
    }
    if let Some(o) = checkpoint(shared, id) {
        return o;
    }

    if tmp_file.exists() {
        let _ = std::fs::remove_file(&tmp_file);
    }
    if let Err(e) = std::fs::rename(&part_file, &tmp_file) {
        return Outcome::Failed(tfmt("中转件转正失败：{0}", &[&e]));
    }
    let base = std::fs::metadata(&tmp_file).map(|m| m.len()).unwrap_or(0);
    log(
        shared,
        Some(dst_prof),
        tfmt("[中转] 阶段 2/2 上传：本地中转 → {0}", &[&remote_dst]),
    );
    let mut dst_conn = match connect_profile(dst_prof, &cfg).await {
        Ok(c) => Some(c),
        Err(e) => return Outcome::Failed(e),
    };
    let ul = upload_file(
        shared,
        &mut dst_conn,
        id,
        &tmp_file,
        remote_dst,
        base,
        Some(dst_prof),
    )
    .await;
    drop(dst_conn);

    if matches!(ul, Outcome::Done) {
        let _ = std::fs::remove_file(&tmp_file);
    }
    ul
}

async fn copy_local(shared: &Shared, id: u64, from: &Path, to: &Path) -> Outcome {
    let cfg = cfg_of(shared, id);
    let mut src = match std::fs::File::open(from) {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(tfmt("读取源文件失败：{0}", &[&e])),
    };
    let mut dst = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(to)
    {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(tfmt("打开目标失败：{0}", &[&e])),
    };
    let mut buf = vec![0u8; CHUNK];
    let mut transferred = 0u64;
    let mut speed_base = Instant::now();
    let mut speed_bytes = 0u64;
    let mut cur_speed = 0u64;
    loop {
        if let Some(o) = checkpoint(shared, id) {
            return o;
        }
        use std::io::{Read, Write};
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Outcome::Failed(tfmt("读取源文件失败：{0}", &[&e])),
        };
        if let Err(e) = dst.write_all(&buf[..n]) {
            return Outcome::Failed(tfmt("写入目标失败：{0}", &[&e]));
        }
        transferred += n as u64;
        let now = Instant::now();
        if now.duration_since(speed_base) >= SPEED_WINDOW {
            let dt = now.duration_since(speed_base).as_secs_f64();
            cur_speed = ((transferred - speed_bytes) as f64 / dt) as u64;
            speed_base = now;
            speed_bytes = transferred;
        }
        progress(shared, id, transferred, cur_speed);
    }

    if cfg.preserve_mtime
        && let Ok(m) = src.metadata()
        && let Ok(modt) = m.modified()
        && let Ok(d) = modt.duration_since(std::time::UNIX_EPOCH)
    {
        let ft = filetime::FileTime::from_unix_time(d.as_secs() as i64, d.subsec_nanos());
        if let Err(e) = filetime::set_file_handle_times(&dst, Some(ft), Some(ft)) {
            log(
                shared,
                None,
                tfmt(
                    "[mtime] 保留修改时间失败（不影响传输结果）：{0}（{1}）",
                    &[&to.display(), &e],
                ),
            );
        }
    }
    Outcome::Done
}

fn child_item(
    id: u64,
    t: &NewTask,
    parent: u64,
    batch: u64,
    force: bool,
    probe_incomplete: bool,
    profile: &Option<Profile>,
    probe: &Probe,
    cfg: &TransferCfg,
) -> Item {
    Item {
        id,
        source: t.source.clone(),
        dest: t.dest.clone(),
        size: if t.kind.is_relay() {
            t.size.saturating_mul(2)
        } else {
            t.size
        },
        transferred: 0,
        status: Status::Waiting,
        speed: 0,
        resumed: false,
        phase: 0,
        time: String::new(),
        verify_result: VerifyKind::None,
        reason: String::new(),
        cancel_req: false,
        owner: t.owner,
        batch,
        force,
        probe_incomplete,
        kind: t.kind.clone(),
        profile: profile.clone(),
        probe: probe.clone(),
        parent,
        precheck: false,
        cfg: cfg.clone(),
    }
}

async fn dir_tasks(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    kind: &Kind,
    profile: &Option<Profile>,
    owner: TaskOwner,
) -> (bool, Result<Vec<NewTask>, String>) {
    let cfg = cfg_of(shared, id);

    let force = shared
        .st
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .map(|i| i.force)
        == Some(true);
    let tasks: Result<Vec<NewTask>, String> = match kind {
        Kind::UploadDir { local, remote } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                Err(e)
            } else {

                let mut v = vec![NewTask {
                    source: local.to_string_lossy().into_owned(),
                    dest: remote.clone(),
                    size: 0,
                    force: false,
                    probe_incomplete: false,
                    owner,
                    kind: Kind::MakeDir {
                        remote: Some(remote.clone()),
                        local: None,
                    },
                }];
                match walk_local(local) {
                    Ok(list) => {
                        for (rel, is_dir, size) in list {
                            let r = join_remote(remote, &rel_to_remote(&rel));
                            let l = local.join(&rel);
                            if is_dir {
                                v.push(NewTask {
                                    source: l.to_string_lossy().into_owned(),
                                    dest: r.clone(),
                                    size: 0,
                                    force: false,
                                    probe_incomplete: false,
                                    owner,
                                    kind: Kind::MakeDir {
                                        remote: Some(r),
                                        local: None,
                                    },
                                });
                            } else {
                                v.push(NewTask {
                                    source: l.to_string_lossy().into_owned(),
                                    dest: r.clone(),
                                    size,
                                    force: false,
                                    probe_incomplete: false,
                                    owner,
                                    kind: Kind::UploadFile {
                                        local: l,
                                        remote: r,
                                    },
                                });
                            }
                        }
                        Ok(v)
                    }
                    Err(e) => Err(tfmt("枚举源目录失败：{0}", &[&e])),
                }
            }
        }
        Kind::DownloadDir { remote, local } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of, &cfg).await {
                Err(e)
            } else {
                let sftp = &conn.as_ref().unwrap().sftp;
                let mut v = vec![NewTask {
                    source: remote.clone(),
                    dest: local.to_string_lossy().into_owned(),
                    size: 0,
                    force: false,
                    probe_incomplete: false,
                    owner,
                    kind: Kind::MakeDir {
                        remote: None,
                        local: Some(local.clone()),
                    },
                }];
                match walk_remote(sftp, remote).await {
                    Ok(list) => {
                        for (rel, is_dir, size) in list {
                            let r = join_remote(remote, &rel);
                            let l = local.join(rel.replace('/', "\\"));
                            if is_dir {
                                v.push(NewTask {
                                    source: r.clone(),
                                    dest: l.to_string_lossy().into_owned(),
                                    size: 0,
                                    force: false,
                                    probe_incomplete: false,
                                    owner,
                                    kind: Kind::MakeDir {
                                        remote: None,
                                        local: Some(l),
                                    },
                                });
                            } else {
                                v.push(NewTask {
                                    source: r.clone(),
                                    dest: l.to_string_lossy().into_owned(),
                                    size,
                                    force: false,
                                    probe_incomplete: false,
                                    owner,
                                    kind: Kind::DownloadFile {
                                        remote: r,
                                        local: l,
                                    },
                                });
                            }
                        }
                        Ok(v)
                    }
                    Err(e) => Err(tfmt("枚举源目录失败：{0}", &[&e])),
                }
            }
        }
        Kind::CopyLocalDir { from, to } => {
            let mut v = vec![NewTask {
                source: from.to_string_lossy().into_owned(),
                dest: to.to_string_lossy().into_owned(),
                size: 0,
                force: false,
                probe_incomplete: false,
                owner,
                kind: Kind::MakeDir {
                    remote: None,
                    local: Some(to.clone()),
                },
            }];
            match walk_local(from) {
                Ok(list) => {
                    for (rel, is_dir, size) in list {
                        let f = from.join(&rel);
                        let t = to.join(&rel);
                        if is_dir {
                            v.push(NewTask {
                                source: f.to_string_lossy().into_owned(),
                                dest: t.to_string_lossy().into_owned(),
                                size: 0,
                                force: false,
                                probe_incomplete: false,
                                owner,
                                kind: Kind::MakeDir {
                                    remote: None,
                                    local: Some(t),
                                },
                            });
                        } else {
                            v.push(NewTask {
                                source: f.to_string_lossy().into_owned(),
                                dest: t.to_string_lossy().into_owned(),
                                size,
                                force: false,
                                probe_incomplete: false,
                                owner,
                                kind: Kind::CopyLocal { from: f, to: t },
                            });
                        }
                    }
                    Ok(v)
                }
                Err(e) => Err(tfmt("枚举源目录失败：{0}", &[&e])),
            }
        }
        Kind::RelayDir {
            src,
            dst,
            remote_src,
            remote_dst,
            temp,
        } => {

            match connect_profile(src, &cfg).await {
                Ok(sc) => {
                    let sftp = &sc.sftp;
                    let mut v = vec![NewTask {
                        source: remote_src.clone(),
                        dest: remote_dst.clone(),
                        size: 0,
                        force: false,
                        probe_incomplete: false,
                        owner,

                        kind: Kind::MakeDir {
                            remote: Some(remote_dst.clone()),
                            local: Some(temp.clone()),
                        },
                    }];
                    match walk_remote(sftp, remote_src).await {
                        Ok(list) => {
                            for (rel, is_dir, size) in list {
                                let r = join_remote(remote_dst, &rel);
                                let l = temp.join(rel.replace('/', "\\"));
                                if is_dir {
                                    v.push(NewTask {
                                        source: r.clone(),
                                        dest: l.to_string_lossy().into_owned(),
                                        size: 0,
                                        force: false,
                                        probe_incomplete: false,
                                        owner,
                                        kind: Kind::MakeDir {
                                            remote: Some(r),
                                            local: Some(l),
                                        },
                                    });
                                } else {
                                    v.push(NewTask {
                                        source: join_remote(remote_src, &rel),
                                        dest: r.clone(),
                                        size,
                                        force: false,
                                        probe_incomplete: false,
                                        owner,
                                        kind: Kind::RelayFile {
                                            src: src.clone(),
                                            dst: dst.clone(),
                                            remote_src: join_remote(remote_src, &rel),
                                            remote_dst: r,
                                            temp: l.parent().unwrap_or(temp).to_path_buf(),
                                        },
                                    });
                                }
                            }
                            Ok(v)
                        }
                        Err(e) => Err(tfmt("枚举源目录失败：{0}", &[&e])),
                    }
                }
                Err(e) => Err(e),
            }
        }
        _ => Err(t("内部错误：非目录任务").into()),
    };
    (force, tasks)
}

#[allow(clippy::too_many_arguments)]
async fn expand(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    kind: Kind,
    profile: &Option<Profile>,
    probe: &Probe,
    owner: TaskOwner,
) {
    let cfg = cfg_of(shared, id);
    let (force, tasks) = dir_tasks(shared, conn, conn_of, id, &kind, profile, owner).await;

    let mut st = shared.st.lock().unwrap();
    let Some(pos) = st.items.iter().position(|i| i.id == id) else {
        return;
    };
    match tasks {
        Ok(list) => {

            let list = list
                .into_iter()
                .map(|mut t| {
                    t.force = force;
                    t
                })
                .collect::<Vec<_>>();

            let cancelled = shared.stopping.load(Ordering::Relaxed) || st.items[pos].cancel_req;

            let batch = st.items[pos].batch;

            let probe_incomplete = st.items[pos].probe_incomplete;
            st.items.remove(pos);
            for (k, t) in list.into_iter().enumerate() {
                let tid = st.next_id;
                st.next_id += 1;
                let mut it = child_item(
                    tid,
                    &t,
                    id,
                    batch,
                    force,
                    probe_incomplete,
                    profile,
                    probe,
                    &cfg,
                );
                if cancelled {
                    it.status = Status::Cancelled;
                    it.time = local::now_hms();
                }
                st.items.insert(pos + k, it);
            }
        }
        Err(e) => {
            let item = &mut st.items[pos];
            item.status = Status::Failed;
            item.reason = e;
            item.time = local::now_hms();
        }
    }
}

async fn scan_run(shared: Arc<Shared>, mut rx: tokio::sync::mpsc::UnboundedReceiver<ScanJob>) {
    let mut conn: Option<SftpConn> = None;
    let mut conn_of: Option<Profile> = None;
    while let Some(job) = rx.recv().await {
        scan_job(&shared, job, &mut conn, &mut conn_of).await;
        if rx.is_empty() {
            conn = None;
            conn_of = None;
        }
    }
}

async fn scan_job(
    shared: &Shared,
    job: ScanJob,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
) {
    let ScanJob {
        id,
        kind,
        profile,
        owner,
    } = job;

    if !shared
        .st
        .lock()
        .unwrap()
        .items
        .iter()
        .any(|i| i.id == id && i.status == Status::Scanning)
    {
        return;
    }

    let cfg = cfg_of(shared, id);

    let (on, threshold) = (cfg.tar_accel, cfg.tar_threshold);
    if on && threshold > 0 {
        match &kind {
            Kind::UploadDir { local, .. } => {
                let count = tar::count_local_files(local, threshold as i64);
                if let Err(e) = &count {
                    log(
                        shared,
                        profile.as_ref(),
                        tfmt("[tar 加速] 本地目录扫描失败，回退逐文件：{0}", &[e]),
                    );
                }
                if tar::gate_open(on, threshold as i64, true, count.ok()) {
                    handback_tar(shared, id);
                    return;
                }
            }
            Kind::DownloadDir { remote, .. } => {
                if ensure_conn(&profile, conn, conn_of, &cfg).await.is_ok() {
                    let n = match conn.as_ref() {
                        Some(c) => tar::count_remote_files(c, remote, threshold as i64).await,
                        None => -1,
                    };
                    if tar::gate_open(on, threshold as i64, true, (n >= 0).then_some(n)) {
                        handback_tar(shared, id);
                        return;
                    }
                }

            }
            _ => {}
        }
    }

    let (_, tasks) = dir_tasks(shared, conn, conn_of, id, &kind, &profile, owner).await;
    let list = match tasks {
        Ok(l) => l,
        Err(e) => {
            let failed = {
                let mut st = shared.st.lock().unwrap();
                match st.items.iter_mut().find(|i| i.id == id) {
                    Some(i) if i.status == Status::Scanning => {
                        i.status = Status::Failed;
                        i.reason = e;
                        i.time = local::now_hms();
                        true
                    }
                    _ => false,
                }
            };
            if failed {
                shared.wake.notify_one();
            }
            return;
        }
    };

    let mut placed = 0usize;
    for chunk in list.chunks(SCAN_BATCH) {
        let n = {
            let mut st = shared.st.lock().unwrap();
            let Some(pos) = st.items.iter().position(|i| i.id == id) else {
                return;
            };
            if st.items[pos].status != Status::Scanning {
                return;
            }
            let batch = st.items[pos].batch;
            let force = st.items[pos].force;

            let probe_incomplete = st.items[pos].probe_incomplete;
            let prof = st.items[pos].profile.clone();
            let prb = st.items[pos].probe.clone();

            let at = st
                .items
                .iter()
                .rposition(|i| i.parent == id)
                .map(|p| p + 1)
                .unwrap_or(pos + 1);
            let mut ins = Vec::with_capacity(chunk.len());
            for t in chunk {
                let tid = st.next_id;
                st.next_id += 1;
                ins.push(child_item(
                    tid,
                    t,
                    id,
                    batch,
                    force,
                    probe_incomplete,
                    &prof,
                    &prb,
                    &cfg,
                ));
            }
            st.items.splice(at..at, ins);
            chunk.len()
        };
        placed += n;

        #[cfg(test)]
        shared.scan_batches_placed.fetch_add(1, Ordering::Relaxed);
    }
    if placed > 0 {
        shared.wake.notify_one();
    }

    let removed = {
        let mut st = shared.st.lock().unwrap();
        match st.items.iter().position(|i| i.id == id) {
            Some(pos) if st.items[pos].status == Status::Scanning => {
                st.items.remove(pos);
                true
            }
            _ => false,
        }
    };
    if removed {
        shared.wake.notify_one();
    }
}

fn handback_tar(shared: &Shared, id: u64) {
    let handed = {
        let mut st = shared.st.lock().unwrap();
        match st.items.iter_mut().find(|i| i.id == id) {
            Some(i) if i.status == Status::Scanning => {
                i.status = Status::Waiting;
                i.precheck = true;
                true
            }
            _ => false,
        }
    };
    if handed {
        shared.wake.notify_one();
    }
}

fn walk_local(root: &Path) -> std::io::Result<Vec<(PathBuf, bool, u64)>> {
    const MAX_DEPTH: usize = 128;
    enum Frame {

        Emit(PathBuf, bool, u64),

        Dir {
            abs: PathBuf,
            rel: PathBuf,
            depth: usize,
        },
    }
    let mut out = Vec::new();
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));

    let mut stack: Vec<Frame> = vec![Frame::Dir {
        abs: root.to_path_buf(),
        rel: PathBuf::new(),
        depth: 0,
    }];
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Emit(rel, is_dir, size) => out.push((rel, is_dir, size)),
            Frame::Dir { abs, rel, depth } => {
                let mut list: Vec<std::fs::DirEntry> =
                    std::fs::read_dir(&abs)?.collect::<Result<_, _>>()?;
                list.sort_by_key(|e| e.file_name());
                for e in list.into_iter().rev() {
                    let meta = std::fs::symlink_metadata(e.path())?;

                    let is_dir = if meta.file_type().is_symlink() {
                        std::fs::metadata(e.path())
                            .map(|m| m.is_dir())
                            .unwrap_or(false)
                    } else {
                        meta.is_dir()
                    };
                    let r = rel.join(e.file_name());
                    if is_dir {
                        if depth + 1 >= MAX_DEPTH {
                            return Err(std::io::Error::other(format!(
                                "目录嵌套超过 {MAX_DEPTH} 层上限：{}",
                                e.path().display()
                            )));
                        }
                        let canon = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
                        if !visited.insert(canon) {
                            return Err(std::io::Error::other(format!(
                                "检测到目录环（符号链接/联接循环）：{}",
                                e.path().display()
                            )));
                        }
                        stack.push(Frame::Dir {
                            abs: e.path(),
                            rel: r.clone(),
                            depth: depth + 1,
                        });
                        stack.push(Frame::Emit(r, true, 0));
                    } else if meta.is_file() {
                        stack.push(Frame::Emit(r, false, meta.len()));
                    }
                }
            }
        }
    }
    Ok(out)
}

async fn walk_remote(
    sftp: &russh_sftp::client::SftpSession,
    root: &str,
) -> Result<Vec<(String, bool, u64)>, String> {
    async fn rec(
        sftp: &russh_sftp::client::SftpSession,
        dir: &str,
        rel: &str,
        out: &mut Vec<(String, bool, u64)>,
    ) -> Result<(), String> {
        let rd = sftp.read_dir(dir).await.map_err(|e| e.to_string())?;
        let mut list: Vec<russh_sftp::client::fs::DirEntry> = rd.collect();
        list.sort_by_key(|e| e.file_name());
        for de in list {
            let name = de.file_name();
            let meta = de.metadata();
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if meta.is_dir() {
                out.push((r.clone(), true, 0));
                Box::pin(rec(sftp, &join_remote(dir, &name), &r, out)).await?;
            } else {
                out.push((r, false, meta.len()));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    rec(sftp, root, "", &mut out).await?;
    Ok(out)
}

pub(crate) fn join_remote(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

fn rel_to_remote(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);

const VERIFY_ALGO_NAMES: [&str; 3] = ["SHA256", "SHA512", "SM3"];

fn pick_algorithm(preferred: usize, probe: crate::ssh::HashProbe) -> Option<usize> {
    match preferred {
        1 if probe.sha256 => Some(0),
        2 if probe.sha512 => Some(1),
        3 if probe.sm3 => Some(2),
        1 | 2 | 3 => None,
        _ if probe.sha256 => Some(0),
        _ if probe.sha512 => Some(1),
        _ if probe.sm3 => Some(2),
        _ => None,
    }
}

#[cfg(feature = "official")]
pub(crate) use crate::ifree_core::transfer::ResumeDecision;

#[cfg(not(feature = "official"))]

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub(crate) enum ResumeDecision {

    Append,

    Overwrite,

    Conflict,
}

pub(crate) fn resume_decision(
    src: u64,
    dst: u64,
    dst_exists: bool,
    resume_enabled: bool,
    force: bool,
    threshold: u64,
    probe_incomplete: bool,
) -> ResumeDecision {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::transfer::resume_decision(
            src,
            dst,
            dst_exists,
            resume_enabled,
            force,
            threshold,
            probe_incomplete,
        )
    }
    #[cfg(not(feature = "official"))]
    {

        if force || !resume_enabled || !dst_exists || dst == 0 {
            return ResumeDecision::Overwrite;
        }
        if dst > src {
            return ResumeDecision::Conflict;
        }
        if probe_incomplete {
            return ResumeDecision::Overwrite;
        }
        if src < threshold {
            return ResumeDecision::Overwrite;
        }
        if dst == src {
            ResumeDecision::Overwrite
        } else {
            ResumeDecision::Append
        }
    }
}

fn verify_decision(local: Option<&str>, remote: Option<&str>) -> VerifyKind {
    let Some(a) = local.map(str::trim).filter(|s| !s.is_empty()) else {
        return VerifyKind::Skipped;
    };
    let Some(b) = remote.map(str::trim).filter(|s| !s.is_empty()) else {
        return VerifyKind::Skipped;
    };
    if a.eq_ignore_ascii_case(b) {
        VerifyKind::Match
    } else {
        VerifyKind::Mismatch
    }
}

async fn verify_run(shared: Arc<Shared>, mut rx: tokio::sync::mpsc::UnboundedReceiver<VerifyJob>) {

    let mut conn: Option<SftpConn> = None;
    let mut conn_of: Option<Profile> = None;
    while let Some(job) = rx.recv().await {

        let live = shared
            .st
            .lock()
            .unwrap()
            .items
            .iter()
            .any(|i| i.id == job.id && i.status == Status::Verifying);
        if !live {
            continue;
        }
        let outcome = match ensure_conn(&job.prof, &mut conn, &mut conn_of, &job.cfg).await {
            Err(_) => {
                set_verify_result(&shared, job.id, VerifyKind::Skipped);
                log(
                    &shared,
                    job.prof.as_ref(),
                    tfmt("[校验] 校验跳过：{0}", &[&job.log_path]),
                );
                Outcome::Done
            }
            Ok(()) => {

                let fut = run_verify(&shared, &mut conn, &job);
                match tokio::time::timeout(VERIFY_TIMEOUT, fut).await {
                    Ok(o) => o,

                    Err(_) => {
                        set_verify_result(&shared, job.id, VerifyKind::Skipped);
                        log(
                            &shared,
                            job.prof.as_ref(),
                            tfmt("[校验] 校验跳过：{0}", &[&job.log_path]),
                        );
                        Outcome::Done
                    }
                }
            }
        };

        settle(&shared, &mut conn, &mut conn_of, job.id, true, outcome);

        if rx.is_empty() {
            conn = None;
            conn_of = None;
        }
    }
}

fn set_verify_result(shared: &Shared, id: u64, r: VerifyKind) {
    let mut st = shared.st.lock().unwrap();
    if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
        i.verify_result = r;
    }
}

async fn run_verify(shared: &Shared, conn: &mut Option<SftpConn>, job: &VerifyJob) -> Outcome {

    let probe = job.probe.lock().unwrap().1.unwrap_or_default();

    let Some(algo) = pick_algorithm(job.cfg.verify_algo, probe) else {
        set_verify_result(shared, job.id, VerifyKind::Skipped);
        log(
            shared,
            job.prof.as_ref(),
            tfmt("[校验] 校验跳过：{0}", &[&job.log_path]),
        );
        return Outcome::Done;
    };
    let name = VERIFY_ALGO_NAMES[algo];

    let lp = job.local.to_owned();
    let lh = tokio::task::spawn_blocking(move || local_hash(&lp, algo))
        .await
        .unwrap_or(None);
    let rh = server_hash(conn, &job.remote, algo).await;

    let (Some(a), Some(b)) = (lh, rh) else {
        set_verify_result(shared, job.id, VerifyKind::Skipped);
        log(
            shared,
            job.prof.as_ref(),
            tfmt("[校验] 校验跳过：{0}", &[&job.log_path]),
        );
        return Outcome::Done;
    };
    match verify_decision(Some(&a), Some(&b)) {
        VerifyKind::Skipped => {
            set_verify_result(shared, job.id, VerifyKind::Skipped);
            log(
                shared,
                job.prof.as_ref(),
                tfmt("[校验] 校验跳过：{0}", &[&job.log_path]),
            );
            Outcome::Done
        }
        VerifyKind::Match => {
            set_verify_result(shared, job.id, VerifyKind::Match);

            log(
                shared,
                job.prof.as_ref(),
                tfmt("[校验] 校验通过：{0}（{1}）", &[&job.log_path, &name]),
            );
            Outcome::Done
        }

        VerifyKind::None => {
            set_verify_result(shared, job.id, VerifyKind::Skipped);
            log(
                shared,
                job.prof.as_ref(),
                tfmt("[校验] 校验跳过：{0}", &[&job.log_path]),
            );
            Outcome::Done
        }
        VerifyKind::Mismatch => {

            set_verify_result(shared, job.id, VerifyKind::Mismatch);
            log(
                shared,
                job.prof.as_ref(),
                tfmt(
                    "[校验] 校验不匹配：{0}（本地 {1} / 远端 {2}）",
                    &[&job.log_path, &a, &b],
                ),
            );
            Outcome::Failed(tfmt("校验不匹配（本地 {0} / 远端 {1}）", &[&a, &b]))
        }
    }
}

fn local_hash(path: &Path, algo: usize) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 256 * 1024];
    use std::io::Read;
    if algo == 2 {

        let mut h = crate::sm3::Sm3::new();
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => h.update(&buf[..n]),
                Err(_) => return None,
            }
        }
        return Some(h.finish_hex());
    }
    use ring::digest as dg;
    let alg = if algo == 0 { &dg::SHA256 } else { &dg::SHA512 };
    let mut ctx = dg::Context::new(alg);
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => ctx.update(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(
        ctx.finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

async fn server_hash(conn: &Option<SftpConn>, path: &str, algo: usize) -> Option<String> {
    let c = conn.as_ref()?;
    let cmd = match algo {
        0 => format!("sha256sum {}", ssh::shell_quote(path)),
        1 => format!("sha512sum {}", ssh::shell_quote(path)),
        _ => format!("openssl dgst -sm3 {}", ssh::shell_quote(path)),
    };
    let (exit, out, _err) = c.exec(&cmd).await.ok()?;
    if exit != 0 {
        return None;
    }
    let line = out.lines().next()?.trim();
    if algo <= 1 {
        let s = line.split_whitespace().next()?;
        ((s.len() == 64 || s.len() == 128) && s.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| s.to_ascii_lowercase())
    } else {
        let s = line.rsplit('=').next()?.trim();
        (s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod owner_tests {
    use super::{Status, TaskOwner, collect_owner_tabs, count_owned_active};

    #[test]
    fn count_src_or_dst_hit_waiting_transferring_only() {
        let o_tab = TaskOwner {
            src: (0, 7),
            dst: (1, 9),
        };
        let o_other = TaskOwner {
            src: (0, 8),
            dst: (1, 8),
        };
        let rows = [
            (&o_tab, Status::Waiting),
            (&o_tab, Status::Transferring),
            (&o_tab, Status::Verifying),
            (&o_tab, Status::Done),
            (&o_other, Status::Waiting),
        ];
        assert_eq!(count_owned_active(rows, (0, 7)), 2);
        assert_eq!(count_owned_active(rows, (1, 9)), 2);
        assert_eq!(count_owned_active(rows, (1, 8)), 1);
        assert_eq!(count_owned_active(rows, (0, 99)), 0);
    }

    #[test]
    fn collect_dedup_sorted_includes_verifying() {
        let a = TaskOwner {
            src: (1, 5),
            dst: (0, 2),
        };
        let b = TaskOwner {
            src: (0, 2),
            dst: (0, 2),
        };
        let c = TaskOwner {
            src: (0, 3),
            dst: (1, 4),
        };
        let rows = [
            (&a, Status::Transferring),
            (&b, Status::Waiting),
            (&c, Status::Done),
        ];
        assert_eq!(collect_owner_tabs(rows), vec![(0, 2), (1, 5)]);

        let d = TaskOwner::ZERO;
        assert_eq!(collect_owner_tabs([(&d, Status::Verifying)]), vec![(0, 0)]);
    }
}

#[cfg(test)]
mod resume_tests {
    use super::{ResumeDecision::*, resume_decision};

    const OFF: u64 = 0;

    #[test]
    fn remote_partial_appends() {

        assert_eq!(
            resume_decision(1000, 400, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn local_partial_appends() {

        assert_eq!(
            resume_decision(5000, 4999, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn dest_missing_overwrites() {
        assert_eq!(
            resume_decision(1000, 0, false, true, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn dest_zero_overwrites() {

        assert_eq!(
            resume_decision(1000, 0, true, true, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn resume_disabled_overwrites() {

        assert_eq!(
            resume_decision(1000, 400, true, false, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn equal_size_overwrites() {

        assert_eq!(
            resume_decision(1000, 1000, true, true, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn dest_larger_is_conflict() {

        assert_eq!(
            resume_decision(400, 1000, true, true, false, OFF, false),
            Conflict
        );
    }

    #[test]
    fn empty_source_never_appends() {

        assert_eq!(
            resume_decision(0, 0, true, true, false, OFF, false),
            Overwrite
        );
        assert_eq!(
            resume_decision(0, 5, true, true, false, OFF, false),
            Conflict
        );
    }

    #[test]
    fn single_byte_smaller_appends() {

        assert_eq!(
            resume_decision(101, 100, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn dest_flag_false_overwrites_even_with_plausible_size() {

        assert_eq!(
            resume_decision(1000, 500, false, true, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn force_overrides_resume() {

        assert_eq!(
            resume_decision(1000, 400, true, true, true, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn force_overrides_conflict_too() {

        assert_eq!(
            resume_decision(400, 1000, true, true, true, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn threshold_small_source_overwrites() {

        const T: u64 = 1000;
        assert_eq!(
            resume_decision(999, 500, true, true, false, T, false),
            Overwrite
        );
        assert_eq!(
            resume_decision(1000, 500, true, true, false, T, false),
            Append
        );
        assert_eq!(
            resume_decision(2000, 500, true, true, false, T, false),
            Append
        );
        assert_eq!(
            resume_decision(100, 500, true, true, false, T, false),
            Conflict
        );
    }

    #[test]
    fn threshold_disabled_keeps_legacy_behavior() {

        assert_eq!(
            resume_decision(101, 100, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn probe_incomplete_blocks_append() {

        assert_eq!(
            resume_decision(1000, 400, true, true, false, OFF, true),
            Overwrite
        );

        assert_eq!(
            resume_decision(1000, 400, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn probe_incomplete_after_conflict_branch() {

        assert_eq!(
            resume_decision(400, 1000, true, true, false, OFF, true),
            Conflict
        );
    }

    #[test]
    fn probe_incomplete_not_force_semantics() {

        assert_eq!(
            resume_decision(1000, 0, true, true, true, OFF, true),
            Overwrite
        );
        assert_eq!(
            resume_decision(400, 1000, true, true, true, OFF, false),
            Overwrite
        );
    }
}

#[cfg(test)]
mod verify_tests {
    use super::*;
    use crate::ssh::HashProbe;

    fn probe(s256: bool, s512: bool, sm3: bool) -> HashProbe {
        HashProbe {
            posix: false,
            sha256: s256,
            sha512: s512,
            sm3,
        }
    }

    #[test]
    fn decide_equal_is_match() {
        assert!(matches!(
            verify_decision(Some("abc123"), Some("abc123")),
            VerifyKind::Match
        ));
    }
    #[test]
    fn decide_case_insensitive_is_match() {
        assert!(matches!(
            verify_decision(Some("ABC123"), Some("abc123")),
            VerifyKind::Match
        ));
    }
    #[test]
    fn decide_differ_is_mismatch() {
        assert!(matches!(
            verify_decision(Some("abc123"), Some("abc124")),
            VerifyKind::Mismatch
        ));
    }
    #[test]
    fn decide_none_local_skipped() {
        assert!(matches!(
            verify_decision(None, Some("abc123")),
            VerifyKind::Skipped
        ));
    }
    #[test]
    fn decide_none_remote_skipped() {
        assert!(matches!(
            verify_decision(Some("abc123"), None),
            VerifyKind::Skipped
        ));
    }
    #[test]
    fn decide_both_none_skipped() {
        assert!(matches!(verify_decision(None, None), VerifyKind::Skipped));
    }
    #[test]
    fn decide_empty_local_skipped() {
        assert!(matches!(
            verify_decision(Some(""), Some("abc123")),
            VerifyKind::Skipped
        ));
    }
    #[test]
    fn decide_whitespace_local_skipped() {
        assert!(matches!(
            verify_decision(Some("   "), Some("abc123")),
            VerifyKind::Skipped
        ));
    }

    #[test]
    fn pick_auto_fallback_order() {
        assert_eq!(pick_algorithm(0, probe(true, true, true)), Some(0));
        assert_eq!(pick_algorithm(0, probe(false, true, true)), Some(1));
        assert_eq!(pick_algorithm(0, probe(false, false, true)), Some(2));
    }
    #[test]
    fn pick_explicit_sm3_available() {
        assert_eq!(pick_algorithm(3, probe(true, true, true)), Some(2));
    }
    #[test]
    fn pick_explicit_sha256_unavailable_is_none() {
        assert_eq!(pick_algorithm(1, probe(false, true, true)), None);
    }
    #[test]
    fn pick_explicit_sm3_unavailable_is_none() {
        assert_eq!(pick_algorithm(3, probe(true, true, false)), None);
    }
    #[test]
    fn pick_auto_all_unavailable_is_none() {
        assert_eq!(pick_algorithm(0, probe(false, false, false)), None);
    }
    #[test]
    fn pick_explicit_all_unavailable_is_none() {
        assert_eq!(pick_algorithm(1, probe(false, false, false)), None);
    }

    #[test]
    fn pick_out_of_domain_treated_as_auto_divergence() {
        assert_eq!(pick_algorithm(99, probe(true, true, true)), Some(0));
    }

    #[test]
    fn local_hash_sm3_official_vector() {
        let mut p = std::env::temp_dir();
        p.push("ifree-sm3-local-hash-case.tmp");

        std::fs::write(&p, b"abcd".repeat(16).to_vec()).unwrap();
        let h64 = local_hash(&p, 2).expect("SM3 本机侧必须可算");
        std::fs::write(&p, format!("{}abc", "abcd".repeat(16))).unwrap();
        let h67 = local_hash(&p, 2).expect("SM3 本机侧必须可算");
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            h64,
            "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732"
        );
        assert_eq!(
            h67,
            "952eb84cacee9c10bde4d6882d29d63140ba72af6fe485085095dccd5b872453"
        );

        assert_eq!(local_hash(Path::new("Z:/definitely/not/exists"), 2), None);
    }

    #[test]
    fn settings_defaults_off_and_auto() {
        let s = crate::settings::AppSettings::default();
        assert!(!s.verify);
        assert_eq!(s.verify_algo, 0);
        assert_eq!(s.resume_threshold_kb, 1024);
        assert_eq!(s.tar_threshold, 50);
    }

    #[test]
    fn join_remote_trailing_slash_normalized() {
        assert_eq!(join_remote("/home/user/", "a.txt"), "/home/user/a.txt");
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    fn prof(host: &str, user: &str) -> Profile {
        Profile {
            host: host.to_owned(),
            port: 22,
            user: user.to_owned(),
            auth: crate::ssh::Auth::Password("pw".to_owned()),
            fingerprint: Some("SHA256:first-connect".to_owned()),
        }
    }

    #[test]
    fn h2_two_batches_two_hosts_do_not_cross_contaminate() {
        let pa = prof("host-a.example", "alice");
        let pb = prof("host-b.example", "bob");
        let shared = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),

            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,

            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        });
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(
            Some(pa.clone()),
            Arc::clone(&probe),
            vec![NewTask {
                source: "C:/a.bin".to_owned(),
                dest: "/srv/a.bin".to_owned(),
                size: 1,
                force: false,
                probe_incomplete: false,
                owner: TaskOwner::ZERO,
                kind: Kind::UploadFile {
                    local: "C:/a.bin".into(),
                    remote: "/srv/a.bin".to_owned(),
                },
            }],
            0,
        );
        q.enqueue(
            Some(pb.clone()),
            probe,
            vec![NewTask {
                source: "C:/b.bin".to_owned(),
                dest: "/srv/b.bin".to_owned(),
                size: 1,
                force: false,
                probe_incomplete: false,
                owner: TaskOwner::ZERO,
                kind: Kind::UploadFile {
                    local: "C:/b.bin".into(),
                    remote: "/srv/b.bin".to_owned(),
                },
            }],
            0,
        );

        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 2);
        assert_eq!(st.items[0].profile.as_ref().unwrap().host, "host-a.example");
        assert_eq!(st.items[0].profile.as_ref().unwrap().user, "alice");
        assert_eq!(st.items[1].profile.as_ref().unwrap().host, "host-b.example");
        assert_eq!(st.items[1].profile.as_ref().unwrap().user, "bob");
        drop(st);

        log(&shared, Some(&pa), "批次A行".to_owned());
        log(&shared, Some(&pb), "批次B行".to_owned());
        let logs = shared.log.lock().unwrap();
        assert!(
            logs.iter()
                .any(|l| l.starts_with("[alice@host-a.example] ") && l.ends_with("批次A行")),
            "批次A日志前缀失真：{logs:?}"
        );
        assert!(
            logs.iter()
                .any(|l| l.starts_with("[bob@host-b.example] ") && l.ends_with("批次B行")),
            "批次B日志前缀失真：{logs:?}"
        );
    }

    #[test]
    fn h1_unpersisted_fingerprint_does_not_block_transfer() {
        let confirmed = prof("host-c.example", "carol");

        assert_eq!(
            confirmed.fingerprint.as_deref(),
            Some("SHA256:first-connect")
        );

        assert_eq!(confirmed, prof("host-c.example", "carol"));

        let rotated = Profile {
            fingerprint: Some("SHA256:rotated".to_owned()),
            ..confirmed.clone()
        };
        assert_ne!(confirmed, rotated);
    }
}

#[cfg(test)]
mod mkdir_tests {
    use super::{Outcome, mkdir_judge};

    #[test]
    fn remote_created_done() {
        assert!(matches!(mkdir_judge("/a/b", None, false), Outcome::Done));
    }

    #[test]
    fn remote_failed_but_recheck_is_dir_done() {
        assert!(matches!(
            mkdir_judge("/a/b", Some("failure".into()), true),
            Outcome::Done
        ));
    }

    #[test]
    fn remote_failed_and_missing_failed() {
        assert!(matches!(
            mkdir_judge("/a/b", Some("permission denied".into()), false),
            Outcome::Failed(_)
        ));
    }

    #[test]
    fn local_created_done() {
        assert!(matches!(
            mkdir_judge("C:\\a\\b", None, false),
            Outcome::Done
        ));
    }

    #[test]
    fn local_failed_but_recheck_is_dir_done() {
        assert!(matches!(
            mkdir_judge("C:\\a\\b", Some("已存在同名文件".into()), true),
            Outcome::Done
        ));
    }

    #[test]
    fn local_failed_and_missing_failed() {
        assert!(matches!(
            mkdir_judge("C:\\a\\b", Some("拒绝访问。".into()), false),
            Outcome::Failed(_)
        ));
    }

    #[test]
    fn failure_message_carries_path_and_error() {
        match mkdir_judge("/a/b", Some("disk full".into()), false) {
            Outcome::Failed(m) => {
                assert!(m.contains("/a/b") && m.contains("disk full"), "{m}");
            }
            _ => panic!("expected Failed"),
        }
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::{Kind, Status, TaskOwner, dest_dir, refresh_due, refresh_targets, same_dest_dir};

    #[test]
    fn refresh_due_interval_not_met_skips() {
        assert!(!refresh_due(
            10.0,
            10.0 + crate::transfer::REFRESH_THROTTLE_SECS - 0.1,
            3,
            true
        ));
    }

    #[test]
    fn refresh_due_interval_boundary_three_secs() {
        assert!(refresh_due(
            10.0,
            10.0 - crate::transfer::REFRESH_THROTTLE_SECS,
            1,
            true
        ));
    }

    #[test]
    fn refresh_due_counts_frozen_skips() {
        assert!(!refresh_due(10.0, 7.0, 0, true));
    }

    #[test]
    fn refresh_due_tab_left_skips() {
        assert!(!refresh_due(10.0, 7.0, 2, false));
    }

    #[test]
    fn refresh_due_tab_back_refreshes() {
        assert!(refresh_due(13.0, 7.0, 1, true));
    }

    #[test]
    fn same_dest_dir_remote_case_sensitive() {
        assert!(!same_dest_dir("/srv/Data", "/srv/data", true));
        assert!(same_dest_dir("/srv/data", "/srv/data", true));
    }

    #[test]
    fn same_dest_dir_local_loose() {
        assert!(same_dest_dir("D:\\Data", "d:\\data", false));
    }

    #[test]
    fn dest_dir_remote_parent_and_root() {
        assert_eq!(dest_dir("/r/name", true).as_deref(), Some("/r"));
        assert_eq!(
            dest_dir("/r/name/sub/f", true).as_deref(),
            Some("/r/name/sub")
        );
        assert_eq!(dest_dir("/f", true).as_deref(), Some("/"));
    }

    #[test]
    fn dest_dir_local_parent() {
        assert_eq!(
            dest_dir("D:\\target\\f", false).as_deref(),
            Some("D:\\target")
        );
        assert_eq!(
            dest_dir("D:\\target\\name", false).as_deref(),
            Some("D:\\target")
        );
    }

    #[test]
    fn refresh_targets_done_failed_counts_and_dst_only_dirs() {
        let o = TaskOwner {
            src: (0, 1),
            dst: (1, 2),
        };
        let rows = [
            (&o, true, "/r/f1", Status::Done),
            (&o, true, "/r/f2", Status::Failed),
            (&o, true, "/r/f3", Status::Transferring),
        ];
        let out = refresh_targets(rows.iter().map(|&(a, b, c, d)| (a, b, c, d)));
        assert_eq!(out.len(), 2);

        assert_eq!(out[0], ((0, 1), 2, Vec::<String>::new()));

        assert_eq!(out[1], ((1, 2), 2, vec!["/r".to_owned()]));
    }

    #[test]
    fn refresh_targets_dir_batch_dest_parent() {
        let o = TaskOwner {
            src: (0, 1),
            dst: (1, 2),
        };
        let rows = [(&o, true, "/r/name", Status::Verifying)];
        let out = refresh_targets(rows.iter().map(|&(a, b, c, d)| (a, b, c, d)));
        assert_eq!(out[1], ((1, 2), 0, vec!["/r".to_owned()]));
    }

    #[test]
    fn refresh_targets_download_dest_local() {
        let o = TaskOwner {
            src: (1, 2),
            dst: (0, 1),
        };
        let rows = [(&o, false, "D:\\dl\\a.bin", Status::Transferring)];
        let out = refresh_targets(rows.iter().map(|&(a, b, c, d)| (a, b, c, d)));

        assert_eq!(out[0], ((0, 1), 0, vec!["D:\\dl".to_owned()]));
        assert_eq!(out[1], ((1, 2), 0, Vec::<String>::new()));
    }

    #[test]
    fn kind_dest_is_remote_split() {
        use super::PathBuf;
        assert!(
            Kind::UploadFile {
                local: PathBuf::from("a"),
                remote: "/r/f".into()
            }
            .dest_is_remote()
        );
        assert!(
            Kind::MakeDir {
                remote: Some("/r".into()),
                local: None
            }
            .dest_is_remote()
        );
        assert!(
            !Kind::DownloadFile {
                remote: "/r/f".into(),
                local: PathBuf::from("a")
            }
            .dest_is_remote()
        );
        assert!(
            !Kind::MakeDir {
                remote: None,
                local: Some(PathBuf::from("a"))
            }
            .dest_is_remote()
        );
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),

            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,

            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        })
    }

    fn enqueue_upload(shared: &Arc<Shared>) -> u64 {
        let q = TransferQueue {
            shared: Arc::clone(shared),
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(
            None,
            probe,
            vec![NewTask {
                source: "C:/a.bin".to_owned(),
                dest: "/srv/a.bin".to_owned(),
                size: 100,
                force: false,
                probe_incomplete: false,
                owner: TaskOwner::ZERO,
                kind: Kind::UploadFile {
                    local: "C:/a.bin".into(),
                    remote: "/srv/a.bin".to_owned(),
                },
            }],
            0,
        );
        shared.st.lock().unwrap().items[0].id
    }

    #[test]
    fn verifying_done_receipt_settles_done_not_cancelled() {
        let shared = shared();
        let id = enqueue_upload(&shared);
        {
            let mut st = shared.st.lock().unwrap();
            let i = &mut st.items[0];
            i.status = Status::Verifying;
            i.transferred = i.size;
            i.verify_result = VerifyKind::Match;
        }
        settle(&shared, &mut None, &mut None, id, true, Outcome::Done);
        let st = shared.st.lock().unwrap();
        let i = &st.items[0];
        assert_eq!(i.status, Status::Done);
        assert_eq!(i.transferred, i.size);
        assert!(!i.time.is_empty());
    }

    #[test]
    fn verifying_skipped_receipt_settles_done_keeps_verify_result() {
        let shared = shared();
        let id = enqueue_upload(&shared);
        {
            let mut st = shared.st.lock().unwrap();
            let i = &mut st.items[0];
            i.status = Status::Verifying;
            i.transferred = i.size;
            i.verify_result = VerifyKind::Skipped;
        }
        settle(&shared, &mut None, &mut None, id, true, Outcome::Done);
        let st = shared.st.lock().unwrap();
        let i = &st.items[0];
        assert_eq!(i.status, Status::Done);
        assert_eq!(i.verify_result, VerifyKind::Skipped);
        assert!(!i.time.is_empty());
    }

    #[test]
    fn verifying_mismatch_receipt_fails_row_with_hash_reason() {
        let shared = shared();
        let id = enqueue_upload(&shared);
        {
            let mut st = shared.st.lock().unwrap();
            let i = &mut st.items[0];
            i.status = Status::Verifying;
            i.transferred = i.size;
            i.verify_result = VerifyKind::Mismatch;
        }
        let local = "aa1122334455667788990011223344556677889900112233445566778899aabb";
        let remote = "bb1122334455667788990011223344556677889900112233445566778899ccdd";
        let msg = format!("校验不匹配（本地 {local} / 远端 {remote}）");
        settle(
            &shared,
            &mut None,
            &mut None,
            id,
            true,
            Outcome::Failed(msg),
        );
        let st = shared.st.lock().unwrap();
        let i = &st.items[0];
        assert_eq!(i.status, Status::Failed);
        assert!(i.reason.contains(local) && i.reason.contains(remote));
        assert!(!i.time.is_empty());
        let logs = shared.log.lock().unwrap();
        let line = logs.last().unwrap();
        assert!(line.contains("[传输] 失败") && line.contains(local) && line.contains(remote));
    }

    #[test]
    fn late_cancel_edge_after_transfer_settles_cancelled() {

        let shared = shared();
        let id = enqueue_upload(&shared);
        {
            let mut st = shared.st.lock().unwrap();
            let i = &mut st.items[0];
            i.cfg.verify = true;
            i.status = Status::Transferring;
            i.cancel_req = true;
            i.transferred = i.size;
        }
        settle(&shared, &mut None, &mut None, id, true, Outcome::Done);
        let st = shared.st.lock().unwrap();
        let i = &st.items[0];
        assert_eq!(i.status, Status::Cancelled);
        assert!(!i.time.is_empty());
        assert_eq!(i.verify_result, VerifyKind::None);
    }

    #[test]
    fn cancel_batches_settles_only_targeted_batches() {
        let shared = shared();
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        let task = || NewTask {
            source: "C:/a.bin".to_owned(),
            dest: "/srv/a.bin".to_owned(),
            size: 100,
            force: false,
            probe_incomplete: false,
            owner: TaskOwner::ZERO,
            kind: Kind::UploadFile {
                local: "C:/a.bin".into(),
                remote: "/srv/a.bin".to_owned(),
            },
        };
        let b1 = q.enqueue(None, probe.clone(), vec![task()], 0);
        let b2 = q.enqueue(None, probe, vec![task()], 0);
        assert_ne!(b1, b2, "逐批入队批次号递增");
        assert_eq!(q.cancel_batches(&[b1]), 1);
        assert_eq!(q.cancel_batches(&[]), 0);
        assert_eq!(q.cancel_batches(&[b1]), 0, "已取消行重复取消不计");
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items[0].status, Status::Cancelled);
        assert_eq!(st.items[1].status, Status::Waiting, "他批不牵连");
        assert!(!st.items[0].time.is_empty());
    }

    #[test]
    fn enqueue_freezes_cfg_and_cfg_of_reads_the_row() {
        let shared = shared();
        let id = enqueue_upload(&shared);
        let snap = shared.st.lock().unwrap().items[0].cfg.clone();
        assert_eq!(cfg_of(&shared, id), snap);
        shared.st.lock().unwrap().items[0].cfg.verify = !snap.verify;
        let got = cfg_of(&shared, id);
        assert_ne!(got, snap, "行上那份才是消费源");
        assert_eq!(got.verify, !snap.verify);
    }

    #[test]
    fn late_batch_keeps_own_cfg() {
        let shared = shared();
        enqueue_upload(&shared);
        enqueue_upload(&shared);
        let (a, b) = {
            let st = shared.st.lock().unwrap();
            (st.items[0].id, st.items[1].id)
        };
        assert_ne!(a, b, "两批是两个独立的行");
        {
            let mut st = shared.st.lock().unwrap();
            let first = st.items.iter().position(|i| i.id == a).unwrap();
            st.items[first].cfg.resume = false;
            st.items[first].cfg.tar_accel = true;
        }
        let st = shared.st.lock().unwrap();
        let second = st.items.iter().find(|i| i.id == b).unwrap();
        assert!(second.cfg.resume, "后批不吃前批的改动");
        assert!(!second.cfg.tar_accel);
    }

    #[test]
    fn dir_child_inherits_cfg() {
        let task = NewTask {
            source: "/a.bin".into(),
            dest: "/b.bin".into(),
            size: 10,
            force: false,
            probe_incomplete: false,
            owner: TaskOwner::ZERO,
            kind: Kind::UploadFile {
                local: "/a.bin".into(),
                remote: "/b.bin".into(),
            },
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        let mut cfg = TransferCfg::default();
        cfg.verify = true;
        cfg.tar_threshold = 7;
        cfg.verify_algo = 2;
        let child = child_item(9, &task, 1, 3, false, false, &None, &probe, &cfg);
        assert_eq!(child.cfg, cfg);
    }

    #[test]
    fn verify_uses_batch_cfg() {
        let on = shared();
        let id_on = enqueue_upload(&on);
        {
            let mut st = on.st.lock().unwrap();
            st.items[0].cfg.verify = true;
            st.items[0].status = Status::Transferring;
        }
        settle(&on, &mut None, &mut None, id_on, true, Outcome::Done);
        assert_eq!(on.st.lock().unwrap().items[0].status, Status::Verifying);

        let off = shared();
        let id_off = enqueue_upload(&off);
        {
            let mut st = off.st.lock().unwrap();
            st.items[0].cfg.verify = false;
            st.items[0].status = Status::Transferring;
        }
        settle(&off, &mut None, &mut None, id_off, true, Outcome::Done);
        assert_eq!(off.st.lock().unwrap().items[0].status, Status::Done);
    }

    #[test]
    fn lane_count_still_live_outside_cfg() {
        let mut a = crate::settings::AppSettings::default();
        let mut b = a.clone();
        a.concurrent_transfers = 1;
        b.concurrent_transfers = 3;
        assert_eq!(TransferCfg::from(&a), TransferCfg::from(&b));
    }

    #[test]
    fn snapshot_ignores_internal_and_ui_state() {
        let a = crate::settings::AppSettings::default();
        let b = crate::settings::AppSettings {
            theme: "Dark".into(),
            language: "en-US".into(),
            editor_builtin: false,
            editor_path: "X".into(),
            connect_open_mode: 2,
            update_check: false,
            dnd_stream_batch: 7,
            dirty: true,
            last_change: Some(std::time::Instant::now()),
            last_attempt: Some(std::time::Instant::now()),
            ..a.clone()
        };
        assert_eq!(TransferCfg::from(&a), TransferCfg::from(&b));
    }

    #[test]
    fn resume_threshold_uses_batch_cfg() {
        let mut cfg = TransferCfg::default();
        cfg.resume_threshold_kb = 200;
        assert_eq!(resume_threshold(&cfg), 200 * 1024);
        cfg.resume_threshold_kb = 1024;
        assert_eq!(resume_threshold(&cfg), 1024 * 1024);
    }

    #[test]
    fn snapshot_maps_all_eight_fields() {
        let s = crate::settings::AppSettings {
            resume: false,
            resume_threshold_kb: 321,
            verify: true,
            verify_algo: 3,
            preserve_mtime: false,
            tar_accel: true,
            tar_threshold: 99,
            connect_timeout: 42,
            ..crate::settings::AppSettings::default()
        };
        assert_eq!(
            TransferCfg::from(&s),
            TransferCfg {
                resume: false,
                resume_threshold_kb: 321,
                verify: true,
                verify_algo: 3,
                preserve_mtime: false,
                tar_accel: true,
                tar_threshold: 99,
                connect_timeout: 42,
            }
        );
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn retry_clears_stale_state_from_previous_run() {
        let shared = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        });
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(
            None,
            probe,
            vec![NewTask {
                source: "C:/a.bin".to_owned(),
                dest: "/srv/a.bin".to_owned(),
                size: 100,
                force: false,
                probe_incomplete: false,
                owner: TaskOwner::ZERO,
                kind: Kind::UploadFile {
                    local: "C:/a.bin".into(),
                    remote: "/srv/a.bin".to_owned(),
                },
            }],
            0,
        );
        let id = shared.st.lock().unwrap().items[0].id;

        {
            let mut st = shared.st.lock().unwrap();
            let i = &mut st.items[0];
            i.status = Status::Failed;
            i.transferred = 100;
            i.resumed = true;
            i.verify_result = VerifyKind::Mismatch;
            i.time = "12:00:00".to_owned();
            i.reason = "旧错误".to_owned();
            i.speed = 999;
            i.cancel_req = true;
        }
        q.retry(id);
        let st = shared.st.lock().unwrap();
        let i = &st.items[0];
        assert_eq!(i.status, Status::Waiting);
        assert!(!i.cancel_req);
        assert_eq!(i.speed, 0);
        assert!(i.reason.is_empty());
        assert!(i.time.is_empty());
        assert_eq!(i.transferred, 0);
        assert!(!i.resumed);
        assert_eq!(i.verify_result, VerifyKind::None);
    }
}

#[cfg(test)]
mod pause_gate_tests {
    use super::*;

    fn queue() -> (Arc<Shared>, TransferQueue) {
        let shared = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        });
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        (shared, q)
    }

    fn enqueue_one(shared: &Arc<Shared>) -> u64 {
        let q = TransferQueue {
            shared: Arc::clone(shared),
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(
            None,
            probe,
            vec![NewTask {
                source: "C:/a.bin".to_owned(),
                dest: "/srv/a.bin".to_owned(),
                size: 100,
                force: false,
                probe_incomplete: false,
                owner: TaskOwner::ZERO,
                kind: Kind::UploadFile {
                    local: "C:/a.bin".into(),
                    remote: "/srv/a.bin".to_owned(),
                },
            }],
            0,
        );
        shared.st.lock().unwrap().items[0].id
    }

    #[test]
    fn paused_gate_holds_claim_and_resumes() {
        let (shared, q) = queue();
        let id = enqueue_one(&shared);
        q.pause();
        assert!(q.paused());
        assert!(matches!(claim(&shared), Gate::Held));
        assert_eq!(shared.st.lock().unwrap().items[0].status, Status::Waiting);
        q.resume();
        match claim(&shared) {
            Gate::Job(got, ..) => assert_eq!(got, id),
            _ => panic!("resume 后应可领取"),
        }
        assert_eq!(
            shared.st.lock().unwrap().items[0].status,
            Status::Transferring
        );
    }

    #[test]
    fn idle_queue_returns_idle_gate() {
        let (shared, _q) = queue();
        assert!(matches!(claim(&shared), Gate::Idle));
    }
}

#[cfg(test)]
mod batch_semantics_tests {
    use super::*;

    fn queue() -> (Arc<Shared>, TransferQueue) {
        let shared = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        });
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        (shared, q)
    }

    fn upload_task(name: &str) -> NewTask {
        NewTask {
            source: format!("C:/{name}.bin"),
            dest: format!("/srv/{name}.bin"),
            size: 100,
            force: false,
            probe_incomplete: false,
            owner: TaskOwner::ZERO,
            kind: Kind::UploadFile {
                local: format!("C:/{name}.bin").into(),
                remote: format!("/srv/{name}.bin"),
            },
        }
    }

    #[test]
    fn stop_flag_resets_on_enqueue() {
        let (shared, q) = queue();
        q.enqueue(
            None,
            Arc::new(Mutex::new((0, None))),
            vec![upload_task("a")],
            0,
        );
        shared.stopping.store(true, Ordering::Relaxed);
        assert!(shared.stopping.load(Ordering::Relaxed));
        q.enqueue(
            None,
            Arc::new(Mutex::new((0, None))),
            vec![upload_task("b")],
            0,
        );
        assert!(!shared.stopping.load(Ordering::Relaxed));
    }

    #[test]
    fn batch_start_and_done_logs_flow() {
        let (shared, q) = queue();
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(None, probe, vec![upload_task("a"), upload_task("b")], 0);

        let logs = shared.log.lock().unwrap().clone();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("[本地] 开始传输 2 项 → /srv"));

        assert!(q.drain_batch_reports().is_empty());
        {
            let mut st = shared.st.lock().unwrap();
            st.items[0].status = Status::Done;
            st.items[1].status = Status::Failed;
            st.items[1].reason = "boom".to_owned();
        }
        let reports = q.drain_batch_reports();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].contains("[本地] 传输完成：成功 1 项，跳过 0 项，失败 1 项"));

        assert!(q.drain_batch_reports().is_empty());

        let id = shared.st.lock().unwrap().items[1].id;
        q.retry(id);
        shared.st.lock().unwrap().items[1].status = Status::Done;
        let reports = q.drain_batch_reports();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].contains("传输完成：成功 2 项，跳过 0 项，失败 0 项"));
    }

    #[test]
    fn batch_report_counts_cancelled_and_skipped() {
        let (shared, q) = queue();
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(
            None,
            probe,
            vec![upload_task("a"), upload_task("b")],
            2,
        );
        {
            let mut st = shared.st.lock().unwrap();
            st.items[0].status = Status::Done;
            st.items[1].status = Status::Cancelled;
        }
        let reports = q.drain_batch_reports();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].contains("传输完成：成功 1 项，跳过 2 项，失败 1 项"));
    }

    #[test]
    fn clear_done_drops_verifying() {
        let (shared, q) = queue();
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(
            None,
            probe,
            vec![upload_task("a"), upload_task("b"), upload_task("c")],
            0,
        );
        {
            let mut st = shared.st.lock().unwrap();
            st.items[0].status = Status::Done;
            st.items[1].status = Status::Verifying;
            st.items[2].status = Status::Waiting;
        }
        q.clear_done();
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 1);
        assert_eq!(st.items[0].status, Status::Waiting);
    }

    #[test]
    fn reset_all_unpauses_and_resets_done() {
        let (shared, q) = queue();
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(None, probe, vec![upload_task("a")], 0);
        {
            let mut st = shared.st.lock().unwrap();
            st.items[0].status = Status::Done;
            st.items[0].reason = "旧原因".to_owned();
            st.items[0].transferred = 100;
            st.items[0].resumed = true;
            st.items[0].verify_result = VerifyKind::Match;
        }
        q.pause();
        assert!(q.paused());
        q.reset_all();
        assert!(!q.paused());
        let st = shared.st.lock().unwrap();
        let i = &st.items[0];
        assert_eq!(i.status, Status::Waiting);
        assert!(i.reason.is_empty());
        assert_eq!(i.transferred, 0);
        assert!(!i.resumed);
        assert_eq!(i.verify_result, VerifyKind::None);
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),

            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        })
    }

    fn shared_paired() -> (Arc<Shared>, tokio::sync::mpsc::UnboundedReceiver<ScanJob>) {
        let (scan_tx, scan_rx) = tokio::sync::mpsc::unbounded_channel::<ScanJob>();
        let s = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            scan_tx,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        });
        (s, scan_rx)
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn temp_tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ifree-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(root.join("b.txt"), b"bb").unwrap();
        std::fs::write(sub.join("c.txt"), b"ccc").unwrap();
        root
    }

    fn enqueue_dir(shared: &Arc<Shared>, root: &Path) -> (u64, PathBuf) {
        let q = TransferQueue {
            shared: Arc::clone(shared),
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        let dst = root.with_file_name(format!(
            "{}-dst",
            root.file_name().unwrap().to_string_lossy()
        ));
        q.enqueue(
            None,
            probe,
            vec![NewTask {
                source: root.to_string_lossy().into_owned(),
                dest: dst.to_string_lossy().into_owned(),
                size: 0,
                force: false,
                probe_incomplete: false,
                owner: TaskOwner::ZERO,
                kind: Kind::CopyLocalDir {
                    from: root.to_path_buf(),
                    to: dst.clone(),
                },
            }],
            0,
        );
        (shared.st.lock().unwrap().items[0].id, dst)
    }

    fn copy_dir_job(id: u64, from: &Path, to: &Path) -> ScanJob {
        ScanJob {
            id,
            kind: Kind::CopyLocalDir {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            },
            profile: None,
            owner: TaskOwner::ZERO,
        }
    }

    #[test]
    fn paused_scan_places_children_waiting_parent_removed() {
        let shared = shared();
        let root = temp_tree("paused");
        let (id, dst) = enqueue_dir(&shared, &root);
        shared.paused.store(true, Ordering::Relaxed);
        rt().block_on(scan_job(
            &shared,
            copy_dir_job(id, &root, &dst),
            &mut None,
            &mut None,
        ));
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 5);
        assert!(st.items.iter().all(|i| i.parent == id));
        for i in &st.items {
            assert_eq!(i.status, Status::Waiting);
            assert_eq!(i.transferred, 0);
        }
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cancel_scanning_parent_cancels_whole_tree() {
        let shared = shared();
        let root = temp_tree("cancel");
        let (id, _) = enqueue_dir(&shared, &root);
        {
            let mut st = shared.st.lock().unwrap();
            let mut w = Item::fixture(Status::Waiting, 10, 0, 0, false);
            w.id = st.next_id;
            st.next_id += 1;
            w.parent = id;
            let mut t = Item::fixture(Status::Transferring, 10, 5, 0, false);
            t.id = st.next_id;
            st.next_id += 1;
            t.parent = id;
            st.items.push(w);
            st.items.push(t);
        }
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        q.cancel(id);
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 3);
        for i in &st.items {
            if i.transferred == 5 {

                assert_eq!(i.status, Status::Transferring);
                assert!(i.cancel_req);
            } else {
                assert_eq!(i.status, Status::Cancelled);
                assert!(!i.time.is_empty());
            }
        }
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn claim_ignores_scanning_rows() {
        let shared = shared();
        let root = temp_tree("claim");
        let (id, _) = enqueue_dir(&shared, &root);
        assert!(matches!(claim(&shared), Gate::Idle));
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 1);
        assert_eq!(st.items[0].id, id);
        assert_eq!(st.items[0].status, Status::Scanning);
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn retry_dir_row_rescans_and_drops_old_children() {
        let (shared, mut rx) = shared_paired();
        let root = temp_tree("retry");
        let (id, dst) = enqueue_dir(&shared, &root);
        let _ = rx.try_recv();
        {
            let mut st = shared.st.lock().unwrap();
            st.items[0].status = Status::Cancelled;

            let mut c = Item::fixture(Status::Done, 10, 10, 0, false);
            c.id = st.next_id;
            st.next_id += 1;
            c.parent = id;
            st.items.push(c);
        }
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        q.retry(id);
        {
            let st = shared.st.lock().unwrap();
            assert_eq!(st.items.len(), 1);
            assert_eq!(st.items[0].status, Status::Scanning);
            assert!(!st.items[0].precheck);
        }
        let job = rx.try_recv().unwrap();
        assert_eq!(job.id, id);
        assert!(matches!(job.kind, Kind::CopyLocalDir { .. }));
        drop(dst);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scanning_counts_as_active_for_tabs_and_batch_reports() {
        let shared = shared();
        let i = Item::fixture(Status::Scanning, 0, 0, 0, false);
        let owner = i.owner;
        let status = i.status;
        assert_eq!(count_owned_active([(&owner, status)], (0, 0)), 1);
        assert_eq!(collect_owner_tabs([(&owner, status)]), vec![(0u8, 0u32)]);
        {
            let mut st = shared.st.lock().unwrap();
            st.items.push(i);
        }
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        q.clear_done();
        assert_eq!(shared.st.lock().unwrap().items.len(), 1);
        assert!(q.drain_batch_reports().is_empty());
    }

    #[test]
    fn tar_precheck_handbacks_parent_with_precheck_flag() {

        let shared = shared();
        let root = temp_tree("tarpre");
        let id = {
            let mut st = shared.st.lock().unwrap();
            let mut i = Item::fixture(Status::Scanning, 0, 0, 0, false);
            i.id = st.next_id;
            st.next_id += 1;
            i.cfg.tar_accel = true;
            i.cfg.tar_threshold = 2;
            i.kind = Kind::UploadDir {
                local: root.clone(),
                remote: "/srv/dir".into(),
            };
            let id = i.id;
            st.items.push(i);
            id
        };
        rt().block_on(scan_job(
            &shared,
            ScanJob {
                id,
                kind: Kind::UploadDir {
                    local: root.clone(),
                    remote: "/srv/dir".into(),
                },
                profile: None,
                owner: TaskOwner::ZERO,
            },
            &mut None,
            &mut None,
        ));
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 1);
        assert_eq!(st.items[0].status, Status::Waiting);
        assert!(st.items[0].precheck);
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_job_abandons_when_parent_no_longer_scanning() {
        let shared = shared();
        let root = temp_tree("abandon");
        let (id, dst) = enqueue_dir(&shared, &root);
        shared.st.lock().unwrap().items[0].status = Status::Cancelled;
        rt().block_on(scan_job(
            &shared,
            copy_dir_job(id, &root, &dst),
            &mut None,
            &mut None,
        ));
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 1);
        assert_eq!(st.items[0].status, Status::Cancelled);
        assert_eq!(st.items[0].parent, 0);
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reset_all_rescans_dir_rows_and_drops_children() {
        let (shared, mut rx) = shared_paired();
        let root = temp_tree("reset");
        let (id, _) = enqueue_dir(&shared, &root);
        let _ = rx.try_recv();
        {
            let mut st = shared.st.lock().unwrap();
            st.items[0].status = Status::Done;
            let mut c = Item::fixture(Status::Done, 10, 10, 0, false);
            c.id = st.next_id;
            st.next_id += 1;
            c.parent = id;
            st.items.push(c);
        }
        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        q.reset_all();
        {
            let st = shared.st.lock().unwrap();
            assert_eq!(st.items.len(), 1);
            assert_eq!(st.items[0].status, Status::Scanning);
        }
        let job = rx.try_recv().unwrap();
        assert_eq!(job.id, id);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_children_inherit_parent_force() {
        let shared = shared();
        let root = temp_tree("force");
        let (id, dst) = enqueue_dir(&shared, &root);
        shared.st.lock().unwrap().items[0].force = true;
        rt().block_on(scan_job(
            &shared,
            copy_dir_job(id, &root, &dst),
            &mut None,
            &mut None,
        ));
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 5);
        assert!(st.items.iter().all(|i| i.parent == id));
        for i in &st.items {
            assert!(i.force, "子行 force 必须继承父行（覆盖语义不可静默降级）");
        }
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn stop_all_then_new_batch_old_scan_job_leaves_no_rows() {
        let shared = shared();
        let root = temp_tree("a3");
        let (old_id, dst) = enqueue_dir(&shared, &root);
        TransferQueue {
            shared: Arc::clone(&shared),
        }
        .stop_all();

        let q = TransferQueue {
            shared: Arc::clone(&shared),
        };
        q.enqueue(
            None,
            Arc::new(Mutex::new((0, None))),
            vec![NewTask {
                source: root.join("a.txt").to_string_lossy().into_owned(),
                dest: root.join("a-copy.txt").to_string_lossy().into_owned(),
                size: 1,
                force: false,
                probe_incomplete: false,
                owner: TaskOwner::ZERO,
                kind: Kind::CopyLocal {
                    from: root.join("a.txt"),
                    to: root.join("a-copy.txt"),
                },
            }],
            1,
        );
        let new_id = {
            let st = shared.st.lock().unwrap();
            st.items.iter().find(|i| i.id != old_id).unwrap().id
        };

        rt().block_on(scan_job(
            &shared,
            copy_dir_job(old_id, &root, &dst),
            &mut None,
            &mut None,
        ));
        let st = shared.st.lock().unwrap();
        assert!(st.items.iter().all(|i| i.parent != old_id));
        let old = st.items.iter().find(|i| i.id == old_id).unwrap();
        assert_eq!(old.status, Status::Cancelled);
        let new = st.items.iter().find(|i| i.id == new_id).unwrap();
        assert_eq!(new.status, Status::Waiting);
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn large_dir_places_in_batches_with_exact_final_shape() {
        let shared = shared();

        let root = std::env::temp_dir().join(format!("ifree-scan-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        for k in 0..2000 {
            std::fs::write(root.join(format!("f{k:04}.txt")), b"x").unwrap();
        }
        std::fs::write(sub.join("c.txt"), b"x").unwrap();
        let (id, dst) = enqueue_dir(&shared, &root);
        rt().block_on(scan_job(
            &shared,
            copy_dir_job(id, &root, &dst),
            &mut None,
            &mut None,
        ));

        assert_eq!(shared.scan_batches_placed.load(Ordering::Relaxed), 3);
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 2003);
        assert!(st.items.iter().all(|i| i.parent == id));
        let children: Vec<&Item> = st.items.iter().collect();

        for w in children.windows(2) {
            assert!(w[0].id < w[1].id);
        }
        assert!(matches!(children[0].kind, Kind::MakeDir { .. }));
        assert!(matches!(children[1].kind, Kind::CopyLocal { .. }));
        assert!(matches!(children[2001].kind, Kind::MakeDir { .. }));
        let tail = children[2002];
        assert!(matches!(tail.kind, Kind::CopyLocal { .. }));
        assert!(tail.source.ends_with("c.txt"));
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn relay_row_size_doubled_and_resumed_percent() {
        let prof = Profile {
            host: "h".into(),
            port: 22,
            user: "u".into(),
            auth: crate::ssh::Auth::Password("pw".to_owned()),
            fingerprint: None,
        };
        let task = NewTask {
            source: "/srv/a.bin".into(),
            dest: "/dst/a.bin".into(),
            size: 500,
            force: false,
            probe_incomplete: false,
            owner: TaskOwner::ZERO,
            kind: Kind::RelayFile {
                src: prof.clone(),
                dst: prof,
                remote_src: "/srv/a.bin".into(),
                remote_dst: "/dst/a.bin".into(),
                temp: PathBuf::from("unused"),
            },
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        let mut it = child_item(
            7,
            &task,
            0,
            0,
            false,
            false,
            &None,
            &probe,
            &TransferCfg::default(),
        );

        assert_eq!(it.size, 1000);
        assert!(matches!(it.kind, Kind::RelayFile { .. }));

        it.status = Status::Transferring;
        it.resumed = true;
        it.transferred = 250;
        assert_eq!(
            crate::ui::panes::queue_status_text(&it, false),
            format!("{} 25%", t("续传中"))
        );
    }

    #[test]
    fn child_item_inherits_probe_incomplete() {
        let task = NewTask {
            source: "/srv/a.bin".into(),
            dest: "/dst/a.bin".into(),
            size: 10,
            force: false,
            probe_incomplete: true,
            owner: TaskOwner::ZERO,
            kind: Kind::UploadFile {
                local: "/srv/a.bin".into(),
                remote: "/dst/a.bin".into(),
            },
        };
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        assert!(
            child_item(
                1,
                &task,
                0,
                0,
                false,
                true,
                &None,
                &probe,
                &TransferCfg::default(),
            )
            .probe_incomplete,
            "父行透传位真 → 子行继承"
        );
        assert!(
            !child_item(
                2,
                &task,
                0,
                0,
                false,
                false,
                &None,
                &probe,
                &TransferCfg::default(),
            )
            .probe_incomplete,
            "父行透传位关 → 子行不带"
        );
    }
}
