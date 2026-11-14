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
//! - 远程→远程中转（Relay 两阶段：下载到本地中转区再上传目标端）、传输后校验
//!   （本机 + 服务端双侧计算，任一侧不可得记跳过）、mtime 保留（上传 SETSTAT、
//!   下载本地 filetime，tar 路径经头字段保真）：均已实现（曾列后续任务，M-14 勘误）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
// AtomicUsize 仅测试插桩用（Shared.scan_batches_placed，分批边界钉子）。
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
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

/// 批次配置快照（CR-4）：入队这一刻从 settings 取一次，随队列行固化；目录
/// 展开子行 / 重试行继承父行——跑到一半改设置不影响在跑批次。
/// 只装 transfer 侧实际消费的 8 项：并发数（`lane_run` 每轮即时生效的队列级
/// 调度）、UI 显示项、SSH 连接级参数、内部防抖态一律不进，各自保持原读取口径。
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
    /// 行已不在队列时的回落档：与 AppSettings 默认值同源，不产生新口径。
    fn default() -> Self {
        Self::from(&crate::settings::AppSettings::default())
    }
}

/// 续传阈值：settings.json resumeThresholdKb（KB，200-1024，默认 1024）。
/// 改从批次快照取，不再每步读全局。
fn resume_threshold(cfg: &TransferCfg) -> u64 {
    (cfg.resume_threshold_kb as u64) * 1024
}

/// 取任务行固化的批次配置（行已被移除 → 回落默认档；此时已无传输动作）。
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

/// P3 纯逻辑：活动任务（Waiting/Transferring/Scanning）中归属 key（src/dst
/// 任一端命中）的条数。关闭确认门（request_close_tab）与按归属等待消费。
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

/// P3 纯逻辑：活动任务（含 Verifying——结算后仍需刷新目标视图；含 Scanning
/// ——扫描期 tab 亦忙）归属 tab 集合（src ∪ dst 去重升序）。忙→静沿定向刷新消费。
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

// ── T16 传输中目标目录节流刷新（C# RefreshOppositeWhileTransferringAsync）──

/// 忙期间节流刷新间隔（C# PeriodicTimer 3s；忙期间每帧判定，不逐帧打请求）。
pub const REFRESH_THROTTLE_SECS: f64 = 3.0;

/// 节流刷新到期判定（三守卫全过才到期，C# :3311-3352 按帧等价）：
/// ① 间隔：距上次刷新 ≥ 3s；
/// ② 增量：完成计数（Done+Failed）相对上次刷新快照有增量（C# finished !=
///    lastSeen；大文件慢传期间计数不变即不刷，不给服务器打无谓列目录请求）；
/// ③ 同目录：该 tab 当前目录仍是归属任务的目标目录（C# sameDir，翻走不刷）。
pub fn refresh_due(now: f64, last_refresh: f64, done_delta: u64, same_dir: bool) -> bool {
    (now - last_refresh) >= REFRESH_THROTTLE_SECS && done_delta > 0 && same_dir
}

/// 同目录比对（C# :3336-3339）：远程 Ordinal 大小写敏感；本地 Windows 宽松
/// （IgnoreCases 同款近似——ASCII 大小写不敏感，CJK 不受影响）。
pub fn same_dest_dir(current: &str, dest: &str, remote: bool) -> bool {
    if remote {
        current == dest
    } else {
        current.eq_ignore_ascii_case(dest)
    }
}

/// 任务目标目录 = dest 父目录（dest 为目标全路径，文件/目录同口径；「目标
/// 目录」= 新内容落地的浏览目录）。远程串按 '/' 取父（根上传 "/f" → "/"）；
/// 本地走 Path::parent（Windows 分隔符由解析器处理）。
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

/// 忙期间按帧聚合各归属 tab 的刷新判定输入（C# baseline/lastSeen 的 per-tab
/// 等价）：归属 tab -> (完成计数 Done+Failed, 目标目录候选集)。完成计数对
/// src/dst 任一端命中者计（与 collect_owner_tabs 同口径，src==dst 只计一次）；
/// 目标目录候选只取 dst 端命中者——C# 只刷目标窗格（src 侧不出新内容）。
/// 返回按 tab 键排序的去重结果，UI 据此逐 tab 判 refresh_due。
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

/// UI 构造的入队任务。
pub struct NewTask {
    pub source: String,
    pub dest: String,
    pub size: u64,
    pub force: bool,
    /// 预检超窗透传位（F-12）：该任务未经远端存在性确认放行，续传决策禁
    /// Append（防把远端同名旧目标当断点静默拼接）；PendingItem 同名标记
    /// 经 UI 提取点透传入队（消费=透传，只写不读即失去意义）。
    pub probe_incomplete: bool,
    pub kind: Kind,
    pub owner: TaskOwner,
}

/// 任务种类（MVP 子集：本地↔远程 + 本地→本地 + 远程→远程中转；tar 由
/// worker 领取时按远端能力接管）。
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

    /// 目标侧是否远程（T16 同目录守卫分流：远程 Ordinal / 本地宽松）。
    /// 中转目标端是远端（dst 档案），下载/本地复制目标是本地。
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

/// 任务状态（页签归属：队列中 = 等待+传输中+校验中+已取消；失败；完成）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Waiting,
    Transferring,
    /// 传输完成、后置校验进行中（G12；C# QStateVerifying，瞬态）。
    Verifying,
    /// 状态语义单一：扫描期独立于等待/传输，UI 显式区分。
    Scanning,
    Done,
    Failed,
    Cancelled,
}

/// 传输后校验结论（G12；C# VerifyOutcome：Match/Mismatch/Skipped）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    /// tar 加速阶段旗标（0 无；1 服务端打包中——仅下载方向，上传打包在本地
    /// 不置 1；2 解包中——下载=本机解包 / 上传=服务端解包；C# IsTarPacking/IsTarUnpacking）。
    pub phase: u8,
    /// 完成/失败时刻 HH:mm:ss（Done/Failed 第四列）。
    pub time: String,
    /// 传输后校验结论（G12；第四列「已校验/校验跳过/校验不匹配」前缀）。
    pub verify_result: VerifyKind,
    pub reason: String,
    pub cancel_req: bool,
    /// 任务归属（P3）：UI 入队时盖章，随任务全程保留（子任务继承）。
    pub owner: TaskOwner,
    /// 所属批次号（enqueue 一次 = 一批，C# RunAsync workItems 语义）：批次开始/
    /// 结束日志与结束汇总的键（T18-2）。
    batch: u64,
    force: bool,
    /// 预检超窗透传位（F-12，自 NewTask 透传）：续传决策禁 Append（resume_decision
    /// 增参消费；tar/expand 子行经 child_item 继承）。
    probe_incomplete: bool,
    kind: Kind,
    /// 任务自带连接档案（H2：入队时随任务固化，多路并发下批次被新入队覆盖
    /// 不串档；relay 任务为 None——档案内嵌 Kind，日志前缀按程显式传参）。
    profile: Option<Profile>,
    /// 任务自带校验探测句柄（H2-c：随任务固化，G12 校验跟任务所属批次侧走）。
    probe: Probe,
    /// 父整目录任务 id（0=顶层行；目录展开子行指向父，整树取消/摘除按此走）。
    parent: u64,
    /// tar 预检旗标（T20-2：扫描线程已达 tar 门 → handback 后 maybe_run 免重复计数）。
    precheck: bool,
    /// 批次配置快照（CR-4：入队时固化，子行继承父行；传输全程只读这一份）。
    cfg: TransferCfg,
}

impl Item {
    /// 是否涉及远端侧（队列行菜单「重连」显隐依据；C# AttachReconnect 的
    /// remotes 非空同口径：本地↔本地任务无重连入口）。
    pub fn is_remote(&self) -> bool {
        self.kind.is_remote()
    }

    /// 目标侧是否远程（T16 节流刷新同目录守卫分流；kind 私有经此出）。
    pub fn dest_is_remote(&self) -> bool {
        self.kind.dest_is_remote()
    }

