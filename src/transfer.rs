//! 传输队列（T4 传输真实化）：独立工作线程 + 独立 SSH 传输连接。
//!
//! 对齐 C# ConcurrentTransferService / TransferQueueService 的 MVP 子集：
//! - 队列状态放 `Arc<Mutex<_>>` 共享：UI 直改（入队/取消/移除），worker 顺序执行，
//!   无命令通道；worker 每步轮询，空闲 120ms 一拍（egui 侧每帧直读，无事件依赖）。
//! - 传输一律走独立连接（新 SSH 握手，不复用主会话）——「关闭标签页后台继续」的前提；
//! - 256KB 块边界检查点：暂停/停止/单任务取消在块边界生效（C# §2 检查点定案）；
//! - 断点续传：ResumePolicy 语义（0<目标<源 且 源≥阈值 → 追加，否则全量；
//!   用户明确选「覆盖」时置 force 跳过决策直接全量）；
//! - 目录任务由 worker 展开为逐文件 + 建目录（本地走树 / 远程走 read_dir 递归），
//!   顺序执行下无并发建目录竞态（C# §3 的 Lazy<Task> 去重只在并发>1 时需要）；
//! - tar 加速：worker 领取整目录任务时接管（transfer/tar.rs，C# TarAccelService
//!   全套：服务端打包/解包/卡死检测/临时命名/原子 rename；走不通回退逐文件展开）；
//! - 远程→远程中转、传输后校验、mtime 保留：随后续任务接入（无本地
//!   SetFileTime 依赖前不做下载侧 mtime，R5 红线待请示）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::i18n::{t, tfmt};
use crate::local;
use crate::ssh::{self, SftpConn};

/// 传输核心三层（C# ITransferCore 范式）：trait/Stub 开源、决策逻辑 ifree-core 闭源。
pub mod core;
mod tar;

/// 传输块（C# 传输缓冲 256KB；检查点粒度 = 块边界）。
const CHUNK: usize = 256 * 1024;
/// 续传阈值：settings.json resumeThresholdKb（KB，200-1024，默认 1024）。
fn resume_threshold() -> u64 {
    (crate::settings::with(|s| s.resume_threshold_kb) as u64) * 1024
}
/// 实时速度采样窗（队列第四列）。
const SPEED_WINDOW: Duration = Duration::from_millis(400);

/// 远端连接档案（独立传输连接用；UI 在连接提交时留存一份）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Profile {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: crate::ssh::Auth,
    /// 存档指纹（G16 传输静默门用；None = 无存档，C# SetTrustedFingerprint 时机）。
    pub fingerprint: Option<String>,
}

/// 校验探测快照实时句柄（该侧主会话 hash 探测的共享状态；随任务固化，H2-c）。
pub type Probe = Arc<Mutex<(u64, Option<crate::ssh::HashProbe>)>>;

/// 任务归属（P3）：src/dst 各为 (侧, tab id)，UI 入队时按发起/目标两侧活动
/// tab 盖章；随任务在队列全程保留（重试/目录展开子任务继承）。仅随行数据，
/// 三层传输核心不解释——关闭确认计数 / 定向刷新 / 按归属取消消费它。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TaskOwner {
    pub src: (u8, u32),
    pub dst: (u8, u32),
}

impl TaskOwner {
    /// 零值（测试夹具 / 无上下文构造占位；主代码路径均带真实归属）。
    #[allow(dead_code)]
    pub const ZERO: Self = Self {
        src: (0, 0),
        dst: (0, 0),
    };

    /// 该任务是否归属 key（源或目标任一端命中即算）。
    pub fn owned_by(&self, key: (u8, u32)) -> bool {
        self.src == key || self.dst == key
    }
}

/// P3 纯逻辑：活动任务（Waiting/Transferring）中归属 key（src/dst 任一端命中）
/// 的条数。关闭确认门（request_close_tab）与按归属等待消费。
pub fn count_owned_active<'a>(
    rows: impl IntoIterator<Item = (&'a TaskOwner, Status)>,
    key: (u8, u32),
) -> usize {
    rows.into_iter()
        .filter(|(o, s)| o.owned_by(key) && matches!(s, Status::Waiting | Status::Transferring))
        .count()
}

/// P3 纯逻辑：活动任务（含 Verifying——结算后仍需刷新目标视图）归属 tab 集合
/// （src ∪ dst 去重升序）。忙→静沿定向刷新消费。
pub fn collect_owner_tabs<'a>(
    rows: impl IntoIterator<Item = (&'a TaskOwner, Status)>,
) -> Vec<(u8, u32)> {
    let mut v: Vec<(u8, u32)> = rows
        .into_iter()
        .filter(|(_, s)| {
            matches!(
                s,
                Status::Waiting | Status::Transferring | Status::Verifying
            )
        })
        .flat_map(|(o, _)| [o.src, o.dst])
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// UI 构造的入队任务。
pub struct NewTask {
    pub source: String,
    pub dest: String,
    pub size: u64,
    pub force: bool,
    pub kind: Kind,
    pub owner: TaskOwner,
}

/// 任务种类（MVP 子集：本地↔远程 + 本地→本地；远程→远程中转与 tar 随后续任务）。
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
    /// 建目录（目录展开产物）：远程 Some / 本地 Some。
    MakeDir {
        remote: Option<String>,
        local: Option<PathBuf>,
    },
    /// 整目录任务：worker 领取时展开为 建目录 + 逐文件。
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
    /// 服务端 cp（G13，C# S7）：整树单任务不展开，走 exec 远程 cp；
    /// 失败不回退本地中转（C# S7.2 实际语义）。无 UI 入口（本地左远程右布局
    /// 无远程→远程触发路径），plumbing 就绪。
    #[allow(dead_code)] // 同上：无触发路径，供后续接入（C# 同为内建能力位）
    ServerCopy {
        src: String,
        dst: String,
        is_dir: bool,
        /// 目标已存在且为目录时合并内容（cp src/. dst）。
        merge: bool,
    },
    /// 远程 → 远程 中转单文件（C# RelayRemoteToRemoteAsync）：两阶段——源端
    /// 下载到本地 temp → 目标端上传；src/dst 为两侧会话档案（各自独立建连）。
    RelayFile {
        src: Profile,
        dst: Profile,
        remote_src: String,
        remote_dst: String,
        /// 本地中转目录（%TEMP%/iFreeSftp/RemoteTransfer/<sha256 前 24hex>）。
        temp: PathBuf,
    },
    /// 远程 → 远程 中转整目录：worker 领取时展开为 建目录（目标端 + temp 树）
    /// + 逐文件 RelayFile。
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

    /// 中转任务（C# RelayRemoteToRemoteAsync）：进度总量 ×2（下载 + 上传两程）。
    fn is_relay(&self) -> bool {
        matches!(self, Kind::RelayFile { .. } | Kind::RelayDir { .. })
    }
}

