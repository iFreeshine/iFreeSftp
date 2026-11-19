use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::i18n::{t, tfmt};
use crate::ui::panes::Entry;

#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    Password(String),

    KeyFile(String),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Password(_) => f.write_str("Auth::Password(****)"),
            Auth::KeyFile(p) => f.debug_tuple("Auth::KeyFile").field(p).finish(),
        }
    }
}

fn connect_timeout() -> Option<Duration> {
    let s = crate::settings::with(|v| v.connect_timeout);
    (s > 0).then(|| Duration::from_secs(s as u64))
}

fn auth_timeout() -> Option<Duration> {
    connect_timeout()
}

fn operation_timeout_secs() -> i32 {
    crate::settings::with(|v| v.operation_timeout)
}

fn keepalive_interval() -> Option<Duration> {
    let (on, secs) = crate::settings::with(|v| (v.keep_alive, v.keep_alive_interval));
    (on && secs > 0).then(|| Duration::from_secs(secs as u64))
}

const EXEC_TIMEOUT: Duration = Duration::from_secs(10);

const STAT_CONCURRENCY: usize = 32;

pub enum Cmd {
    Connect {
        host: String,
        port: u16,
        user: String,
        auth: Auth,

        label: String,

        fingerprint: Option<String>,
    },

    List {
        tag: u64,
        path: String,
    },

    Stat {
        path: String,
        req: u64,
    },

    StatBatch {
        paths: Vec<String>,
        req: u64,
    },

    DeleteBatch {
        tag: u64,
        items: Vec<(String, bool)>,
    },

    ExecDone {
        tag: u64,
        result: Result<(i32, String, String), String>,
    },

    ReadFile {
        path: String,
        req: u64,
        max: u64,
    },

    WriteFile {
        path: String,
        req: u64,
        data: Vec<u8>,
    },

    Mkdir {
        tag: u64,
        path: String,
    },

    CreateFile {
        tag: u64,
        path: String,
    },

    Rename {
        tag: u64,
        path: String,
        new_path: String,
    },
    Disconnect,
}

pub enum ExecReq {

    Command { req: u64, cmd: String },

    Delete { tag: u64, cmd: String },
}

pub enum Ev {

    Connected {
        label: String,
        root: String,
        confirmed: Option<String>,
    },

    AskFingerprint {
        first: bool,
        host: String,
        stored: Option<String>,
        presented: String,
        reply: mpsc::Sender<bool>,
    },

    Failed {
        what: &'static str,
        msg: String,
        tag: Option<u64>,
    },

    Listing {
        tag: u64,
        path: String,
        entries: Vec<Entry>,
    },

    Stat {
        req: u64,
        path: String,
        exists: bool,

        size: Option<u64>,

        mtime: Option<i64>,
    },

    FileData {
        req: u64,
        result: Result<FileData, String>,
    },

    FileWritten {
        req: u64,
        path: String,
        result: Result<Option<i64>, String>,
    },

    Deleted { tag: u64 },

    Created { tag: u64 },

    Log { msg: String },

    Exec {
        req: u64,
        exit: i32,
        out: String,
        err: String,
    },

    Disconnected,
}

pub struct Ssh {

    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    exec_tx: tokio::sync::mpsc::UnboundedSender<ExecReq>,
    rx: Arc<Mutex<mpsc::Receiver<Ev>>>,

    connected: Arc<AtomicBool>,

    probe: Arc<Mutex<(u64, Option<HashProbe>)>>,

    exec_spec: Arc<Mutex<Option<ExecSpec>>>,
}

pub struct FileData {
    pub data: Vec<u8>,
    pub mtime: Option<i64>,
}

impl Ssh {

