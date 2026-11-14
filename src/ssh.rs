//! SSH/SFTP 会话层（T3 远程真实数据接入）：russh 0.65（ring 后端）+ russh-sftp 3.0。
//!
//! russh 是异步库：独立工作线程自建 current-thread tokio runtime，UI 线程经
//! std::sync::mpsc 收发命令/事件，绝不在 egui 帧内阻塞。
//! MVP 口径（对齐 C# SftpSessionService 的连接/枚举语义）：
//! - 密码认证；密钥登录随主机管理任务接入；
//! - 指纹校验（FpGate，对齐 C# S3.6/S5.2/S9）：Interactive = 主连接与测试连接
//!   （本次已确认复用 → 存档精确匹配 → 弹确认框，拒绝即中止；G15 测试连接同门，
//!   label 空 = 日志全局广播无前缀）；Silent = exec 通道与传输连接静默 fail-closed
//!   （仅存档精确匹配才信任，G16）；
//! - 连接/认证各包一层超时（对齐设置页「连接超时」默认 20s）。
//!
//! T4 起暴露独立连接入口给传输队列（connect_sftp_silent 静默门，不复用主会话，C# 同语义）。
//! T5 起增加：Delete（SFTP 递归删除，C# 回退路径同款）+ 独立 exec 通道
//! （独立线程；CR-7-S4 起缓存已认证连接按档案复用，命令超时 10s / 目录删除
//! 120s，exit 超时归一 -1 —— C# §2 语义；档案变化或连接失败即丢缓存重建）。
//!
//! IMPL-connect P1（对齐 C# S3.2-3.5 / S5.5 / S6）：
//! - G5 重试：connect_with_retry 闭环，maxAttempts = 1 + MaxRetries；认证失败与
//!   指纹拒绝不重试；重试复用本次已确认指纹（FpGate 跨尝试共享）；
//! - G14 操作超时：sftp_op 包装全部 SFTP 操作（OperationTimeout，0=不限）；
//! - G11 保活：russh keepalive_interval 接 settings（keep_alive + 间隔秒），
//!   主连接与 exec 独立连接同口径；
//! - G8 探测：首个列表成功后 fire-and-forget 探测服务端哈希能力（POSIX →
//!   sha256sum/sha512sum/openssl SM3 一次往返），代际守卫防重连串档，
//!   结果快照经 `Ssh::hash_probe` 供 UI 消费（G9）。
//!
//! IMPL-connect P2：
//! - G10 删除分流：设置「服务端命令删除」开 → 目录删除先经 exec rm -rf
//!   （独立 120s，退出码 0 判成），迟到删净核对后回退 SFTP 递归（C# 同语义）；

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::i18n::{t, tfmt};
use crate::ui::panes::Entry;

/// 认证方式（T6 主机库）：密码 / 私钥文件（档案只存路径，私钥内容不读取复制 —— C# 红线）。
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    Password(String),
    /// 私钥文件路径；加密私钥无口令字段，加载失败按认证失败呈现（C# 同口径）。
    KeyFile(String),
}

/// Debug 手写脱敏：Password 不出明文（防日志/测试断言泄漏密码）。
impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Password(_) => f.write_str("Auth::Password(****)"),
            Auth::KeyFile(p) => f.debug_tuple("Auth::KeyFile").field(p).finish(),
        }
    }
}

/// 连接（TCP+握手）超时：settings.json connectTimeout（0=禁用），默认 20s。
fn connect_timeout() -> Option<Duration> {
    let s = crate::settings::with(|v| v.connect_timeout);
    (s > 0).then(|| Duration::from_secs(s as u64))
}

/// 认证超时：与连接超时共用设置（C# 同用 ConnectTimeoutSeconds）。
fn auth_timeout() -> Option<Duration> {
    connect_timeout()
}

/// SFTP 操作超时秒数（G14，C# OperationTimeout 默认 15；0 = 不限）。
fn operation_timeout_secs() -> i32 {
    crate::settings::with(|v| v.operation_timeout)
}

/// keep-alive 间隔（G11，C# KeepAliveEnabled + KeepAliveInterval 默认 30s）：
/// 开关开且间隔 > 0 才启用。
fn keepalive_interval() -> Option<Duration> {
    let (on, secs) = crate::settings::with(|v| (v.keep_alive, v.keep_alive_interval));
    (on && secs > 0).then(|| Duration::from_secs(secs as u64))
}
/// exec 命令超时（含建连；C# CommandTimeout 默认 10s，ExitStatus null 归一 -1）。
const EXEC_TIMEOUT: Duration = Duration::from_secs(10);

/// SFTP 批量请求并发度（StatBatch / DeleteBatch 共用）：块内 join_all 同时在途，
/// 块耗时 ≈ 单次 RTT 而非 n×RTT；russh-sftp 按请求 id 配对响应，天然支持多在途。
/// CR-7 前为 StatBatch 分支内的局部常量，批量删除复用同一口径故提升为模块级。
const STAT_CONCURRENCY: usize = 32;

/// UI → 工作线程命令。
pub enum Cmd {
    Connect {
        host: String,
        port: u16,
        user: String,
        auth: Auth,
        /// 终端日志来源标签（"名称 (IP)"，C# HostTabNaming）。
        label: String,
        /// 存档指纹（SHA256 base64 无前缀；None = 无存档）。
        fingerprint: Option<String>,
    },
    /// 列远端目录（SSH_FXP_READDIR 一次整层）；tag 随 Ev::Listing / Ev::Failed
    /// 原样回声，UI 据此定位发起 tab 并丢弃过期响应。
    List {
        tag: u64,
        path: String,
    },
    /// 探测远端目标是否存在（传输冲突决策）；req 用于丢弃过期响应。
    Stat {
        path: String,
        req: u64,
    },
    /// 批量探测远端目标（T20-G③ 批量传输冲突预检）：paths 逐条回 Ev::Stat
    /// （同 req）。会话 worker 分块并发——此前逐条串行 = O(n×RTT)，万级批次
    /// 分钟级才回流，队列长时间无法形成；russh-sftp 底层按请求 id 配对响应，
    /// 天然支持多在途。
    StatBatch {
        paths: Vec<String>,
        req: u64,
    },
    /// 批量删除远端条目（CR-7）：文件项按 STAT_CONCURRENCY 分块并发发 REMOVE，
    /// 目录项不进并发块仍逐个走 exec / SFTP 递归；逐项成败经 Ev::Log 留终端日志
    /// （C5 不可逆操作）。整批只回一条 Ev::Deleted，tag 原样回声 —— UI 的
    /// del_pending 按「批」计数（发批 +1、回执 -1），不是按项。
    DeleteBatch {
        tag: u64,
        items: Vec<(String, bool)>,
    },
    /// CR-7-S4 内部回执：exec 线程跑完一条目录删除命令后送回主 worker，
    /// 由 worker 继续做迟到删净核对 / 回退，并在整批目录结束时发唯一一条
    /// Ev::Deleted。不是 UI 命令——仅 `exec_run` 经 worker_tx 回送。
    ExecDone {
        tag: u64,
        result: Result<(i32, String, String), String>,
    },
    /// 读远端文件（远程编辑下载；req 丢弃过期响应，max 字节上限）。
    ReadFile {
        path: String,
        req: u64,
        max: u64,
    },
    /// 写远端文件（远程编辑回传；整体替换 CREATE|TRUNCATE|WRITE）。
    WriteFile {
        path: String,
        req: u64,
        data: Vec<u8>,
    },
    /// 新建远端目录（SSH_FXP_MKDIR；成败细节走 Ev::Log，完成统一 Ev::Created）。
    /// tag 随 Ev::Created 原样回声。
    Mkdir {
        tag: u64,
        path: String,
    },
    /// 新建远端空文件（CREATE|WRITE 打开后即关；完成统一 Ev::Created）。
    /// tag 随 Ev::Created 原样回声。
    CreateFile {
        tag: u64,
        path: String,
    },
    /// 远端重命名（SSH_FXP_RENAME，P0-1 落盘；成败细节走 Ev::Log，完成统一
    /// Ev::Created）。tag 随 Ev::Created 原样回声。
    Rename {
        tag: u64,
        path: String,
        new_path: String,
    },
    Disconnect,
}

/// exec 作业（tokio unbounded —— std 阻塞 recv 会冻结 current_thread 调度器，
/// 缓存连接的后台任务没人轮询，下一次 exec_on_channel 会挂死，G8 同款教训）。
/// CR-7-S4：命令窗与目录删除走两条独立通道，避免 120s 长删除把命令窗排队饿死；
/// 两条通道各自持一份缓存，均只在创建它的任务内复用 / 丢弃，不跨 runtime 传 Handle。
pub enum ExecReq {
    /// 命令窗：结果走 Ev::Exec，req 原样回声。
    Command { req: u64, cmd: String },
    /// 服务端命令删除目录：结果经 Cmd::ExecDone 回主 worker（worker 再核对 / 回退）。
    Delete { tag: u64, cmd: String },
}

/// 工作线程 → UI 事件（UI 每帧 poll 取空）。
pub enum Ev {
    /// 认证成功且 SFTP 子系统就绪；root = 规范化起始目录；
    /// confirmed = 本次连接确认的指纹（UI 据此落盘存档，C# S1.9）。
    Connected {
        label: String,
        root: String,
        confirmed: Option<String>,
    },
    /// 主机指纹确认请求（C# HostKeyConfirmDialog；worker 阻塞等 reply，false = 拒绝）。
    AskFingerprint {
        first: bool,
        host: String,
        stored: Option<String>,
        presented: String,
        reply: mpsc::Sender<bool>,
    },
    /// 失败（what = "连接" / "列目录"，UI 据此分流提示位置）。
    /// tag = 发起命令的回声：列目录失败 Some(tag)（UI 按 tag 定位发起 tab），
    /// 连接失败 None（会话级）。worker 仅透传，零逻辑。
    Failed {
        what: &'static str,
        msg: String,
        tag: Option<u64>,
    },
    /// 目录枚举结果；tag = Cmd::List 的回声（UI 按 tag 定位发起 tab）。
    Listing {
        tag: u64,
        path: String,
        entries: Vec<Entry>,
    },
    /// Stat 响应（查不到/出错一律 exists=false）。
    Stat {
        req: u64,
        path: String,
        exists: bool,
        /// 远端大小（字节；同上 None）。传输冲突对比表「目标」列。
        size: Option<u64>,
        /// 远端 mtime（Unix 秒；存在但无属性时 None）。外部编辑回传冲突比对用。
        mtime: Option<i64>,
    },
    /// 远程编辑读文件完成（超 max 截断不报错 —— 编辑器只收 ≤10MB 的探测结果）。
    FileData {
        req: u64,
        result: Result<FileData, String>,
    },
    /// 远程编辑写回完成；mtime = 写后远端 mtime（回填 base，避免自冲突）。
    FileWritten {
        req: u64,
        path: String,
        result: Result<Option<i64>, String>,
    },
    /// 删除完成（无论成败；成败细节已走 Ev::Log）；tag = Cmd::DeleteBatch 回声。
    /// CR-7：一条回执代表整批，del_pending 按批归零。
    Deleted { tag: u64 },
    /// 新建条目完成（无论成败；成败细节已走 Ev::Log；UI 收到即重列当前目录）；
    /// tag = Cmd::Mkdir / Cmd::CreateFile 回声。
    Created { tag: u64 },
    /// 工作线程产生的终端日志（已带来源前缀）。
    Log { msg: String },
    /// exec 命令完成（exit=-1 = 超时/通道异常，C# ExitStatus null 归一）。
    Exec {
        req: u64,
        exit: i32,
        out: String,
        err: String,
    },
    /// 主动断开完成。
    Disconnected,
}

/// UI 侧句柄：命令入口 + 事件队列（Mutex 包给 &self 用）。
pub struct Ssh {
    /// 命令通道（tokio unbounded：UI 同步 send；worker 真 async recv——
    /// std 阻塞 recv 会冻结 current_thread 调度器，饿死 tokio::spawn 的
    /// fire-and-forget 探测任务，G8 日志永不出现）。
    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    exec_tx: tokio::sync::mpsc::UnboundedSender<ExecReq>,
    rx: Arc<Mutex<mpsc::Receiver<Ev>>>,
    /// 会话探针（C# SftpSessionService.IsConnected 等价）：工作线程当前是否有
    /// 已认证会话——连接成功置位、连接失败/主动断开复位；被动掉线随 keepalive
    /// 接入后同口径复位。队列行菜单「重连」按此决定可点/置灰。
    connected: Arc<AtomicBool>,
    /// 服务端哈希探测快照（G8）：(代际, 结果)；重连推进代际并清空，
    /// 在途探测回写前比对代际防串档。
    probe: Arc<Mutex<(u64, Option<HashProbe>)>>,
    /// exec 通道连接档案（双会话改造：从全局单例改为 per-Ssh 实例——左右两份
    /// 会话各自维护独立 exec 档案，探测/校验码/命令删除落到正确的会话主机）。
    exec_spec: Arc<Mutex<Option<ExecSpec>>>,
}