/// 任务状态（页签归属：队列中 = 等待+传输中+校验中+已取消；失败；完成）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Transferring,
    /// 传输完成、后置校验进行中（G12；C# QStateVerifying，瞬态）。
    Verifying,
    Done,
    Failed,
    Cancelled,
}

/// 传输后校验结论（G12；C# VerifyOutcome：Match/Mismatch/Skipped）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VerifyKind {
    /// 未校验（设置关闭 / 非 Done 结算）。
    None,
    Match,
    Mismatch,
    Skipped,
}

/// 队列行（UI 直读；cancel_req 由 UI 置位，worker 检查点读取）。
#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub source: String,
    pub dest: String,
    pub size: u64,
    pub transferred: u64,
    pub status: Status,
    /// B/s（传输中实时；其余状态 0）。
    pub speed: u64,
    /// 续传任务（第四列前缀「续传中」）。
    pub resumed: bool,
    /// tar 加速阶段旗标（0 无；1 服务端打包中；2 解包中；C# IsTarPacking/IsTarUnpacking）。
    pub phase: u8,
    /// 完成/失败时刻 HH:mm:ss（Done/Failed 第四列）。
    pub time: String,
    /// 传输后校验结论（G12；第四列「已校验/校验跳过/校验不匹配」前缀）。
    pub verify_result: VerifyKind,
    pub reason: String,
    pub cancel_req: bool,
    /// 任务归属（P3）：UI 入队时盖章，随任务全程保留（子任务继承）。
    pub owner: TaskOwner,
    force: bool,
    kind: Kind,
    /// 任务自带连接档案（H2：入队时随任务固化，多路并发下批次被新入队覆盖
    /// 不串档；relay 任务为 None——档案内嵌 Kind，日志前缀按程显式传参）。
    profile: Option<Profile>,
    /// 任务自带校验探测句柄（H2-c：随任务固化，G12 校验跟任务所属批次侧走）。
    probe: Probe,
}

impl Item {
    /// 是否涉及远端侧（队列行菜单「重连」显隐依据；C# AttachReconnect 的
    /// remotes 非空同口径：本地↔本地任务无重连入口）。
    pub fn is_remote(&self) -> bool {
        self.kind.is_remote()
    }
}

struct QueueState {
    items: Vec<Item>,
    next_id: u64,
}

struct Shared {
    st: Mutex<QueueState>,
    paused: AtomicBool,
    stopping: AtomicBool,
    log: Mutex<Vec<String>>,
    /// 空闲工作路唤醒（Notify）：入队/结算即时驱动，替代固定轮询。
    wake: tokio::sync::Notify,
}

/// UI 侧队列句柄（惰性启动工作线程）。
pub struct TransferQueue {
    shared: Arc<Shared>,
}