    /// 测试夹具（UI 层单测构造展示用行）：可公开字段（size/transferred/status/
    /// speed/resumed 等）构造后直接改；kind 占位 UploadFile、probe 空句柄。
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
    /// 批次号计数（enqueue 自增；T18-2 批次日志键）。
    batch_no: u64,
    /// 已上报结束汇总的批次（防重复上报；重试/重置摘除 → 重跑批次再结算要再记）。
    reported: HashSet<u64>,
    /// 批次 → 覆盖确认「跳过冲突项」条数（结束汇总 {1} 口径，上报时取走）。
    skips: HashMap<u64, u64>,
}

/// 传输后校验任务（#5 fire-and-forget：结算时投递校验专用线程，不占传输
/// 并发；C# task.VerifyHook 等价物）。probe 为任务批次侧会话的探测快照实时
/// 句柄（H2-c 随任务固化）。
struct VerifyJob {
    id: u64,
    local: PathBuf,
    remote: String,
    /// 日志口径（C# task.Source：上传=本地源路径 / 下载=远端源路径）。
    log_path: String,
    prof: Option<Profile>,
    probe: Probe,
    /// 校验参数随任务固化（H2-c 同款：算法 / 建连超时不吃入队后的设置改动）。
    cfg: TransferCfg,
}

/// 整目录任务扫描作业（T20-2）：仅带定位字段，force/batch/probe 落位时从父行现读。
struct ScanJob {
    id: u64,
    kind: Kind,
    profile: Option<Profile>,
    owner: TaskOwner,
}

/// 扫描线程单批落位子行数上限（大目录分批进锁，避免长持锁）。
const SCAN_BATCH: usize = 1000;

struct Shared {
    st: Mutex<QueueState>,
    paused: AtomicBool,
    stopping: AtomicBool,
    log: Mutex<Vec<String>>,
    /// 空闲工作路唤醒（Notify）：入队/结算即时驱动，替代固定轮询。
    wake: tokio::sync::Notify,
    /// 校验任务投递口（unbounded：结算投递永不阻塞传输 lane）。
    verify_tx: tokio::sync::mpsc::UnboundedSender<VerifyJob>,
    /// 目录扫描作业投递口（T20-2：入队/重试/重置投递，扫描线程串行消费）。
    scan_tx: tokio::sync::mpsc::UnboundedSender<ScanJob>,
    /// 测试插桩（分批边界钉子）：scan_job 逐批落位批次计数（每批 +1），
    /// 按实例隔离防并行用例串扰。仅测试编译可见，release 零开销。
    #[cfg(test)]
    scan_batches_placed: AtomicUsize,
}