/// 远程编辑读文件结果（data + 写前 mtime）。
pub struct FileData {
    pub data: Vec<u8>,
    pub mtime: Option<i64>,
}

impl Ssh {
    /// 启动工作线程（调用方保证只建一次：State.ssh get_or_insert）。
    pub fn spawn() -> Self {
        // G8：命令通道走 tokio unbounded（见 Ssh.tx 注释）；Ev 通道保持 std
        // （UI 侧 try_recv 同步轮询，worker 侧 unbounded send 永不阻塞）。
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
        let (exec_tx, exec_rx) = tokio::sync::mpsc::unbounded_channel::<ExecReq>();
        let (delete_tx, delete_rx) = tokio::sync::mpsc::unbounded_channel::<ExecReq>();
        let (etx, erx) = mpsc::channel();
        let erx = Arc::new(Mutex::new(erx));
        let etx2 = etx.clone();
        let connected = Arc::new(AtomicBool::new(false));
        let connected_worker = connected.clone();
        let probe = Arc::new(Mutex::new((0u64, None)));
        let probe_worker = probe.clone();
        // exec 档案槽：实例持有、UI 经方法注入、worker 确认指纹后回写。
        let exec_spec: Arc<Mutex<Option<ExecSpec>>> = Arc::new(Mutex::new(None));
        let exec_spec_run = exec_spec.clone();
        let exec_spec_exec = exec_spec.clone();
        // CR-7-S4：worker 只发删除作业；exec 线程把回执送回 worker 的命令队列。
        let worker_tx = tx.clone();
        std::thread::Builder::new()
            .name("ssh".into())
            .spawn(move || {
                run(
                    rx,
                    etx2,
                    connected_worker,
                    probe_worker,
                    exec_spec_run,
                    delete_tx,
                )
            })
            .expect("spawn ssh worker");
        std::thread::Builder::new()
            .name("ssh-exec".into())
            .spawn(move || exec_run(exec_rx, delete_rx, etx, exec_spec_exec, worker_tx))
            .expect("spawn ssh exec worker");
        Self {
            tx,
            exec_tx,
            rx: erx,
            connected,
            probe,
            exec_spec,
        }
    }

    pub fn connect(
        &self,
        host: String,
        port: u16,
        user: String,
        auth: Auth,
        label: String,
        fingerprint: Option<String>,
    ) {
        let _ = self.tx.send(Cmd::Connect {
            host,
            port,
            user,
            auth,
            label,
            fingerprint,
        });
    }

    pub fn list(&self, tag: u64, path: String) {
        let _ = self.tx.send(Cmd::List { tag, path });
    }

    pub fn stat(&self, path: String, req: u64) {
        let _ = self.tx.send(Cmd::Stat { path, req });
    }

    /// 批量探测远端目标（T20-G③）：一次批请求替代逐条 stat——会话 worker
    /// 分块并发，万级批次秒级回流；单条 stat 保留给外部编辑等单点探测。
    pub fn stat_batch(&self, paths: Vec<String>, req: u64) {
        let _ = self.tx.send(Cmd::StatBatch { paths, req });
    }

    /// 批量删除（CR-7）：一次发一个批、一个 tag；文件项块内并发、目录项串行。
    /// 全库仅 ui/dialogs.rs::apply_delete 一个调用点，单条 Delete 已随之取消。
    pub fn delete_batch(&self, tag: u64, items: Vec<(String, bool)>) {
        let _ = self.tx.send(Cmd::DeleteBatch { tag, items });
    }

    /// 读远端文件（远程编辑下载；结果走 Ev::FileData）。
    pub fn read_file(&self, path: String, req: u64, max: u64) {
        let _ = self.tx.send(Cmd::ReadFile { path, req, max });
    }

    /// 写远端文件（远程编辑回传；结果走 Ev::FileWritten）。
    pub fn write_file(&self, path: String, req: u64, data: Vec<u8>) {
        let _ = self.tx.send(Cmd::WriteFile { path, req, data });
    }

    /// 新建远端目录（结果走 Ev::Created，成败细节走 Ev::Log）。
    pub fn mkdir(&self, tag: u64, path: String) {
        let _ = self.tx.send(Cmd::Mkdir { tag, path });
    }

    /// 新建远端空文件（结果走 Ev::Created，成败细节走 Ev::Log）。
    pub fn create_file(&self, tag: u64, path: String) {
        let _ = self.tx.send(Cmd::CreateFile { tag, path });
    }

    /// 远端重命名（P0-1 落盘；结果走 Ev::Created，成败细节走 Ev::Log）。
    pub fn rename(&self, tag: u64, path: String, new_path: String) {
        let _ = self.tx.send(Cmd::Rename {
            tag,
            path,
            new_path,
        });
    }

    /// exec 通道命令（独立线程；结果走 Ev::Exec）。
    pub fn exec(&self, req: u64, cmd: String) {
        let _ = self.exec_tx.send(ExecReq::Command { req, cmd });
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(Cmd::Disconnect);
    }

    /// 会话探针（C# SftpSessionService.IsConnected）：工作线程当前是否有已认证
    /// 会话（快照读，非实时探测）。
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// 服务端哈希探测快照（G8/G9）：None = 未探测 / 不可用 / 重连已清。
    pub fn hash_probe(&self) -> Option<HashProbe> {
        self.probe.lock().unwrap().1
    }

    /// 探测快照共享句柄（G12）：传输后校验实时读——连接提交时快照会错过
    /// 首次探测（首连接时必然为空，整个会话校验被误判 Skipped）。
    pub fn probe_handle(&self) -> Arc<Mutex<(u64, Option<HashProbe>)>> {
        self.probe.clone()
    }

    /// 取走当前积压事件（每帧一次；不阻塞）。
    pub fn poll(&self) -> Vec<Ev> {
        let rx = self.rx.lock().unwrap();
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

fn run(
    rx: tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    etx: mpsc::Sender<Ev>,
    connected: Arc<AtomicBool>,
    probe: Arc<Mutex<(u64, Option<HashProbe>)>>,
    exec_spec: Arc<Mutex<Option<ExecSpec>>>,
    delete_jobs: tokio::sync::mpsc::UnboundedSender<ExecReq>,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(worker(rx, etx, connected, probe, exec_spec, delete_jobs));
}

/// 一条已认证的 SFTP 连接（主会话与传输连接共用；drop 即断开）。
pub(crate) struct SftpConn {
    _handle: russh::client::Handle<Handler>,
    pub(crate) sftp: russh_sftp::client::SftpSession,
}

impl SftpConn {
    /// 已建立会话上开 exec 通道跑命令（G12 传输后校验的服务端哈希用；
    /// Handle 的 channel_open_session 是 &self，无需可变借用。
    /// C# SftpSessionService.RunCommandAsync 同语义）。
    pub(crate) async fn exec(&self, cmd: &str) -> Result<(i32, String, String), String> {
        exec_on_channel(&self._handle, cmd).await
    }
}

/// 连接失败分类（G5 重试口径，对齐 C# S3.2：SshAuthenticationException 不重试，
/// 其余异常可重试；私钥读取失败非认证异常 → Io 同口径）。
pub(crate) enum ConnErr {
    /// 认证被服务器拒绝 —— 不重试。
    Auth(String),
    /// 连接/握手/超时/私钥读取等 —— 可重试。
    Io(String),
}

impl From<ConnErr> for String {
    fn from(e: ConnErr) -> String {
        match e {
            ConnErr::Auth(m) | ConnErr::Io(m) => m,
        }
    }
}

/// 服务端哈希能力快照（G8，C# S6 探测结果缓存；Copy 便于 `Ssh::hash_probe` 取值）。
#[derive(Clone, Copy, Default)]
pub struct HashProbe {
    /// POSIX shell（`echo $env:OS` 输出不含 Windows_NT/$env）。
    /// 本机拉流回退未实现（S6.4 更正），当前仅随快照留存。
    #[allow(dead_code)]
    pub posix: bool,
    pub sha256: bool,
    pub sha512: bool,
    /// SM3 仅经 `openssl dgst -sm3`（sm3sum 非通用命令）。
    pub sm3: bool,
}

impl HashProbe {
    /// 任一服务端哈希算法可用（C# ServerHashAny）。
    pub fn any(self) -> bool {
        self.sha256 || self.sha512 || self.sm3
    }
}

// ── 主机指纹门（C# S3.6 三分支语义）─────────────────────────

/// 终端日志行组装：label 空（G15 测试连接，C# S9 无 LogScope）= 全局广播无前缀。
fn log_line(label: &str, body: String) -> String {
    if label.is_empty() {
        body
    } else {
        format!("[{label}] {body}")
    }
}

/// 指纹门模式。
#[derive(Clone, Copy)]
enum FpMode {
    /// 静默 fail-closed：仅存档精确匹配才接受，无弹窗无日志
    /// （C# S5.2 exec 通道 + G16 传输连接）。
    Silent,
    /// 交互三分支：本次已确认复用 → 存档精确匹配 → 弹确认框
    /// （主连接 + G15 测试连接，C# S3.6）。
    Interactive,
}

/// 指纹门状态（check_server_key 回调持有；Clone 共享 confirmed/denied 给 establish）。
#[derive(Clone)]
struct FpGate {
    mode: FpMode,
    /// 存档指纹（SHA256 base64 无前缀）。
    trusted: Option<String>,
    /// 终端日志来源标签（"名称 (IP)"）。
    label: String,
    /// 对端主机地址（弹框正文用）。
    host: String,
    /// UI 事件通道（Interactive 弹框用；Accept/Silent 用哑通道）。
    etx: mpsc::Sender<Ev>,
    /// 本次连接已确认指纹（重试复用免二次弹框；establish 回传给 UI 落盘）。
    confirmed: Arc<Mutex<Option<String>>>,
    /// 用户拒绝（失败文案归一「你拒绝了本次主机指纹，连接中止」且不重试）。
    denied: Arc<Mutex<bool>>,
}

impl FpGate {
    /// 交互门（主连接，C# S3.6）。
    fn new(trusted: Option<String>, label: String, host: String, etx: mpsc::Sender<Ev>) -> Self {
        Self {
            mode: FpMode::Interactive,
            trusted,
            label,
            host,
            etx,
            confirmed: Arc::new(Mutex::new(None)),
            denied: Arc::new(Mutex::new(false)),
        }
    }

    /// 静默门（exec 通道：仅存档精确匹配才接受，fail-closed）。
    fn silent(trusted: Option<String>) -> Self {
        let (tx, _rx) = mpsc::channel();
        Self {
            mode: FpMode::Silent,
            trusted,
            label: String::new(),
            host: String::new(),
            etx: tx,
            confirmed: Arc::new(Mutex::new(None)),
            denied: Arc::new(Mutex::new(false)),
        }
    }

    /// 交互门（共享 confirmed/denied，G15 测试连接：结果经 Arc 回传调用方）。
    fn interactive_shared(
        trusted: Option<String>,
        label: String,
        host: String,
        etx: mpsc::Sender<Ev>,
        confirmed: Arc<Mutex<Option<String>>>,
        denied: Arc<Mutex<bool>>,
    ) -> Self {
        Self {
            mode: FpMode::Interactive,
            trusted,
            label,
            host,
            etx,
            confirmed,
            denied,
        }
    }

    /// 静默门（共享 denied，G16 传输连接：拒绝可被调用方感知归类）。
    fn silent_denied(trusted: Option<String>, denied: Arc<Mutex<bool>>) -> Self {
        let (tx, _rx) = mpsc::channel();
        Self {
            mode: FpMode::Silent,
            trusted,
            label: String::new(),
            host: String::new(),
            etx: tx,
            confirmed: Arc::new(Mutex::new(None)),
            denied,
        }
    }

    /// 用户是否拒绝过指纹（失败文案归一用）。
    fn denied(&self) -> bool {
        *self.denied.lock().unwrap()
    }
}

/// 客户端回调：按 FpMode 校验服务器指纹。
pub(crate) struct Handler {
    fp: FpGate,
}

impl russh::client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = match server_public_key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(_) => return Ok(false),
        };
        // C# 同格式：SHA256 base64 无前缀（43 字符，如 6R2R3p2J…）。
        let fp = key.fingerprint(russh::keys::HashAlg::Sha256).to_string();
        let presented = fp.strip_prefix("SHA256:").unwrap_or(&fp).to_owned();
        match self.fp.mode {
            FpMode::Silent => {
                // G16：fail-closed 拒绝置 denied（调用方归类「指纹校验未通过」不重试）。
                let ok = self.fp.trusted.as_deref() == Some(presented.as_str());
                if !ok {
                    *self.fp.denied.lock().unwrap() = true;
                }
                Ok(ok)
            }
            FpMode::Interactive => self.interactive_check(&presented),
        }
    }
}