impl TransferQueue {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("transfer".into())
            .spawn(move || run(worker))
            .expect("spawn transfer worker");
        Self { shared }
    }

    /// 入队（远程任务需带连接档案；重复入队以最后一份为准）。档案与探测句柄
    /// 随任务固化（H2：多路并发下后批入队只影响后批任务，在跑任务不串档；
    /// probe 为该批所属侧会话的探测快照实时句柄，G12 校验跟批次侧走）。
    pub fn enqueue(&self, profile: Option<Profile>, probe: Probe, tasks: Vec<NewTask>) {
        let mut st = self.shared.st.lock().unwrap();
        for t in tasks {
            let id = st.next_id;
            st.next_id += 1;
            st.items.push(Item {
                id,
                source: t.source,
                dest: t.dest,
                // 中转任务进度总量 ×2（下载 + 上传两程）。
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
                force: t.force,
                kind: t.kind,
                profile: profile.clone(),
                probe: probe.clone(),
            });
        }
        // 空闲工作路即时唤醒（Notify 驱动，替代固定轮询）。
        self.shared.wake.notify_one();
    }

    /// 单任务取消：等待中直接结算「已取消」，传输中置请求（块边界生效）。
    pub fn cancel(&self, id: u64) {
        let mut st = self.shared.st.lock().unwrap();
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

    /// 取消全部活动任务的全局版已由按归属取消取代（P3：G17 per-tab
    /// LogScope 语义，见 cancel_owned；单任务取消见 cancel）。
    /// 按归属取消（P3，G17 per-tab「取消队列并关闭」）：只取消 src/dst 任一端
    /// 命中 key 的活动任务（C# per-tab LogScope 语义）。返回是否有取消动作。
    pub fn cancel_owned(&self, key: (u8, u32)) -> bool {
        let mut st = self.shared.st.lock().unwrap();
        let mut any = false;
        for i in st.items.iter_mut().filter(|i| i.owner.owned_by(key)) {
            match i.status {
                Status::Waiting => {
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

    /// 重试：失败/已取消 → 回等待（半成品由续传决策接管）。
    pub fn retry(&self, id: u64) {
        let mut st = self.shared.st.lock().unwrap();
        if let Some(i) = st.items.iter_mut().find(|i| i.id == id)
            && matches!(i.status, Status::Failed | Status::Cancelled)
        {
            i.status = Status::Waiting;
            i.cancel_req = false;
            i.reason.clear();
            i.speed = 0;
        }
    }

    /// 移除：任意状态可摘除（僵死行唯一逃生口）；传输中行删除后
    /// worker 检查点按「任务不存在 = 已取消」退出。
    pub fn remove(&self, id: u64) {
        self.shared.st.lock().unwrap().items.retain(|i| i.id != id);
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
    }

    pub fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// 停止全部：解除暂停 + 等待中直接取消，传输中在块边界取消（半成品留断点）。
    pub fn stop_all(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        self.shared.stopping.store(true, Ordering::Relaxed);
        let mut st = self.shared.st.lock().unwrap();
        for i in &mut st.items {
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

    /// 清除已完结（完成/失败/已取消；校验中为活动态保留，C# Get() 队列中含 Verifying）。
    pub fn clear_done(&self) {
        self.shared.st.lock().unwrap().items.retain(|i| {
            matches!(
                i.status,
                Status::Waiting | Status::Transferring | Status::Verifying
            )
        });
    }

    /// 清除所有（含排队/传输中；传输中行删除后检查点按已取消退出）。
    pub fn clear_all(&self) {
        self.shared.st.lock().unwrap().items.clear();
    }

    /// UI 只读遍历。
    pub fn with_items<R>(&self, f: impl FnOnce(&[Item]) -> R) -> R {
        f(&self.shared.st.lock().unwrap().items)
    }

    /// 取走 worker 日志（终端面板）。
    pub fn drain_log(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.log.lock().unwrap())
    }
}

// ── 工作线程 ───────────────────────────────────────────────

/// 并发路数上限（settings.concurrent_transfers 钳 1-3，改钳位需同步）。
const MAX_LANES: usize = 3;

fn run(shared: Arc<Shared>) {
    // 并发 lane（对齐 C# ConcurrentTransferService）：每路独立传输连接真并发。
    // 固定 spawn MAX_LANES 条，第 i 路仅当 concurrent_transfers > i 时领任务；
    // 设置运行时可调：调小不打断在跑任务（该任务 await 结束后下一轮停用并
    // 断开连接），调大下一轮即生效。
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
    // 每路自己的连接与归属档案（ensure_conn 按归属比对决定复用/重连）。
    let mut conn: Option<SftpConn> = None;
    let mut conn_of: Option<Profile> = None;
    loop {
        let enabled = lane < crate::settings::with(|s| s.concurrent_transfers.max(1) as usize);
        if !enabled {
            // 停用的路释放连接（省服务器会话）。
            conn = None;
            conn_of = None;
        } else {
            step(&shared, &mut conn, &mut conn_of).await;
        }
        // 唤醒驱动（Notify）替代固定 120ms 轮询：入队/结算即时唤醒空转路；
        // 100ms 兜底覆盖无唤醒源的状态变化（设置改路数、暂停开关、等待行被
        // UI 直接结算等）。忽略结果：唤醒或超时都进入下一循环重新评估。
        // （tokio 未开 macros feature，用 timeout 包装 notified 而非 select!）
        let _ = tokio::time::timeout(Duration::from_millis(100), shared.wake.notified()).await;
    }
}

/// 任务结局。
enum Outcome {
    Done,
    Failed(String),
    /// 暂停命中：回等待，断点保留（transferred 不清）。
    Paused,
    /// 取消/停止/任务被移除：按已取消结算，半成品留断点。
    Cancelled,
}

async fn step(shared: &Shared, conn: &mut Option<SftpConn>, conn_of: &mut Option<Profile>) {
    // 领取（原子：锁内即标记传输中，多路并发下防两路抢到同一任务）；顺带探测
    // 队列是否已空闲。档案与探测句柄随任务自带（H2：多路并发下批次可能被新
    // 入队覆盖，在跑任务持领取时固化的那份，连接归属比对见 ensure_conn）。
    // 目录任务同样先占位——展开期间不可被其他路领取。
    let (idle, job) = {
        let mut st = shared.st.lock().unwrap();
        let idle = !st
            .items
            .iter()
            .any(|i| matches!(i.status, Status::Waiting | Status::Transferring));
        let job = if idle {
            None
        } else {
            st.items
                .iter_mut()
                .find(|i| i.status == Status::Waiting && !i.cancel_req)
                .map(|i| {
                    i.status = Status::Transferring;
                    (
                        i.id,
                        i.owner,
                        i.kind.clone(),
                        i.profile.clone(),
                        i.probe.clone(),
                    )
                })
        };
        (idle, job)
    };
    if idle {
        // 队列空闲：复位停止标志（C# 批次开始重置的等价物）、断开传输连接。
        shared.stopping.store(false, Ordering::Relaxed);
        *conn = None;
        *conn_of = None;
        return;
    }
    let Some((id, owner, kind, profile, probe)) = job else {
        return; // 只剩带取消请求的等待行（等待 UI 结算）
    };

    // 目录任务：tar 加速先行（设置开 + 文件数达标 + tar 可用才接管，C# 判定在
    // UI 入队时、此处等价后移到 worker 领取时）；未接管才展开为建目录 + 逐文件。
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

/// 结算 + 日志（step 与 tar 加速共用；任何结算都复位 tar 阶段旗标）。
fn settle(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    is_remote: bool,
    outcome: Outcome,
) {
    let (source, dest, msg, prof) = {
        let mut st = shared.st.lock().unwrap();
        match st.items.iter_mut().find(|i| i.id == id) {
            Some(i) => {
                match &outcome {
                    Outcome::Done => {
                        i.status = Status::Done;
                        i.transferred = i.size;
                        i.time = local::now_hms();
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
    if let Outcome::Failed(m) = &outcome {
        log(
            shared,
            prof.as_ref(),
            tfmt("[传输] 失败：{0} → {1}：{2}", &[&source, &dest, &m]),
        );
        // 链路类失败：丢弃连接（含归属档案），下个任务重握手。
        if is_remote {
            *conn = None;
            *conn_of = None;
        }
    }
    let _ = msg;
    // 结算即唤醒空闲路领下一任务（Notify 驱动）。
    shared.wake.notify_one();
}

async fn run_kind(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
    id: u64,
    kind: Kind,
    profile: &Option<Profile>,
) -> Outcome {
    match kind {
        Kind::MakeDir { remote, local } => {
            if let Some(r) = remote {
                if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                    return Outcome::Failed(e);
                }
                let sftp = &conn.as_ref().unwrap().sftp;
                // 建目录失败吞异常：已存在无害；权限问题由文件操作显式报错（C# 同语义）。
                let _ = sftp.create_dir(&r).await;
            }
            if let Some(l) = local {
                let _ = std::fs::create_dir_all(&l);
            }
            Outcome::Done
        }
        Kind::CopyLocal { from, to } => copy_local(shared, id, &from, &to).await,
        Kind::UploadFile { local, remote } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                return Outcome::Failed(e);
            }
            // G12：本地→远程方向挂传输后校验（日志路径对齐 C# task.Source=本地源）。
            let src = local.to_string_lossy().into_owned();
            let outcome = upload_file(shared, conn, id, &local, &remote, 0, profile.as_ref()).await;
            verify_after(
                shared,
                conn,
                id,
                &local,
                &remote,
                &src,
                outcome,
                profile.as_ref(),
            )
            .await
        }
        Kind::DownloadFile { remote, local } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                return Outcome::Failed(e);
            }
            // G12：远程→本地方向挂传输后校验（日志路径对齐 C# task.Source=远端源）。
            let outcome =
                download_file(shared, conn, id, &remote, &local, 0, profile.as_ref()).await;
            verify_after(
                shared,
                conn,
                id,
                &local,
                &remote,
                &remote,
                outcome,
                profile.as_ref(),
            )
            .await
        }
        Kind::ServerCopy {
            src,
            dst,
            is_dir,
            merge,
        } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                return Outcome::Failed(e);
            }
            // 传输核心三层（C# ITransferCore）：Stub（社区构建）显式提示「仅官方构建」，
            // 远程→远程保底路径 = 本地中转（RelayFile/RelayDir，用户改用两步拖拽即得）；
            // 官方构建走闭源决策逻辑（C# FullTransferCore）：命令组装 + CP_OK 回执判定
            // （防 Windows 服务器 shell 下 cp 非法但 exit=0 误判成功——比旧实现增强）+
            // 600s 超时（C# ServerCopyTimeoutSeconds）。退出码非 0 / 无回执即失败，
            // 不回退本地中转（C# S7.2 实际语义）。
            let core = core::make_transfer_core();
            if !core.is_full() {
                return Outcome::Failed(t("服务端直达复制仅官方构建可用；社区构建请分两步传输（先下载到本机，再上传到目标）").into());
            }
            let cmd = match core.server_copy_cmd(&src, &dst, is_dir, merge) {
                Ok(c) => c,
                Err(_) => {
                    return Outcome::Failed(t("服务端直达复制仅官方构建可用；社区构建请分两步传输（先下载到本机，再上传到目标）").into());
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

/// 按档案建独立传输连接（G16 静默指纹门，C# S5.2 同语义：仅存档精确匹配才
/// 信任，否则 fail-closed 静默拒绝、无弹窗）。
async fn connect_profile(p: &Profile) -> Result<SftpConn, String> {
    match tokio::time::timeout(
        Duration::from_secs(crate::settings::with(|s| s.connect_timeout).max(1) as u64),
        ssh::connect_sftp_silent(&p.host, p.port, &p.user, &p.auth, p.fingerprint.clone()),
    )
    .await
    {
        Ok(Ok(c)) => Ok(c),
        Ok(Err(e)) => Err(tfmt("传输连接失败：{0}", &[&String::from(e)])),
        Err(_) => Err(t("传输连接超时").into()),
    }
}

/// 惰性建立独立传输连接（整批复用，队列空闲时在 step 里断开）；连接归属
/// 档案与当前任务不符（多路并发下批次可能切主机/账号）时断开重连，
/// 防止旧连接把文件传到错误目标。
async fn ensure_conn(
    profile: &Option<Profile>,
    conn: &mut Option<SftpConn>,
    conn_of: &mut Option<Profile>,
) -> Result<(), String> {
    let Some(p) = profile else {
        return Err(t("无远程连接档案").into());
    };
    if conn.is_some() && conn_of.as_ref().is_some_and(|o| o == p) {
        return Ok(());
    }
    match connect_profile(p).await {
        Ok(c) => {
            *conn = Some(c);
            *conn_of = Some(p.clone());
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// 检查点（C# ThrowIfCheckpoint 顺序：暂停 → 停止 → 单任务取消/任务不存在）。
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

/// 块级进度回写。
fn progress(shared: &Shared, id: u64, transferred: u64, speed: u64) {
    let mut st = shared.st.lock().unwrap();
    if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
        i.transferred = transferred;
        i.speed = speed;
    }
}

fn log(shared: &Shared, prof: Option<&Profile>, line: String) {
    // 来源前缀（C# scope 标签语义）：远程任务带 "[user@host]"，本地任务 "[本地]"。
    // H2-b：取任务自带档案（入队时固化）——批次被新入队覆盖后，在跑任务的日志
    // 仍标自己的主机；relay 两程在调用点按程显式传参。
    let pfx = match prof {
        Some(p) => format!("[{}@{}]", p.user, p.host),
        None => format!("[{}]", t("本地")),
    };
    shared.log.lock().unwrap().push(format!("{pfx} {line}"));
}

/// 本地 → 远程 单文件上传（自写读写循环，检查点在自有调用栈内抛——C# 崩溃定案）。
async fn upload_file(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local_path: &Path,
    remote: &str,
    base: u64,
    prof: Option<&Profile>,
) -> Outcome {
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
    // 续传决策（C# ResumePolicy.Decide，语义钉在 resume_tests）：
    // Overwrite=全量不打续传日志；Conflict=目标大于源记日志后全量；
    // Append=打续传日志后 seek。force（用户选覆盖）直接全量（C# FileView
    // forceOverwrite 分支不调 Decide 同语义，冲突日志随之不打）。
    let force = {
        let st = shared.st.lock().unwrap();
        st.items.iter().find(|i| i.id == id).map(|i| i.force) == Some(true)
    };
    let decision = resume_decision(
        src,
        dest_len,
        dest_exists,
        crate::settings::with(|s| s.resume),
        force,
        resume_threshold(),
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
                // 定位不支持 → 回退全量（先截断）。
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
            // 追加打开失败 → 回退全量。
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
    drop(rf); // 发送 close（半成品闭合为完整文件）
    Outcome::Done
}

/// 远程 → 本地 单文件下载。
async fn download_file(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    remote: &str,
    local_path: &Path,
    base: u64,
    prof: Option<&Profile>,
) -> Outcome {
    let sftp = &conn.as_ref().unwrap().sftp;
    let src = match sftp.metadata(remote).await {
        Ok(m) if !m.is_dir() => m.len(),
        Ok(_) => return Outcome::Failed(t("源不是文件").into()),
        Err(e) => return Outcome::Failed(tfmt("源不可读：{0}", &[&e])),
    };

    let (dest_exists, dest_len) = match std::fs::metadata(local_path) {
        Ok(m) => (true, m.len()),
        Err(_) => (false, 0),
    };
    let force = {
        let st = shared.st.lock().unwrap();
        st.items.iter().find(|i| i.id == id).map(|i| i.force) == Some(true)
    };
    let decision = resume_decision(
        src,
        dest_len,
        dest_exists,
        crate::settings::with(|s| s.resume),
        force,
        resume_threshold(),
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
            // 定位失败回退全量。
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
    Outcome::Done
}

/// 远程 → 远程 中转单文件两阶段（C# RelayRemoteToRemoteAsync）：
/// 阶段 1 源端下载到本地 temp（专用连接，即用即断）→ 阶段 2 目标端上传
/// （目标侧专用连接——两程各建独立连接，档案内嵌 Kind 随任务自带，多路
/// 并发下批次覆盖不串档）；进度基数接续（总量 ×2）。
async fn relay_file(
    shared: &Shared,
    id: u64,
    src_prof: &Profile,
    dst_prof: &Profile,
    remote_src: &str,
    remote_dst: &str,
    temp_dir: &Path,
) -> Outcome {
    let name = remote_src.rsplit('/').next().unwrap_or(remote_src);
    // .part 归属标记：阶段 1 落 `<name>.part`（未完整中转件），完成后转正为
    // `<name>`（阶段 2 上传源 + 续传基）。temp 目录任意时刻只含这两种形态，
    // 残留可归属（.part = 阶段 1 未完成；<name> = 阶段 2 未完成，重试即续）。
    let tmp_file = temp_dir.join(name);
    let part_file = temp_dir.join(format!("{name}.part"));
    // 中转目录兜底（单文件任务无展开建目录环节）。
    if let Err(e) = std::fs::create_dir_all(temp_dir) {
        return Outcome::Failed(tfmt("创建中转目录失败：{0}", &[&e]));
    }
    log(
        shared,
        Some(src_prof),
        tfmt("[中转] 阶段 1/2 下载：{0} → 本地中转", &[&remote_src]),
    );
    let mut src_conn = match connect_profile(src_prof).await {
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
    drop(src_conn); // 阶段 1 连接即用即断
    if !matches!(dl, Outcome::Done) {
        return dl;
    }
    if let Some(o) = checkpoint(shared, id) {
        return o; // 两阶段间检查点（C# ThrowIfCheckpoint 同顺序）
    }
    // 阶段 1 完成：.part 转正为完整中转件（Windows rename 目标存在即失败，
    // 重做场景先清旧件）。
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
    let mut dst_conn = match connect_profile(dst_prof).await {
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
    drop(dst_conn); // 阶段 2 连接即用即断
    // 完成即清中转件（失败/取消保留，供重试接续）。
    if matches!(ul, Outcome::Done) {
        let _ = std::fs::remove_file(&tmp_file);
    }
    ul
}

/// 本地 → 本地 单文件复制（分块走同一检查点，进度口径一致）。
async fn copy_local(shared: &Shared, id: u64, from: &Path, to: &Path) -> Outcome {
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
    Outcome::Done
}

// ── 目录展开（worker 领取时做，UI 不走树）──────────────────

/// 展开：整目录任务 → 建目录 + 逐文件，插回原位置；展开失败按任务失败结算。
/// 子任务继承父任务归属（P3：随行数据，关闭确认/定向刷新按父 tab 计）。
#[allow(clippy::too_many_arguments)] // worker 领取上下文直传，拆结构体反增噪音
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
    let tasks: Result<Vec<NewTask>, String> = match &kind {
        Kind::UploadDir { local, remote } => {
            if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                Err(e)
            } else {
                // 目标根目录本身也要建（同名目录覆盖合并语义由逐文件路径保证）。
                let mut v = vec![NewTask {
                    source: local.to_string_lossy().into_owned(),
                    dest: remote.clone(),
                    size: 0,
                    force: false,
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
            if let Err(e) = ensure_conn(profile, conn, conn_of).await {
                Err(e)
            } else {
                let sftp = &conn.as_ref().unwrap().sftp;
                let mut v = vec![NewTask {
                    source: remote.clone(),
                    dest: local.to_string_lossy().into_owned(),
                    size: 0,
                    force: false,
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
            // 源端树枚举需源侧专用连接（主批次连接按目标侧档案建立，列不了源树）。
            match connect_profile(src).await {
                Ok(sc) => {
                    let sftp = &sc.sftp;
                    let mut v = vec![NewTask {
                        source: remote_src.clone(),
                        dest: remote_dst.clone(),
                        size: 0,
                        force: false,
                        owner,
                        // 目标端建源树根 + 本地 temp 根（MakeDir 两端同时建）。
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

    let mut st = shared.st.lock().unwrap();
    let Some(pos) = st.items.iter().position(|i| i.id == id) else {
        return; // 展开期间被移除
    };
    match tasks {
        Ok(list) => {
            st.items.remove(pos);
            for (k, t) in list.into_iter().enumerate() {
                let tid = st.next_id;
                st.next_id += 1;
                st.items.insert(
                    pos + k,
                    Item {
                        id: tid,
                        source: t.source,
                        dest: t.dest,
                        // 中转任务进度总量 ×2（下载 + 上传两程）。
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
                        force: t.force,
                        kind: t.kind,
                        profile: profile.clone(),
                        probe: probe.clone(),
                    },
                );
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

/// 本地树枚举：返回 (相对路径, 是否目录, 字节数)；目录条目先于其内容，
/// 同层按文件名序。显式栈迭代（禁递归防爆栈）：深度上限 128、
/// symlink_metadata 不跟链接（符号链接一律跳过）、规范化路径 visited 集合
/// 防环（目录联接/循环符号链接）；命中上限/环/IO 错误 = 显式 Err（带路径），
/// 绝不静默跳过。
fn walk_local(root: &Path) -> std::io::Result<Vec<(PathBuf, bool, u64)>> {
    const MAX_DEPTH: usize = 128;
    enum Frame {
        /// 叶子动作：产出一条 (相对路径, 是否目录, 字节数)。
        Emit(PathBuf, bool, u64),
        /// 待展开目录（绝对路径, 相对路径, 深度）。
        Dir {
            abs: PathBuf,
            rel: PathBuf,
            depth: usize,
        },
    }
    let mut out = Vec::new();
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));
    // LIFO：子项按逆序压栈 → 出栈保持字典序；Dir 帧压在同名 Emit 帧之下 →
    // 目录条目先产出、其内容先于后续兄弟展开（与递归版 DFS 前序完全一致）。
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
                    // 链接/目录联接不盲跟：目标是目录才按目录进入，环由下方
                    // canonicalize visited 集合拦截（自环即显式 Err，不会死循环）。
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

/// 远程树枚举：返回 (相对路径, 是否目录, 字节数)；目录条目先于其内容。
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

/// SFTP 绝对路径拼接，全项目唯一入口（C# RemotePath.Join 同口径：
/// `dir.TrimEnd('/') + "/" + name`）——尾斜杠/多斜杠先规整，根 "/" 与
/// "/home//" 拼 name 均不产生双斜杠；原 ui 层 join_remote_path 为同代码
/// 复制，已合并至此（re-export 保留原名）。
pub(crate) fn join_remote(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// 相对路径 → 远程相对段（Windows 分隔符转 "/"）。
fn rel_to_remote(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

// ── G12 传输后校验（C# SettleAfterRun / RunVerifyAsync / PureHelpers）──

/// 校验超时（C# VerifyTimeoutSeconds=120）：罩顶整个取哈希+比对。
const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);
/// 算法名（C# AlgorithmName；pick_algorithm 返回下标 → 显示名）。
const VERIFY_ALGO_NAMES: [&str; 3] = ["SHA256", "SHA512", "SM3"];

/// 选算法（C# PureHelpers.PickAlgorithm）：0=Auto 按 SHA256→SHA512→SM3 顺位；
/// 1/2/3 显式选择（服务端不支持 → None → 校验跳过）。返回 VERIFY_ALGO_NAMES 下标。
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

// ── 决策纯函数（C# ResumePolicy.Decide / VerifyPolicy.Decide 对应物）──────

/// 续传决策（C# ResumePolicy.Decide；语义钉在 resume_tests）。
/// 分支顺序对齐 C#，勿调换：force/续传关闭/目标缺失或空 → 全量；
/// 目标>源 → Conflict（调用方记日志后全量）；源<阈值 / 大小相等 → 全量；
/// 否则追加。
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub(crate) enum ResumeDecision {
    /// 从断点续传（seek 到 dst 偏移追加）
    Append,
    /// 全量重传（TRUNCATE）
    Overwrite,
    /// 目标大于源：内容已变的强信号。按全量处理，但必须记日志（不静默）。
    Conflict,
}

pub(crate) fn resume_decision(
    src: u64,
    dst: u64,
    dst_exists: bool,
    resume_enabled: bool,
    force: bool,
    threshold: u64,
) -> ResumeDecision {
    // 分支顺序对齐 C# ResumePolicy.Decide，勿调换。
    if force || !resume_enabled || !dst_exists || dst == 0 {
        // 目标不存在 / 空目标 / 续传关闭 / 用户显式覆盖：全量。
        // 目标 0 字节时"从 0 追加"等于全量，且避免队列显示误导性"续传中"。
        return ResumeDecision::Overwrite;
    }
    if dst > src {
        return ResumeDecision::Conflict;
    }
    if src < threshold {
        // 小文件阈值：重传代价可忽略，阈值宜取大不取小（200KB–1MB）。
        return ResumeDecision::Overwrite;
    }
    if dst == src {
        // 大小相等：无法区分"恰好传完"与"同尺寸的另一份内容"，重传是唯一不丢数据的答案。
        ResumeDecision::Overwrite
    } else {
        ResumeDecision::Append
    }
}

/// 比对决策（C# VerifyPolicy.Decide；语义钉在 verify_tests）。
/// 语义：任一侧 None / 空串 / 全空白 → Skipped；ASCII 大小写不敏感相等 →
/// Match；否则 Mismatch。C# 的 available=false 分支在 Rust 由 pick_algorithm
/// 返回 None 承担（run_verify 内），不进本函数。
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

/// 传输后校验入口：仅 Done 且设置开启才跑（C# AttachVerifyHook 方向条件 +
/// SettleAfterRun）。置 Verifying → 120s 罩顶跑比对 → 按结论结算。
/// `log_path` 对齐 C# task.Source 日志口径（上传=本地路径 / 下载=远端路径）。
#[allow(clippy::too_many_arguments)]
async fn verify_after(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local: &Path,
    remote: &str,
    log_path: &str,
    outcome: Outcome,
    prof: Option<&Profile>,
) -> Outcome {
    if !matches!(outcome, Outcome::Done) || !crate::settings::with(|s| s.verify) {
        return outcome;
    }
    // 置校验中（行被移除 / 带取消请求 → 按已取消结算，C# 行态检查）。
    {
        let mut st = shared.st.lock().unwrap();
        match st.items.iter_mut().find(|i| i.id == id) {
            Some(i) if i.status == Status::Transferring && !i.cancel_req => {
                i.status = Status::Verifying;
                i.speed = 0;
            }
            _ => return Outcome::Cancelled,
        }
    }
    let fut = run_verify(shared, conn, id, local, remote, log_path, prof);
    match tokio::time::timeout(VERIFY_TIMEOUT, fut).await {
        Ok(o) => o,
        // 超时 → 校验跳过 + 正常完成（C# RunVerifyAsync 超时分支）。
        Err(_) => {
            set_verify_result(shared, id, VerifyKind::Skipped);
            log(shared, prof, tfmt("[校验] 校验跳过：{0}", &[&log_path]));
            Outcome::Done
        }
    }
}

/// 写回校验结论（行被移除则静默，C# SetVerifyResult 同语义）。
fn set_verify_result(shared: &Shared, id: u64, r: VerifyKind) {
    let mut st = shared.st.lock().unwrap();
    if let Some(i) = st.items.iter_mut().find(|i| i.id == id) {
        i.verify_result = r;
    }
}

/// 校验执行（C# RunVerifyAsync）：选算法 → 本地哈希 → 服务端哈希 → 比对。
/// 任一侧取不到 → Skipped（正常完成）；Match → 正常完成；Mismatch → 先写
/// 结论再 Failed（失败详情交 step 统一结算，C# SetVerifyResult → Fail 顺序）。
async fn run_verify(
    shared: &Shared,
    conn: &mut Option<SftpConn>,
    id: u64,
    local: &Path,
    remote: &str,
    log_path: &str,
    prof: Option<&Profile>,
) -> Outcome {
    // G12：读任务自带侧的探测快照（H2-c：句柄随任务入队固化，连接期一直有效、
    // 重连清空重测自动跟随；连接提交时快照会错过首次探测，导致整会话校验误判
    // Skipped）。
    let probe = shared
        .st
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .and_then(|i| i.probe.lock().unwrap().1)
        .unwrap_or_default();
    // 算法不可用 → Skipped（C# PickAlgorithm 返回 null）。
    let Some(algo) = pick_algorithm(crate::settings::with(|s| s.verify_algo), probe) else {
        set_verify_result(shared, id, VerifyKind::Skipped);
        log(shared, prof, tfmt("[校验] 校验跳过：{0}", &[&log_path]));
        return Outcome::Done;
    };
    let name = VERIFY_ALGO_NAMES[algo];
    // 本地哈希走阻塞线程池（同步 IO 不卡 worker 的 timer，120s 罩顶才有效）。
    let lp = local.to_owned();
    let lh = tokio::task::spawn_blocking(move || local_hash(&lp, algo))
        .await
        .unwrap_or(None);
    let rh = server_hash(conn, remote, algo).await;
    // 任一侧取不到 → Skipped（C# Decide：算法不可用或任一侧取不到）。
    let (Some(a), Some(b)) = (lh, rh) else {
        set_verify_result(shared, id, VerifyKind::Skipped);
        log(shared, prof, tfmt("[校验] 校验跳过：{0}", &[&log_path]));
        return Outcome::Done;
    };
    match verify_decision(Some(&a), Some(&b)) {
        VerifyKind::Skipped => {
            set_verify_result(shared, id, VerifyKind::Skipped);
            log(shared, prof, tfmt("[校验] 校验跳过：{0}", &[&log_path]));
            Outcome::Done
        }
        VerifyKind::Match => {
            set_verify_result(shared, id, VerifyKind::Match);
            // C# LogVerifyOutcome：路径用 task.Source。
            log(
                shared,
                prof,
                tfmt("[校验] 校验通过：{0}（{1}）", &[&log_path, &name]),
            );
            Outcome::Done
        }
        // verify_decision 不产生 None（未运行校验）；防御性映射为跳过
        //（M-7 教训：映射不炸，不用 unreachable）。
        VerifyKind::None => {
            set_verify_result(shared, id, VerifyKind::Skipped);
            log(shared, prof, tfmt("[校验] 校验跳过：{0}", &[&log_path]));
            Outcome::Done
        }
        VerifyKind::Mismatch => {
            // Mismatch：先写结论，失败详情交 step 统一结算（reason=校验不匹配详情）。
            set_verify_result(shared, id, VerifyKind::Mismatch);
            log(
                shared,
                prof,
                tfmt(
                    "[校验] 校验不匹配：{0}（本地 {1} / 远端 {2}）",
                    &[&log_path, &a, &b],
                ),
            );
            Outcome::Failed(tfmt("校验不匹配（本地 {0} / 远端 {1}）", &[&a, &b]))
        }
    }
}

/// 本地文件哈希（ring 256KB 分块流式，C# LocalComputeAsync 同口径；
/// SM3 暂不支持 → None，IO 失败 → None —— 均归校验跳过）。
fn local_hash(path: &Path, algo: usize) -> Option<String> {
    use ring::digest as dg;
    let alg = match algo {
        0 => &dg::SHA256,
        1 => &dg::SHA512,
        _ => return None,
    };
    let mut f = std::fs::File::open(path).ok()?;
    let mut ctx = dg::Context::new(alg);
    let mut buf = vec![0u8; 256 * 1024];
    use std::io::Read;
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

/// 服务端哈希（C# ComputeServerHashAsync / ComputeServerSm3Async）：
/// sha256sum/sha512sum 取 stdout 首个空白字段（64/128 位 hex 校验）；
/// SM3 走 `openssl dgst -sm3`（无 --，C# 同格式）取最后一个 '=' 后段。
/// 非零退出 / 解析失败 → None（校验跳过）。
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

// ============================================================================
// 断点续传决策测试 —— 翻译自 C# tests/iFreeSftp.Tests/ResumePolicyTests.cs
// T1 安全核心："目标严格小于源才追加"，防缝合文件（覆盖被静默降级为追加）。
//
// 集成方式（三步）：
//   1. 把下方【重构】段的 enum + fn 加进 transfer.rs（run_kind 上方任意位置）；
//   2. 上传路径（约 L749）与下载路径（约 L883）的内联判定替换为对
//      resume_decision 的调用：Conflict → 打"目标大于源"日志 + 全量；
//      Append → 打"从断点续传"日志 + seek；Overwrite → 全量（不打续传日志）；
//   3. 把下方整个 `mod resume_tests` 追加到 transfer.rs 文件末尾。
//   运行：cargo test    预期：resume_tests 14 passed
// ============================================================================

// ---- 重构段已并入上文「决策纯函数」区（enum + resume_decision 各只有一份）----

// ---- 用例（与 C# 一一对应） ------------------------------------------------

// P3 归属纯逻辑：关闭确认计数 / 忙静沿定向刷新的集合收集。
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
            (&o_tab, Status::Verifying), // 瞬态不进关闭计数（与全局判据同口径）
            (&o_tab, Status::Done),
            (&o_other, Status::Waiting), // 非归属
        ];
        assert_eq!(count_owned_active(rows, (0, 7)), 2); // src 命中
        assert_eq!(count_owned_active(rows, (1, 9)), 2); // dst 命中
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
        }; // 自传（src=dst）去重
        let c = TaskOwner {
            src: (0, 3),
            dst: (1, 4),
        };
        let rows = [
            (&a, Status::Transferring),
            (&b, Status::Waiting),
            (&c, Status::Done), // 已完结不计
        ];
        assert_eq!(collect_owner_tabs(rows), vec![(0, 2), (1, 5)]);
        // Verifying 属活动态（结算后目标视图仍需刷新）。
        let d = TaskOwner::ZERO;
        assert_eq!(collect_owner_tabs([(&d, Status::Verifying)]), vec![(0, 0)]);
    }
}

#[cfg(test)]
mod resume_tests {
    use super::{ResumeDecision::*, resume_decision};

    const OFF: u64 = 0; // 阈值 0 = 不启用（C# 默认参数）

    #[test]
    fn remote_partial_appends() {
        // 上传: 远端半成品 (0 < remote < local) → 追加
        assert_eq!(resume_decision(1000, 400, true, true, false, OFF), Append);
    }

    #[test]
    fn local_partial_appends() {
        // 下载: 本地半成品——远端大小作源、本地大小作目标，归一到同一判定
        assert_eq!(resume_decision(5000, 4999, true, true, false, OFF), Append);
    }

    #[test]
    fn dest_missing_overwrites() {
        assert_eq!(resume_decision(1000, 0, false, true, false, OFF), Overwrite);
    }

    #[test]
    fn dest_zero_overwrites() {
        // 目标存在但 0 字节: "从 0 追加"等于全量, 队列不显示误导性"续传中"
        assert_eq!(resume_decision(1000, 0, true, true, false, OFF), Overwrite);
    }

    #[test]
    fn resume_disabled_overwrites() {
        // 续传关闭 → 全量（零行为变化）
        assert_eq!(
            resume_decision(1000, 400, true, false, false, OFF),
            Overwrite
        );
    }

    #[test]
    fn equal_size_overwrites() {
        // 大小相等 → 全量: 无法区分"恰好传完"与"同尺寸的另一份内容"
        assert_eq!(
            resume_decision(1000, 1000, true, true, false, OFF),
            Overwrite
        );
    }

    #[test]
    fn dest_larger_is_conflict() {
        // 目标比源大 → 冲突（内容已变的强信号; 调用方按全量处理并记日志, 不静默追加）
        assert_eq!(resume_decision(400, 1000, true, true, false, OFF), Conflict);
    }

    #[test]
    fn empty_source_never_appends() {
        // 空源文件: 目标存在任意大小都不追加
        assert_eq!(resume_decision(0, 0, true, true, false, OFF), Overwrite);
        assert_eq!(resume_decision(0, 5, true, true, false, OFF), Conflict);
    }

    #[test]
    fn single_byte_smaller_appends() {
        // 1 字节之差也要追加: 半成品判定不放过任何可续传量
        assert_eq!(resume_decision(101, 100, true, true, false, OFF), Append);
    }

    #[test]
    fn dest_flag_false_overwrites_even_with_plausible_size() {
        // 防御性: dst_exists=false 但 size 合理（调用方矛盾）时按不存在处理, 绝不追加
        assert_eq!(
            resume_decision(1000, 500, false, true, false, OFF),
            Overwrite
        );
    }

    #[test]
    fn force_overrides_resume() {
        // Rust 特有（C# 在调用方处理 force）：冲突解决选了"覆盖"的条目，
        // 不得被续传策略静默降级为追加——N1 覆盖红线的判定层表达。
        assert_eq!(resume_decision(1000, 400, true, true, true, OFF), Overwrite);
    }

    #[test]
    fn force_overrides_conflict_too() {
        // force + 目标大于源：C# FileView forceOverwrite 提前走全量、不调
        // Decide（FileView.xaml.cs:3813），冲突日志随之不打 —— 钉住该组合。
        assert_eq!(resume_decision(400, 1000, true, true, true, OFF), Overwrite);
    }

    #[test]
    fn threshold_small_source_overwrites() {
        // —— 续传阈值: 低于阈值的小文件只能全量重传（重传代价可忽略）——
        const T: u64 = 1000;
        assert_eq!(resume_decision(999, 500, true, true, false, T), Overwrite); // 本可追加, 强制全量
        assert_eq!(resume_decision(1000, 500, true, true, false, T), Append); // 恰等于阈值: 不"小于", 正常追加
        assert_eq!(resume_decision(2000, 500, true, true, false, T), Append); // 大于阈值: 正常追加
        assert_eq!(resume_decision(100, 500, true, true, false, T), Conflict); // 目标更大仍是冲突, 不被阈值吞掉
    }

    #[test]
    fn threshold_disabled_keeps_legacy_behavior() {
        // 默认阈值 0 = 功能关闭, 既有行为零变化
        assert_eq!(resume_decision(101, 100, true, true, false, OFF), Append);
    }
}

// ============================================================================
// 传输后校验测试（第三批）—— 翻译自 C# VerifyPolicyTests
//   Decide 9 用例 + PickAlgorithm 9 用例 + Defaults 3 断言
// 集成三步：
//   1. 在 run_verify（约 L1491）把 `if a.eq_ignore_ascii_case(&b) {` 起的
//      if/else 替换为下方【重构 diff】的三支 match；
//   2. 在 transfer.rs 中加入本 mod（连同 verify_decision 纯函数）；
//   3. cargo test。预期本 mod 全过。
// ============================================================================

// ---- 重构段已并入上文「决策纯函数」区（verify_decision 只有一份）----

/* ----【重构 diff】run_verify 比对段（L1491 起）替换为：----
    match verify_decision(Some(&a), Some(&b)) {
        VerifyKind::Skipped => {
            set_verify_result(shared, id, VerifyKind::Skipped);
            log(shared, tfmt("[校验] 校验跳过：{0}", &[&log_path]));
            Outcome::Done
        }
        VerifyKind::Match => {
            set_verify_result(shared, id, VerifyKind::Match);
            log(shared, tfmt("[校验] 校验通过：{0}（{1}）", &[&log_path, &name]));
            Outcome::Done
        }
        _ => {
            set_verify_result(shared, id, VerifyKind::Mismatch);
            log(shared, tfmt("[校验] 校验不匹配：{0}（本地 {1} / 远端 {2}）", &[&log_path, &a, &b]));
            Outcome::Failed(tfmt("校验不匹配（本地 {0} / 远端 {1}）", &[&a, &b]))
        }
    }
--------------------------------------------------------------------------- */

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

    // ── C# Decide 9 用例（available=false 项由 pick_algorithm None 承担）──
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

    // ── C# PickAlgorithm 9 用例（preferred 映射：Auto=0 SHA256=1 SHA512=2 SM3=3；
    //    返回 Some(下标) 对应 C# 返回算法名，None 对应 C# null）──
    #[test]
    fn pick_auto_fallback_order() {
        assert_eq!(pick_algorithm(0, probe(true, true, true)), Some(0)); // SHA256
        assert_eq!(pick_algorithm(0, probe(false, true, true)), Some(1)); // SHA512
        assert_eq!(pick_algorithm(0, probe(false, false, true)), Some(2)); // SM3
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
    // C# "MD5"（未知值安全跳过 → null）。Rust 的 preferred 域是 0-3（设置层映射），
    // 域外值按 Auto 处理——与 C# 不同，但不可达；此用例作为行为记录钉住，
    // 若未来设置层新增算法枚举而忘改 pick_algorithm，此测试会红。
    #[test]
    fn pick_out_of_domain_treated_as_auto_divergence() {
        assert_eq!(pick_algorithm(99, probe(true, true, true)), Some(0));
    }

    // ── C# Defaults_OffAndAuto：功能默认关 + 算法默认 Auto + 续传阈值 1MB ──
    #[test]
    fn settings_defaults_off_and_auto() {
        let s = crate::settings::AppSettings::default();
        assert!(!s.verify);
        assert_eq!(s.verify_algo, 0); // Auto
        assert_eq!(s.resume_threshold_kb, 1024);
        assert_eq!(s.tar_threshold, 50);
    }

    // ── F4 已修复（join_remote 尾斜杠规整与 C# RemotePath.Join 语义一致），
    //    本用例转为回归守护：原为 F4 分歧钉子，2026-11-06 修复后翻转 ──
    #[test]
    fn join_remote_trailing_slash_normalized() {
        assert_eq!(join_remote("/home/user/", "a.txt"), "/home/user/a.txt");
    }
}

// ============================================================================
// 回归契约用例（T2 补完）—— H2 批次档案隔离 / H1 指纹未落盘不阻断。
// ============================================================================

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

    /// H2 契约：两主机并行的两个批次——档案随任务自带（后批入队不覆盖前批，
    /// 各连各自主机），日志前缀按任务自带档案标 [user@host]，不串档。
    /// 手工构造 TransferQueue（不经 new()，避免拉起 worker 线程碰网络）。
    #[test]
    fn h2_two_batches_two_hosts_do_not_cross_contaminate() {
        let pa = prof("host-a.example", "alice");
        let pb = prof("host-b.example", "bob");
        let shared = Arc::new(Shared {
            st: Mutex::new(QueueState {
                items: Vec::new(),
                next_id: 1,
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
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
                owner: TaskOwner::ZERO,
                kind: Kind::UploadFile {
                    local: "C:/a.bin".into(),
                    remote: "/srv/a.bin".to_owned(),
                },
            }],
        );
        q.enqueue(
            Some(pb.clone()),
            probe,
            vec![NewTask {
                source: "C:/b.bin".to_owned(),
                dest: "/srv/b.bin".to_owned(),
                size: 1,
                force: false,
                owner: TaskOwner::ZERO,
                kind: Kind::UploadFile {
                    local: "C:/b.bin".into(),
                    remote: "/srv/b.bin".to_owned(),
                },
            }],
        );
        // 各连各自主机：档案随任务固化，后批入队不覆盖前批
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 2);
        assert_eq!(st.items[0].profile.as_ref().unwrap().host, "host-a.example");
        assert_eq!(st.items[0].profile.as_ref().unwrap().user, "alice");
        assert_eq!(st.items[1].profile.as_ref().unwrap().host, "host-b.example");
        assert_eq!(st.items[1].profile.as_ref().unwrap().user, "bob");
        drop(st);
        // 日志 [user@host] 按任务自带档案，不串档
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

    /// H1 契约：首连指纹尚未落盘不阻断传输——G16 静默门输入 = 任务自带档案的
    /// fingerprint（首连 Ev::Connected 确认值随任务固化），不查 hosts.json 存档；
    /// save_single_session 尚未写盘（Err NotFound「仅本会话生效」）不构成阻断。
    /// 档案全字段比对：指纹一致 ⇒ 同档案连接可复用（门照常开）；指纹变更 ⇒
    /// 新档案 ⇒ ensure_conn 重建连接，绝不沿用旧会话。
    #[test]
    fn h1_unpersisted_fingerprint_does_not_block_transfer() {
        let confirmed = prof("host-c.example", "carol");
        // 首连确认值在档案里（盘上可能还没落盘）——门只看这个值
        assert_eq!(
            confirmed.fingerprint.as_deref(),
            Some("SHA256:first-connect")
        );
        // 指纹一致 ⇒ 同档案（PartialEq 全字段）：连接复用，传输不被阻断
        assert_eq!(confirmed, prof("host-c.example", "carol"));
        // 密钥轮换 ⇒ 不同档案 ⇒ 旧连接不复用
        let rotated = Profile {
            fingerprint: Some("SHA256:rotated".to_owned()),
            ..confirmed.clone()
        };
        assert_ne!(confirmed, rotated);
    }
}