/// UI 侧队列句柄（惰性启动工作线程）。
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
        // 校验专用线程（#5，C# SettleAfterRun fire-and-forget 等价物）：结算时
        // 投递、本线程独立消费——校验不占传输并发，队列后续传输不被阻塞。
        // 连接为独立建连（russh Handle 不可克隆共享、主会话是 UI actor 不可
        // 后台等待；C# VerifyHook 挂主会话，此处等价语义为专用连接缓存复用）。
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
        // 目录扫描线程（T20-2）：整目录任务入队即扫描（枚举源树产出子行），
        // 与传输 lane 解耦——扫描不占传输并发、暂停不拦扫描（C# 目录展开在
        // worker 领取时做，此处等价前移；expand 仅作 tar 回交后探测不过的
        // 回退链末级）。
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

    /// 入队（远程任务需带连接档案；重复入队以最后一份为准）。档案与探测句柄
    /// 随任务固化（H2：多路并发下后批入队只影响后批任务，在跑任务不串档；
    /// probe 为该批所属侧会话的探测快照实时句柄，G12 校验跟批次侧走）。
    /// `skipped` = 覆盖确认「跳过冲突项」条数（C# skipped 计数，批次结束汇总
    /// {1} 消费；无冲突直传路径恒 0）。
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
        // CR-4：批次开始这一刻取一次配置快照（H2 档案固化的同款口径）。
        // 后批入队改设置只影响后批，本批已入队的行全程读自己这一份。
        let cfg = crate::settings::with(|s| TransferCfg::from(s));
        // 停止标志批次开始即复位（C# ConcurrentTransferService.cs:536）。原复位
        // 只挂在工作路 Gate::Idle——停止后旧任务尚在检查点排水、路未到空闲就
        // 入新批，新批任务会被旧 stopping 在块边界误取消（T18-6）。
        self.shared.stopping.store(false, Ordering::Relaxed);
        let mut st = self.shared.st.lock().unwrap();
        st.batch_no += 1;
        let batch = st.batch_no;
        if skipped > 0 {
            st.skips.insert(batch, skipped);
        }
        // 批次开始日志（C# FileView.xaml.cs:3260-3265 LogTransferStart）。
        let n = tasks.len().to_string();
        let dest = dest_dir(&tasks[0].dest, tasks[0].kind.dest_is_remote())
            .unwrap_or_else(|| tasks[0].dest.clone());
        let line = tfmt("开始传输 {0} 项 → {1}", &[&n, &dest]);
        // 整目录任务（T20-2）：入队即扫描态，作业投扫描线程展开子行；文件任务
        // 照旧等待态。作业先攒下，drop(st) 后统一投递。
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
                // 中转任务进度总量 ×2（下载 + 上传两程）。
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
        // 空闲工作路即时唤醒（Notify 驱动，替代固定轮询）。
        drop(st);
        for j in jobs {
            let _ = self.shared.scan_tx.send(j);
        }
        log(&self.shared, profile.as_ref(), line);
        self.shared.wake.notify_one();
        batch
    }

    /// 按批次号取消（T20-H 流式落位取消对账）：流式入队逐批返回批次号，冲突
    /// 确认弹窗「取消」时按批次回溯清行——只结算给定批次的等待/扫描行为
    /// 「已取消」，传输中置请求（块边界生效）。返回取消条数。
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

    /// 单任务取消：等待中直接结算「已取消」，传输中置请求（块边界生效）。
    /// 扫描中整目录行（T20-2）：取消即整树结算——已落位子行一并取消、其中
    /// 传输中的置请求，扫描线程后续批次按父行非扫描态放弃。
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

    /// 取消全部活动任务的全局版已由按归属取消取代（P3：G17 per-tab
    /// LogScope 语义，见 cancel_owned；单任务取消见 cancel）。
    /// 按归属取消（P3，G17 per-tab「取消队列并关闭」）：只取消 src/dst 任一端
    /// 命中 key 的活动任务（C# per-tab LogScope 语义）。返回是否有取消动作。
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

    /// 重试：失败/已取消 → 回等待（半成品由续传决策接管）。清场对齐 C#
    /// RetryAsync :855-864 字段集——速度/原因/完成时刻/已传字节/续传旗标/
    /// 校验结论一次复位：续传决策与校验每次执行重新做（C# :863-864 注释），
    /// 旧值残留会让重传成功的行仍显示旧「续传中」前缀与「已校验/校验不匹配」
    /// 结论、等待/传输中带着上一轮的完成时刻（#6 审计项）。
    pub fn retry(&self, id: u64) {
        let mut st = self.shared.st.lock().unwrap();
        let mut requeued = None;
        let mut job = None;
        if let Some(i) = st.items.iter_mut().find(|i| i.id == id)
            && matches!(i.status, Status::Failed | Status::Cancelled)
        {
            // 整目录行（T20-2）：重试 = 重扫，旧展开子行在锁内一并摘除。
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
            // 重跑批次结束要重新上报汇总（批次号摘出已上报集合）。
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

    /// 重置并重新请求所有（C# TransferQueueService.RequeueAllAsync :890-911）：
    /// 全部行回等待（含 Done——C# 遍历 Tasks 全量复位，不筛状态），字段复位集
    /// 同 retry（:900-907），并显式解除暂停（:908 IsPaused=false）。菜单显隐规则
    /// 只在队列静止时露出该入口（batch_menu_rules），此处不再重复防御（C#
    /// :885-888 守卫同位置）。重跑批次结束要重新上报汇总（reported 摘除）。
    pub fn reset_all(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        let mut st = self.shared.st.lock().unwrap();
        // 整目录行（T20-2）：回扫描态并重投作业，旧展开子行整树摘除（重扫后
        // 重新落位）。
        let dir_ids: HashSet<u64> = st
            .items
            .iter()
            .filter(|i| i.kind.is_dir_job())
            .map(|i| i.id)
            .collect();
        st.items
            .retain(|i| i.parent == 0 || !dir_ids.contains(&i.parent));
        // 重跑批次结束要重新上报汇总：先收集批次号再摘 reported（items 与
        // reported 同属 st，循环内直接 remove 会双重可变借用）。
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
        // 空闲工作路即刻重领（C# RunAsync 立即开跑）。
        self.shared.wake.notify_waiters();
    }

    /// 移除：任意状态可摘除（僵死行唯一逃生口）；传输中行删除后
    /// worker 检查点按「任务不存在 = 已取消」退出。
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
        // 暂停门挂起的路即刻唤醒重领（否则等 100ms 轮询超时）。
        self.shared.wake.notify_waiters();
    }

    pub fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// 停止全部：解除暂停 + 等待中直接取消，传输中在块边界取消（半成品留断点）。
    pub fn stop_all(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        self.shared.stopping.store(true, Ordering::Relaxed);
        // 暂停门挂起的路即刻唤醒排水（等待行已就地取消，路空转收尾）。
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

    /// 清除已完结（完成/失败/已取消/校验中；C# ClearAll :706-715 同口径含
    /// Verifying——校验 hook 卡住时该行不会自行结算，保留会把列表锁死：
    /// 重置入口被活动计数挡住、重试对校验中行无效，只剩移除单行一条路）。
    pub fn clear_done(&self) {
        self.shared.st.lock().unwrap().items.retain(|i| {
            matches!(
                i.status,
                Status::Waiting | Status::Transferring | Status::Scanning
            )
        });
    }

    /// 清除所有（含排队/传输中；传输中行删除后检查点按已取消退出）。
    pub fn clear_all(&self) {
        self.shared.st.lock().unwrap().items.clear();
    }

    /// 批次结束汇总（C# FileView.xaml.cs:3289-3294 LogTransferDone「传输完成：
    /// 成功 {0} 项，跳过 {1} 项，失败 {2} 项」；重传路径 TransferQueueService.cs:928
    /// 同文案，跳过恒 0）。批次内全部行到终态且未上报过 → 出一条，取走即不再出
    /// （重试/重置后重跑批次会再记）。UI 帧循环轮询（worker 日志同通路），覆盖
    /// worker 结算与 UI 直改（停止/按归属取消）两条结算路径。已取消行计入失败
    /// （C# 停止路径同口径：RunAsync 收尾把未处理行记失败）。「清除所有」整体
    /// 摘走的批次随行消亡，不出空汇总。返回带来源前缀的行，UI 直接入终端。
    pub fn drain_batch_reports(&self) -> Vec<String> {
        let mut st = self.shared.st.lock().unwrap();
        // 活跃批号（任一行等待/传输中/校验中/扫描中——批次未结束不汇总）。
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
        // 候选 = 在册批号 - 活跃 - 已上报。
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

/// 领取门（#7 暂停拦领取）：同步决策函数，锁内原子完成空闲探测 + 领取置位。
/// - Idle：队列无活动行（复位停止标志、断连由调用方处理）；
/// - Held：无可领取——暂停中（C# :483-492/:559-572 暂停门取走信号量、worker
///   挂门上不领取的等价物：无检查点任务 MakeDir/ServerCopy/树展开/tar 服务端
///   打包在暂停期间拿不到执行权，不再对服务器产生写操作）或仅剩带取消请求的
///   等待行（等 UI 结算）；
/// - Job：已锁内置 Transferring（多路并发下防两路抢同一任务；目录任务同样先
///   占位——展开期间不可被其他路领取）。档案与探测句柄随任务自带（H2）。
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
        return Gate::Held; // 暂停门：不领取新任务，路挂起等 resume 唤醒
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
            // 队列空闲：复位停止标志（C# 批次开始重置的等价物）、断开传输连接。
            shared.stopping.store(false, Ordering::Relaxed);
            *conn = None;
            *conn_of = None;
            return;
        }
        Gate::Held => return, // 暂停中 / 只剩带取消请求的等待行：挂起等唤醒
        Gate::Job(id, owner, kind, profile, probe) => (id, owner, kind, profile, probe),
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
/// G12 传输后校验在此出队（#5 fire-and-forget，C# SettleAfterRun :467-506）：
/// 文件级上传/下载且设置开启 → 行转「校验中」并投递校验专用线程——传输并发
/// 许可已随 run_kind 返回释放，校验不占传输并发，队列后续传输不被阻塞；
/// 其余立即 Done。校验中清空队列：行移除后校验侧结算静默无害。
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
                // CR-4：校验开关读本批次快照（锁内现读行，避免二次加锁）。
                let do_verify = i.cfg.verify;
                match &outcome {
                    Outcome::Done => {
                        // 行态收口（#5 返工，三分支互斥）：
                        // ① Verifying 行 = 校验线程回执（Match/Skipped）→ 按完成
                        //    结算（C# Complete）——否则二次 settle 落入下方投递分支
                        //    误判「已取消」（校验通过却被打回已取消）；
                        // ② 校验范围（文件级上传/下载）且设置开启：行态检查（C#
                        //    :479-493）——行移除走下方 None 分支；带取消请求（传输
                        //    末段检查点之后落下的取消）→ 按已取消结算；否则转
                        //    「校验中」并投递 fire-and-forget 校验（C# SettleAfterRun
                        //    :491-504）——传输并发许可已随 run_kind 返回释放，校验
                        //    不占传输并发，队列后续传输不被阻塞；
                        // ③ 其余（校验范围外 / 校验关闭）立即完成。
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
        // fire-and-forget 投递（unbounded，投递永不阻塞）；通道对端仅在
        // 校验线程崩溃时消失，行为同 C# `_ = RunVerifyAsync` 丢弃。
        let _ = shared.verify_tx.send(job);
    }
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

/// 校验目标抽取（C# AttachVerifyHook :3544-3553 方向条件）：仅文件级上传/
/// 下载参与校验；远程→远程 / 本地→本地不校验（C# hook = null 立即完成）。
/// 返回 (本地路径, 远端路径, 日志路径)——日志口径 C# task.Source
/// （上传=本地源路径 / 下载=远端源路径）。
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

/// MakeDir 成败二分判定（T14 假成功纠错；照 tar.rs gate_open 纯函数范式）。
/// 建目录请求失败 ≠ 目录没建成——复查确认已是目录则视为成功（「已存在无害」，
/// 吞错唯一正当理由，对齐 C# 注释）；其余真实失败（无权限/磁盘满等）进
/// 「传输失败」页签，可单独重试（重跑时已存在走复查分支记成功）。远端
/// （meta/stat is_dir）与本地（Path::is_dir）共用本判定；错误文案复用表内
/// 既有键（path, err），不新增 i18n 字面量。
fn mkdir_judge(path: &str, create_err: Option<String>, recheck_is_dir: bool) -> Outcome {
    match create_err {
        None => Outcome::Done,
        // 已存在无害：错误不再展示，仅绑定占位（_e）。
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
                        // 请求失败后 meta/stat 复查该路径：确已是目录 → 已存在
                        // 无害；复查不到/不是目录 → 真实失败（T14：不再吞错恒
                        // 记成功）。远端失败即短路，不再继续建本地端。
                        let recheck = sftp.metadata(&r).await.map(|m| m.is_dir()).unwrap_or(false);
                        return mkdir_judge(&r, Some(e.to_string()), recheck);
                    }
                }
            }
            if let Some(l) = local {
                match std::fs::create_dir_all(&l) {
                    Ok(()) => {}
                    Err(e) => {
                        // 本地同二分：失败后 Path::is_dir 复查（已存在无害）。
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
            // 校验在 settle 出队（#5 fire-and-forget，不占传输并发）。
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
            // 传输核心三层（C# ITransferCore）：Stub（源码编译版）显式提示「不含服务端直达复制组件」，
            // 远程→远程保底路径 = 本地中转（RelayFile/RelayDir，用户改用两步拖拽即得）；
            // 正式发布构建走闭源决策逻辑（C# FullTransferCore）：命令组装 + CP_OK 回执判定
            // （防 Windows 服务器 shell 下 cp 非法但 exit=0 误判成功——比旧实现增强）+
            // 600s 超时（C# ServerCopyTimeoutSeconds）。退出码非 0 / 无回执即失败，
            // 不回退本地中转（C# S7.2 实际语义）。
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

/// 按档案建独立传输连接（G16 静默指纹门，C# S5.2 同语义：仅存档精确匹配才
/// 信任，否则 fail-closed 静默拒绝、无弹窗）。
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

/// 惰性建立独立传输连接（整批复用，队列空闲时在 step 里断开）；连接归属
/// 档案与当前任务不符（多路并发下批次可能切主机/账号）时断开重连，
/// 防止旧连接把文件传到错误目标。
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

/// 日志来源前缀（C# scope 标签语义）：远程 [user@host] / 本地 [本地]。
fn log_pfx(prof: Option<&Profile>) -> String {
    match prof {
        Some(p) => format!("[{}@{}]", p.user, p.host),
        None => format!("[{}]", t("本地")),
    }
}

fn log(shared: &Shared, prof: Option<&Profile>, line: String) {
    // H2-b：取任务自带档案（入队时固化）——批次被新入队覆盖后，在跑任务的日志
    // 仍标自己的主机；relay 两程在调用点按程显式传参。
    let pfx = log_pfx(prof);
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
    // 续传决策（C# ResumePolicy.Decide，语义钉在 resume_tests）：
    // Overwrite=全量不打续传日志；Conflict=目标大于源记日志后全量；
    // Append=打续传日志后 seek。force（用户选覆盖）直接全量（C# FileView
    // forceOverwrite 分支不调 Decide 同语义，冲突日志随之不打）。
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
    // P0-3 保留修改时间（C# preserveMtimeEnabled 默认开；上传成功后远端
    // SETSTAT mtime，SftpClient.SetAttributes 同语义）。尽力而为：失败不改
    // 变传输结论（C# 同为非致命）。
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
    // P0-3 保留修改时间（C# preserveMtimeEnabled 默认开；下载成功后本地
    // SetLastWriteTime 同语义，filetime 走未关句柄——Windows 允许）。尽力
    // 而为：失败不改变传输结论。
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
    let cfg = cfg_of(shared, id);
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
    drop(dst_conn); // 阶段 2 连接即用即断
    // 完成即清中转件（失败/取消保留，供重试接续）。
    if matches!(ul, Outcome::Done) {
        let _ = std::fs::remove_file(&tmp_file);
    }
    ul
}

/// 本地 → 本地 单文件复制（分块走同一检查点，进度口径一致）。
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
    // P0-3 保留修改时间（C# 本地→本地 = File.Copy 天然保留 mtime，FileView
    // :4189/:4441；此处手写分块是检查点/进度需要不能换 std::fs::copy，结算点
    // 补设同语义）。尽力而为：失败记日志不改传输结论。
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

// ── 目录展开（worker 领取时做，UI 不走树）──────────────────
// T20-2 拆分：child_item（子行构造共用）/ dir_tasks（源树枚举四分支）/
// expand（回退链末级落位）。正常流程的枚举已由扫描线程 scan_job 完成
// （入队即扫），仅 tar 回交后 worker 侧探测不过才走 expand 现场枚举。

/// 子行构造（expand 回退链 / 扫描线程落位共用）：继承父任务 force（P0：C#
/// forceOverwrite 传入每个子文件执行体，FileView.xaml.cs:3125/3239——覆盖
/// 绝不能被静默降级为追加）、预检超窗透传位（F-12：tar/expand 子行不吃
/// 静默 Append）、批次号与归属；中转任务进度总量 ×2。
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

/// 目录任务源树枚举（原 expand 四分支 verbatim 迁移）：返回 (force, 任务列表)。
/// force 现读父行——扫描线程场景父行状态可能中途变化，落位前以现值为准。
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
    // P0：目录任务的「覆盖」随展开继承（C# forceOverwrite 传入每个子文件执行体，
    // FileView.xaml.cs:3125/3239）。单文件路径经队列原样取 force（resume_decision
    // 处同款读法），展开路径此前硬编码 false——用户显式选覆盖的子文件重新走
    // 续传决策，匹配指纹的 .part 可被判 Append（N1 红线：覆盖绝不能被静默
    // 降级为追加）。构造处的 force:false 仅为占位，落位前统一改写。
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
                // 目标根目录本身也要建（同名目录覆盖合并语义由逐文件路径保证）。
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
            // 源端树枚举需源侧专用连接（主批次连接按目标侧档案建立，列不了源树）。
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

/// 展开：整目录任务 → 建目录 + 逐文件，插回原位置；展开失败按任务失败结算。
/// 子任务继承父任务归属（P3：随行数据，关闭确认/定向刷新按父 tab 计）。
/// T20-2 起仅作回退链末级（见模块头注释）：正常流程枚举已由扫描线程完成，
/// 走到这里的多为 tar 回交后远端探测不过的小目录（现场枚举，代价可接受）。
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
    let cfg = cfg_of(shared, id);
    let (force, tasks) = dir_tasks(shared, conn, conn_of, id, &kind, profile, owner).await;

    let mut st = shared.st.lock().unwrap();
    let Some(pos) = st.items.iter().position(|i| i.id == id) else {
        return; // 展开期间被移除
    };
    match tasks {
        Ok(list) => {
            // 覆盖语义继承的唯一收口：子任务统一改写为父任务 force（跳过续传
            // 决策直接全量，C# forceOverwrite 分支不调 Decide 同语义）。
            let list = list
                .into_iter()
                .map(|mut t| {
                    t.force = force;
                    t
                })
                .collect::<Vec<_>>();
            // 展开期间父任务被取消（单任务取消 / 全局停止）→ 子任务直接按已取消
            // 落位：展开段无块边界，取消请求只在落位点生效（C# 检查点语义）；
            // 不落请求会整棵树继续传完，用户视角即「取消/停止无效」。
            let cancelled = shared.stopping.load(Ordering::Relaxed) || st.items[pos].cancel_req;
            // 子任务继承父任务批次号（T18-2 批次结束汇总按批次聚合，树展开不拆批）。
            let batch = st.items[pos].batch;
            // F-12：透传位与 force 同款继承——父行超窗放行 → 子行不吃静默 Append。
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

// ── 目录扫描线程（T20-2）：入队即扫，枚举产出子行分批落位 ──

/// 扫描线程主循环：串行消费作业；连接槽跨任务复用（同档案免重复握手），
/// 队列排空即弃连（档案可能已过期/改向）。
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

/// 单个扫描作业（T20-2）：① 存活快查（行已移除 / 已取消 = 不再扫描态即放弃）；
/// ② tar 预检（设置开且阈值 >0：本地/远端计数达标 → handback 父行走 tar 整体
/// 传输，worker 侧 maybe_run 凭 precheck 旗标免重复计数）；③ 枚举源树分批落位
/// 子行（每批一把锁，batch/force/profile/probe 落位时从父行现读）。暂停不拦
/// 扫描（拍板 E）：子行一律落等待态，worker 暂停门自然扣住不发。
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
    // ① 存活快查：行被移除 / 已取消（不再是扫描态）即放弃。
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
    // CR-4：tar 门与建连超时都读本批次快照（与 worker 侧 maybe_run 同一份）。
    let cfg = cfg_of(shared, id);
    // ② tar 预检（与 maybe_run 同判据 gate_open；远端探测仍留给 worker 侧——
    // 此处 tar_ok 恒 true 只按计数卡门，探测不过时 worker 回退 expand 现场枚举）。
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
                // 连接失败不在此结算：交给 ③ 枚举的 ensure_conn 报同错面。
            }
            _ => {}
        }
    }
    // ③ 枚举源树（弃返回的 force——落位时逐批现读父行）。
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
    // 分批落位（SCAN_BATCH 上限防大目录长持锁）：每批进锁重找父行——父行被
    // 整树移除 = 树已不在，放弃；已取消（cancel/stop_all）→ 剩余批次不落位。
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
            // F-12：透传位随批现读父行（force 同款，落位前以现值为准）。
            let probe_incomplete = st.items[pos].probe_incomplete;
            let prof = st.items[pos].profile.clone();
            let prb = st.items[pos].probe.clone();
            // 插入点 = 同父子行尾部（rposition 兜住子行被单独移除的次序抖动）。
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
        // 测试插桩（分批边界钉子）：每批落位 +1，证明落位逐批发生非瞬间全量
        // （计数器按 Shared 实例隔离，防并行用例串扰）。
        #[cfg(test)]
        shared.scan_batches_placed.fetch_add(1, Ordering::Relaxed);
    }
    if placed > 0 {
        shared.wake.notify_one();
    }
    // 收尾（拍板 D）：扫描完成父行移除、子行承接；已转非扫描态（取消/移除
    // 竞态）的父行保留现场不动。
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

/// tar 预检达标：父行回交等待态（worker 领取后走 tar::maybe_run 整体传输，
/// precheck 旗标免重复计数）。
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
/// 目标>源 → Conflict（调用方记日志后全量）；预检超窗透传位 → 全量
/// （F-12：超窗放行项远端存在性未确认，禁 Append——插在 Conflict 之后，
/// 强信号日志不因透传位丢失）；源<阈值 / 大小相等 → 全量；否则追加。
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
    probe_incomplete: bool,
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
    if probe_incomplete {
        // F-12：预检超窗放行项（远端存在性未确认）禁 Append——0<dst<src 的
        // 旧目标可能是同名完整文件，续传=把旧内容当断点拼接（数据损坏）。
        // 全量重传是唯一不损数据的答案；不置 force——那是「用户显式覆盖」
        // 语义，超时放行是新维度，两者不可混。
        return ResumeDecision::Overwrite;
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

/// 校验专用线程主循环（#5，C# RunVerifyAsync fire-and-forget 等价物）：结算
/// 投递的任务在此逐个消费——传输 lane 已继续领下一任务，校验全程不占传输
/// 并发。行被移除（预检不过）→ 静默跳过（C# 「校验中清空队列：任务移除后
/// 结算静默无害」）；连接失败/超时 → 校验跳过但传输照常完成（C# :508-511
/// 「校验本身失败不得连累已完成的传输」）；Mismatch 经 settle 进「传输失败」
/// 页签（与原 lane 内校验同一口径）。
async fn verify_run(shared: Arc<Shared>, mut rx: tokio::sync::mpsc::UnboundedReceiver<VerifyJob>) {
    // 校验独立连接（缓存复用判据同 ensure_conn：归属档案比对）。
    let mut conn: Option<SftpConn> = None;
    let mut conn_of: Option<Profile> = None;
    while let Some(job) = rx.recv().await {
        // 行态预检：行已被移除 → 静默丢弃（C# 校验中清空队列语义）。
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
                // 120s 罩顶整个取哈希 + 比对（C# VerifyTimeoutSeconds=120）。
                let fut = run_verify(&shared, &mut conn, &job);
                match tokio::time::timeout(VERIFY_TIMEOUT, fut).await {
                    Ok(o) => o,
                    // 超时 → 校验跳过 + 正常完成（C# RunVerifyAsync 超时分支）。
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
        // 结论结算（复用 settle：Done/Failed 页签、时间戳、失败日志同口径）。
        settle(&shared, &mut conn, &mut conn_of, job.id, true, outcome);
        // 无待校验任务 → 断开连接省服务器会话（对齐 lane 空闲断连）。
        if rx.is_empty() {
            conn = None;
            conn_of = None;
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
/// 结论再 Failed（失败详情交 settle 统一结算，C# SetVerifyResult → Fail 顺序）。
async fn run_verify(shared: &Shared, conn: &mut Option<SftpConn>, job: &VerifyJob) -> Outcome {
    // G12：读任务批次侧的探测快照（H2-c：句柄随任务入队固化，连接期一直有效、
    // 重连清空重测自动跟随；连接提交时快照会错过首次探测，导致整会话校验误判
    // Skipped）。
    let probe = job.probe.lock().unwrap().1.unwrap_or_default();
    // 算法不可用 → Skipped（C# PickAlgorithm 返回 null）。
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
    // 本地哈希走阻塞线程池（同步 IO 不卡 worker 的 timer，120s 罩顶才有效）。
    let lp = job.local.to_owned();
    let lh = tokio::task::spawn_blocking(move || local_hash(&lp, algo))
        .await
        .unwrap_or(None);
    let rh = server_hash(conn, &job.remote, algo).await;
    // 任一侧取不到 → Skipped（C# Decide：算法不可用或任一侧取不到）。
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
            // C# LogVerifyOutcome：路径用 task.Source。
            log(
                shared,
                job.prof.as_ref(),
                tfmt("[校验] 校验通过：{0}（{1}）", &[&job.log_path, &name]),
            );
            Outcome::Done
        }
        // verify_decision 不产生 None（未运行校验）；防御性映射为跳过
        //（M-7 教训：映射不炸，不用 unreachable）。
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
            // Mismatch：先写结论，失败详情交 settle 统一结算（reason=校验不匹配详情）。
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

/// 本地文件哈希（ring / SM3 自带实现，256KB 分块流式，C# LocalComputeAsync
/// 同口径；IO 失败 → None 归校验跳过）。
fn local_hash(path: &Path, algo: usize) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 256 * 1024];
    use std::io::Read;
    if algo == 2 {
        // M-11：SM3 自带流式实现（src/sm3.rs，GB/T 32905-2016）。
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
        assert_eq!(
            resume_decision(1000, 400, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn local_partial_appends() {
        // 下载: 本地半成品——远端大小作源、本地大小作目标，归一到同一判定
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
        // 目标存在但 0 字节: "从 0 追加"等于全量, 队列不显示误导性"续传中"
        assert_eq!(
            resume_decision(1000, 0, true, true, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn resume_disabled_overwrites() {
        // 续传关闭 → 全量（零行为变化）
        assert_eq!(
            resume_decision(1000, 400, true, false, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn equal_size_overwrites() {
        // 大小相等 → 全量: 无法区分"恰好传完"与"同尺寸的另一份内容"
        assert_eq!(
            resume_decision(1000, 1000, true, true, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn dest_larger_is_conflict() {
        // 目标比源大 → 冲突（内容已变的强信号; 调用方按全量处理并记日志, 不静默追加）
        assert_eq!(
            resume_decision(400, 1000, true, true, false, OFF, false),
            Conflict
        );
    }

    #[test]
    fn empty_source_never_appends() {
        // 空源文件: 目标存在任意大小都不追加
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
        // 1 字节之差也要追加: 半成品判定不放过任何可续传量
        assert_eq!(
            resume_decision(101, 100, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn dest_flag_false_overwrites_even_with_plausible_size() {
        // 防御性: dst_exists=false 但 size 合理（调用方矛盾）时按不存在处理, 绝不追加
        assert_eq!(
            resume_decision(1000, 500, false, true, false, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn force_overrides_resume() {
        // Rust 特有（C# 在调用方处理 force）：冲突解决选了"覆盖"的条目，
        // 不得被续传策略静默降级为追加——N1 覆盖红线的判定层表达。
        assert_eq!(
            resume_decision(1000, 400, true, true, true, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn force_overrides_conflict_too() {
        // force + 目标大于源：C# FileView forceOverwrite 提前走全量、不调
        // Decide（FileView.xaml.cs:3813），冲突日志随之不打 —— 钉住该组合。
        assert_eq!(
            resume_decision(400, 1000, true, true, true, OFF, false),
            Overwrite
        );
    }

    #[test]
    fn threshold_small_source_overwrites() {
        // —— 续传阈值: 低于阈值的小文件只能全量重传（重传代价可忽略）——
        const T: u64 = 1000;
        assert_eq!(
            resume_decision(999, 500, true, true, false, T, false),
            Overwrite
        ); // 本可追加, 强制全量
        assert_eq!(
            resume_decision(1000, 500, true, true, false, T, false),
            Append
        ); // 恰等于阈值: 不"小于", 正常追加
        assert_eq!(
            resume_decision(2000, 500, true, true, false, T, false),
            Append
        ); // 大于阈值: 正常追加
        assert_eq!(
            resume_decision(100, 500, true, true, false, T, false),
            Conflict
        ); // 目标更大仍是冲突, 不被阈值吞掉
    }

    #[test]
    fn threshold_disabled_keeps_legacy_behavior() {
        // 默认阈值 0 = 功能关闭, 既有行为零变化
        assert_eq!(
            resume_decision(101, 100, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn probe_incomplete_blocks_append() {
        // F-12：预检超窗放行项（透传位真）——dst_exists + 0<dst<src + 源≥阈值
        // 的本可追加场景必须全量重传：远端存在性未确认，旧目标可能是同名
        // 完整文件，续传=把旧内容当断点静默拼接（数据损坏）。
        assert_eq!(
            resume_decision(1000, 400, true, true, false, OFF, true),
            Overwrite
        );
        // 透传位关 → 旧行为不变（回归）。
        assert_eq!(
            resume_decision(1000, 400, true, true, false, OFF, false),
            Append
        );
    }

    #[test]
    fn probe_incomplete_after_conflict_branch() {
        // 透传位新维度插在 Conflict 之后：目标>源的强信号日志不因透传位丢失。
        assert_eq!(
            resume_decision(400, 1000, true, true, false, OFF, true),
            Conflict
        );
    }

    #[test]
    fn probe_incomplete_not_force_semantics() {
        // 透传位 ≠ force：force（用户显式覆盖）提前吞掉 Conflict 分支不打
        // 冲突日志，透传位不吞——超时放行是新维度，两者不可混。
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

    // ── M-11：传输后校验本机侧 SM3 接通（local_hash algo=2 走 src/sm3.rs，
    //    钉 GB/T 32905-2016 附录 A 向量；此前 SM3 本机侧恒 None=校验跳过）──
    #[test]
    fn local_hash_sm3_official_vector() {
        let mut p = std::env::temp_dir();
        p.push("ifree-sm3-local-hash-case.tmp");
        // 67 字节：附录 A 样例 2 的 64 字节块 + 3 字节尾巴（跨块 + 填充边界）。
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
        // IO 失败仍归校验跳过。
        assert_eq!(local_hash(Path::new("Z:/definitely/not/exists"), 2), None);
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
                batch_no: 0,
                reported: HashSet::new(),
                skips: HashMap::new(),
            }),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
            // 校验投递口（测试夹具无校验线程，Sender 丢弃 = 投递静默失效）。
            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            // 扫描投递口（同上：无扫描线程，投递静默失效）。
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

/// T14：MakeDir 成败二分判定（mkdir_judge）钉住——远端三态 + 本地三态 +
/// 失败文案带路径与原因。不存在「目录未建成却打 ✓」的路径。
#[cfg(test)]
mod mkdir_tests {
    use super::{Outcome, mkdir_judge};

    /// 远端：create_dir 成功 → Done。
    #[test]
    fn remote_created_done() {
        assert!(matches!(mkdir_judge("/a/b", None, false), Outcome::Done));
    }

    /// 远端：create_dir 失败但复查已是目录（已存在/并发建）→ Done（已存在无害）。
    #[test]
    fn remote_failed_but_recheck_is_dir_done() {
        assert!(matches!(
            mkdir_judge("/a/b", Some("failure".into()), true),
            Outcome::Done
        ));
    }

    /// 远端：create_dir 失败且复查不到/不是目录（无权限/磁盘满）→ Failed。
    #[test]
    fn remote_failed_and_missing_failed() {
        assert!(matches!(
            mkdir_judge("/a/b", Some("permission denied".into()), false),
            Outcome::Failed(_)
        ));
    }

    /// 本地：create_dir_all 成功 → Done。
    #[test]
    fn local_created_done() {
        assert!(matches!(
            mkdir_judge("C:\\a\\b", None, false),
            Outcome::Done
        ));
    }

    /// 本地：create_dir_all 失败但 Path::is_dir 复查为真 → Done。
    #[test]
    fn local_failed_but_recheck_is_dir_done() {
        assert!(matches!(
            mkdir_judge("C:\\a\\b", Some("已存在同名文件".into()), true),
            Outcome::Done
        ));
    }

    /// 本地：create_dir_all 失败且复查不是目录 → Failed。
    #[test]
    fn local_failed_and_missing_failed() {
        assert!(matches!(
            mkdir_judge("C:\\a\\b", Some("拒绝访问。".into()), false),
            Outcome::Failed(_)
        ));
    }

    /// 失败文案复用表内既有键，带路径与原因（i18n 守卫零新增键）。
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

// ── T16 单测：节流刷新三守卫 + 同目录比对 + 目标目录 + 聚合 ──
#[cfg(test)]
mod refresh_tests {
    use super::{Kind, Status, TaskOwner, dest_dir, refresh_due, refresh_targets, same_dest_dir};

    /// ① 间隔守卫：3s 内不刷（即使有增量且同目录）。
    #[test]
    fn refresh_due_interval_not_met_skips() {
        assert!(!refresh_due(
            10.0,
            10.0 + crate::transfer::REFRESH_THROTTLE_SECS - 0.1,
            3,
            true
        ));
    }

    /// ① 间隔边界：恰 3s 到期（C# PeriodicTimer 首拍语义）。
    #[test]
    fn refresh_due_interval_boundary_three_secs() {
        assert!(refresh_due(
            10.0,
            10.0 - crate::transfer::REFRESH_THROTTLE_SECS,
            1,
            true
        ));
    }

    /// ② 增量守卫钉子：完成计数不变不刷（大文件慢传期间不打无谓列目录）。
    #[test]
    fn refresh_due_counts_frozen_skips() {
        assert!(!refresh_due(10.0, 7.0, 0, true));
    }

    /// ③ 同目录守卫钉子：翻走不刷（用户去别的目录浏览不干扰）。
    #[test]
    fn refresh_due_tab_left_skips() {
        assert!(!refresh_due(10.0, 7.0, 2, false));
    }

    /// ③ 钉子：翻回目标目录后再次到期可见（三守卫齐 → true）。
    #[test]
    fn refresh_due_tab_back_refreshes() {
        assert!(refresh_due(13.0, 7.0, 1, true));
    }

    /// 同目录：远程 Ordinal 大小写敏感。
    #[test]
    fn same_dest_dir_remote_case_sensitive() {
        assert!(!same_dest_dir("/srv/Data", "/srv/data", true));
        assert!(same_dest_dir("/srv/data", "/srv/data", true));
    }

    /// 同目录：本地宽松（Windows IgnoreCase 近似）。
    #[test]
    fn same_dest_dir_local_loose() {
        assert!(same_dest_dir("D:\\Data", "d:\\data", false));
    }

    /// 目标目录：远程父目录（根上传 "/f" → "/"；目录任务 "/r/name" → "/r"）。
    #[test]
    fn dest_dir_remote_parent_and_root() {
        assert_eq!(dest_dir("/r/name", true).as_deref(), Some("/r"));
        assert_eq!(
            dest_dir("/r/name/sub/f", true).as_deref(),
            Some("/r/name/sub")
        );
        assert_eq!(dest_dir("/f", true).as_deref(), Some("/"));
    }

    /// 目标目录：本地父目录（文件/目录同口径）。
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

    /// 聚合：完成计数 Done+Failed（src/dst 各计），目标目录候选只取 dst 端；
    /// 传输中不计完成。
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
        // 按 tab 键排序：src (0,1) 在前——完成计 2（Done+Failed），无目标目录。
        assert_eq!(out[0], ((0, 1), 2, Vec::<String>::new()));
        // dst (1,2)：完成计 2，候选去重同为 "/r"。
        assert_eq!(out[1], ((1, 2), 2, vec!["/r".to_owned()]));
    }

    /// 聚合：整目录任务根 dest 的父目录即浏览目录（展开根 MakeDir dest 同口径）。
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

    /// 聚合：本地任务目标目录宽松比对口径的候选形态（下载目标 = 本地父目录）。
    #[test]
    fn refresh_targets_download_dest_local() {
        let o = TaskOwner {
            src: (1, 2),
            dst: (0, 1),
        };
        let rows = [(&o, false, "D:\\dl\\a.bin", Status::Transferring)];
        let out = refresh_targets(rows.iter().map(|&(a, b, c, d)| (a, b, c, d)));
        // 按 tab 键排序：dst (0,1) 在前——目标目录候选 = 本地父目录。
        assert_eq!(out[0], ((0, 1), 0, vec!["D:\\dl".to_owned()]));
        assert_eq!(out[1], ((1, 2), 0, Vec::<String>::new()));
    }

    /// Kind::dest_is_remote 分流（下载/本地复制=本地，上传/建目录/中转=远程）。
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

// ============================================================================
// #5 返工钉子——settle 行态收口：校验线程二次回执（Verifying 行）与 cancel
// 末段边沿。手工构造 Shared（不经 new()，避免拉起 worker/校验线程碰网络）。
// ============================================================================

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
            // 校验投递口（测试夹具无校验线程，Sender 丢弃 = 投递静默失效）。
            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            // 扫描投递口（同上：无扫描线程，投递静默失效）。
            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        })
    }

    /// 入队一条文件上传任务（size=100）并返回行 id。
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

    /// 钉子 1：校验线程 Match 回执（Verifying 行二次 settle(Done)）→ 按完成
    /// 结算（C# Complete），不得误判「已取消」（返工前落入投递分支的错误面）。
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

    /// 钉子 2：校验线程 Skipped 回执 → 按完成结算，结论列保留「校验跳过」。
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

    /// 钉子 3：校验线程 Mismatch 回执（Failed 携带两侧哈希）→ 行进「传输
    /// 失败」页签，reason 与队列日志均带两侧哈希（C# SetVerifyResult → Fail）。
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

    /// 钉子 4：传输末段检查点之后落下的取消（Transferring + cancel_req）→
    /// 按已取消结算（C# 行态检查），不进校验投递（结论保持未校验）。
    #[test]
    fn late_cancel_edge_after_transfer_settles_cancelled() {
        // CR-4：校验开关改读批次快照——只改本行 cfg，不碰全局设置单例。
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
    /// 钉子（T20-H cancel_batches）：流式落位逐批返回批次号——按批次号回溯
    /// 取消只结算指定批次的等待行（他批不受牵连），返回取消条数；空批次清单
    /// 零取消。
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

    // ── CR-4 批次配置快照钉子（只操作行上 cfg，零全局态写入 / 零复位夹具）──

    /// 入队即固化，且消费口 `cfg_of` 读的是行上那一份（不是每步重读全局）。
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

    /// 后批入队 / 前批改动互不串档（H2 档案固化守卫的同款语义）。
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

    /// 目录展开子行继承父行的批次 cfg（不是重新取一次设置）。
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

    /// settle 的校验投递读批次 cfg：全局默认 verify=false，把行上 cfg 置 true
    /// 仍应转 Verifying，置 false 则立即 Done（读全局的话两次都是 Done）。
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

    /// B 类并发数不进快照：lane_run 每轮即时生效的设计不被反转。
    #[test]
    fn lane_count_still_live_outside_cfg() {
        let mut a = crate::settings::AppSettings::default();
        let mut b = a.clone();
        a.concurrent_transfers = 1;
        b.concurrent_transfers = 3;
        assert_eq!(TransferCfg::from(&a), TransferCfg::from(&b));
    }

    /// 内部防抖态与 C 类 UI 字段不进快照：相等性只由 8 个 A 类字段决定。
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

    /// 续传阈值从快照换算（KB → 字节），与全局态无关。
    #[test]
    fn resume_threshold_uses_batch_cfg() {
        let mut cfg = TransferCfg::default();
        cfg.resume_threshold_kb = 200;
        assert_eq!(resume_threshold(&cfg), 200 * 1024);
        cfg.resume_threshold_kb = 1024;
        assert_eq!(resume_threshold(&cfg), 1024 * 1024);
    }

    /// 8 个字段逐一映射（防日后扩字段时漏改 From）。
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

// ============================================================================
// #6 钉子——重试清场：失败行 retry 后回等待，上一轮旧状态（完成时刻/已传
// 字节/续传旗标/校验结论/原因/速度/取消旗标）全部复位（C# RetryAsync
// :855-864 字段集），重传成功的行不再显示旧「续传中」「已校验」残留。
// ============================================================================

#[cfg(test)]
mod retry_tests {
    use super::*;

    /// 钉子：全套旧状态残留的失败行 retry → 回等待 + 全清（含末段边沿
    /// cancel_req 残留的取消后重跑路径）。
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
        // 模拟上一轮遗留：已失败 + 全套旧状态（校验不匹配 / 续传中 / 旧时刻）。
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

// ============================================================================
// #7 钉子——暂停拦领取：claim() 暂停门（C# :483-492 信号量门等价）——暂停
// 期间等待行不被领取（无检查点任务 MakeDir/ServerCopy/树展开/tar 打包拿不到
// 执行权），resume 后立即可重领。
// ============================================================================

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

    /// 钉子 1：暂停 → claim 挂门（行保持 Waiting，不置 Transferring）；resume
    /// 后门开，同一行立即被领取。
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

    /// 钉子 2：空队列 → Idle（复位停止标志/断连归 step 处理，门不领取）。
    #[test]
    fn idle_queue_returns_idle_gate() {
        let (shared, _q) = queue();
        assert!(matches!(claim(&shared), Gate::Idle));
    }
}

// ============================================================================
// T18 队列语义五条钉子：批次开始/结束日志（T18-2）、重置并重新请求所有
// （T18-3）、清除已完成含校验中（T18-5）、停止标志批次开始即复位（T18-6）。
// ============================================================================

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

    /// 钉子（T18-6）：停止后批次开始（入队）即复位停止标志——旧任务有
    /// cancel_req 兜底仍会取消，新批不被旧 stopping 在块边界误取消。
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

    /// 钉子（T18-2 活键）：批次开始/结束日志出键——入队记「开始传输 N 项 → 目标」，
    /// 批次全终态后 drain 记「传输完成：…」一条，重复 drain 不再出；重试重跑后
    /// 再记一条（重置并重新请求所有路径同 C# TransferQueueService.cs:920-929）。
    #[test]
    fn batch_start_and_done_logs_flow() {
        let (shared, q) = queue();
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(None, probe, vec![upload_task("a"), upload_task("b")], 0);
        // 开始日志（C# LogTransferStart）。
        let logs = shared.log.lock().unwrap().clone();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("[本地] 开始传输 2 项 → /srv"));
        // 批次未结算：无结束汇总。
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
        // 已上报：不再重复出。
        assert!(q.drain_batch_reports().is_empty());
        // 重试重跑（C# 重传路径结束后再记一条）。
        let id = shared.st.lock().unwrap().items[1].id;
        q.retry(id);
        shared.st.lock().unwrap().items[1].status = Status::Done;
        let reports = q.drain_batch_reports();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].contains("传输完成：成功 2 项，跳过 0 项，失败 0 项"));
    }

    /// 钉子（T18-2 口径）：已取消行计入失败（C# 停止路径 RunAsync 收尾把未
    /// 处理行记失败）；「跳过冲突项」条数进汇总 {1}，上报后取走。
    #[test]
    fn batch_report_counts_cancelled_and_skipped() {
        let (shared, q) = queue();
        let probe: Probe = Arc::new(Mutex::new((0, None)));
        q.enqueue(
            None,
            probe,
            vec![upload_task("a"), upload_task("b")],
            2, // 覆盖确认「跳过冲突项」2 条
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

    /// 钉子（T18-5）：清除已完结连校验中一起清（C# ClearAll :706-715）——
    /// 只留等待/传输中。
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

    /// 钉子（T18-3）：重置并重新请求所有——全部行回等待（含 Done）、字段集
    /// 同 retry 一次复位、显式解除暂停（C# TransferQueueService :890-911）。
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

// ============================================================================
// T20-2 目录扫描线程钉子：入队即扫 / 暂停不拦 / 整树取消 / 重试重扫 / tar 预检。
// 手工构造 Shared（不经 new()，避免拉起 worker/扫描线程碰网络）。
// ============================================================================

// CR-4：配置随批次固化后，settle / tar 预检用例只改本行 cfg，不再改写全局
// 设置单例——原 SETTINGS_TEST_LOCK 串行锁与「结束手工复位」夹具随之失去必要，已删。

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
            // 校验/扫描投递口：测试夹具无线程，Sender 丢弃 = 投递静默失效。
            verify_tx: tokio::sync::mpsc::unbounded_channel::<VerifyJob>().0,
            scan_tx: tokio::sync::mpsc::unbounded_channel::<ScanJob>().0,
            #[cfg(test)]
            scan_batches_placed: AtomicUsize::new(0),
        })
    }

    /// 带真扫描接收端的夹具（验证投递面：enqueue/retry/reset_all 作业可收）。
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

    /// 临时目录树：root/{a.txt,b.txt,sub/c.txt}。测试尾自行清理。
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

    /// 入队一个本地复制整目录任务（枚举产物 = 根 MakeDir + a/b/sub MakeDir + c
    /// 共 5 条），返回 (行 id, 目标目录)。
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

    /// 钉子（拍板 E）：暂停中扫描照跑——入队目录 → 扫描完成 → 父行移除、
    /// 子行全等待、零字节传输（恢复后 worker 暂停门扣住，正常开传）。
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
        assert_eq!(st.items.len(), 5); // 根 MakeDir + a/b/sub/c，父行已移除
        assert!(st.items.iter().all(|i| i.parent == id));
        for i in &st.items {
            assert_eq!(i.status, Status::Waiting); // 暂停不拦扫描，子行落等待
            assert_eq!(i.transferred, 0);
        }
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 钉子：扫描中父行单任务取消 → 整树结算已取消；传输中子行置取消请求。
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
                // 传输中子行置取消请求，块边界才结算（C# 检查点语义，与非扫描行一致）。
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

    /// 扫描行不被 worker 领取：仅扫描行 → Gate::Idle（路不会把扫描行当任务）。
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

    /// 重试目录行 → 回扫描态、旧子行摘除、扫描作业带原 id 投递。
    #[test]
    fn retry_dir_row_rescans_and_drops_old_children() {
        let (shared, mut rx) = shared_paired();
        let root = temp_tree("retry");
        let (id, dst) = enqueue_dir(&shared, &root);
        let _ = rx.try_recv(); // 排掉 enqueue 投递
        {
            let mut st = shared.st.lock().unwrap();
            st.items[0].status = Status::Cancelled;
            // 旧展开子行（重试后应被摘除）。
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

    /// 扫描行计活动面：关闭确认计数 / 归属 tab 忙集合 / 清除已完结保留 /
    /// 批次汇总不提前出。
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
        q.clear_done(); // 扫描行非完结，保留
        assert_eq!(shared.st.lock().unwrap().items.len(), 1);
        assert!(q.drain_batch_reports().is_empty()); // 批次未结束不汇总
    }

    /// tar 预检达标 → handback：父行回等待态、置 precheck 旗标、零子行
    /// （worker 侧 maybe_run 凭旗标免重复计数）。
    #[test]
    fn tar_precheck_handbacks_parent_with_precheck_flag() {
        // CR-4：tar 门改读批次快照——只改本行 cfg，不碰全局设置单例。
        let shared = shared();
        let root = temp_tree("tarpre"); // 3 个文件 ≥ 阈值 2
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

    /// 父行已不处扫描态（被取消）→ 扫描作业放弃：不落位、不移除、保留现场。
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

    /// 重置所有：目录行回扫描态并重投作业、旧子行摘除（重扫后重新落位）。
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

    /// 钉子（P0 红线，最高优先）：用户显式选覆盖（force=true）的整目录任务
    /// → 扫描落位子行全量继承 force（含 MakeDir 子任务）——覆盖绝不能被静默
    /// 降级为追加（C# forceOverwrite 传入每个子文件执行体，FileView.xaml.cs
    /// :3125/3239；N1 红线）。
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

    /// 钉子（A3 时序）：stop_all() 取消扫描中批次 → 立即 enqueue() 新批
    /// （enqueue 复位 stopping，:561）→ 再驱动旧 scan_job → 旧父 id 下零残留
    /// 插行、新批不受影响。
    #[test]
    fn stop_all_then_new_batch_old_scan_job_leaves_no_rows() {
        let shared = shared();
        let root = temp_tree("a3");
        let (old_id, dst) = enqueue_dir(&shared, &root);
        TransferQueue {
            shared: Arc::clone(&shared),
        }
        .stop_all(); // 旧批次整树结算已取消（扫描行在列）
        // 立即入队新批（文件任务）：enqueue 内部复位 stopping（:561）。
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
        // 旧批次的扫描作业迟到（stop_all 与 enqueue 之后才轮到执行）。
        rt().block_on(scan_job(
            &shared,
            copy_dir_job(old_id, &root, &dst),
            &mut None,
            &mut None,
        ));
        let st = shared.st.lock().unwrap();
        assert!(st.items.iter().all(|i| i.parent != old_id)); // 零残留插行
        let old = st.items.iter().find(|i| i.id == old_id).unwrap();
        assert_eq!(old.status, Status::Cancelled); // 旧父行保持取消现场
        let new = st.items.iter().find(|i| i.id == new_id).unwrap();
        assert_eq!(new.status, Status::Waiting); // 新批不受影响
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 钉子（分批边界 / 增量过程）：≥SCAN_BATCH×2 条目目录 → 多批落位可观测
    /// （批计数插桩 = 恰 3 批，证明非瞬间全量）+ 终态行数 = MakeDir + 文件
    /// 子任务总数 + 子行 id 严格按枚举序（每批插入点 = 同父子行尾部）。
    #[test]
    fn large_dir_places_in_batches_with_exact_final_shape() {
        let shared = shared();
        // root: 2000 文件（f0000..f1999）+ sub/c.txt → 枚举序 =
        // MakeDir(root), f0000..f1999, MakeDir(sub), sub/c.txt = 2003 条。
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
        // 分批可观测（计数器按实例隔离，无并行串扰）：2003 条 / SCAN_BATCH=1000
        // → 恰 3 批（1000+1000+3），证明落位逐批发生非瞬间全量。
        assert_eq!(shared.scan_batches_placed.load(Ordering::Relaxed), 3);
        let st = shared.st.lock().unwrap();
        assert_eq!(st.items.len(), 2003); // MakeDir×2 + 2001 文件，父行已移除
        assert!(st.items.iter().all(|i| i.parent == id));
        let children: Vec<&Item> = st.items.iter().collect();
        // 子行 id 严格递增 = 枚举序（分批插入点 = 同父子行尾部的行为等价证明）。
        for w in children.windows(2) {
            assert!(w[0].id < w[1].id);
        }
        assert!(matches!(children[0].kind, Kind::MakeDir { .. })); // 根建目录
        assert!(matches!(children[1].kind, Kind::CopyLocal { .. })); // f0000
        assert!(matches!(children[2001].kind, Kind::MakeDir { .. })); // sub 建目录
        let tail = children[2002];
        assert!(matches!(tail.kind, Kind::CopyLocal { .. })); // sub/c.txt
        assert!(tail.source.ends_with("c.txt"));
        drop(st);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 钉子（T20-3 relay 行端到端）：Kind::RelayFile 行经 child_item 真构造路径
    /// size = 原始 ×2（下载 + 上传两程总量）→ 续传百分比按 ×2 后总量整除 →
    /// 队列第四列显「续传中 25%」。测试名与断言对象一致（真 relay 行，非
    /// panes.rs 里 kind 占位的纯公式钉）。
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
        // 构造路径 ×2：下载 + 上传两程总量。
        assert_eq!(it.size, 1000);
        assert!(matches!(it.kind, Kind::RelayFile { .. }));
        // 置传输态（resumed）：百分比按 ×2 后总量整除 → 25%（speed=0 无速度后缀）。
        it.status = Status::Transferring;
        it.resumed = true;
        it.transferred = 250;
        assert_eq!(
            crate::ui::panes::queue_status_text(&it, false),
            format!("{} 25%", t("续传中"))
        );
    }

    /// 钉子（F-12 继承链）：child_item 透传位与调用方传入的父行值一致——
    /// tar/expand 子行不吃静默 Append（resume_decision 消费端钉在 resume_tests）。
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