impl Handler {
    /// 交互三分支（C# S3.6）：① 本次已确认 → 直接信任（重试免二次弹框）；
    /// ② 存档精确匹配 → 直接信任；存档失配 → 先打校验失败日志再弹变更框；
    /// ③ 无存档 → 弹首连框。拒绝 → 标记 denied（失败文案归一，不重试）。
    fn interactive_check(&mut self, presented: &str) -> Result<bool, russh::Error> {
        // ① 本次连接已确认过（重试复用，C# S3.3）。
        if self.fp.confirmed.lock().unwrap().as_deref() == Some(presented) {
            return Ok(true);
        }
        // ② 存档精确匹配。
        if self.fp.trusted.as_deref() == Some(presented) {
            return Ok(true);
        }
        // 存档失配：先打校验失败日志（C# S3.6-1），再弹变更确认框。
        if let Some(stored) = &self.fp.trusted {
            let _ = self.fp.etx.send(Ev::Log {
                msg: log_line(
                    &self.fp.label,
                    tfmt(
                        "主机密钥校验未通过：存档 {0}，本次 {1}（服务器密钥变更时请重新信任）",
                        &[stored, &presented],
                    ),
                ),
            });
        }
        // G6：弹框前打等待日志（C# 指纹回调同款）。
        let _ = self.fp.etx.send(Ev::Log {
            msg: log_line(&self.fp.label, t("等待主机指纹确认 …").to_owned()),
        });
        let (rtx, rrx) = mpsc::channel();
        let _ = self.fp.etx.send(Ev::AskFingerprint {
            first: self.fp.trusted.is_none(),
            host: self.fp.host.clone(),
            stored: self.fp.trusted.clone(),
            presented: presented.to_owned(),
            reply: rtx,
        });
        // 阻塞等 UI 回执（wait_fingerprint_confirm，C# TaskCompletionSource 同
        // 语义 + 120s 罩顶）；worker 是 current-thread runtime 顺序执行，此处无
        // 其他任务可推进，无死锁；通道断开（UI 已丢弃弹框）= 拒绝，fail-closed；
        // 120s 无回执同按拒绝（防 UI 挂起时握手线程永久滞留），并写可检索日志。
        let ok = match wait_fingerprint_confirm(&rrx, std::time::Duration::from_secs(120)) {
            FpConfirm::Receipt(v) => v,
            FpConfirm::Timeout => {
                let _ = self.fp.etx.send(Ev::Log {
                    msg: log_line(&self.fp.label, t("指纹确认超时，连接中止").to_owned()),
                });
                false
            }
            // UI 已丢弃弹框：拒绝（原 fail-closed 语义，弹框路径自身已记日志）。
            FpConfirm::Disconnected => false,
        };
        if ok {
            *self.fp.confirmed.lock().unwrap() = Some(presented.to_owned());
        } else {
            *self.fp.denied.lock().unwrap() = true;
        }
        Ok(ok)
    }
}

/// 指纹确认回执等待结果（C# TaskCompletionSource 同语义 + 罩顶超时）。
pub(crate) enum FpConfirm {
    /// UI 回执（true=确认 / false=拒绝）。
    Receipt(bool),
    /// 罩顶超时：按拒绝 fail-closed（调用点写可检索日志）。
    Timeout,
    /// UI 已丢弃弹框（回执通道关闭）：拒绝。
    Disconnected,
}

/// 阻塞等 UI 指纹回执（握手线程同步等 UI；产品侧 timeout=120s，测试侧可传
/// 更短值钉三态判定）。
pub(crate) fn wait_fingerprint_confirm(
    rrx: &mpsc::Receiver<bool>,
    timeout: std::time::Duration,
) -> FpConfirm {
    use std::sync::mpsc::RecvTimeoutError;
    match rrx.recv_timeout(timeout) {
        Ok(v) => FpConfirm::Receipt(v),
        Err(RecvTimeoutError::Timeout) => FpConfirm::Timeout,
        Err(RecvTimeoutError::Disconnected) => FpConfirm::Disconnected,
    }
}

/// 测试连接专用（G15，C# S9）：Interactive 指纹门（指纹弹窗/落盘与主连接同一套），
/// 不重试；label 空 = 终端日志全局广播无前缀（C# LogScope 空同效）。
/// 成功返回本次确认的指纹（None = 存档精确匹配未弹框），由调用方写回存档；
/// 拒绝 → 归一「你拒绝了本次主机指纹，连接中止」（不重试）。
pub(crate) async fn connect_sftp_interactive(
    host: &str,
    port: u16,
    user: &str,
    auth: &Auth,
    trusted: Option<String>,
    etx: mpsc::Sender<Ev>,
) -> Result<Option<String>, String> {
    // 连接前日志（C# S3.7 指纹审计 + S1.6 正在连接，同主连接口径、无前缀）。
    if let Some(fp) = &trusted {
        let _ = etx.send(Ev::Log {
            msg: tfmt("信任存档指纹 {0}", &[fp]),
        });
    }
    let _ = etx.send(Ev::Log {
        msg: tfmt("正在连接 {0}:{1} …", &[&host.to_string(), &port]),
    });
    let confirmed = Arc::new(Mutex::new(None));
    let denied = Arc::new(Mutex::new(false));
    let handler = Handler {
        fp: FpGate::interactive_shared(
            trusted,
            String::new(),
            host.to_owned(),
            etx.clone(),
            confirmed.clone(),
            denied.clone(),
        ),
    };
    match connect_sftp_with(host, port, user, auth, handler).await {
        Ok(_) => {
            let _ = etx.send(Ev::Log {
                msg: tfmt("已连接 {0}", &[&host.to_string()]),
            });
            Ok(confirmed.lock().unwrap().clone())
        }
        Err(e) => Err(if *denied.lock().unwrap() {
            t("你拒绝了本次主机指纹，连接中止").to_owned()
        } else {
            e.into()
        }),
    }
}

/// 传输连接专用（G16，C# S5.2 静默门同语义）：仅存档指纹精确匹配才信任，
/// 否则静默拒绝 fail-closed（无弹窗无日志）。拒绝归 Auth 不重试。
pub(crate) async fn connect_sftp_silent(
    host: &str,
    port: u16,
    user: &str,
    auth: &Auth,
    trusted: Option<String>,
) -> Result<SftpConn, ConnErr> {
    let denied = Arc::new(Mutex::new(false));
    let handler = Handler {
        fp: FpGate::silent_denied(trusted, denied.clone()),
    };
    match connect_sftp_with(host, port, user, auth, handler).await {
        Ok(c) => Ok(c),
        Err(e) => Err(if *denied.lock().unwrap() {
            ConnErr::Auth(t("主机指纹校验未通过，连接中止").to_owned())
        } else {
            e
        }),
    }
}

/// 带指纹门的连接实现体（主连接 / 测试连接 Interactive，传输 / exec Silent 各自组装 Handler）。
pub(crate) async fn connect_sftp_with(
    host: &str,
    port: u16,
    user: &str,
    auth: &Auth,
    handler: Handler,
) -> Result<SftpConn, ConnErr> {
    let config = russh::client::Config {
        nodelay: true,
        inactivity_timeout: None,
        keepalive_interval: keepalive_interval(),
        ..Default::default()
    };
    let addr = (host.to_owned(), port);
    let mut handle = match connect_timeout() {
        Some(t) => tokio::time::timeout(t, russh::client::connect(Arc::new(config), addr, handler))
            .await
            .map_err(|_| ConnErr::Io(tfmt("连接超时（{0} 秒）", &[&t.as_secs()])))?,
        None => russh::client::connect(Arc::new(config), addr, handler).await,
    }
    .map_err(|e| ConnErr::Io(e.to_string()))?;

    let authed = match auth {
        Auth::Password(password) => {
            auth_with_timeout(|| handle.authenticate_password(user, password)).await?
        }
        Auth::KeyFile(path) => {
            // 私钥加载为同步 IO（本地文件）；读取失败非认证异常 → Io 可重试（C# 同口径）。
            let key = russh::keys::load_secret_key(path, None)
                .map_err(|e| ConnErr::Io(tfmt("私钥读取失败：{0}", &[&e])))?;
            // RSA 传 None 会落到 SHA-1 的 ssh-rsa（多数新服务器已禁用），统一升到 SHA-256；
            // 非 RSA 密钥 hash 位被 PrivateKeyWithHashAlg::new 忽略。
            let hash = key
                .algorithm()
                .is_rsa()
                .then_some(russh::keys::HashAlg::Sha256);
            let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            auth_with_timeout(|| handle.authenticate_publickey(user, key)).await?
        }
    };
    if !authed.success() {
        return Err(ConnErr::Auth(match auth {
            Auth::KeyFile(_) => t("认证失败：私钥被服务器拒绝").to_owned(),
            Auth::Password(_) => t("认证失败：用户名或密码错误").to_owned(),
        }));
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| ConnErr::Io(e.to_string()))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| ConnErr::Io(e.to_string()))?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| ConnErr::Io(e.to_string()))?;
    Ok(SftpConn {
        _handle: handle,
        sftp,
    })
}

/// 已建立会话的句柄集合（drop Handle 即断开）。
struct Ctx {
    conn: SftpConn,
    /// 终端日志来源标签（"名称 (IP)"，C# HostTabNaming）。
    label: String,
}

/// 认证一步包超时（连接/认证共用 connectTimeout 设置；失败归 Io 可重试，
/// 仅 connect_sftp_with 的 !success() 分支归 Auth 不重试）。
async fn auth_with_timeout<F, Fut>(make: F) -> Result<russh::client::AuthResult, ConnErr>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<russh::client::AuthResult, russh::Error>>,
{
    match auth_timeout() {
        Some(t) => tokio::time::timeout(t, make())
            .await
            .map_err(|_| ConnErr::Io(tfmt("认证超时（{0} 秒）", &[&t.as_secs()])))?,
        None => make().await,
    }
    .map_err(|e| ConnErr::Io(e.to_string()))
}

/// SFTP 单操作超时包装（G14，C# OperationTimeout 同语义：0 = 不限）。
/// 超时归一「操作超时（{0} 秒）」；底层错误转 String。
async fn sftp_op<T, E: std::fmt::Display, Fut>(fut: Fut) -> Result<T, String>
where
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let secs = operation_timeout_secs();
    if secs <= 0 {
        return fut.await.map_err(|e| e.to_string());
    }
    match tokio::time::timeout(Duration::from_secs(secs as u64), fut).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) => Err(tfmt("操作超时（{0} 秒）", &[&secs])),
    }
}

/// CR-7 删除批拆分：文件项进并发块，目录项保持原顺序串行（exec rm / SFTP 递归
/// 都要独占 sftp 句柄与回退路径，不与其它删除同时在途）。
fn split_delete_items(items: &[(String, bool)]) -> (Vec<&str>, Vec<&str>) {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for (path, is_dir) in items {
        if *is_dir {
            dirs.push(path.as_str());
        } else {
            files.push(path.as_str());
        }
    }
    (files, dirs)
}

/// CR-7 文件项分块计划：块数 = ceil(n / STAT_CONCURRENCY)，每块 ≤ 并发度；
/// 块内 join_all 首 poll 全发，请求总数恒等于文件项数（不多发不漏发）。
fn delete_file_chunks<'a, 'b>(files: &'b [&'a str]) -> Vec<&'b [&'a str]> {
    files.chunks(STAT_CONCURRENCY).collect()
}