    pub fn spawn() -> Self {

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

        let exec_spec: Arc<Mutex<Option<ExecSpec>>> = Arc::new(Mutex::new(None));
        let exec_spec_run = exec_spec.clone();
        let exec_spec_exec = exec_spec.clone();

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

    pub fn stat_batch(&self, paths: Vec<String>, req: u64) {
        let _ = self.tx.send(Cmd::StatBatch { paths, req });
    }

    pub fn delete_batch(&self, tag: u64, items: Vec<(String, bool)>) {
        let _ = self.tx.send(Cmd::DeleteBatch { tag, items });
    }

    pub fn read_file(&self, path: String, req: u64, max: u64) {
        let _ = self.tx.send(Cmd::ReadFile { path, req, max });
    }

    pub fn write_file(&self, path: String, req: u64, data: Vec<u8>) {
        let _ = self.tx.send(Cmd::WriteFile { path, req, data });
    }

    pub fn mkdir(&self, tag: u64, path: String) {
        let _ = self.tx.send(Cmd::Mkdir { tag, path });
    }

    pub fn create_file(&self, tag: u64, path: String) {
        let _ = self.tx.send(Cmd::CreateFile { tag, path });
    }

    pub fn rename(&self, tag: u64, path: String, new_path: String) {
        let _ = self.tx.send(Cmd::Rename {
            tag,
            path,
            new_path,
        });
    }

    pub fn exec(&self, req: u64, cmd: String) {
        let _ = self.exec_tx.send(ExecReq::Command { req, cmd });
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(Cmd::Disconnect);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn hash_probe(&self) -> Option<HashProbe> {
        self.probe.lock().unwrap().1
    }

    pub fn probe_handle(&self) -> Arc<Mutex<(u64, Option<HashProbe>)>> {
        self.probe.clone()
    }

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

pub(crate) struct SftpConn {
    _handle: russh::client::Handle<Handler>,
    pub(crate) sftp: russh_sftp::client::SftpSession,
}

impl SftpConn {

    pub(crate) async fn exec(&self, cmd: &str) -> Result<(i32, String, String), String> {
        exec_on_channel(&self._handle, cmd).await
    }
}

pub(crate) enum ConnErr {

    Auth(String),

    Io(String),
}

impl From<ConnErr> for String {
    fn from(e: ConnErr) -> String {
        match e {
            ConnErr::Auth(m) | ConnErr::Io(m) => m,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct HashProbe {

    #[allow(dead_code)]
    pub posix: bool,
    pub sha256: bool,
    pub sha512: bool,

    pub sm3: bool,
}

impl HashProbe {

    pub fn any(self) -> bool {
        self.sha256 || self.sha512 || self.sm3
    }
}

fn log_line(label: &str, body: String) -> String {
    if label.is_empty() {
        body
    } else {
        format!("[{label}] {body}")
    }
}

#[derive(Clone, Copy)]
enum FpMode {

    Silent,

    Interactive,
}

#[derive(Clone)]
struct FpGate {
    mode: FpMode,

    trusted: Option<String>,

    label: String,

    host: String,

    etx: mpsc::Sender<Ev>,

    confirmed: Arc<Mutex<Option<String>>>,

    denied: Arc<Mutex<bool>>,
}

impl FpGate {

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

    fn denied(&self) -> bool {
        *self.denied.lock().unwrap()
    }
}

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

        let fp = key.fingerprint(russh::keys::HashAlg::Sha256).to_string();
        let presented = fp.strip_prefix("SHA256:").unwrap_or(&fp).to_owned();
        match self.fp.mode {
            FpMode::Silent => {

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

    fn interactive_check(&mut self, presented: &str) -> Result<bool, russh::Error> {

        if self.fp.confirmed.lock().unwrap().as_deref() == Some(presented) {
            return Ok(true);
        }

        if self.fp.trusted.as_deref() == Some(presented) {
            return Ok(true);
        }

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

        let ok = match wait_fingerprint_confirm(&rrx, std::time::Duration::from_secs(120)) {
            FpConfirm::Receipt(v) => v,
            FpConfirm::Timeout => {
                let _ = self.fp.etx.send(Ev::Log {
                    msg: log_line(&self.fp.label, t("指纹确认超时，连接中止").to_owned()),
                });
                false
            }

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

pub(crate) enum FpConfirm {

    Receipt(bool),

    Timeout,

    Disconnected,
}

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

pub(crate) async fn connect_sftp_interactive(
    host: &str,
    port: u16,
    user: &str,
    auth: &Auth,
    trusted: Option<String>,
    etx: mpsc::Sender<Ev>,
) -> Result<Option<String>, String> {

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

            let key = russh::keys::load_secret_key(path, None)
                .map_err(|e| ConnErr::Io(tfmt("私钥读取失败：{0}", &[&e])))?;

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

struct Ctx {
    conn: SftpConn,

    label: String,
}

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

fn delete_file_chunks<'a, 'b>(files: &'b [&'a str]) -> Vec<&'b [&'a str]> {
    files.chunks(STAT_CONCURRENCY).collect()
}

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

    let mut pending_deletes: Vec<PendingDelete> = Vec::new();

    let mut listed_once = false;

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

                cancel_pending_deletes(&mut pending_deletes, &etx);

                ctx = None;

                {
                    let mut g = probe.lock().unwrap();
                    g.0 += 1;
                    g.1 = None;
                }
                listed_once = false;

                if let Some(fp) = &fingerprint {
                    let _ = etx.send(Ev::Log {
                        msg: format!("[{label}] {}", tfmt("信任存档指纹 {0}", &[fp])),
                    });
                }

                let _ = etx.send(Ev::Log {
                    msg: format!("[{label}] {}", tfmt("正在连接 {0}:{1} …", &[&host, &port])),
                });
                let gate = FpGate::new(fingerprint, label.clone(), host.clone(), etx.clone());
                match connect_with_retry(&host, port, &user, &auth, gate, &label, &etx).await {
                    Ok((conn, root, confirmed)) => {

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
                    continue;
                };
                let (path, label) = {
                    let p = &pending_deletes[pos];
                    (p.dirs[p.next].clone(), p.label.clone())
                };
                if let Some(c) = &mut ctx {

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

                cancel_pending_deletes(&mut pending_deletes, &etx);
                if let Some(c) = &ctx {
                    let _ = etx.send(Ev::Log {
                        msg: format!("[{}] {}", c.label, t("已断开")),
                    });
                }
                ctx = None;
                connected.store(false, Ordering::Relaxed);
                let _ = etx.send(Ev::Disconnected);
            }
        }
    }
}

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

            Err(ConnErr::Auth(msg)) => return Err(tfmt("连接失败：{0}", &[&msg])),
            Err(ConnErr::Io(msg)) => {

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

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn delete_dir_exec_cmd(path: &str) -> String {
    format!("rm -rf -- {}", shell_quote(path))
}

fn exec_delete_succeeded(result: &Result<(i32, String, String), String>) -> bool {
    matches!(result, Ok((0, _, _)))
}

struct PendingDelete {
    tag: u64,
    label: String,
    dirs: Vec<String>,

    next: usize,
}

enum PendingStep {
    Next { tag: u64, path: String },
    Done { tag: u64 },
}

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

fn cancel_pending_deletes(pending: &mut Vec<PendingDelete>, etx: &mpsc::Sender<Ev>) {
    for p in pending.drain(..) {
        let _ = etx.send(Ev::Deleted { tag: p.tag });
    }
}

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

    let secs = operation_timeout_secs();
    let meta = if secs > 0 {
        tokio::time::timeout(Duration::from_secs(secs as u64), sftp.metadata(path)).await
    } else {
        Ok(sftp.metadata(path).await)
    };
    match meta {

        Ok(Err(russh_sftp::client::error::Error::Status(s)))
            if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile =>
        {
            let _ = etx.send(Ev::Log {
                msg: format!("[{label}] {}", tfmt("远程删除：{0}", &[&path])),
            });
        }

        Ok(Ok(_)) => {
            let _ = etx.send(Ev::Log {
                msg: format!(
                    "[{label}] {}",
                    t("服务端命令删除失败，已回退 SFTP 递归删除")
                ),
            });
            let _ = delete_dir_recursive(sftp, path, label, etx).await;
        }

        _ => {
            let _ = delete_dir_recursive(sftp, path, label, etx).await;
        }
    }
}

#[derive(Default)]
struct DeleteTreeReport {

    ok: usize,

    first_err: Option<(String, String)>,
}

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
                *cached = None;
                Err(tfmt("操作超时（{0} 秒）", &[&secs]))
            }
        };
    if should_drop_exec_cache(&outcome) {
        *cached = None;
    }
    outcome
}

fn exec_cache_usable(cached: Option<&ExecSpec>, current: Option<&ExecSpec>) -> bool {
    cached.is_some() && cached == current
}

fn should_drop_exec_cache(result: &Result<(i32, String, String), String>) -> bool {
    result.is_err()
}

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

async fn exec_connect(spec: &ExecSpec) -> Result<russh::client::Handle<Handler>, String> {
    let config = russh::client::Config {
        nodelay: true,
        inactivity_timeout: None,
        keepalive_interval: keepalive_interval(),
        ..Default::default()
    };
    let addr = (spec.host.clone(), spec.port);

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

#[derive(Clone, PartialEq)]
pub struct ExecSpec {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,

    pub fingerprint: Option<String>,
}

impl Ssh {

    pub fn set_exec_profile(&self, p: Option<ExecSpec>) {
        *self.exec_spec.lock().unwrap() = p;
    }
}

fn set_exec_fingerprint(exec_spec: &Arc<Mutex<Option<ExecSpec>>>, fp: Option<String>) {
    let mut g = exec_spec.lock().unwrap();
    if let (Some(spec), Some(f)) = (g.as_mut(), fp) {
        spec.fingerprint = Some(f);
    }
}

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

fn probe_store(cur: u64, probe: &Mutex<(u64, Option<HashProbe>)>, v: Option<HashProbe>) {
    let mut g = probe.lock().unwrap();
    if g.0 == cur {
        g.1 = v;
    }
}

async fn hash_probe_run(
    cur: u64,
    probe: Arc<Mutex<(u64, Option<HashProbe>)>>,
    etx: mpsc::Sender<Ev>,
    exec_spec: Arc<Mutex<Option<ExecSpec>>>,
) {

    let mut cached: Option<(ExecSpec, russh::client::Handle<Handler>)> = None;
    let posix = match probe_once("echo $env:OS", &exec_spec, &mut cached).await {
        Ok((_, out)) => {
            let out = out.trim();
            !(out.contains("Windows_NT") || out.contains("$env"))
        }

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

    file.flush()
        .await
        .map_err(|e| tfmt("写入失败：{0}", &[&e]))?;
    let meta = sftp_op(sftp.metadata(path))
        .await
        .map_err(|e| tfmt("写入后取元数据失败：{0}", &[&e]))?;
    Ok(meta.mtime.map(|t| t as i64))
}

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

#[cfg(test)]
mod shell_quote_tests {
    use super::shell_quote;

    #[test]
    fn neutralizes_injection() {
        assert_eq!(shell_quote("/var/www"), "'/var/www'");
        assert_eq!(shell_quote("/var/my site"), "'/var/my site'");
        assert_eq!(shell_quote("/var/a'b"), "'/var/a'\\''b'");
        assert_eq!(shell_quote("/var/a;rm -rf /"), "'/var/a;rm -rf /'");
        assert_eq!(shell_quote("/var/$(id)"), "'/var/$(id)'");
        assert_eq!(shell_quote("/var/a\"b"), "'/var/a\"b'");
        assert_eq!(shell_quote("/var/`id`"), "'/var/`id`'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn result_is_single_quoted_segment() {

        let q = shell_quote("/a'b'c");
        assert!(q.starts_with('\''));
        assert!(q.ends_with('\''));

        let inner = &q[1..q.len() - 1];
        let count = inner.chars().filter(|c| *c == '\'').count();
        assert_eq!(count % 2, 0);
    }
}

#[cfg(test)]
mod perms_tests {
    use super::perms_str;

    #[test]
    fn symbolic_table() {

        assert_eq!(&perms_str(Some(0b111_101_101), false)[1..], "rwxr-xr-x");
        assert_eq!(&perms_str(Some(0b110_100_100), false)[1..], "rw-r--r--");
        assert_eq!(&perms_str(Some(0b111_111_111), false)[1..], "rwxrwxrwx");
        assert_eq!(&perms_str(Some(0b000_000_000), false)[1..], "---------");
        assert_eq!(&perms_str(Some(0b100_000_000), false)[1..], "r--------");
        assert_eq!(&perms_str(Some(0b000_000_001), false)[1..], "--------x");

        assert_eq!(&perms_str(Some(0b100_111_101_101), false)[1..], "rwxr-xr-x");
    }

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

    #[test]
    fn fp_confirm_times_out_without_receipt() {
        let (_tx, rx) = std::sync::mpsc::channel::<bool>();
        let started = std::time::Instant::now();
        assert!(matches!(
            wait_fingerprint_confirm(&rx, std::time::Duration::from_millis(200)),
            FpConfirm::Timeout
        ));

        assert!(started.elapsed() >= std::time::Duration::from_millis(200));
    }

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

#[cfg(test)]
mod cr7_delete_batch_tests {
    use super::*;

    fn file_items(n: usize) -> Vec<(String, bool)> {
        (0..n).map(|i| (format!("/r/f{i:04}.bin"), false)).collect()
    }

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

    #[test]
    fn recursive_delete_log_has_upper_bound() {
        for n in [1usize, 1000, 10000] {
            let mut report = DeleteTreeReport::default();
            for _ in 0..n {
                report.ok += 1;
            }
            report.ok += 1;
            let lines = [delete_tree_summary_msg("srv", "/r/tree", &report)];
            assert_eq!(lines.len(), 1, "n={n} 时整树日志条数仍为 1（上界）");
            assert!(
                lines[0].contains(&(n + 1).to_string()),
                "汇总带成功数 {n} 子文件 + 目录本身"
            );
        }
    }

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

#[cfg(test)]
mod cr7_s4_tests {
    use super::*;

    fn mk_spec(host: &str, port: u16, user: &str, fp: Option<&str>, key: &str) -> ExecSpec {
        ExecSpec {
            host: host.to_owned(),
            port,
            user: user.to_owned(),
            auth: Auth::KeyFile(key.to_owned()),
            fingerprint: fp.map(str::to_owned),
        }
    }

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