/// CR-7 单文件删除成败日志：文案与单条 Delete 时代逐字一致，返回 (日志, 是否成功)。
/// 每项独立判定，块内某一项失败不影响其余项执行，也不写「已删除 N 项」汇总。
fn delete_file_log_msg(label: &str, path: &str, r: Result<(), String>) -> (String, bool) {
    match r {
        Ok(_) => (
            format!("[{label}] {}", tfmt("远程删除：{0}", &[&path])),
            true,
        ),
        Err(e) => (
            format!("[{label}] {}", tfmt("删除失败：{0}（{1}）", &[&path, &e])),
            false,
        ),
    }
}

async fn worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    etx: mpsc::Sender<Ev>,
    connected: Arc<AtomicBool>,
    probe: Arc<Mutex<(u64, Option<HashProbe>)>>,
    exec_spec: Arc<Mutex<Option<ExecSpec>>>,
    delete_jobs: tokio::sync::mpsc::UnboundedSender<ExecReq>,
) {
    let mut ctx: Option<Ctx> = None;
    // CR-7-S4：在途的目录删除批（exec 等待移到 exec 线程后，主 worker 只在
    // 收回执时做核对 / 回退；期间 Cmd::List / 传输等照常处理，不占死命令循环）。
    let mut pending_deletes: Vec<PendingDelete> = Vec::new();
    // G8：首个列表成功后只触发一次哈希探测（重连 Connect 时复位）。
    let mut listed_once = false;
    // 真 async recv：空闲 Pending 让出调度器，tokio::spawn 的探测任务才被轮询。
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Connect {
                host,
                port,
                user,
                auth,
                label,
                fingerprint,
            } => {
                // CR-7-S4：换主机 / 重连先作废在途目录删除，且必须给每个 tag 补发
                // 回执（否则 UI 的 del_pending 永不归零，卡「正在删除…」）。
                cancel_pending_deletes(&mut pending_deletes, &etx);
                // 重连场景：先丢弃旧会话。
                ctx = None;
                // G8 S3.1：重连/换主机清探测缓存并推进代际（作废在途探测回写）。
                {
                    let mut g = probe.lock().unwrap();
                    g.0 += 1;
                    g.1 = None;
                }
                listed_once = false;
                // G3：存档指纹审计（C# SetTrustedFingerprint 时机，连接前）。
                if let Some(fp) = &fingerprint {
                    let _ = etx.send(Ev::Log {
                        msg: format!("[{label}] {}", tfmt("信任存档指纹 {0}", &[fp])),
                    });
                }
                // G2：正在连接（C# S1.6）。
                let _ = etx.send(Ev::Log {
                    msg: format!("[{label}] {}", tfmt("正在连接 {0}:{1} …", &[&host, &port])),
                });
                let gate = FpGate::new(fingerprint, label.clone(), host.clone(), etx.clone());
                match connect_with_retry(&host, port, &user, &auth, gate, &label, &etx).await {
                    Ok((conn, root, confirmed)) => {
                        // C# S5.2：exec 命令通道信任基准含本次确认指纹；否则首次连接
                        // （存档为空）时探测/校验码的 exec 独立连接被静默门全拒。
                        set_exec_fingerprint(&exec_spec, confirmed.clone());
                        let _ = etx.send(Ev::Log {
                            msg: format!("[{label}] {}", tfmt("已连接 {0}", &[&host])),
                        });
                        let _ = etx.send(Ev::Connected {
                            label: label.clone(),
                            root,
                            confirmed,
                        });
                        ctx = Some(Ctx { conn, label });
                        connected.store(true, Ordering::Relaxed);
                    }
                    Err(msg) => {
                        connected.store(false, Ordering::Relaxed);
                        let _ = etx.send(Ev::Failed {
                            what: "连接",
                            msg,
                            tag: None,
                        });
                    }
                }
            }
            Cmd::List { tag, path } => {
                if let Some(c) = &mut ctx {
                    match sftp_op(c.conn.sftp.read_dir(&path)).await {
                        Ok(list) => {
                            let _ = etx.send(Ev::Listing {
                                tag,
                                path,
                                entries: to_entries(list),
                            });
                            // G8 S6.1：首个列表成功后 fire-and-forget 探测服务端哈希
                            // 能力（快照当前代际，重连推进后回写即被丢弃）。
                            if !listed_once {
                                listed_once = true;
                                let cur = probe.lock().unwrap().0;
                                tokio::spawn(hash_probe_run(
                                    cur,
                                    probe.clone(),
                                    etx.clone(),
                                    exec_spec.clone(),
                                ));
                            }
                        }
                        Err(e) => {
                            let _ = etx.send(Ev::Failed {
                                what: "列目录",
                                msg: e,
                                tag: Some(tag),
                            });
                        }
                    }
                }
            }
            Cmd::Stat { path, req } => {
                let reply = match &ctx {
                    Some(c) => match sftp_op(c.conn.sftp.metadata(&path)).await {
                        Ok(meta) => Ev::Stat {
                            req,
                            path,
                            exists: true,
                            size: Some(meta.len()),
                            mtime: meta.mtime.map(|t| t as i64),
                        },
                        Err(_) => Ev::Stat {
                            req,
                            path,
                            exists: false,
                            size: None,
                            mtime: None,
                        },
                    },
                    None => Ev::Stat {
                        req,
                        path,
                        exists: false,
                        size: None,
                        mtime: None,
                    },
                };
                let _ = etx.send(reply);
            }
            Cmd::StatBatch { paths, req } => {
                // T20-G③：块内并发（join_all 首 poll 全发，块耗时 ≈ 单次 RTT 而非
                // n×RTT），逐条走 sftp_op 保单条超时语义；无会话时全部按不存在回。
                let stat_ev =
                    |path: String, r: Result<russh_sftp::protocol::FileAttributes, String>| match r
                    {
                        Ok(meta) => Ev::Stat {
                            req,
                            path,
                            exists: true,
                            size: meta.size,
                            mtime: meta.mtime.map(|t| t as i64),
                        },
                        Err(_) => Ev::Stat {
                            req,
                            path,
                            exists: false,
                            size: None,
                            mtime: None,
                        },
                    };
                match &ctx {
                    Some(c) => {
                        let sftp = &c.conn.sftp;
                        for chunk in paths.chunks(STAT_CONCURRENCY) {
                            let futs = chunk.iter().map(|p| async move {
                                (p.clone(), sftp_op(sftp.metadata(p)).await)
                            });
                            for (path, r) in futures_util::future::join_all(futs).await {
                                let _ = etx.send(stat_ev(path, r));
                            }
                        }
                    }
                    None => {
                        for path in paths {
                            let _ = etx.send(stat_ev(path, Err("无会话".to_owned())));
                        }
                    }
                }
            }
            Cmd::ReadFile { path, req, max } => {
                let reply = match &mut ctx {
                    Some(c) => read_file_with_mtime(&c.conn.sftp, &path, max)
                        .await
                        .map(|(data, mtime)| Ev::FileData {
                            req,
                            result: Ok(FileData { data, mtime }),
                        })
                        .unwrap_or_else(|e| Ev::FileData {
                            req,
                            result: Err(e),
                        }),
                    None => Ev::FileData {
                        req,
                        result: Err(t("未连接远程主机").to_owned()),
                    },
                };
                let _ = etx.send(reply);
            }
            Cmd::WriteFile { path, req, data } => {
                let reply = match &mut ctx {
                    Some(c) => write_file_with_mtime(&c.conn.sftp, &path, &data)
                        .await
                        .map(|mtime| Ev::FileWritten {
                            req,
                            path: path.clone(),
                            result: Ok(mtime),
                        })
                        .unwrap_or_else(|e| Ev::FileWritten {
                            req,
                            path: path.clone(),
                            result: Err(e),
                        }),
                    None => Ev::FileWritten {
                        req,
                        path: path.clone(),
                        result: Err(t("未连接远程主机").to_owned()),
                    },
                };
                let _ = etx.send(reply);
            }
            Cmd::DeleteBatch { tag, items } => {
                // CR-7：文件项按 STAT_CONCURRENCY 分块并发（对齐 StatBatch 范式），
                // 目录项不进并发块；逐项独立走 sftp_op 超时 + 独立 Ev::Log 成败留痕，
                // 整批只回一条 Ev::Deleted（UI 侧 del_pending 按批计数）。
                if let Some(c) = &mut ctx {
                    let label = c.label.clone();
                    let (files, dirs) = split_delete_items(&items);
                    let sftp = &c.conn.sftp;
                    for chunk in delete_file_chunks(&files) {
                        let futs = chunk
                            .iter()
                            .map(|p| async move { (*p, sftp_op(sftp.remove_file(*p)).await) });
                        for (path, r) in futures_util::future::join_all(futs).await {
                            let (msg, _) = delete_file_log_msg(&label, path, r);
                            let _ = etx.send(Ev::Log { msg });
                        }
                    }
                    if dirs.is_empty() {
                        let _ = etx.send(Ev::Deleted { tag });
                    } else if crate::settings::with(|s| s.remote_delete_exec) {
                        // CR-7-S4：exec 等待不留在主 worker —— 首个目录送 exec 线程
                        // （已认证连接缓存在该线程内复用），本帧即返回命令循环，
                        // Cmd::List / 传输照常处理；回执到 Cmd::ExecDone 再核对 / 回退。
                        let first = dirs[0].to_owned();
                        let _ = delete_jobs.send(ExecReq::Delete {
                            tag,
                            cmd: delete_dir_exec_cmd(&first),
                        });
                        pending_deletes.push(PendingDelete {
                            tag,
                            label,
                            dirs: dirs.iter().map(|p| (*p).to_owned()).collect(),
                            next: 0,
                        });
                    } else {
                        // 「服务端命令删除」关：仍走 SFTP 递归（无 exec 等待，保持原语义）。
                        for path in dirs {
                            let _ = delete_dir_recursive(sftp, path, &label, &etx).await;
                        }
                        let _ = etx.send(Ev::Deleted { tag });
                    }
                } else {
                    let _ = etx.send(Ev::Deleted { tag });
                }
            }
            Cmd::ExecDone { tag, result } => {
                let Some(pos) = pending_deletes.iter().position(|p| p.tag == tag) else {
                    continue; // 换主机 / 断连后迟到的回执：已作废，不碰新会话
                };
                let (path, label) = {
                    let p = &pending_deletes[pos];
                    (p.dirs[p.next].clone(), p.label.clone())
                };
                if let Some(c) = &mut ctx {
                    // 四项语义全部保留：shell_quote 已在发作业时用；退出码 0 才成功；
                    // 非 0 / 超时 / 断连先经 SFTP 核对迟到删净；仍存在则回退递归。
                    delete_dir_settle(&c.conn.sftp, &path, &label, &etx, result).await;
                } else {
                    let _ = etx.send(Ev::Log {
                        msg: format!(
                            "[{label}] {}",
                            tfmt("删除失败：{0}（{1}）", &[&path, &t("未连接远程主机")])
                        ),
                    });
                }
                match advance_pending_delete(&mut pending_deletes[pos]) {
                    PendingStep::Next { tag, path } => {
                        let _ = delete_jobs.send(ExecReq::Delete {
                            tag,
                            cmd: delete_dir_exec_cmd(&path),
                        });
                    }
                    PendingStep::Done { tag } => {
                        pending_deletes.remove(pos);
                        let _ = etx.send(Ev::Deleted { tag });
                    }
                }
            }
            Cmd::Mkdir { tag, path } => {
                // 新建目录（SSH_FXP_MKDIR）；成败日志与 C# CreateRemoteEntryAsync
                // 的 catch 吞错仅留痕一致，完成统一回 Ev::Created 触发重列。
                if let Some(c) = &mut ctx {
                    let label = c.label.clone();
                    match sftp_op(c.conn.sftp.create_dir(&path)).await {
                        Ok(_) => {
                            let _ = etx.send(Ev::Log {
                                msg: format!("[{label}] {}", tfmt("远程新建文件夹：{0}", &[&path])),
                            });
                        }
                        Err(e) => {
                            let _ = etx.send(Ev::Log {
                                msg: format!(
                                    "[{label}] {}",
                                    tfmt("新建文件夹失败：{0}（{1}）", &[&path, &e])
                                ),
                            });
                        }
                    }
                } else {
                    let _ = etx.send(Ev::Log {
                        msg: t("未连接远程主机").to_owned(),
                    });
                }
                let _ = etx.send(Ev::Created { tag });
            }
            Cmd::CreateFile { tag, path } => {
                // 新建空文件：CREATE|WRITE 打开即关（0 字节落盘，C# File.Create 同义）。
                if let Some(c) = &mut ctx {
                    let label = c.label.clone();
                    match sftp_op(c.conn.sftp.open_with_flags(
                        &path,
                        russh_sftp::protocol::OpenFlags::CREATE
                            | russh_sftp::protocol::OpenFlags::WRITE,
                    ))
                    .await
                    {
                        Ok(f) => {
                            drop(f);
                            let _ = etx.send(Ev::Log {
                                msg: format!("[{label}] {}", tfmt("远程新建文件：{0}", &[&path])),
                            });
                        }
                        Err(e) => {
                            let _ = etx.send(Ev::Log {
                                msg: format!(
                                    "[{label}] {}",
                                    tfmt("新建文件失败：{0}（{1}）", &[&path, &e])
                                ),
                            });
                        }
                    }
                } else {
                    let _ = etx.send(Ev::Log {
                        msg: t("未连接远程主机").to_owned(),
                    });
                }
                let _ = etx.send(Ev::Created { tag });
            }
            Cmd::Rename {
                tag,
                path,
                new_path,
            } => {
                // 重命名（SSH_FXP_RENAME，C# SftpClient.RenameAsync FileView
                // :2560-2562）；成败留终端日志，完成统一回 Ev::Created 触发重列。
                if let Some(c) = &mut ctx {
                    let label = c.label.clone();
                    match sftp_op(c.conn.sftp.rename(&path, &new_path)).await {
                        Ok(_) => {
                            let _ = etx.send(Ev::Log {
                                msg: format!(
                                    "[{label}] {}",
                                    tfmt("远程重命名：{0} → {1}", &[&path, &new_path])
                                ),
                            });
                        }
                        Err(e) => {
                            let _ = etx.send(Ev::Log {
                                msg: format!(
                                    "[{label}] {}",
                                    tfmt("重命名失败：{0}（{1}）", &[&path, &e])
                                ),
                            });
                        }
                    }
                } else {
                    let _ = etx.send(Ev::Log {
                        msg: t("未连接远程主机").to_owned(),
                    });
                }
                let _ = etx.send(Ev::Created { tag });
            }
            Cmd::Disconnect => {
                // CR-7-S4：断连同样作废在途目录删除并补发回执。
                cancel_pending_deletes(&mut pending_deletes, &etx);
                if let Some(c) = &ctx {
                    let _ = etx.send(Ev::Log {
                        msg: format!("[{}] {}", c.label, t("已断开")),
                    });
                }
                ctx = None; // drop → 关连接
                connected.store(false, Ordering::Relaxed);
                let _ = etx.send(Ev::Disconnected);
            }
        }
    }
}

/// 连接 + 认证 + SFTP 子系统 + 规范化起始目录；返回 (连接, 起始目录, 本次确认指纹)。
async fn establish(
    host: &str,
    port: u16,
    user: &str,
    auth: &Auth,
    gate: FpGate,
) -> Result<(SftpConn, String, Option<String>), ConnErr> {
    let confirmed = gate.confirmed.clone();
    let conn = connect_sftp_with(host, port, user, auth, Handler { fp: gate }).await?;
    let root = sftp_op(conn.sftp.canonicalize("."))
        .await
        .map_err(ConnErr::Io)?;
    Ok((conn, root, confirmed.lock().unwrap().clone()))
}

/// 带重试的连接闭环（G5，C# S3.2-3.4）：maxAttempts = 1 + MaxRetries；
/// 认证失败与用户拒绝指纹不重试；重试复用本次已确认指纹（gate 跨尝试共享）；
/// 每次失败打重试日志并等待 RetryDelay 秒；终局归一「连接失败：{0}」。
async fn connect_with_retry(
    host: &str,
    port: u16,
    user: &str,
    auth: &Auth,
    gate: FpGate,
    label: &str,
    etx: &mpsc::Sender<Ev>,
) -> Result<(SftpConn, String, Option<String>), String> {
    let max_retries = crate::settings::with(|v| v.max_retries).max(0);
    let attempts = 1 + max_retries as u32;
    let delay = crate::settings::with(|v| v.retry_delay).max(0) as u64;
    for attempt in 1..=attempts {
        match establish(host, port, user, auth, gate.clone()).await {
            Ok(v) => return Ok(v),
            // 认证失败不重试（C# SshAuthenticationException 同口径），终局归一。
            Err(ConnErr::Auth(msg)) => return Err(tfmt("连接失败：{0}", &[&msg])),
            Err(ConnErr::Io(msg)) => {
                // 用户拒绝指纹：不重试，文案归一（C# S3.6-3）。
                if gate.denied() {
                    return Err(t("你拒绝了本次主机指纹，连接中止").to_owned());
                }
                if attempt < attempts {
                    let _ = etx.send(Ev::Log {
                        msg: format!(
                            "[{label}] {}",
                            tfmt(
                                "连接失败：{0}，{1} 秒后进行第 {2}/{3} 次重试 …",
                                &[&msg, &delay, &attempt, &max_retries]
                            )
                        ),
                    });
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                } else {
                    return Err(tfmt("连接失败：{0}", &[&msg]));
                }
            }
        }
    }
    unreachable!("retry loop")
}

/// POSIX 单引号转义（C# ShellQuote 同式：' → '\''）。exec 命令拼路径统一走此
/// （G10 删除、G12 校验、G13 服务端 cp、G8 探测、命令窗、校验菜单共用）。
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// CR-7-S4 目录 exec 命令：shell_quote 与 `rm -rf --` 语义逐字保留（G10）。
fn delete_dir_exec_cmd(path: &str) -> String {
    format!("rm -rf -- {}", shell_quote(path))
}

/// 退出码 0 才成功（防 Windows 服务器 shell 下 rm 非法被误判成功）；
/// 非零 / 超时 / 断连一律走 SFTP 迟到删净核对，不写假成功。
fn exec_delete_succeeded(result: &Result<(i32, String, String), String>) -> bool {
    matches!(result, Ok((0, _, _)))
}

/// CR-7-S4 在途目录删除批：主 worker 等 Cmd::ExecDone 时靠它记住整批上下文。
struct PendingDelete {
    tag: u64,
    label: String,
    dirs: Vec<String>,
    /// 当前已发出 exec 作业的目录下标。
    next: usize,
}

enum PendingStep {
    Next { tag: u64, path: String },
    Done { tag: u64 },
}

/// 推进到下一目录；整批结束才给出「发 Ev::Deleted」的信号。纯函数，便于钉住
/// 「20 目录 = 19 次续发 + 1 次结算」与「发作业当帧不结算」。
fn advance_pending_delete(p: &mut PendingDelete) -> PendingStep {
    p.next += 1;
    if p.next < p.dirs.len() {
        PendingStep::Next {
            tag: p.tag,
            path: p.dirs[p.next].clone(),
        }
    } else {
        PendingStep::Done { tag: p.tag }
    }
}

/// 换主机 / 重连 / 断连时作废在途批：每个 tag 补发一条 Ev::Deleted，
/// 防 UI 的 del_pending 永不归零卡在「正在删除…」。
fn cancel_pending_deletes(pending: &mut Vec<PendingDelete>, etx: &mpsc::Sender<Ev>) {
    for p in pending.drain(..) {
        let _ = etx.send(Ev::Deleted { tag: p.tag });
    }
}

/// CR-7-S4 目录 exec 结果结算（原 delete_dir_via_exec 的核对 / 回退段逐字保留）。
/// 严格四项：shell_quote（发作业侧）、退出码 0 才成功、NoSuchFile 迟到删净核对、
/// 失败回退 SFTP 递归。exec 的等待已移到 exec 线程，这里只跑主 worker 的 SFTP 操作。
async fn delete_dir_settle(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
    label: &str,
    etx: &mpsc::Sender<Ev>,
    result: Result<(i32, String, String), String>,
) {
    if exec_delete_succeeded(&result) {
        let _ = etx.send(Ev::Log {
            msg: format!("[{label}] {}", tfmt("远程删除：{0}", &[&path])),
        });
        return;
    }
    // 回退前核对「迟到删净」：rm 可能已完成，只是回执丢失 / 超时。
    // 不经 sftp_op（此处错误需保留类型判「不存在」），OperationTimeout 手工罩顶。
    let secs = operation_timeout_secs();
    let meta = if secs > 0 {
        tokio::time::timeout(Duration::from_secs(secs as u64), sftp.metadata(path)).await
    } else {
        Ok(sftp.metadata(path).await)
    };
    match meta {
        // 目录已不存在 = 迟到删净成功（rm 已完成，只是回执丢失 / 超时）。
        Ok(Err(russh_sftp::client::error::Error::Status(s)))
            if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile =>
        {
            let _ = etx.send(Ev::Log {
                msg: format!("[{label}] {}", tfmt("远程删除：{0}", &[&path])),
            });
        }
        // 仍存在 → 打回退日志并回退 SFTP 递归。
        Ok(Ok(_)) => {
            let _ = etx.send(Ev::Log {
                msg: format!(
                    "[{label}] {}",
                    t("服务端命令删除失败，已回退 SFTP 递归删除")
                ),
            });
            let _ = delete_dir_recursive(sftp, path, label, etx).await;
        }
        // 核对失败（断连 / 超时等）：静默回退，由 SFTP 递归路径给出明确报错。
        _ => {
            let _ = delete_dir_recursive(sftp, path, label, etx).await;
        }
    }
}

/// CR-7 递归删除结果累计：整树只发一条汇总日志，不再逐子文件刷屏。
#[derive(Default)]
struct DeleteTreeReport {
    /// 已成功删除的条目数（含空目录本身）。
    ok: usize,
    /// 首错 (路径, 原因)：首错即止，失败路径仍逐条可定位。
    first_err: Option<(String, String)>,
}

/// 整树一条汇总（CR-7）：全成功带成功数；首错带失败路径 + 原因 + 首错前成功数。
/// 部分失败绝不出现「已删除 N 项」的假成功汇总；全部复用既有中英键，零新增文案。
fn delete_tree_summary_msg(label: &str, dir: &str, r: &DeleteTreeReport) -> String {
    match &r.first_err {
        None => format!(
            "[{label}] {}（{}）",
            tfmt("远程删除：{0}", &[&dir]),
            tfmt("已删除 {0} 项", &[&r.ok])
        ),
        Some((path, err)) => format!(
            "[{label}] {}；{}（{}）",
            tfmt("远程删除：{0}", &[&dir]),
            tfmt("删除失败：{0}（{1}）", &[&path, &err]),
            tfmt("{0} 项", &[&r.ok])
        ),
    }
}

/// SFTP 递归删除（C# DeleteRemoteDirRecursive 同语义：子文件逐个删、目录空后删、
/// 符号链接按文件处理不跟随 —— C5）。首错即止；CR-7 起每棵树只在结束时发一条
/// 汇总 Ev::Log（成功数 / 首错路径），日志条数与子文件数无关。
async fn delete_dir_recursive(
    sftp: &russh_sftp::client::SftpSession,
    dir: &str,
    label: &str,
    etx: &mpsc::Sender<Ev>,
) -> Result<(), ()> {
    let mut report = DeleteTreeReport::default();
    let r = delete_dir_recursive_into(sftp, dir, &mut report).await;
    let _ = etx.send(Ev::Log {
        msg: delete_tree_summary_msg(label, dir, &report),
    });
    r
}

async fn delete_dir_recursive_into(
    sftp: &russh_sftp::client::SftpSession,
    dir: &str,
    report: &mut DeleteTreeReport,
) -> Result<(), ()> {
    let rd = match sftp_op(sftp.read_dir(dir)).await {
        Ok(rd) => rd,
        Err(e) => {
            report.first_err = Some((dir.to_owned(), e));
            return Err(());
        }
    };
    for de in rd {
        let name = de.file_name();
        let child = if dir == "/" {
            format!("/{name}")
        } else {
            format!("{dir}/{name}")
        };
        if de.metadata().is_dir() {
            Box::pin(delete_dir_recursive_into(sftp, &child, report)).await?;
        } else if let Err(e) = sftp_op(sftp.remove_file(&child)).await {
            report.first_err = Some((child, e));
            return Err(());
        } else {
            report.ok += 1;
        }
    }
    match sftp_op(sftp.remove_dir(dir)).await {
        Ok(_) => {
            report.ok += 1;
            Ok(())
        }
        Err(e) => {
            report.first_err = Some((dir.to_owned(), e));
            Err(())
        }
    }
}

// ── exec 通道（独立线程；C# §2 命令窗命令路径）──────────────

fn exec_run(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ExecReq>,
    mut del_rx: tokio::sync::mpsc::UnboundedReceiver<ExecReq>,
    etx: mpsc::Sender<Ev>,
    exec_spec: Arc<Mutex<Option<ExecSpec>>>,
    worker_tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime (exec)");
    rt.block_on(async move {
        // CR-7-S4：目录删除（最长 120s）单独一个 task，不与命令窗排队；
        // 两条路径各自持一份缓存，Handle 只在创建它的任务内复用 / 丢弃，
        // 不跨 runtime 传（russh 后台任务绑定创建它的 runtime）。
        // 20 个目录由主 worker 逐个发、逐个等回执，故删除缓存只建 1 次连接。
        let del_spec = exec_spec.clone();
        tokio::spawn(async move {
            let mut cached: Option<(ExecSpec, russh::client::Handle<Handler>)> = None;
            while let Some(job) = del_rx.recv().await {
                let ExecReq::Delete { tag, cmd } = job else {
                    continue;
                };
                let current = del_spec.lock().unwrap().clone();
                let outcome = run_exec_job(&cmd, 120, current, &mut cached).await;
                let _ = worker_tx.send(Cmd::ExecDone {
                    tag,
                    result: outcome,
                });
            }
        });
        let mut cached: Option<(ExecSpec, russh::client::Handle<Handler>)> = None;
        while let Some(job) = rx.recv().await {
            let ExecReq::Command { req, cmd } = job else {
                continue;
            };
            let current = exec_spec.lock().unwrap().clone();
            let outcome = run_exec_job(&cmd, EXEC_TIMEOUT.as_secs(), current, &mut cached).await;
            let (exit, out, err) = match outcome {
                Ok(v) => v,
                Err(msg) => (-1, String::new(), msg),
            };
            let _ = etx.send(Ev::Exec {
                req,
                exit,
                out,
                err,
            });
        }
    });
}

/// 单条 exec 作业：罩顶超时 + 复用 / 重建缓存 + 失败丢缓存。
/// 命令窗 10s / 目录删除 120s / 探测 10s 共用，语义一致。
async fn run_exec_job(
    cmd: &str,
    secs: u64,
    current: Option<ExecSpec>,
    cached: &mut Option<(ExecSpec, russh::client::Handle<Handler>)>,
) -> Result<(i32, String, String), String> {
    let outcome =
        match tokio::time::timeout(Duration::from_secs(secs), exec_cached(cmd, current, cached))
            .await
        {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(msg)) => Err(msg),
            Err(_) => {
                *cached = None; // 超时后连接可能半死，丢弃重建
                Err(tfmt("操作超时（{0} 秒）", &[&secs]))
            }
        };
    if should_drop_exec_cache(&outcome) {
        *cached = None; // 建连 / 认证 / 通道失败：下一次作业重建
    }
    outcome
}

/// 缓存命中判据（CR-7-S4 红线）：连接档案逐字段相同才复用；换主机 / 重连 /
/// 改指纹 / 档案清空立即失效，绝不拿旧主机的已认证连接去删新主机目录。
fn exec_cache_usable(cached: Option<&ExecSpec>, current: Option<&ExecSpec>) -> bool {
    cached.is_some() && cached == current
}

/// 连接级失败才丢缓存；命令非零退出是服务端结果，连接仍可复用。
fn should_drop_exec_cache(result: &Result<(i32, String, String), String>) -> bool {
    result.is_err()
}

/// 复用缓存连接跑一条命令：档案变了 / 缓存为空才新建连接并认证。
async fn exec_cached(
    cmd: &str,
    current: Option<ExecSpec>,
    cached: &mut Option<(ExecSpec, russh::client::Handle<Handler>)>,
) -> Result<(i32, String, String), String> {
    if !exec_cache_usable(cached.as_ref().map(|(s, _)| s), current.as_ref()) {
        *cached = None;
    }
    let spec = current.ok_or_else(|| t("未连接远程主机").to_owned())?;
    if cached.is_none() {
        let handle = exec_connect(&spec).await?;
        *cached = Some((spec, handle));
    }
    exec_on_channel(&cached.as_ref().unwrap().1, cmd).await
}

/// 新建 + 认证一条 exec 独立连接（原 exec_once 的建连段，语义逐字保留）。
async fn exec_connect(spec: &ExecSpec) -> Result<russh::client::Handle<Handler>, String> {
    let config = russh::client::Config {
        nodelay: true,
        inactivity_timeout: None,
        keepalive_interval: keepalive_interval(),
        ..Default::default()
    };
    let addr = (spec.host.clone(), spec.port);
    // S5.2：exec 独立连接静默信任（仅存档精确匹配才接受，fail-closed，无弹窗无日志）。
    let handler = Handler {
        fp: FpGate::silent(spec.fingerprint.clone()),
    };
    let mut handle = match connect_timeout() {
        Some(t) => tokio::time::timeout(t, russh::client::connect(Arc::new(config), addr, handler))
            .await
            .map_err(|_| tfmt("连接超时（{0} 秒）", &[&t.as_secs()]))?,
        None => russh::client::connect(Arc::new(config), addr, handler).await,
    }
    .map_err(|e| e.to_string())?;
    let authed = match &spec.auth {
        Auth::Password(password) => {
            auth_with_timeout(|| handle.authenticate_password(&spec.user, password))
                .await
                .map_err(String::from)?
        }
        Auth::KeyFile(path) => {
            let key = russh::keys::load_secret_key(path, None)
                .map_err(|e| tfmt("私钥读取失败：{0}", &[&e]))?;
            let hash = key
                .algorithm()
                .is_rsa()
                .then_some(russh::keys::HashAlg::Sha256);
            let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            auth_with_timeout(|| handle.authenticate_publickey(&spec.user, key))
                .await
                .map_err(String::from)?
        }
    };
    if !authed.success() {
        return Err(t("认证失败").to_owned());
    }
    Ok(handle)
}

/// 已建立会话上开 exec 通道跑命令：channel_open_session → exec → wait 收流，
/// 返回 (退出码, stdout, stderr)；通道开/执行失败归 Err。wait 无界，调用方
/// 按需罩超时（exec_run 的 EXEC_TIMEOUT / 目录删除 120s；G12 校验走 VERIFY_TIMEOUT）。
async fn exec_on_channel(
    handle: &russh::client::Handle<Handler>,
    cmd: &str,
) -> Result<(i32, String, String), String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| e.to_string())?;
    channel.exec(true, cmd).await.map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut exit = -1i32;
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Data { data }) => out.extend_from_slice(&data),
            Some(russh::ChannelMsg::ExtendedData { data, .. }) => err.extend_from_slice(&data),
            Some(russh::ChannelMsg::ExitStatus { exit_status }) => exit = exit_status as i32,
            Some(russh::ChannelMsg::Eof) => {}
            Some(russh::ChannelMsg::Close) => break,
            Some(_) => {}
            None => break,
        }
    }
    Ok((
        exit,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    ))
}

/// exec 通道连接档案槽（per-Ssh 实例；双会话改造：原全局 static EXEC_PROFILE
/// 改为实例字段，由 `Ssh::set_exec_profile` 注入，worker 确认指纹后回写）。
/// exec 独立连接用的连接规格。
#[derive(Clone, PartialEq)]
pub struct ExecSpec {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    /// 存档指纹（SHA256 base64 无前缀；exec 静默门仅精确匹配才接受，C# S5.2）。
    pub fingerprint: Option<String>,
}

impl Ssh {
    /// 注入 exec 通道连接档案（连接提交时调用；None = 清除）。
    pub fn set_exec_profile(&self, p: Option<ExecSpec>) {
        *self.exec_spec.lock().unwrap() = p;
    }
}

/// 主连接确认指纹回写 exec 档案（C# S5.2：命令通道信任基准 = 存档 ∪ 本次确认；
/// 否则首次连接存档为空时，探测/校验码的 exec 独立连接被静默门 fail-closed 全拒）。
fn set_exec_fingerprint(exec_spec: &Arc<Mutex<Option<ExecSpec>>>, fp: Option<String>) {
    let mut g = exec_spec.lock().unwrap();
    if let (Some(spec), Some(f)) = (g.as_mut(), fp) {
        spec.fingerprint = Some(f);
    }
}

// ── G8 服务端哈希能力探测（C# S6；首个列表成功后 fire-and-forget）──

/// 探测单命令：在探测任务自己的缓存连接上跑（两次探测复用一条连接），
/// EXEC_TIMEOUT 罩顶；超时 / 错误 / 非零退出归一 Err。不进 exec 线程的作业队列
/// —— 长删除排队超时会被误报成「Windows 服务器」的假诊断。
async fn probe_once(
    cmd: &str,
    exec_spec: &Arc<Mutex<Option<ExecSpec>>>,
    cached: &mut Option<(ExecSpec, russh::client::Handle<Handler>)>,
) -> Result<(i32, String), String> {
    let current = exec_spec.lock().unwrap().clone();
    let (exit, out, err) = run_exec_job(cmd, EXEC_TIMEOUT.as_secs(), current, cached).await?;
    if exit != 0 {
        let e = err.trim();
        return Err(if e.is_empty() {
            format!("exit {exit}")
        } else {
            e.to_owned()
        });
    }
    Ok((exit, out))
}

/// 代际未变才回写快照（重连/换主机已推进代际则丢弃，S3.1）。
fn probe_store(cur: u64, probe: &Mutex<(u64, Option<HashProbe>)>, v: Option<HashProbe>) {
    let mut g = probe.lock().unwrap();
    if g.0 == cur {
        g.1 = v;
    }
}

/// 探测流程（C# S6.2-6.4）：先 POSIX（`echo $env:OS`；输出含 Windows_NT/$env
/// 或探测失败按非 POSIX）→ 非 POSIX 打禁用文案即止；POSIX 后一次往返探测
/// sha256sum / sha512sum / openssl SM3，解析 3 位。日志前缀「[命令通道] 」
/// （英文态 [cmd]），走 i18n 跟随应用语言。
async fn hash_probe_run(
    cur: u64,
    probe: Arc<Mutex<(u64, Option<HashProbe>)>>,
    etx: mpsc::Sender<Ev>,
    exec_spec: Arc<Mutex<Option<ExecSpec>>>,
) {
    // 探测任务自己的缓存：两次探测复用一条连接，随任务结束释放。
    let mut cached: Option<(ExecSpec, russh::client::Handle<Handler>)> = None;
    let posix = match probe_once("echo $env:OS", &exec_spec, &mut cached).await {
        Ok((_, out)) => {
            let out = out.trim();
            !(out.contains("Windows_NT") || out.contains("$env"))
        }
        // S6.2：探测失败按非 POSIX。
        Err(_) => false,
    };
    if !posix {
        let _ = etx.send(Ev::Log {
            msg: t("[命令通道] Windows 服务器, 暂不支持服务端哈希, 校验码菜单已禁用").to_owned(),
        });
        probe_store(cur, &probe, None);
        return;
    }
    let cmd = "a=0; b=0; c=0; command -v sha256sum >/dev/null 2>&1 && a=1; \
               command -v sha512sum >/dev/null 2>&1 && b=1; \
               openssl dgst -sm3 /dev/null >/dev/null 2>&1 && c=1; echo \"$a$b$c\"";
    let h = match probe_once(cmd, &exec_spec, &mut cached).await {
        Ok((_, out)) => {
            let s = out.trim();
            if s.len() == 3 && s.bytes().all(|b| b == b'0' || b == b'1') {
                HashProbe {
                    posix: true,
                    sha256: s.as_bytes()[0] == b'1',
                    sha512: s.as_bytes()[1] == b'1',
                    sm3: s.as_bytes()[2] == b'1',
                }
            } else {
                let _ = etx.send(Ev::Log {
                    // 注意：本机拉流回退未实现（C# 原文案承诺了不存在的能力），
                    // 探测失败 = 校验跳过，不承诺回退。
                    msg: tfmt(
                        "[命令通道] 不可用, 校验跳过（本机回退未实现）: 解析异常输出 {0}",
                        &[&format!("{s:?}")],
                    ),
                });
                probe_store(cur, &probe, None);
                return;
            }
        }
        Err(e) => {
            let _ = etx.send(Ev::Log {
                msg: tfmt("[命令通道] 不可用, 校验跳过（本机回退未实现）: {0}", &[&e]),
            });
            probe_store(cur, &probe, None);
            return;
        }
    };
    let mark = |ok| if ok { t("可用") } else { t("不可用") };
    let _ = etx.send(Ev::Log {
        msg: tfmt(
            "[命令通道] 服务端哈希探测: sha256sum={0} sha512sum={1} SM3(openssl)={2}",
            &[&mark(h.sha256), &mark(h.sha512), &mark(h.sm3)],
        ),
    });
    probe_store(cur, &probe, Some(h));
}

// ── 远程编辑文件读写（T7；256KB 块同传输口径）────────────────

/// 读远端文件（max 字节上限截断）+ 写前 mtime（远程编辑冲突检测基线）。
async fn read_file_with_mtime(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
    max: u64,
) -> Result<(Vec<u8>, Option<i64>), String> {
    let meta = sftp_op(sftp.metadata(path))
        .await
        .map_err(|e| tfmt("读取失败：{0}", &[&e]))?;
    let mtime = meta.mtime.map(|t| t as i64);
    let size = meta.len();
    let cap = size.min(max);
    let mut file = sftp_op(sftp.open(path))
        .await
        .map_err(|e| tfmt("读取失败：{0}", &[&e]))?;
    use tokio::io::AsyncReadExt;
    let mut data = Vec::with_capacity(cap as usize);
    let mut buf = vec![0u8; 256 * 1024];
    let mut left = cap;
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = sftp_op(file.read(&mut buf[..want]))
            .await
            .map_err(|e| tfmt("读取失败：{0}", &[&e]))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        left -= n as u64;
    }
    Ok((data, mtime))
}

/// 整体替换写远端文件（CREATE|TRUNCATE|WRITE，256KB 块）+ 写后 mtime。
async fn write_file_with_mtime(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
    data: &[u8],
) -> Result<Option<i64>, String> {
    let mut file = sftp_op(sftp.open_with_flags(
        path,
        russh_sftp::protocol::OpenFlags::CREATE
            | russh_sftp::protocol::OpenFlags::TRUNCATE
            | russh_sftp::protocol::OpenFlags::WRITE,
    ))
    .await
    .map_err(|e| tfmt("写入失败：{0}", &[&e]))?;
    use tokio::io::AsyncWriteExt;
    for chunk in data.chunks(256 * 1024) {
        sftp_op(file.write_all(chunk))
            .await
            .map_err(|e| tfmt("写入失败：{0}", &[&e]))?;
    }
    // 收尾 flush：drain 所有 FXP_WRITE 的 ACK。File::Drop 只 close_nowait，
    // 不 flush 则写错误被静默吞掉、假报「已回传」（只读远端必须在此报失败）。
    file.flush()
        .await
        .map_err(|e| tfmt("写入失败：{0}", &[&e]))?;
    let meta = sftp_op(sftp.metadata(path))
        .await
        .map_err(|e| tfmt("写入后取元数据失败：{0}", &[&e]))?;
    Ok(meta.mtime.map(|t| t as i64))
}

/// SFTP 目录迭代 → UI 条目（展示口径对齐 C# FileEntry；"."/".." 已由库过滤）。
fn to_entries(rd: russh_sftp::client::fs::ReadDir) -> Vec<Entry> {
    rd.map(|de| {
        let name = de.file_name();
        let meta = de.metadata();
        let is_dir = meta.is_dir();
        let size_bytes = if is_dir { 0 } else { meta.len() };
        let perms = perms_str(meta.permissions, is_dir);
        let modified = meta
            .mtime
            .map(|t| crate::local::fmt_mtime_unix(t as i64))
            .unwrap_or_default();
        let kind = if is_dir {
            t("文件夹").to_owned()
        } else {
            crate::local::kind_of(&name)
        };
        let size = if is_dir {
            String::new()
        } else {
            crate::local::format_size(size_bytes)
        };
        Entry {
            name,
            is_dir,
            size,
            kind,
            modified,
            perms,
            size_bytes,
            mtime_secs: meta.mtime.map(|t| t as i64).unwrap_or(0),
        }
    })
    .collect()
}

/// 权限位 → "drwxr-xr-x" 符号串（无权限属性用 "-" 占位）。
fn perms_str(mode: Option<u32>, is_dir: bool) -> String {
    let mut s = String::with_capacity(10);
    s.push(if is_dir { 'd' } else { '-' });
    if let Some(m) = mode {
        for shift in [6, 3, 0] {
            let tri = (m >> shift) & 7;
            s.push(if tri & 4 != 0 { 'r' } else { '-' });
            s.push(if tri & 2 != 0 { 'w' } else { '-' });
            s.push(if tri & 1 != 0 { 'x' } else { '-' });
        }
    } else {
        s.push_str("---------");
    }
    s
}

// ============================================================================
// POSIX shell 单引号转义测试 —— 翻译自 C# ShellQuoteTests（同文件内）
// exec 命令拼接用户可控路径前的唯一入口（T12 与 rm -rf 共用）。
//
// 集成方式：把整个 `mod shell_quote_tests` 追加到 ssh.rs 文件末尾。
//   运行：cargo test    预期：shell_quote_tests 2 passed（9 处断言）
// ============================================================================

#[cfg(test)]
mod shell_quote_tests {
    use super::shell_quote;

    #[test]
    fn neutralizes_injection() {
        assert_eq!(shell_quote("/var/www"), "'/var/www'");
        assert_eq!(shell_quote("/var/my site"), "'/var/my site'"); // 空格
        assert_eq!(shell_quote("/var/a'b"), "'/var/a'\\''b'"); // 单引号: 关闭-转义-重开
        assert_eq!(shell_quote("/var/a;rm -rf /"), "'/var/a;rm -rf /'"); // 分号命令串: 引号内无效
        assert_eq!(shell_quote("/var/$(id)"), "'/var/$(id)'"); // 命令替换: 单引号不展开
        assert_eq!(shell_quote("/var/a\"b"), "'/var/a\"b'"); // 双引号无需转义
        assert_eq!(shell_quote("/var/`id`"), "'/var/`id`'"); // 反引号同理
        assert_eq!(shell_quote(""), "''"); // 空串: 空参数
    }

    #[test]
    fn result_is_single_quoted_segment() {
        // 转义结果自身必须仍是"单引号包裹的一段"——首尾为引号且内部无未转义裸引号
        let q = shell_quote("/a'b'c");
        assert!(q.starts_with('\''));
        assert!(q.ends_with('\''));
        // 去掉首尾引号后, 剩下的引号必须都成对出现（'\'' 结构）
        let inner = &q[1..q.len() - 1];
        let count = inner.chars().filter(|c| *c == '\'').count();
        assert_eq!(count % 2, 0);
    }
}

// ============================================================================
// 权限符号串测试（第三批）—— 翻译自 C# PermissionTextTests（7 用例）
// 追加到 src/ssh.rs 末尾。
// 语义差异（有意）：C# Symbolic 返回纯 9 位；Rust perms_str 前面多一个
// 类型位（d/-）且 mode=None 时 9 位全 '-'（C# 无此路径）。断言取 [1..] 对齐。
// ============================================================================

#[cfg(test)]
mod perms_tests {
    use super::perms_str;

    #[test]
    fn symbolic_table() {
        // (mode, is_dir, 期望 9 位)——C# PermissionText.Symbolic 的 7 条
        assert_eq!(&perms_str(Some(0b111_101_101), false)[1..], "rwxr-xr-x"); // 755
        assert_eq!(&perms_str(Some(0b110_100_100), false)[1..], "rw-r--r--"); // 644
        assert_eq!(&perms_str(Some(0b111_111_111), false)[1..], "rwxrwxrwx"); // 777
        assert_eq!(&perms_str(Some(0b000_000_000), false)[1..], "---------"); // 000
        assert_eq!(&perms_str(Some(0b100_000_000), false)[1..], "r--------"); // 400
        assert_eq!(&perms_str(Some(0b000_000_001), false)[1..], "--------x"); // 001
        // 4755：仅取低 9 位，setuid 位忽略
        assert_eq!(&perms_str(Some(0b100_111_101_101), false)[1..], "rwxr-xr-x");
    }

    // Rust 特有路径（C# 无对应）：无权限属性占位、目录前缀。
    #[test]
    fn none_mode_yields_placeholder_and_dir_prefix() {
        assert_eq!(perms_str(None, false), "----------");
        assert_eq!(perms_str(None, true), "d---------");
        assert!(perms_str(Some(0o755), true).starts_with('d'));
        assert!(perms_str(Some(0o755), false).starts_with('-'));
    }
}

#[cfg(test)]
mod fp_wait_tests {
    use super::*;

    /// 罩顶超时：200ms 无回执 → Timeout 分支（产品侧传 120s，测试侧 200ms）。
    /// 注：未做端到端 —— 需 GUI 指纹弹框 + 人工不点确认 + 可连 SSH 服务端，
    /// 此处仅钉 wait_fingerprint_confirm 的三态判定（超时按拒绝 fail-closed
    /// 由调用点 match 分支保证）。
    #[test]
    fn fp_confirm_times_out_without_receipt() {
        let (_tx, rx) = std::sync::mpsc::channel::<bool>();
        let started = std::time::Instant::now();
        assert!(matches!(
            wait_fingerprint_confirm(&rx, std::time::Duration::from_millis(200)),
            FpConfirm::Timeout
        ));
        // 实际等待 ≥ 设定值（罩顶真实生效，非立即返回）。
        assert!(started.elapsed() >= std::time::Duration::from_millis(200));
    }

    /// 三态透传：回执 true/false 原样；发端丢弃 → Disconnected（拒绝）。
    #[test]
    fn fp_confirm_receipt_and_disconnect() {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        tx.send(true).unwrap();
        assert!(matches!(
            wait_fingerprint_confirm(&rx, std::time::Duration::from_secs(1)),
            FpConfirm::Receipt(true)
        ));
        tx.send(false).unwrap();
        assert!(matches!(
            wait_fingerprint_confirm(&rx, std::time::Duration::from_secs(1)),
            FpConfirm::Receipt(false)
        ));
        drop(tx);
        assert!(matches!(
            wait_fingerprint_confirm(&rx, std::time::Duration::from_secs(1)),
            FpConfirm::Disconnected
        ));
    }
}

// ============================================================================
// CR-7 批量删除 / 递归日志节流钉子（纯逻辑：本机无远程 SSH 主机，受控 RTT
// 计时与真实连接计数按「未取证」留账，不用钉子绿冒充）。
// ============================================================================
#[cfg(test)]
mod cr7_delete_batch_tests {
    use super::*;

    fn file_items(n: usize) -> Vec<(String, bool)> {
        (0..n).map(|i| (format!("/r/f{i:04}.bin"), false)).collect()
    }

    /// 200 文件 → ceil(200/32)=7 块，块内 ≤32，计划 REMOVE 请求数恒等于项数。
    #[test]
    fn delete_batch_chunks_by_stat_concurrency() {
        let items = file_items(200);
        let (files, dirs) = split_delete_items(&items);
        assert!(dirs.is_empty());
        let chunks = delete_file_chunks(&files);
        assert_eq!(STAT_CONCURRENCY, 32);
        assert_eq!(chunks.len(), 200_usize.div_ceil(STAT_CONCURRENCY));
        assert_eq!(chunks.len(), 7, "200 项分 7 个并发块");
        assert!(chunks.iter().all(|c| c.len() <= STAT_CONCURRENCY));
        assert_eq!(chunks[0].len(), 32);
        assert_eq!(chunks[6].len(), 8);
        let planned: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(planned, 200, "计划 REMOVE 请求数 = 文件项数，不多发不漏发");
        let logs: Vec<String> = chunks
            .iter()
            .flat_map(|c| {
                c.iter()
                    .map(|p| delete_file_log_msg("srv", p, Ok(())).0)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(logs.len(), 200, "每项独立一条成败日志");
    }

    /// 块内一项失败其余仍执行：成功+失败=入参数，失败路径逐条可定位。
    #[test]
    fn delete_batch_partial_failure_still_reports_each_path() {
        let items = file_items(32);
        let (files, _) = split_delete_items(&items);
        let (mut ok, mut fail) = (0usize, 0usize);
        let mut logs = Vec::new();
        for (i, p) in files.iter().enumerate() {
            let r = if i % 11 == 0 {
                Err("permission denied".to_owned())
            } else {
                Ok(())
            };
            let (msg, good) = delete_file_log_msg("srv", p, r);
            if good {
                ok += 1;
            } else {
                fail += 1;
            }
            logs.push(msg);
        }
        assert_eq!(ok + fail, 32, "每项独立判定成败，无静默吞项");
        assert_eq!(fail, 3);
        assert_eq!(logs.len(), 32, "失败不阻断其余项执行");
        for (i, p) in files.iter().enumerate() {
            if i % 11 == 0 {
                assert!(
                    logs.iter()
                        .any(|m| m.contains(*p) && m.contains("permission denied")),
                    "失败路径 {p} 与原因逐条可定位"
                );
            }
        }
        assert!(
            !logs
                .iter()
                .any(|m| m.contains("已删除") || m.contains("Deleted "))
        );
    }

    /// 单文件仍只发一条 REMOVE；混合批里目录项不进并发块且保持原顺序。
    #[test]
    fn single_file_delete_still_one_remove_and_dirs_stay_serial() {
        let single = [("/r/a.bin".to_owned(), false)];
        let (files, dirs) = split_delete_items(&single);
        let chunks = delete_file_chunks(&files);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1, "单文件删除仍只发一条 REMOVE");
        assert!(dirs.is_empty());
        let mixed = vec![
            ("/r/a.bin".to_owned(), false),
            ("/r/d1".to_owned(), true),
            ("/r/b.bin".to_owned(), false),
            ("/r/d2".to_owned(), true),
        ];
        let (files, dirs) = split_delete_items(&mixed);
        assert_eq!(files, vec!["/r/a.bin", "/r/b.bin"]);
        assert_eq!(dirs, vec!["/r/d1", "/r/d2"], "目录项不进并发块、保持原顺序");
        assert_eq!(delete_file_chunks(&files).len(), 1);
    }

    /// 递归删 1000 / 10000 子文件：整树恒定 1 条汇总日志，条数有上界。
    #[test]
    fn recursive_delete_log_has_upper_bound() {
        for n in [1usize, 1000, 10000] {
            let mut report = DeleteTreeReport::default();
            for _ in 0..n {
                report.ok += 1; // 模拟逐子文件删除成功，不再产生 Ev::Log
            }
            report.ok += 1; // 目录本身
            let lines = [delete_tree_summary_msg("srv", "/r/tree", &report)];
            assert_eq!(lines.len(), 1, "n={n} 时整树日志条数仍为 1（上界）");
            assert!(
                lines[0].contains(&(n + 1).to_string()),
                "汇总带成功数 {n} 子文件 + 目录本身"
            );
        }
    }

    /// 部分失败不写「已删除 N 项」的假成功汇总；失败路径、原因、成功数都留痕。
    #[test]
    fn no_fake_success_summary_on_partial_failure() {
        let report = DeleteTreeReport {
            ok: 999,
            first_err: Some(("/r/tree/f0999".to_owned(), "permission denied".to_owned())),
        };
        let line = delete_tree_summary_msg("srv", "/r/tree", &report);
        assert!(
            !line.contains("已删除") && !line.contains("Deleted "),
            "部分失败不写假成功汇总：{line}"
        );
        assert!(line.contains("/r/tree/f0999"), "首错路径可定位");
        assert!(line.contains("permission denied"), "首错原因可定位");
        assert!(line.contains("999"), "首错前成功数留痕");
        let all_ok = DeleteTreeReport {
            ok: 1000,
            first_err: None,
        };
        let ok_line = delete_tree_summary_msg("srv", "/r/tree", &all_ok);
        assert!(!ok_line.contains("删除失败") && !ok_line.contains("Delete failed"));
    }

    /// 一个批 = 一条 Ev::Deleted 回执（无会话 worker 直接回执，不产生逐条日志）；
    /// 与 UI 侧 del_pending 按批 -1 闭环，200 项不会等 200 条回执。
    #[test]
    fn delete_batch_emits_single_deleted_reply() {
        let ssh = Ssh::spawn();
        let items: Vec<(String, bool)> =
            (0..200).map(|i| (format!("/r/f{i}.bin"), false)).collect();
        ssh.delete_batch(77, items);
        let mut evs = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while evs.is_empty() && std::time::Instant::now() < deadline {
            evs.extend(ssh.poll());
            if evs.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        assert_eq!(evs.len(), 1, "整批只回一条 Deleted");
        assert!(matches!(&evs[0], Ev::Deleted { tag } if *tag == 77));
    }
}

// ============================================================================
// CR-7-S4 exec 连接复用 / 目录删除异步化钉子（纯逻辑 + 离线 worker）。
// 实机连接计数 20→1、主链路不阻塞、同主机受控计时须真实 SSH 主机，
// 本机取不到，按「未取证」留账，不用钉子绿冒充。
// ============================================================================
#[cfg(test)]
mod cr7_s4_tests {
    use super::*;

    /// 档案占位：按 R3，单测示例不放任何真实密码 / 私钥内容，只用私钥路径字符串。
    fn mk_spec(host: &str, port: u16, user: &str, fp: Option<&str>, key: &str) -> ExecSpec {
        ExecSpec {
            host: host.to_owned(),
            port,
            user: user.to_owned(),
            auth: Auth::KeyFile(key.to_owned()),
            fingerprint: fp.map(str::to_owned),
        }
    }

    /// 换主机 / 换端口 / 换用户 / 改指纹 / 换密钥 / 档案清空，任一变化即丢缓存。
    #[test]
    fn exec_cache_invalidated_on_profile_change() {
        let a = mk_spec("h1", 22, "u", Some("fp1"), "k1");
        assert!(exec_cache_usable(Some(&a), Some(&a)), "同一档案才复用");
        assert!(!exec_cache_usable(Some(&a), None), "档案清空即失效");
        assert!(!exec_cache_usable(None, Some(&a)), "无缓存不算复用");
        let variants = [
            mk_spec("h2", 22, "u", Some("fp1"), "k1"),
            mk_spec("h1", 2222, "u", Some("fp1"), "k1"),
            mk_spec("h1", 22, "u2", Some("fp1"), "k1"),
            mk_spec("h1", 22, "u", Some("fp2"), "k1"),
            mk_spec("h1", 22, "u", None, "k1"),
            mk_spec("h1", 22, "u", Some("fp1"), "k2"),
        ];
        for b in &variants {
            assert!(
                !exec_cache_usable(Some(&a), Some(b)),
                "任一档案字段变化即丢缓存，绝不拿旧主机连接删新主机目录"
            );
        }
    }

    /// 连接级失败丢缓存；命令非零退出保留连接。20 目录同一档案的「复用计划」恒命中
    /// （计划层断言，不等于实机连接计数）。
    #[test]
    fn exec_cache_dropped_on_failure_kept_on_nonzero_exit() {
        assert!(should_drop_exec_cache(&Err("connect refused".to_owned())));
        assert!(!should_drop_exec_cache(&Ok((
            0,
            String::new(),
            String::new()
        ))));
        assert!(!should_drop_exec_cache(&Ok((
            1,
            String::new(),
            "rm: denied".to_owned()
        ))));
        let a = mk_spec("h1", 22, "u", Some("fp1"), "k1");
        let reuse: Vec<bool> = (0..20)
            .map(|_| exec_cache_usable(Some(&a), Some(&a)))
            .collect();
        assert_eq!(
            reuse.iter().filter(|v| **v).count(),
            20,
            "20 个目录作业档案不变 → 计划只建 1 次连接（非实机计数）"
        );
    }

    /// 四项语义里可纯逻辑钉住的两项：shell_quote 转义、退出码 0 才成功。
    /// NoSuchFile 迟到核对与失败回退递归在 delete_dir_settle 内，须真实 sftp 句柄。
    #[test]
    fn exec_reuse_keeps_quote_and_exit_zero_semantics() {
        let cmd = delete_dir_exec_cmd("/r/a'b");
        assert!(cmd.starts_with("rm -rf -- '"), "命令式保留: {cmd}");
        assert!(cmd.contains("'\\''"), "单引号按 POSIX 转义保留: {cmd}");
        assert!(exec_delete_succeeded(&Ok((
            0,
            String::new(),
            String::new()
        ))));
        assert!(!exec_delete_succeeded(&Ok((
            1,
            String::new(),
            String::new()
        ))));
        assert!(!exec_delete_succeeded(&Ok((
            -1,
            String::new(),
            String::new()
        ))));
        assert!(!exec_delete_succeeded(&Err("timeout".to_owned())));
    }

    /// 20 目录 = 19 次续发 + 1 次结算；结算只在整批末尾，故 Ev::Deleted 恒一条。
    #[test]
    fn twenty_dirs_need_nineteen_more_jobs_before_one_reply() {
        let mut p = PendingDelete {
            tag: 9,
            label: "srv".to_owned(),
            dirs: (0..20).map(|i| format!("/r/d{i}")).collect(),
            next: 0,
        };
        let (mut nexts, mut done) = (0, 0);
        for _ in 0..20 {
            match advance_pending_delete(&mut p) {
                PendingStep::Next { tag, path } => {
                    assert_eq!(tag, 9);
                    assert!(path.starts_with("/r/d"));
                    nexts += 1;
                }
                PendingStep::Done { tag } => {
                    assert_eq!(tag, 9);
                    done += 1;
                }
            }
        }
        assert_eq!(nexts, 19, "前 19 个目录各续发一条 exec 作业");
        assert_eq!(done, 1, "整批只结算一次 → 只发一条 Ev::Deleted");
    }

    /// 换主机 / 断连作废在途批：每个 tag 补发回执，UI 不会卡「正在删除…」。
    #[test]
    fn cancel_pending_deletes_replies_every_tag() {
        let (tx, rx) = mpsc::channel::<Ev>();
        let mut pending: Vec<PendingDelete> = (0..3)
            .map(|i| PendingDelete {
                tag: 10 + i,
                label: "srv".to_owned(),
                dirs: vec![format!("/r/d{i}")],
                next: 0,
            })
            .collect();
        cancel_pending_deletes(&mut pending, &tx);
        assert!(pending.is_empty(), "在途批全部作废");
        let tags: Vec<u64> = rx
            .try_iter()
            .filter_map(|e| match e {
                Ev::Deleted { tag } => Some(tag),
                _ => None,
            })
            .collect();
        assert_eq!(tags, vec![10, 11, 12], "三个 tag 各补一条回执");
    }

    /// 无 pending 的迟到 ExecDone 必须丢弃，且不污染随后一批的回执。
    #[test]
    fn orphan_exec_done_is_dropped_and_next_batch_still_one_reply() {
        let ssh = Ssh::spawn();
        let _ = ssh.tx.send(Cmd::ExecDone {
            tag: 999,
            result: Ok((0, String::new(), String::new())),
        });
        ssh.delete_batch(7, vec![("/r/a.bin".to_owned(), false)]);
        let mut evs = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while evs.is_empty() && std::time::Instant::now() < deadline {
            evs.extend(ssh.poll());
            if evs.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
        evs.extend(ssh.poll());
        let tags: Vec<u64> = evs
            .iter()
            .filter_map(|e| match e {
                Ev::Deleted { tag } => Some(*tag),
                _ => None,
            })
            .collect();
        assert_eq!(tags, vec![7], "孤儿回执不产生事件，后续批仍只回一条");
    }

    /// 命令窗走新通道后，无连接档案时仍归一回执（exit=-1、原因留痕），
    /// 不因 recv / 缓存空档挂死；也钉住 exec_run 不再被 std 阻塞 recv 冻住。
    #[test]
    fn command_job_without_profile_replies_exit_minus_one() {
        let ssh = Ssh::spawn();
        ssh.exec(42, "echo hi".to_owned());
        let mut evs = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while evs.is_empty() && std::time::Instant::now() < deadline {
            evs.extend(ssh.poll());
            if evs.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert_eq!(evs.len(), 1, "一条命令只回一条 Ev::Exec");
        let Ev::Exec { req, exit, err, .. } = &evs[0] else {
            panic!("期望 Ev::Exec");
        };
        assert_eq!(*req, 42);
        assert_eq!(*exit, -1, "无档案按失败归一，不假成功");
        assert!(!err.is_empty(), "失败原因留痕");
    }
}
