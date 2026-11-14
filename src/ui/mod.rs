//! 主窗口 1:1 布局（对照 C# Views/MainContainer.xaml + Header.xaml）。
//!
//! 结构：Header（33px 标题条）+ ShellBorder（BorderThickness 5,0,5,5，HeaderAreaBrush）
//! → 3 列 `*`,6,`*` × 3 行 `*`,6,120；四容器 FolderAreaBrush 圆角 8；
//! 竖分隔条跨上下两排（RowSpan 3），横分隔条跨左右三列（ColumnSpan 3），均可拖拽。

mod ctx_menu;
mod dialogs;
mod editor_window;
mod header;
mod host_panel;
mod import_dialog;
// crate 内共享（src/local.rs 复用 FilePane/Entry/SortKey）。
pub(crate) mod panes;
mod sync;

use egui::Rect;

use crate::i18n::{t, tfmt};
use crate::theme;

/// 标题栏高：内容 33 + 上下各留 5 逻辑px（C# OS caption Tall 模式实测约 41 逻辑px，
/// Header.xaml.cs UpdateTitleBarMetrics：按钮高 = caption 条高 − 10，上下各留 5；
/// 元素最大高 33（按钮/胶囊），故取 33 + 10 = 43，元素尺寸不变、条内垂直居中）。
pub const HEADER_H: f32 = 43.0;
/// CR-2：设置落盘失败全局提示条高（12px 字 + 上下留白）。
const SAVE_BAR_H: f32 = 26.0;
/// ShellBorder BorderThickness（左,上,右,下）。
const SHELL_BORDER: (f32, f32, f32, f32) = (5.0, 0.0, 5.0, 5.0);
/// 单个窗格最小宽（拖拽钳制，防拖没）。
const PANE_MIN: f32 = 220.0;
/// 下排最小高。
const BOTTOM_MIN: f32 = 40.0;
/// 上排最小高。
const TOP_ROW_MIN: f32 = 80.0;

/// 图标钮 tooltip（C# AccentToolTipStyle，App.xaml L17-35，经 App.MakeAccentTip 挂）：
/// 品牌蓝底（ACCENT #0078D4）白字、字号 12、Padding `10,5,10,6`、圆角 4、
/// 显示在控件下方（Placement=Bottom ↔ egui tooltip 默认 BOTTOM_START）。
/// 亮暗同值（C# 未分主题字典）；无阴影（模板 = Border + ContentPresenter）。
/// 复用 egui `Tooltip` 体系保留延迟/防抖/单层唯一判定，仅替换 frame 与文字样式。
pub(crate) fn accent_tip(resp: egui::Response, text: impl Into<String>) -> egui::Response {
    let mut tip = egui::Tooltip::for_enabled(&resp);
    tip.popup = tip.popup.frame(
        egui::Frame::new()
            .fill(theme::ACCENT)
            .corner_radius(theme::CONTROL_RADIUS)
            .inner_margin(egui::Margin {
                // egui 0.37 Margin 字段为 i8（C# Padding `10,5,10,6` 恰为整数）。
                left: 10,
                right: 10,
                top: 5,
                bottom: 6,
            }),
    );
    tip.show(|ui| {
        // 与 on_hover_text 相同的宽度上限（防长文案无限撑宽）。
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.label(
            egui::RichText::new(text.into())
                .color(theme::TOOLTIP_FG)
                .size(12.0),
        );
    });
    resp
}

/// 待入队传输批次：目标为远程目录时冲突决策需异步 STAT；本地目标同步探测。
pub struct PendingTx {
    /// STAT 请求序号（丢弃过期响应）。
    pub seq: u64,
    /// 批级传输档案所在侧（Upload/Relay = 目标远程侧；Download = 源远程侧；
    /// 本地→本地 = 源侧）。STAT 回流与入队均按此侧取 ssh / 连接档案。
    pub profile_side: usize,
    /// 发起侧面板（S-1「正在准备传输」瞬态状态栏的挂靠与结算清除点；
    /// C# UpdateStatus 打在发起 FileView 的状态栏）。
    pub src_pane: usize,
    /// 计时基线（预检超时兜底，滑动窗语义 T20-H②：任一回流即刷新——60s
    /// 零回流才算断流；批量 STAT 分轮回流的自然间歇不误判超窗）。
    pub at: std::time::Instant,
    pub items: Vec<PendingItem>,
    /// 已回流条目总数（含冲突；集齐判定游标：resolved == items.len() 即全量
    /// 结算。T20-E 策略：冲突只标记不中止，流式全程不停）。
    pub resolved: usize,
    /// 待落位余量：已回流、无冲突、未落位条目数（流式落位判定游标，摘批
    /// 递减；冲突项不计数——冲突项留置到 Settle 统一进弹窗，T20-E）。
    pub ready: usize,
    /// 流式落位阈值（settings.dnd_stream_batch，快照）：余量满 N 即入队一批。
    /// usize::MAX = 禁用流式（本地确认弹窗批次 / 弹窗交接批次）。
    pub batch: usize,
    /// 已落位批次号（enqueue 返回；弹窗「取消」按批次回溯清行，T20-H）。
    pub batches: Vec<u64>,
    /// 已落位累计条数（流式 Place 逐批累加；弹窗范围行「已落位 M 项已开始
    /// 传输」数据源——流式中止后已落位项不可逆，弹窗必须写明范围，T20-H）。
    pub placed: usize,
}

/// 状态栏瞬态文案：正在准备传输（S-1 接通 i18n.rs:428 死键；C#
/// StatusPreparingTransfer，FileView.xaml.cs:3069 入队前冲突探测/统计期间
/// 无任务行承载，状态栏先给一句明确提示避免「点了没反应」）。
fn preparing_msg() -> String {
    t("正在准备传输（探测目标/统计文件）…").to_owned()
}

/// 当前语言的准备提示精确判定（T20-E-R）：语言作入参，纯逻辑便于中英各落
/// 一钉，不改写全局 language 态。旧实现硬编码中文前缀，英文态
/// preparing_msg() 不以之中开头 → 结算后状态栏准备提示永不清除。
fn is_preparing_msg(m: &str, en: bool) -> bool {
    m == crate::i18n::t_for("正在准备传输（探测目标/统计文件）…", en)
}

/// 清除瞬态准备提示：仅当仍是**当前语言**的准备提示才清（期间被列目录
/// 错误等覆写则保留），结算点共用。
fn clear_preparing_msg(st: &mut State, pane: usize) {
    let en = crate::i18n::is_en();
    let p = pane_view_mut(st, pane);
    if p.msg.as_deref().is_some_and(|m| is_preparing_msg(m, en)) {
        p.msg = None;
    }
}

/// 批次回流集齐后的处置（覆盖确认弹窗 / 直接入队）：T20-E 流式集齐 Settle
/// 与预检超时兜底共用。冲突 → 弹窗批次保留 pending_tx（seq 清零，迟到回流
/// 落空匹配；弹窗确认走 enqueue_pending）——此前清空 pending_tx 开弹窗致
/// 确认空转（修复，T20-H）；此前流式已落位批次号随行，弹窗「取消」按批次
/// 清行。流式已落位 M 项不可逆（T20-H）：placed > 0 时弹窗挂范围行写明
/// 「本次确认只处理剩余 N 项」，防止用户以为整批还在手上。
fn settle_batch(st: &mut State, profile_side: usize, src_pane: usize, items: Vec<PendingItem>) {
    let (prev_batches, placed) = st
        .pending_tx
        .take()
        .map(|p| (p.batches, p.placed))
        .unwrap_or((Vec::new(), 0));
    st.pending_tx = None;
    clear_preparing_msg(st, src_pane); // S-1 瞬态提示结算清除
    if items.iter().any(|pi| pi.conflict) {
        let conflicts = items.iter().filter(|pi| pi.conflict).count();
        let note = scope_note_for(placed, items.len(), conflicts);
        st.pending_tx = Some(PendingTx {
            seq: 0,
            profile_side,
            src_pane,
            at: std::time::Instant::now(),
            items,
            resolved: 0,
            ready: 0,
            batch: usize::MAX,
            batches: prev_batches,
            placed: 0,
        });
        let rows = match &st.pending_tx {
            Some(ptx) => confirm_data(&ptx.items),
            None => Vec::new(),
        };
        // F-10 方案A：弹窗勾选态随弹窗创建填默认值（目标较新默认不勾）。
        st.conflict_sel = Some(conflict_defaults(&rows));
        st.dialog = dialogs::Dialog::TransferConfirm { rows, note };
    } else {
        enqueue_tasks(
            st,
            profile_side,
            items.into_iter().map(into_task).collect(),
            0,
        );
    }
}

/// 范围行门控（T20-E-R）：placed > 0（已有流式落位、不可逆）才挂范围行。
/// 抽成函数让 settle_batch 与钉子共用同一判定，钉子不再对拍恒假常量
/// `(0usize > 0).then(...)`（门控改坏也不会红）。
fn scope_note_for(placed: usize, remaining: usize, conflicts: usize) -> Option<String> {
    (placed > 0).then(|| scope_note(placed, remaining, conflicts))
}

/// 弹窗范围行文案（纯逻辑便于钉子）：流式已落位 M 项不可逆，写明本次确认
/// 只处理剩余 N 项（其中冲突 C 项）。placed = 0（无流式前置）→ None。
fn scope_note(placed: usize, remaining: usize, conflicts: usize) -> String {
    tfmt(
        "已落位 {0} 项已开始传输，本次确认只处理剩余 {1} 项（其中冲突 {2} 项）",
        &[
            &placed.to_string(),
            &remaining.to_string(),
            &conflicts.to_string(),
        ],
    )
}

// ── T20-H 流式落位：万级拖拽批次冲突预检 StatBatch 流式回流，每回流满
// dnd_stream_batch 个无冲突条目即入队一批（队列逐批长出，估算与传输并行）。

/// 流式落位判定（每条 STAT 回流后调用，T20-E 策略：冲突只标记不中止——
/// 冲突项留置到集齐统一进弹窗，非冲突项照常逐批落位，流式全程不停）。
enum StreamStep {
    None,
    /// 无冲突余量集满一批 → 入队一批（pending_tx 保留继续收集）。
    Place,
    /// 全部回流已结算 → settle_batch 统一处置（无冲突尾批直接入队收尾；
    /// 含冲突则弹窗批级决策，已落位批次随行供取消撤回）。
    Settle,
}

/// 判定（纯逻辑，便于钉子测试）：集齐 → Settle；无冲突余量满阈值 → Place。
fn stream_step(ptx: &PendingTx) -> StreamStep {
    if ptx.resolved == ptx.items.len() {
        StreamStep::Settle
    } else if ptx.ready >= ptx.batch {
        StreamStep::Place
    } else {
        StreamStep::None
    }
}

/// 从 pending_tx 摘出 `need` 个已回流**无冲突**条目（保持回流序），余下
/// （未回流 + 冲突留置 + 回流未满批）留置；不变式：resolved == items 中
/// stat_path 为 None 的条数（含冲突留置；摘批同步递减）、ready == 其中
/// 非冲突数（冲突项不计入 ready）。
fn stream_take(ptx: &mut PendingTx, need: usize) -> Vec<PendingItem> {
    let mut out = Vec::with_capacity(need);
    let mut rest = Vec::with_capacity(ptx.items.len() - need);
    for pi in ptx.items.drain(..) {
        if pi.stat_path.is_none() && !pi.conflict && out.len() < need {
            out.push(pi);
        } else {
            rest.push(pi);
        }
    }
    ptx.items = rest;
    ptx.ready -= need;
    ptx.resolved -= need;
    out
}

/// 落位一批（Place 步）：无冲突条目立即入队（队列逐批长出），批次号随行；
/// placed 累计（弹窗范围行「已落位 M 项」数据源，T20-H）。
fn stream_place(st: &mut State) {
    let (profile_side, items) = {
        let Some(ptx) = &mut st.pending_tx else {
            return;
        };
        (ptx.profile_side, stream_take(ptx, ptx.ready))
    };
    let placed = items.len();
    let tasks: Vec<crate::transfer::NewTask> = items.into_iter().map(into_task).collect();
    let batch = enqueue_tasks(st, profile_side, tasks, 0);
    if let Some(ptx) = &mut st.pending_tx {
        ptx.batches.push(batch);
        ptx.placed += placed;
    }
}

/// 集齐（Settle 步，T20-E：无冲突与含冲突统一走此收尾）：全部剩余条目
/// （尾批无冲突 + 冲突留置）交 settle_batch——无冲突直接入队收尾；含冲突
/// 走弹窗批级决策（含「已落位 M 项」范围行），非冲突项决策后照常入队。
fn stream_settle(st: &mut State) {
    let items = match st
        .pending_tx
        .as_mut()
        .filter(|ptx| ptx.resolved == ptx.items.len())
    {
        Some(ptx) => std::mem::take(&mut ptx.items),
        None => return,
    };
    let Some(ptx) = &st.pending_tx else {
        return;
    };
    let (side, pane) = (ptx.profile_side, ptx.src_pane);
    settle_batch(st, side, pane, items);
}

/// 弹窗「取消」结算（T20-H）：丢弃 pending 批次；流式已落位行按批次号
/// 回溯清行（等待/扫描 → 已取消，传输中置块边界取消请求）。本地确认弹窗
/// 批次（batches 空）行为同旧——仅丢弃 pending。
/// F1：取消按钮与 shell 关闭路径（Esc / 遮罩 / ×）共用本结算，四路语义
/// 一致不留悬挂 pending（seq=0 批次超窗兜底不覆盖，残留即永久驻留）。
pub(crate) fn cancel_pending_stream(st: &mut State) {
    st.conflict_sel = None; // F-10 方案A：勾选态随弹窗关闭一并清空
    let Some(ptx) = st.pending_tx.take() else {
        return;
    };
    let n = ptx.items.len();
    if !ptx.batches.is_empty()
        && let Some(q) = st.transfer.as_ref()
    {
        q.cancel_batches(&ptx.batches);
    }
    // F1：取消 = 明确放弃整批——终端落对账日志，防「零入队零提示零日志」。
    st.terminal.push((
        crate::local::now_hms(),
        tfmt("已取消传输确认：{0} 项未入队", &[&n.to_string()]),
    ));
}

/// 预检超时兜底窗口（T20-G③ 引入，T20-H② 改滑动窗）：StatBatch 常态秒级
/// 回流，`at` 随任一回流刷新——连续 60s 零回流才算断流。固定窗在万级批次
/// 下会误判（10k÷32 并发 = 313 轮 × RTT ≈ 31~63s，贴窗），误判即把未回流
/// 项清成「远端不存在」静默放行（F-12）。
const STAT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 超窗仍未集齐的批次强制结算：未回流条目清 stat_path 放行（真冲突由
/// worker 传输路径的续传决策兜底），但不再静默——probe_incomplete 标记
/// 置真，结算点终端落对账提示。返回 probe_incomplete 条数（0 = 未触发）。
/// 本地确认弹窗批次（seq=0，无回流等待）不受兜底影响——弹窗可停留任意久。
fn force_settle_expired(ptx: &mut PendingTx, timeout: std::time::Duration) -> usize {
    if ptx.seq != 0 && ptx.at.elapsed() >= timeout {
        let mut n = 0;
        for pi in &mut ptx.items {
            if pi.stat_path.take().is_some() {
                pi.probe_incomplete = true;
                n += 1;
            }
        }
        n
    } else {
        0
    }
}

/// 批内条目。
pub struct PendingItem {
    pub task: crate::transfer::NewTask,
    /// Some(远程目标路径) = 待 STAT 确认；None = 已同步确认（本地目标）。
    pub stat_path: Option<String>,
    pub conflict: bool,
    /// 源 mtime（Unix 秒；面板行快照，冲突对比表「源」列）。
    pub src_mtime: i64,
    /// 目标大小 / mtime（远程 STAT / 本地 metadata；conflict 时填充）。
    pub dst_size: Option<u64>,
    pub dst_mtime: Option<i64>,
    /// 超窗兜底标记（T20-H②）：预检超窗时未回流条目按「远端不存在」放行，
    /// 但不再静默——本位标记置真，终端落对账提示（消费于超窗结算点）。
    pub probe_incomplete: bool,
}

/// 会话标识键（P2 同主机共享会话用：同主机同端口同用户 = 共享会话）。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SessionKey {
    pub host: String,
    pub port: u16,
    pub user: String,
}

/// 会话级上下文（对齐 C# SftpSessionService）：独立 SSH 会话 + 连接档案。
/// P1 每 tab 一会话（行为等同现状）；P2 起同主机多 tab 复用同一会话。
pub struct SideSession {
    /// 会话路由键（每会话唯一；apply_ssh_events 按 sid 把事件路由到挂靠 tab）。
    pub sid: u64,
    /// 会话标识键（P2 复用判定用：同主机同端口同用户 = 共享会话）。
    pub key: SessionKey,
    /// SSH 会话（连接提交时启动工作线程）。
    pub ssh: crate::ssh::Ssh,
    /// 会话的主机档案（连接提交时留存；初始目录/浏览记忆/终端标签用）。
    pub host: Option<crate::hosts::HostProfile>,
    /// 会话传输连接档案（入队远程任务用；Relay 任务两档案均入 Kind）。
    pub profile: Option<crate::transfer::Profile>,
    /// 连接状态（驱动挂靠 tab 的空态文案）。
    pub state: RemoteState,
    /// 登录主目录（Ev::Connected 落位时存档）：P2 会话复用时新 tab 的初始目录
    /// 兜底（Connected 事件只来一次，复用路径拿不到 root）。
    pub root: Option<String>,
    /// 主机指纹确认请求（worker 阻塞等回复；Some = 绘制确认覆盖层）。
    pub fp_ask: Option<dialogs::FpAsk>,
    /// 挂靠 tab 数（归零即断开并出池；P2 多 tab 共享时 >1）。
    pub refs: usize,
}

/// 远程 tab 浏览级上下文（原 RemoteCtx 的窗格/在途态平移；每 tab 独立）。
pub struct RemoteTab {
    /// 稳定 id（tab 条绘制层不解释；State 派发）。
    pub id: u32,
    /// 所属会话（0 = 占位空态 tab 未绑定会话）。
    pub sid: u64,
    /// tab 标题源（P4 §6）：远程 tab = 连接时 host_tab_naming 主机命名（落位/
    /// 复用落位时写）；tab 条合成时在标题后现算拼当前目录末段，空态只显本字段。
    pub title: String,
    /// 该 tab 远程文件面板。
    pub pane: panes::FilePane,
    /// 等待枚举结果的远程目录 (tag, 路径)：tag 全局唯一发号，Ev::Listing /
    /// Ev::Failed 按 tag 定位发起 tab；tag 不匹配 = 过期响应（等待期间用户又
    /// 点了别的目录）静默丢弃。
    pub wait: Option<(u64, String)>,
    /// 初始目录列举失败时的一次性回退目标（登录主目录；C# LastRemotePath 失效回退）。
    pub fallback: Option<String>,
    /// 远程删除在途计数（回零后刷新该 tab 面板）。
    pub del_pending: u64,
    /// 远程新建在途计数（Mkdir/CreateFile 已发未回；busy 链保活到
    /// Ev::Created，防 egui 空闲不重绘 —— 与 del_pending 同模式）。
    pub new_pending: u64,
    /// 远程编辑下载在途：(请求号, 远程路径, 文件名)。
    pub pending_ed: Option<(u64, String, String)>,
    /// 外部编辑器远程编辑会话（按远程路径唯一；C# _editSessions 同口径）。
    pub ext_edits: Vec<ExtEdit>,
    /// 外部编辑回传前远端 mtime 冲突探测在途：(请求号, 远程路径)。
    pub ext_stat: Option<(u64, String)>,
    /// 外部编辑回传写回在途（请求号；Ev::FileWritten 按此分流）。
    pub ext_write: Option<u64>,
    /// 外部编辑会话轮询节流点。
    pub ext_poll: std::time::Instant,
    /// 窗格状态栏当前是否挂着写锁等待文案（项6 独立清除标记——msg 为
    /// Option<String>，不能按中文前缀清除（英文模式残留），置位/清零成对）。
    pub lock_msg_on: bool,
}

/// 单侧远程域：tab 集（永不为空由「最后关闭回占位」维护）+ 活动 tab + 会话池。
pub struct RemoteSide {
    pub tabs: Vec<RemoteTab>,
    /// 活动远程 tab id（None = 该侧活动 tab 是本地 tab）。
    pub active: Option<u32>,
    /// 会话池（P1 每 tab 恰一会话；P2 同主机共享）。
    pub sessions: Vec<SideSession>,
}

/// 本地 tab 浏览级上下文（每 tab 独立本地视图）。
pub struct LocalTab {
    /// 稳定 id（State 派发）。
    pub id: u32,
    /// 该 tab 本地文件面板。
    pub pane: panes::FilePane,
}

/// 单侧本地域：tab 集 + 活动 tab id（None = 该侧活动 tab 是远程 tab）。
pub struct LocalSide {
    pub tabs: Vec<LocalTab>,
    pub active: Option<u32>,
}

/// 外部编辑器远程编辑会话（C# ExternalEditSession 等价物）：外部编辑器无保存
/// 回调，轮询本地临时副本快照（长度+mtime）检测「编辑器保存了」——快照变化起
/// 800ms 静默期（编辑器保存多为临时文件改名/多段写入，成簇到达），静默期后
/// 确认写锁已释放、快照稳定才触发一次询问回传。触发即更新基准：同一次保存不
/// 重复询问；之后再保存快照再次变化，会再次询问（C#「暂不保存」后同款）。
/// （C# 用 FileSystemWatcher 仅作「有动静」信号源，最终判定同为快照比对；
/// 此处 egui 帧驱动轻量轮询，语义等价且零依赖。）
pub struct ExtEdit {
    /// 发起侧主机标签（`[名称 (IP)]`，与终端日志 tag 同口径；重连/换主机后
    /// 旧会话静默作废，等价 C# 会话随视图销毁）。
    pub host_tag: String,
    /// 远程路径（会话键；C# item.FullPath）。
    pub remote_path: String,
    /// 文件名（弹窗正文）。
    pub name: String,
    /// 本地临时副本路径。
    pub local: std::path::PathBuf,
    /// 回传基准 mtime（下载时远端 mtime；None = 远端未报，冲突比对跳过）。
    pub base_mtime: Option<i64>,
    /// 基准快照 (长度, mtime 秒)；触发/回传时更新。
    pub baseline: (u64, i64),
    /// 快照开始变化的时刻（静默期起点）。
    pub quiet_since: Option<std::time::Instant>,
    /// 静默期内最近一次快照（稳定判定）。
    pub quiet_snap: Option<(u64, i64)>,
    /// 写锁等待起点（快照稳定但编辑器仍持写句柄时起算；None = 未在等锁）。
    /// 超时兜底分级依据：≥ SOFT 出软提示、≥ TIMEOUT 进兜底（项6）。
    pub lock_wait_since: Option<std::time::Instant>,
    /// 超时 WARN 已打过（去重硬要求：每次异常最多 1 条 WARN + 1 条 INFO，
    /// 禁止每轮 poll 刷屏——终端 400 行裁剪会冲掉回执）。
    pub lock_warned: bool,
    /// 本次等锁的超时框已弹过，不再重复弹（与 lock_warned 的日志去重分离：
    /// 弹框被占用时本轮不弹不置位，下轮补弹；关闭超时框后标记保持 true，
    /// 等待态照旧——CR-10）。
    pub lock_dialog_done: bool,
}

/// 占位远程 tab（空态引导；无会话 sid=0）：两侧初始态 / 该侧最后 tab 关闭重建用。
fn placeholder_remote_tab(id: u32) -> RemoteTab {
    RemoteTab {
        id,
        sid: 0,
        title: t("远程").to_owned(),
        pane: panes::FilePane {
            empty: true,
            ..Default::default()
        },
        wait: None,
        fallback: None,
        del_pending: 0,
        new_pending: 0,
        pending_ed: None,
        ext_edits: Vec::new(),
        ext_stat: None,
        ext_write: None,
        ext_poll: std::time::Instant::now(),
        lock_msg_on: false,
    }
}

/// 主窗口 UI 状态。
/// CR-1：本地 I/O 工作线程在途槽。req 留副本，线程异常退出时据此清 loading
/// 并回滚，不让窗格永久卡在「加载中…」。
pub struct LocalJob {
    pub side: usize,
    pub tab_id: u32,
    pub req: crate::local::LocalPaneReq,
    pub rx: std::sync::mpsc::Receiver<crate::local::LocalPaneReply>,
}

pub struct State {
    pub icons: crate::icons::Cache,
    /// 系统真实文件图标（C# SystemIconService；文件列表行专用）。
    pub sys_icons: crate::icons_sys::Cache,
    /// 左窗格宽；0 表示未初始化（首帧取中点）。
    pub left_w: f32,
    /// 下排行高（C# 初始 120）。
    pub bottom_h: f32,
    /// 模式胶囊：同步浏览（C# SyncBrowseToggle）。
    pub sync_browse: bool,
    /// 模式胶囊：文件比对（C# CompareToggle）。
    pub compare: bool,
    /// 双侧本地 tab 域（0=左 1=右；每 tab 独立本地视图）。
    pub locals: [LocalSide; 2],
    /// 双侧远程 tab 域（0=左 1=右；每 tab 独立窗格态，会话在 SideSession 池）。
    pub remotes: [RemoteSide; 2],
    /// tab id 派发器（自增；初始占位 tab 占 1、2）。
    pub next_tab_id: u32,
    /// 会话 id 派发器（自增；0 保留给未绑定会话的占位 tab）。
    pub next_sid: u64,
    /// tab 条亮相动画（G17-ANIM）：(tab id, 起始时刻)，按侧；合成 tab 条视图时
    /// 由 id 反查下标喂给 tab_strip（对齐现状 Tabs.born 存下标的绘制约定）。
    pub tab_born: [Option<(u32, std::time::Instant)>; 2],
    /// 当前弹窗（T2-6）。
    pub dialog: dialogs::Dialog,
    /// 右键菜单状态。
    pub menu: Option<dialogs::MenuState>,
    /// 右键菜单体上一帧的屏幕 rect（draw_menu 每帧回写、root() 帧首消费）：
    /// 「体外按下即关」判定用（C# MenuFlyout light dismiss 的按下时机）。
    pub menu_rect: Option<egui::Rect>,
    /// 队列菜单重连在途会话数（S-8）：>0 = 队列行菜单「重连」显示为
    /// 「正在重连…」置灰；Ev::Connected / Ev::Failed(连接) 逐个回账，归零恢复
    /// （C# RowReconnectItem_Click 重连等待态同位）。普通连接提交不记账，
    /// 菜单态与专用日志均不受影响。
    pub reconnecting: u32,
    /// 编辑框全局右键菜单钩子（面包屑编辑框等 TextEdit 的复制/剪切/粘贴）。
    pub ctx_menu: ctx_menu::CtxMenu,
    /// 面包屑编辑态：来源窗格 + 路径文本。
    pub crumb_edit: Option<(usize, String)>,
    /// 设置面板值（演示态）。
    pub settings: dialogs::Settings,
    /// 窗格层发来的远程列目录请求（(侧, tab id, 路径)；root() 统一经该 tab
    /// 所属会话发出；id 由活动 tab 盖章 —— 请求来自发起帧的活动 tab）。
    pub list_reqs: Vec<(usize, u32, String)>,
    /// 窗格层发来的远程新建请求（(侧, tab id, 全路径, 是否目录)；root() 统一经
    /// 该 tab 所属会话发出；完成走 Ev::Created 重列）。
    pub new_reqs: Vec<(usize, u32, String, bool)>,
    /// 窗格层发来的远程重命名请求 (side, tab_id, old, new)——P0-1 落盘管线
    /// （同 new_reqs：inflight 发号 + new_pending 保活，Ev::Created 统一重列）。
    pub rename_reqs: Vec<(usize, u32, String, String)>,
    /// 目录树懒展开请求在途 (侧, tab id, tag, 路径)：盖章处发号，root() 消费
    /// 发出后保留在途，Ev::Listing / Ev::Failed 按 tag 命中填树/复位，过期 tag
    /// 保留在途等待真正响应。
    pub tree_req: Option<(usize, u32, u64, String)>,
    /// 同步挂接对（P4，sync::SyncBase）：带 tab 维度的双侧基准根；None = 未配置。
    pub sync_base: Option<sync::SyncBase>,
    /// 同步镜像在途的 (远程侧, tab id, 目标)（P4 挂接对口径）：Ev::Failed 命中
    /// 即报失配，Ev::Listing 命中（tag→tab 后与 (侧, tab) 比对）即成功。
    pub sync_mirror: Option<(usize, u32, String)>,
    /// 窗格导航事件 (侧, tab id, 路径)（窗格层发 (侧, 路径)，root() 盖活动
    /// tab id 章；消费驱动镜像/比对）。
    pub sync_nav: Option<(usize, u32, String)>,
    /// 比对标记失配重排请求（窗格层置位）。
    pub compare_req: bool,
    /// 拖拽传输请求（FV-25：发起/源侧, 被拖条目行号；窗格层 Drop 置位）。
    pub dnd_transfer: Option<(usize, Vec<usize>)>,
    /// 文件夹选择在途（FV-01：open_dir 后台 STA 线程弹框；root() 每帧轮询回执）。
    pub pick: Option<crate::local::PickJob>,
    /// 传输队列（首次发起传输时惰性启动工作线程）。
    pub transfer: Option<crate::transfer::TransferQueue>,
    /// 队列页签（0 队列中 / 1 传输失败 / 2 传输完成）。
    pub queue_tab: usize,
    /// 终端日志（(HH:mm:ss, 文本)，末尾对齐显示）。
    pub terminal: Vec<(String, String)>,
    /// 待入队传输批次（覆盖确认进行中）。
    pub pending_tx: Option<PendingTx>,
    /// F-10 方案A：冲突确认弹窗逐项勾选态（与弹窗 rows 行序对齐；None = 无
    /// 确认弹窗）。settle_batch / 本地批次创建时填默认值（conflict_defaults），
    /// 应用/取消后清空。存 State 而非 Dialog 内：弹窗每帧重绘，勾选改动须
    /// 跨帧存活且与 rows 解耦（rows 可被流式 Settle 重建）。
    pub conflict_sel: Option<Vec<bool>>,
    /// 统一请求号/tag 发号源（自增）：list/delete/mkdir/create_file 的 tag 与
    /// Stat/ReadFile/WriteFile/Exec 的 req 同一计数空间，全局唯一互不混淆。
    pub stat_seq: u64,
    /// 在途命令 tag → 归属 (侧, tab id)：Ev::Deleted / Ev::Created 按 tag 定位
    /// 重列目标 tab；tab 关闭时清理，迟到响应落「未知 tag 丢弃」。
    pub inflight: std::collections::HashMap<u64, (usize, u32)>,
    /// 忙期间活动任务的归属 tab 集合（owner.src ∪ owner.dst 去重，每帧活动时
    /// 重算）：忙→静沿只定向刷新这些 tab（P3，替代全局两侧刷新）。
    pub q_owner_tabs: Vec<(u8, u32)>,
    /// 上一帧队列是否有活动任务（忙→静沿触发两侧刷新）。
    pub q_active: bool,
    /// T16 忙期间节流刷新快照（归属 tab -> (上次刷新时刻, 完成计数基线)）：
    /// C# baseline+lastSeen 的 per-tab 等价；静沿/空闲清空（每批次重新取基线）。
    pub q_refresh: std::collections::HashMap<(u8, u32), (f64, u64)>,
    /// G17「取消队列并关闭」：(侧, tab id, 起始时刻) 等待块边界退出；退出或
    /// 5s 超时兜底后执行关闭。
    pub tab_close_wait: Option<(usize, u32, std::time::Instant)>,
    /// 校验码：本地结果（None=未启动/在途 exec）。
    pub chk_local: Option<Result<String, String>>,
    /// 校验码：exec 在途（请求号, 算法下标）。
    pub chk_pending: Option<(u64, usize)>,
    /// 校验码：本地线程在途。
    pub chk_computing: bool,
    /// 校验码：本地计算线程回执。
    pub chk_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// CR-1：本地列目录 / 删除的线程在途槽（帧尾统一驱动，UI 帧内零文件系统 I/O）。
    pub local_jobs: Vec<LocalJob>,
    /// CR-2：设置落盘失败在途（帧首由 `settings::save_error` 刷新）。主窗口
    /// 底部全局条据此切出一条、心跳据此走慢档兜底；成功落盘后自动归零。
    pub save_err_pending: bool,
    /// 打开的编辑器文档 + 保存回传事件队列。
    pub editors: crate::editor::EditorHub,
    /// 内置编辑器回传前远端 mtime 冲突探测在途：请求号沿用 stat_seq 发号，
    /// Ev::Stat 按 req 认领；副本内容随槽带入（分派时一次 fs::read，无冲突
    /// 直通分支复用，单次保存不再读两遍全文；冲突路径弹窗「覆盖」走 doc_upload
    /// 重读最新内容——弹框前槽已 take 清空，不会拿旧字节覆盖）。
    pub doc_stat: Option<DocStatSlot>,
    /// 主机指纹确认请求——HostPanel「测试连接」专用槽（无侧归属；窗格会话的
    /// 指纹请求在各会话 SideSession.fp_ask，覆盖层按 会话池 → 测试槽 顺序查找）。
    pub fp_ask_test: Option<dialogs::FpAsk>,
    /// C2（F-9）：弹窗帧滚轮暂存（root 帧首 stash + dialogs::draw 入口回填）。
    /// ScrollArea 读后清零且不看层、frame 内首个命中者独占——不 stash 会被
    /// 遮罩下的主界面滚动区抢先消费，弹窗内部滚动区（设置页/传输确认表/导入
    /// 预览）永远拿不到滚轮。
    pub scroll_stash: egui::Vec2,
    /// C3（F-9）：上一帧弹窗变体判别值（dialogs::draw 入口算 Dialog::kind()，
    /// 变体变更且非 None = 弹窗打开首帧 → dialog_fresh 供 body 聚焦首控件）。
    pub dialog_kind: u8,
    /// C3②（F-9）：弹窗打开首帧旗标（draw 入口重算，body 读它做初焦点）。
    pub dialog_fresh: bool,
    /// C3③（F20/F-9）：Enter→主钮旗标（shell 入口算：无焦点控件或焦点在文本
    /// 框时才置——焦点在可点控件上时 Space/Enter 由 egui 直接触发其点击，防双触发）。
    pub shell_enter: bool,
}

/// 远程连接状态。
pub enum RemoteState {
    Off,
    Connecting,
    Failed(String),
}

/// 统一请求号 / tag 发号（stat_seq 自增）：list/delete/mkdir/create_file 的
/// tag 与 Stat/ReadFile/WriteFile/Exec 的 req 同一计数空间，全局唯一。
pub(crate) fn next_seq(st: &mut State) -> u64 {
    st.stat_seq += 1;
    st.stat_seq
}

/// 连接落位初始目录优先级（C# AttachConnectedSession）：同步目录 > 上次浏览
/// 目录（fallback = 登录主目录）> 登录主目录。返回 (初始目录, fallback, 同步基准)。
/// Ev::Connected 落位与会话复用落位共用，保证两条路径行为一致。
pub(crate) fn initial_remote_dir(
    h: &crate::hosts::HostProfile,
    root: &str,
) -> (String, Option<String>, Option<String>) {
    if h.sync_enabled && !h.remote_sync_path.trim().is_empty() {
        let p = normalize_remote(&h.remote_sync_path);
        (p.clone(), None, Some(p))
    } else if !h.last_remote_path.trim().is_empty() {
        let p = normalize_remote(&h.last_remote_path);
        (p.clone(), Some(root.to_owned()), Some(p))
    } else {
        (root.to_owned(), None, None)
    }
}

/// 侧 → 活动窗格是否本地视图（本地 tab = 是；远程 tab = 否）。两侧对称窗格的
/// 自驱判定（弹窗/传输/同步共用；C# 每 tab 独立 FileView 的等价物）。
/// 不变式：每侧 locals/remotes 恰有一边持活动 tab（异常双 None 兜底本地视图）。
pub(crate) fn pane_is_local(st: &State, side: usize) -> bool {
    match (st.locals[side].active, st.remotes[side].active) {
        (Some(_), _) => true,
        (None, Some(_)) => false,
        // 空态兜底（不变式外路径）：按本地视图兜底。
        (None, None) => true,
    }
}

/// 侧 → 活动窗格视图：本地 tab = 该侧活动本地 tab 的 pane；远程 tab = 该侧
/// 活动 remote tab 的 pane。签名不变（P1 数据模型换血，寻址改经 tab）。
pub(crate) fn pane_view(st: &State, side: usize) -> &panes::FilePane {
    match pane_is_local(st, side) {
        true => {
            &active_local_tab(st, side)
                .expect("local side keeps its active tab")
                .pane
        }
        false => {
            &active_remote_tab(st, side)
                .expect("remote side keeps its active tab")
                .pane
        }
    }
}

pub(crate) fn pane_view_mut(st: &mut State, side: usize) -> &mut panes::FilePane {
    match pane_is_local(st, side) {
        true => {
            &mut active_local_tab_mut(st, side)
                .expect("local side keeps its active tab")
                .pane
        }
        false => {
            &mut active_remote_tab_mut(st, side)
                .expect("remote side keeps its active tab")
                .pane
        }
    }
}

/// 侧 → 活动本地 tab 槽位（root() 先读后借，绕开多字段同时可变借用）。
fn active_local_slot(st: &State, side: usize) -> Option<usize> {
    let ls = &st.locals[side];
    let id = ls.active?;
    ls.tabs.iter().position(|t| t.id == id)
}

/// 侧 → 活动远程 tab 槽位（root() 先读后借用）。
fn active_remote_slot(st: &State, side: usize) -> Option<usize> {
    let rs = &st.remotes[side];
    let id = rs.active?;
    rs.tabs.iter().position(|t| t.id == id)
}

/// 侧 → 活动本地 tab（严格按 active id 寻址；缺位 = None）。
fn active_local_tab(st: &State, side: usize) -> Option<&LocalTab> {
    let ls = &st.locals[side];
    let id = ls.active?;
    ls.tabs.iter().find(|t| t.id == id)
}

/// 侧 → 活动本地 tab（可变版）。
fn active_local_tab_mut(st: &mut State, side: usize) -> Option<&mut LocalTab> {
    let ls = &mut st.locals[side];
    let id = ls.active?;
    ls.tabs.iter_mut().find(|t| t.id == id)
}

/// 侧 → 活动远程 tab（严格按 active id 寻址；缺位 = None）。
pub(crate) fn active_remote_tab(st: &State, side: usize) -> Option<&RemoteTab> {
    let rs = &st.remotes[side];
    let id = rs.active?;
    rs.tabs.iter().find(|t| t.id == id)
}

/// 侧 → 活动远程 tab（可变版）。
pub(crate) fn active_remote_tab_mut(st: &mut State, side: usize) -> Option<&mut RemoteTab> {
    let rs = &mut st.remotes[side];
    let id = rs.active?;
    rs.tabs.iter_mut().find(|t| t.id == id)
}

/// 侧 → 该侧活动 tab id（本地优先；两侧恰一边持活动 tab）。
pub(crate) fn active_tab_id(st: &State, side: usize) -> Option<u32> {
    st.locals[side].active.or(st.remotes[side].active)
}

/// 按 id 找远程 tab（可变）。
pub(crate) fn find_remote_tab_mut(st: &mut State, side: usize, id: u32) -> Option<&mut RemoteTab> {
    st.remotes[side].tabs.iter_mut().find(|t| t.id == id)
}

/// 按会话 sid 找会话（可变）。
pub(crate) fn session_mut(st: &mut State, side: usize, sid: u64) -> Option<&mut SideSession> {
    st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid)
}

/// 按 tab id 找其挂靠会话（P1 每 tab 恰一会话）。
pub(crate) fn session_of_tab(st: &State, side: usize, tab_id: u32) -> Option<&SideSession> {
    let sid = st.remotes[side].tabs.iter().find(|t| t.id == tab_id)?.sid;
    st.remotes[side].sessions.iter().find(|s| s.sid == sid)
}

/// 侧 → 活动远程 tab 的会话（活动 tab 是 Remote 且会话在池；STAT 回流 / 入队
/// 档案取用等处按此定位）。
pub(crate) fn active_remote_session(st: &State, side: usize) -> Option<&SideSession> {
    let tab = active_remote_tab(st, side)?;
    session_of_tab(st, side, tab.id)
}

/// tab id → 窗格（本地 tab = 该侧本地 pane；远程 tab = 该 tab 远程面板）。
/// 挂接对寻址用（sync.rs compare/evaluate 按挂接对端定位，非活动视图）。
pub(crate) fn tab_pane(st: &State, side: usize, tab_id: u32) -> Option<&panes::FilePane> {
    st.locals[side]
        .tabs
        .iter()
        .find(|t| t.id == tab_id)
        .map(|t| &t.pane)
        .or_else(|| {
            st.remotes[side]
                .tabs
                .iter()
                .find(|t| t.id == tab_id)
                .map(|t| &t.pane)
        })
}

/// tab id → 窗格可变借用（同 tab_pane）。
pub(crate) fn tab_pane_mut(
    st: &mut State,
    side: usize,
    tab_id: u32,
) -> Option<&mut panes::FilePane> {
    let local = st.locals[side]
        .tabs
        .iter_mut()
        .find(|t| t.id == tab_id)
        .map(|t| &mut t.pane);
    local.or_else(|| {
        st.remotes[side]
            .tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .map(|t| &mut t.pane)
    })
}

/// 侧 tab 条合成视图：本地 tab 在前（「本地」固定第一，对齐现状）+ 远程 tab 在
/// 后；active 下标 = 活动项在扁平列表中的位置；born 由 tab id 反查下标。绘制层
/// （tab_strip/folder）零改动——每帧合成临时 Tabs 传入，回写经 id 映射。
fn build_tabs_view(st: &State, side: usize) -> panes::Tabs {
    let mut items = Vec::new();
    for lt in &st.locals[side].tabs {
        // 本地 tab 固定标题「本地」，不随导航目录变化。
        items.push(panes::TabItem {
            id: lt.id,
            title: t("本地").to_owned(),
            kind: panes::TabKind::Local,
        });
    }
    for rt in &st.remotes[side].tabs {
        // 远程 tab 标题 = 连接落位写入的主机命名（host_tab_naming：名称+ip），
        // 不随导航目录变化；空态占位 tab 为「远程」。
        items.push(panes::TabItem {
            id: rt.id,
            title: rt.title.clone(),
            kind: panes::TabKind::Remote,
        });
    }
    let active_id = st.locals[side].active.or(st.remotes[side].active);
    let active = items
        .iter()
        .position(|i| Some(i.id) == active_id)
        .unwrap_or(0);
    let born = st.tab_born[side]
        .and_then(|(id, t0)| items.iter().position(|i| i.id == id).map(|p| (p, t0)));
    panes::Tabs {
        items,
        active,
        born,
    }
}

impl Default for State {
    fn default() -> Self {
        // 初始视觉对齐现状：左侧 = 「本地」tab（本地根视图，活动）+ 该侧无远程
        // tab；右侧 = 「远程」占位 tab（空态引导，活动）。占位远程 tab 不进
        // 左侧（tab 条只显示已有 tab，与现状 left_tabs=[本地] 一致）。
        let mut root_local = panes::FilePane::default();
        // 本地根视图：「此电脑」驱动器列表（C# NavigateLocalRoot 初始态）。
        crate::local::navigate_root(&mut root_local);
        Self {
            icons: crate::icons::Cache::default(),
            sys_icons: crate::icons_sys::Cache::default(),
            left_w: 0.0,
            bottom_h: theme::BOTTOM_ROW_MIN,
            sync_browse: false,
            compare: false,
            locals: [
                LocalSide {
                    tabs: vec![LocalTab {
                        id: 1,
                        pane: root_local,
                    }],
                    active: Some(1),
                },
                // 右侧无本地 tab（现状 right_tabs=[远程] 同款）。
                LocalSide {
                    tabs: Vec::new(),
                    active: None,
                },
            ],
            remotes: [
                RemoteSide {
                    tabs: Vec::new(),
                    active: None,
                    sessions: Vec::new(),
                },
                RemoteSide {
                    tabs: vec![placeholder_remote_tab(2)],
                    active: Some(2),
                    sessions: Vec::new(),
                },
            ],
            next_tab_id: 3,
            next_sid: 1,
            tab_born: [None, None],
            dialog: dialogs::Dialog::None,
            menu: None,
            menu_rect: None,
            reconnecting: 0,
            ctx_menu: ctx_menu::CtxMenu::default(),
            crumb_edit: None,
            // 设置弹窗初值来自 settings.json（C# AppSettingsService 同款）。
            settings: dialogs::Settings::from_store(),
            list_reqs: Vec::new(),
            q_refresh: std::collections::HashMap::new(),
            new_reqs: Vec::new(),
            rename_reqs: Vec::new(),
            tree_req: None,
            sync_base: None,
            sync_mirror: None,
            sync_nav: None,
            compare_req: false,
            dnd_transfer: None,
            pick: None,
            transfer: None,
            queue_tab: 0,
            terminal: Vec::new(),
            pending_tx: None,
            conflict_sel: None,
            stat_seq: 0,
            inflight: std::collections::HashMap::new(),
            q_owner_tabs: Vec::new(),
            q_active: false,
            tab_close_wait: None,
            chk_local: None,
            chk_pending: None,
            chk_computing: false,
            chk_rx: None,
            local_jobs: Vec::new(),
            save_err_pending: false,
            editors: crate::editor::EditorHub::new(),
            doc_stat: None,
            fp_ask_test: None,
            scroll_stash: egui::Vec2::ZERO,
            dialog_kind: 0,
            dialog_fresh: false,
            shell_enter: false,
        }
    }
}

// ── 光标意图收口（F-8）────────────────────────────────────────────────
// egui 0.37 的 output.cursor_icon 按视口存放，但过去各控件各自 set_cursor_icon，
// 注册顺序即覆盖顺序：帧尾 edge_resize 会反转菜单 / 弹层的 Default 复位，
// 且「hover 才设形」的控件在指针离开后无人覆盖，光标会跨帧粘滞。
// 改为每帧各控件只上报意图，帧尾按优先级裁决一次写入；无意图即 Default。
/// 光标意图优先级（高→低）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorLayer {
    Dialog,
    Drag,
    Edge,
    Hover,
}

impl CursorLayer {
    /// 数值越大优先级越高；同层保留先写者，避免注册更晚的边缘带反超拖拽。
    fn rank(self) -> u8 {
        match self {
            CursorLayer::Hover => 0,
            CursorLayer::Edge => 1,
            CursorLayer::Drag => 2,
            CursorLayer::Dialog => 3,
        }
    }
}

/// 意图槽按视口分：eframe 多视口共享 `ctx.data_mut` 的 IdTypeMap，
/// 而 output.cursor_icon 是按视口存的，共用固定 Id 会让先裁决的视口取走后者意图。
fn cursor_slot(ctx: &egui::Context) -> egui::Id {
    // 0.37 起 Id::new 已弃用；Id::unique 是固定 seed 的稳定哈希，可作固定槽。
    egui::Id::unique(("ifree.cursor_intent", ctx.viewport_id()))
}

/// 上报本帧光标意图；仅更高优先级覆盖，同层先写者胜。
pub(crate) fn cursor_intent(ctx: &egui::Context, layer: CursorLayer, icon: egui::CursorIcon) {
    let id = cursor_slot(ctx);
    ctx.data_mut(|d| {
        let cur: Option<(CursorLayer, egui::CursorIcon)> = d.get_temp(id).unwrap_or(None);
        if cur
            .map(|(prev, _)| layer.rank() > prev.rank())
            .unwrap_or(true)
        {
            d.insert_temp(id, Some((layer, icon)));
        }
    });
}

/// 帧尾唯一一次 `set_cursor_icon`：取走意图 → 写入；无意图 → Default。
pub fn cursor_decide(ctx: &egui::Context) {
    let id = cursor_slot(ctx);
    let intent = ctx
        .data_mut(|d| d.remove_temp::<Option<(CursorLayer, egui::CursorIcon)>>(id))
        .flatten();
    ctx.set_cursor_icon(
        intent
            .map(|(_, icon)| icon)
            .unwrap_or(egui::CursorIcon::Default),
    );
}

/// 本帧是否有弹层 / 菜单 / 待决指纹确认。main.rs 在 crate 根，访问不到私有的
/// ui::dialogs，故由本模块算好再传给 edge_resize。
pub fn frame_modal(st: &State) -> bool {
    !matches!(st.dialog, dialogs::Dialog::None) || st.menu.is_some() || dialogs::fp_ask_pending(st)
}

/// CR-2：失败条几何（纯函数，可钉）。返回 (剩余主区, 条 rect)。窗口过矮时
/// 不切条，避免负高度 rect 破坏四容器几何（此时失败态仍有终端日志兜底）。
fn save_error_bar_rect(full: Rect, pending: bool) -> (Rect, Option<Rect>) {
    if !pending || full.height() <= HEADER_H + SAVE_BAR_H + TOP_ROW_MIN + BOTTOM_MIN {
        return (full, None);
    }
    let (rest, bar) = full.split_top_bottom_at_y(full.bottom() - SAVE_BAR_H);
    (rest, Some(bar))
}

/// CR-2：设置落盘失败全局条（主窗口底部，持续可见）。只显示、不抢焦点、
/// 不弹窗、不拦操作；恢复可写后 `maybe_flush` 自动落盘并清失败态，下一帧
/// 该条自动消失。错误原因取持续快照，终端日志的一次性 take 不影响它。
fn draw_save_error_bar(ui: &mut egui::Ui, r: Rect, p: &theme::Palette) {
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(r)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.painter().rect_filled(r, 0.0, p.dialog_title_bar);
    bar.painter().line_segment(
        [r.left_top(), r.right_top()],
        egui::Stroke::new(1.0, theme::WARNING),
    );
    bar.add_space(10.0);
    bar.add(
        egui::Label::new(
            egui::RichText::new(t("设置未保存，正在自动重试"))
                .color(theme::WARNING)
                .size(12.0),
        )
        .truncate(),
    );
    bar.add_space(8.0);
    bar.add(
        egui::Label::new(
            egui::RichText::new(crate::settings::save_error().unwrap_or_default())
                .color(p.text_secondary)
                .size(12.0),
        )
        .truncate(),
    );
}

/// 每帧入口。
pub fn root(ui: &mut egui::Ui, st: &mut State) {
    // C2（F-9）：弹窗帧滚轮收口——ScrollArea 读后清零且不看层（frame 内首个
    // 命中者独占），不 stash 会被遮罩下的主界面滚动区抢先消费。帧首 stash +
    // 清零 → dialogs::draw 入口回填：主界面滚动区拿 0，弹窗内部照常。
    if !matches!(st.dialog, dialogs::Dialog::None) || dialogs::fp_ask_pending(st) {
        st.scroll_stash = ui
            .ctx()
            .input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta));
    }
    // 光标复位不在帧首做：各控件只上报意图，帧尾 ui::cursor_decide 统一裁决
    // 一次写入（无意图即 Default），既避免跨帧粘滞，也避免注册顺序反转优先级。
    let p = theme::palette(ui.ctx());

    // 设置防抖落盘（500ms 防抖 + CR-2 失败退避重试；等待期间主动预约重绘）。
    crate::settings::maybe_flush(ui.ctx());
    // 设置落盘失败：终端日志仍一条（不刷屏），CR-2 另置持续失败快照供
    // 主窗口底部全局条每帧绘制——tab 关闭 / 终端抽屉收起都看得见。
    if let Some(e) = crate::settings::take_save_error() {
        st.terminal.push((crate::local::now_hms(), e));
    }
    st.save_err_pending = crate::settings::save_error().is_some();

    // 右键菜单一势关闭+重开（对齐 C# MenuFlyout light dismiss：按下即关，抬起
    // 照常派发给下层控件）：菜单开着且本帧有主/副键「按下」、按下点在上一帧
    // 菜单体 rect 外 → 先关旧菜单。关在窗格绘制之前——抬起时行/表头/foot 的
    // secondary_clicked 照常命中，右键同势关旧开新，不必先手动关菜单（旧实现
    // 覆盖层挂全窗 catcher：最后注册吞掉一切点击且只认左键，右键被静默吞掉，
    // 即「右键很涩」的根因）。按下点在菜单体内：不关，交给 draw_menu 的体感应
    // 面兜住防穿透。位置取 interact_pos（按下帧即按下点；同帧按下+抬起时
    // press_origin 已被清空，interact_pos 仍为点击位置）。
    if st.menu.is_some()
        && let Some(r) = st.menu_rect
    {
        let outside_press = ui.ctx().input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_none_or(|p| !r.contains(p))
        });
        if outside_press {
            st.menu = None;
        }
    }
    // 菜单体 rect 由本帧 draw_menu 重写（菜单刚开/刚关的帧为 None，跳过判定）。
    st.menu_rect = None;

    // SSH 事件入账 + 空态文案随连接状态（先于窗格绘制）。
    apply_ssh_events(st, ui.ctx());

    // 注册服务异步事件（手动检查更新完成 → 更新卡结果文案；C# await 返回值
    // 在 egui 事件轮次里等价落位）。
    for ev in crate::registration::service::global().take_events() {
        match ev {
            crate::registration::service::RegEvent::RemoteRefreshDone(ok) => {
                st.settings.update_checking = false;
                st.settings.update_check_result = if ok {
                    let s = crate::registration::service::global().state();
                    if s.update_available && !s.latest_version.is_empty() {
                        crate::i18n::tfmt("发现新版本 v{0}，可从官网下载", &[&s.latest_version])
                    } else {
                        t("已是最新版本").to_owned()
                    }
                } else {
                    t("无法连接更新服务，请稍后再试").to_owned()
                };
            }
        }
    }

    // 传输队列：worker 日志入终端面板；批次结束汇总（T18-2 LogTransferDone）
    // 同通路入终端；忙→静沿刷新两侧（传输结果立即可见）。
    if let Some(q) = &st.transfer {
        for line in q.drain_log() {
            st.terminal.push((crate::local::now_hms(), line));
        }
        for line in q.drain_batch_reports() {
            st.terminal.push((crate::local::now_hms(), line));
        }
        if st.terminal.len() > 200 {
            st.terminal.drain(0..st.terminal.len() - 200);
        }
        let (active, owners) = q.with_items(|items| {
            let active = items.iter().any(|i| {
                matches!(
                    i.status,
                    crate::transfer::Status::Waiting
                        | crate::transfer::Status::Transferring
                        | crate::transfer::Status::Verifying
                )
            });
            let tabs =
                crate::transfer::collect_owner_tabs(items.iter().map(|i| (&i.owner, i.status)));
            (active, tabs)
        });
        if st.q_active && !active {
            // 忙→静沿：只定向刷新活动任务的归属 tab（P3，C# per-tab 刷新语义；
            // 本地 tab = 本地刷新 + 动效，远程 tab = 按当前目录重列，P2 发号路径）。
            for (side, tab_id) in std::mem::take(&mut st.q_owner_tabs) {
                let side = side as usize;
                if let Some(lt) = st.locals[side].tabs.iter_mut().find(|t| t.id == tab_id) {
                    crate::local::refresh(&mut lt.pane);
                    lt.pane.refresh_at = Some(ui.ctx().time());
                    continue;
                }
                let rp = st.remotes[side]
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .and_then(|t| t.pane.remote_path.clone());
                if let Some(rp) = rp {
                    st.list_reqs.push((side, tab_id, rp));
                }
            }
            // 批次收尾：节流快照清空（T16，下一批次重新取基线）。
            st.q_refresh.clear();
        } else {
            // 忙期间每帧重算归属集合（新入队任务即时纳入定向刷新范围）。
            st.q_owner_tabs = owners;
            if active {
                // T16 忙期间节流刷新（C# RefreshOppositeWhileTransferringAsync
                // :3311-3352 按帧等价）：每 3s 检查归属 tab，完成计数有增量且
                // 用户仍停在目标目录才重列目标窗格——传输中逐步可见，不干扰
                // 翻走浏览、慢传计数不变不打无谓请求。忙→静沿最终刷新仍兜底
                // （上分支），两机制互补：节流管传输中、静沿管收尾。
                let now = ui.ctx().time();
                let targets = q.with_items(|items| {
                    crate::transfer::refresh_targets(
                        items
                            .iter()
                            .map(|i| (&i.owner, i.dest_is_remote(), i.dest.as_str(), i.status)),
                    )
                });
                for ((side, tab_id), done, dests) in targets {
                    let side_idx = side as usize;
                    // 快照：批次忙起点取基线（C# baseline），刷新时更新（lastSeen）。
                    let snap = st.q_refresh.entry((side, tab_id)).or_insert((now, done));
                    // 该 tab 当前目录（本地宽松 / 远程 Ordinal 由 same_dest_dir 分流）。
                    let cur = st
                        .locals
                        .get(side_idx)
                        .and_then(|l| l.tabs.iter().find(|t| t.id == tab_id))
                        .map(|t| {
                            (
                                t.pane
                                    .path
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().into_owned()),
                                false,
                            )
                        })
                        .or_else(|| {
                            st.remotes
                                .get(side_idx)
                                .and_then(|r| r.tabs.iter().find(|t| t.id == tab_id))
                                .map(|t| (t.pane.remote_path.clone(), true))
                        });
                    let same_dir = cur.is_some_and(|(p, remote)| {
                        p.is_some_and(|p| {
                            dests
                                .iter()
                                .any(|d| crate::transfer::same_dest_dir(&p, d, remote))
                        })
                    });
                    if crate::transfer::refresh_due(
                        now,
                        snap.0,
                        done.saturating_sub(snap.1),
                        same_dir,
                    ) {
                        // 刷新时刻与完成计数快照随刷新更新。
                        *snap = (now, done);
                        // 刷新动作与忙→静沿同款：本地 = 刷新 + 动效；远程 = 发号重列。
                        if let Some(lt) =
                            st.locals[side_idx].tabs.iter_mut().find(|t| t.id == tab_id)
                        {
                            crate::local::refresh(&mut lt.pane);
                            lt.pane.refresh_at = Some(now);
                        } else if let Some(rp) = st.remotes[side_idx]
                            .tabs
                            .iter()
                            .find(|t| t.id == tab_id)
                            .and_then(|t| t.pane.remote_path.clone())
                        {
                            st.list_reqs.push((side_idx, tab_id, rp));
                        }
                    }
                }
            } else {
                // 空闲帧：快照清空（下次忙重新取基线）。
                st.q_refresh.clear();
            }
        }
        st.q_active = active;
    }

    // 外部编辑器远程会话轮询（快照静默检测 → 询问回传；C# ExternalEditSession）。
    poll_ext_edits(st);

    // 校验码本地线程回执入账。
    if st.chk_computing
        && let Some(rx) = &st.chk_rx
        && let Ok(r) = rx.try_recv()
    {
        st.chk_local = Some(r);
        st.chk_computing = false;
        st.chk_rx = None;
    }

    // 编辑器保存回传（T7/CR-6）：远程文档写盘成功 → 副本读出 → 会话匹配 →
    // STAT 冲突探测（±1s 判据同外部链路）→ 无冲突写回远端；读失败 / 失配 /
    // 写失败一律回写 doc 失败态（副本保留，再按 Ctrl+S 可重试），禁止吞事件。
    // 本地文档写盘已在编辑器内完成。
    for ev in crate::editor::EditorHub::drain_saved() {
        saved_upload_dispatch(st, ui.ctx(), ev);
    }
    // 编辑器窗口声明/回收（deferred viewport：不声明即关；主题随主窗口）。
    editor_window::show_all(
        ui.ctx(),
        &mut st.editors,
        ui.ctx().theme() == egui::Theme::Dark,
    );

    // 各远程 tab 空态文案随其挂靠会话的连接状态每帧刷新（启动时的 t() 缓存串
    // 不随语言切换；C# EmptyStateTitle / EmptyStateMessage）。会话态先读后写
    // （tab 可变借用与会话只读借用分离）。
    for side in 0..2 {
        for ti in 0..st.remotes[side].tabs.len() {
            let sid = st.remotes[side].tabs[ti].sid;
            let snap = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == sid)
                .map(|s| match &s.state {
                    RemoteState::Off => (0u8, String::new()),
                    RemoteState::Connecting => (1, String::new()),
                    RemoteState::Failed(m) => (2, m.clone()),
                })
                .unwrap_or((0, String::new()));
            let tab = &mut st.remotes[side].tabs[ti];
            match snap.0 {
                1 => {
                    tab.pane.empty = true;
                    tab.pane.empty_title = t("正在连接…").to_owned();
                    tab.pane.empty_sub = t("正在建立 SSH 连接").to_owned();
                }
                2 => {
                    tab.pane.empty = true;
                    tab.pane.empty_title = t("连接失败").to_owned();
                    tab.pane.empty_sub = snap.1.chars().take(60).collect();
                }
                _ => {
                    if tab.pane.empty {
                        tab.pane.empty_title = t("先选择一个主机").to_owned();
                        tab.pane.empty_sub = t("选择要浏览的本地或远端文件系统").to_owned();
                    }
                }
            }
        }
    }

    let full = ui.max_rect();
    // CR-2：落盘失败时从窗口底部切出一条全局提示条（不覆盖四个窗格，也不随
    // tab 关闭 / 终端抽屉收起消失）。成功落盘后 rect 变 None，布局自动还原。
    let (full, save_bar) = save_error_bar_rect(full, st.save_err_pending);

    // 全窗打底：HeaderAreaBrush（C# ShellBorder 背景/边框/两条分隔条同色）。
    ui.painter().rect_filled(full, 0.0, p.header_area);

    let (header_rect, rest) = full.split_top_bottom_at_y(full.top() + HEADER_H);

    let shell = Rect::from_min_max(
        egui::pos2(rest.left() + SHELL_BORDER.0, rest.top()),
        egui::pos2(
            rest.right() - SHELL_BORDER.2,
            rest.bottom() - SHELL_BORDER.3,
        ),
    );

    let gutter = theme::GUTTER;
    if st.left_w <= 0.0 {
        st.left_w = (shell.width() - gutter) / 2.0;
    }
    let max_left = (shell.width() - gutter - PANE_MIN).max(PANE_MIN);
    st.left_w = st.left_w.clamp(PANE_MIN, max_left);
    let vsplit_x = shell.left() + st.left_w;

    let max_bottom = (shell.height() - gutter - TOP_ROW_MIN).max(BOTTOM_MIN);
    st.bottom_h = st.bottom_h.clamp(BOTTOM_MIN, max_bottom);
    let hsplit_y = shell.bottom() - st.bottom_h;

    // 四容器几何。
    let left_top = Rect::from_min_max(shell.left_top(), egui::pos2(vsplit_x, hsplit_y));
    let right_top = Rect::from_min_max(
        egui::pos2(vsplit_x + gutter, shell.top()),
        egui::pos2(shell.right(), hsplit_y),
    );
    let bottom_left = Rect::from_min_max(
        egui::pos2(shell.left(), hsplit_y + gutter),
        egui::pos2(vsplit_x, shell.bottom()),
    );
    let bottom_right = Rect::from_min_max(
        egui::pos2(vsplit_x + gutter, hsplit_y + gutter),
        shell.right_bottom(),
    );

    let pr = ui.painter();
    for r in [left_top, right_top, bottom_left, bottom_right] {
        pr.rect_filled(r, theme::CONTAINER_RADIUS, p.folder_area);
    }

    // 两条分隔条命中面（z 序：先竖后横，与 C# 声明顺序一致，交点归横条）。
    splitter(
        ui,
        Rect::from_min_max(
            egui::pos2(vsplit_x, shell.top()),
            egui::pos2(vsplit_x + gutter, shell.bottom()),
        ),
        true,
        st,
    );
    splitter(
        ui,
        Rect::from_min_max(
            egui::pos2(shell.left(), hsplit_y),
            egui::pos2(shell.right(), hsplit_y + gutter),
        ),
        false,
        st,
    );

    // 请求盖章缓冲：pane 层请求先落 (侧, 路径)，folder 返回后由本帧活动 tab
    // 盖上 tab id（请求来自「发起帧的活动 tab」；活动快照在 folder 前取）。
    // 树请求在两侧 folder 间共享同一缓冲（对齐现状 st.tree_req 跨 folder 共读）。
    let mut list_tmp: Vec<(usize, String)> = Vec::new();
    let mut new_tmp: Vec<(usize, String, bool)> = Vec::new();
    let mut tree_tmp: Option<(usize, String)> = None;
    let mut sync_nav_tmp: Option<(usize, String)> = None;
    // 活动 tab 快照（本帧 pane 层请求的归属 tab；folder 内切换下帧生效）。
    let active_ids = [active_tab_id(st, 0), active_tab_id(st, 1)];
    // 左右窗格：各侧 tab 条合成临时视图（本地 tab 在前 + 远程 tab 在后），活动
    // tab 的 pane 交 folder；绘制像素行为与现状一致（pane 与 tabs 独立借用）。
    // 合成视图 / 槽位判定均为只读，先于 hooks 的 &mut 借用完成，绕开碰撞。
    enum Slot {
        Local(usize),
        Remote(usize),
    }
    let mut views = [build_tabs_view(st, 0), build_tabs_view(st, 1)];
    let builts: [Vec<u32>; 2] = [
        views[0].items.iter().map(|i| i.id).collect(),
        views[1].items.iter().map(|i| i.id).collect(),
    ];
    let slots = [
        if pane_is_local(st, 0) {
            Slot::Local(active_local_slot(st, 0).expect("local side keeps its active tab"))
        } else {
            Slot::Remote(active_remote_slot(st, 0).expect("remote side keeps its active tab"))
        },
        if pane_is_local(st, 1) {
            Slot::Local(active_local_slot(st, 1).expect("local side keeps its active tab"))
        } else {
            Slot::Remote(active_remote_slot(st, 1).expect("remote side keeps its active tab"))
        },
    ];
    // T20-4（C# CanTransferToOpposite :3010-3015）：跨窗格拖拽就绪 = 两侧窗格
    // 已导航（本地=非「此电脑」根态；远程=已入远程目录）+ 远程侧活动 tab 所属
    // 会话在池（断开即出池 = `_sftp.IsConnected` 同口径；本地侧恒就绪）。
    // 每帧一次经 Hooks 下发 pane 层（C# DragOver 逐帧判定同观感）。
    let side_ready = |s: usize| match &slots[s] {
        Slot::Local(i) => {
            let pn = &st.locals[s].tabs[*i].pane;
            (pn.path.is_some() || pn.remote_path.is_some(), true)
        }
        Slot::Remote(j) => {
            let t = &st.remotes[s].tabs[*j];
            (
                t.pane.remote_path.is_some(),
                st.remotes[s].sessions.iter().any(|ss| ss.sid == t.sid),
            )
        }
    };
    let (nav0, conn0) = side_ready(0);
    let (nav1, conn1) = side_ready(1);
    let dnd_ready = panes::can_transfer_to_oppo(nav0, conn0, nav1, conn1);
    let mut hooks = dialogs::Hooks {
        menu: &mut st.menu,
        ctx_menu: &mut st.ctx_menu,
        dialog: &mut st.dialog,
        crumb_edit: &mut st.crumb_edit,
        list_req: &mut list_tmp,
        new_req: &mut new_tmp,
        tree_req: &mut tree_tmp,
        sync_nav: &mut sync_nav_tmp,
        compare_req: &mut st.compare_req,
        transfer: &mut st.dnd_transfer,
        dnd_ready,
        pick: &mut st.pick,
        tab_close: None,
    };
    // 关闭的 tab（id 差集）延后处置：hooks 持 &mut 借用期间不能整借 st。
    let mut closed: Vec<(usize, u32)> = Vec::new();
    for side in 0..2 {
        let rect = if side == 0 { left_top } else { right_top };
        let built = &builts[side];
        match &slots[side] {
            Slot::Local(i) => panes::folder(
                ui,
                rect,
                &mut st.locals[side].tabs[*i].pane,
                &mut views[side],
                &mut st.icons,
                &mut st.sys_icons,
                &mut hooks,
                side,
            ),
            Slot::Remote(j) => panes::folder(
                ui,
                rect,
                &mut st.remotes[side].tabs[*j].pane,
                &mut views[side],
                &mut st.icons,
                &mut st.sys_icons,
                &mut hooks,
                side,
            ),
        }
        // 回写①：点击切换（按临时视图活动项 id 映射回 locals/remotes 活动 id）。
        if let Some(item) = views[side].items.get(views[side].active) {
            let id = item.id;
            if item.kind == panes::TabKind::Local {
                st.locals[side].active = Some(id);
                st.remotes[side].active = None;
            } else {
                st.remotes[side].active = Some(id);
                st.locals[side].active = None;
            }
        }
        // 回写②：非活动 tab 关闭（folder 在临时视图上按下标 remove，此处按
        // id 差集映射回真实状态；整借 st 的会话清理延后到 hooks 释放后）。
        if views[side].items.len() < built.len()
            && let Some(gone) = built
                .iter()
                .find(|id| !views[side].items.iter().any(|v| v.id == **id))
        {
            closed.push((side, *gone));
        }
        // 回写③：亮相动画收尾（tab_strip 动画结束清空 → 清侧级 born）。
        if views[side].born.is_none() && st.tab_born[side].is_some() {
            st.tab_born[side] = None;
        }
    }
    // G17：活动标签关闭请求（C# Tabs_TabCloseRequested → TabCloseRequested 事件）
    // → 确认门；take 后 hooks 借用即结束（NLL），整借 st 处理关闭。
    let tab_close = hooks.tab_close.take();
    // 窗格导航事件盖章（P4）：pane 层只发 (侧, 路径)，补发起 tab id（folder 前
    // 活动快照，与 list_reqs 同口径——请求来自发起帧的活动 tab）。
    if let Some((nav_side, nav_path)) = sync_nav_tmp.take() {
        st.sync_nav = Some((nav_side, active_ids[nav_side].unwrap_or(0), nav_path));
    }
    for (side, tab_id) in closed {
        finish_close_tab(st, side, tab_id);
    }
    if let Some((side, tab_id)) = tab_close {
        request_close_tab(st, side, tab_id);
    }

    // G17「取消队列并关闭」：等该 tab 归属的活动任务到块边界退出再关（P3 收窄
    // 到 per-tab，C# LogScope 语义；超时兜底放行——传输连接独立存活，迟到结算
    // 照常落库）。
    if let Some((side, tab_id, t0)) = st.tab_close_wait {
        let key = (side as u8, tab_id);
        let active = st.transfer.as_ref().is_some_and(|q| {
            q.with_items(|items| {
                crate::transfer::count_owned_active(items.iter().map(|i| (&i.owner, i.status)), key)
                    > 0
            })
        });
        if !active || t0.elapsed() >= std::time::Duration::from_secs(5) {
            st.tab_close_wait = None;
            finish_close_tab(st, side, tab_id);
        }
    }

    // 拖拽 Drop（FV-25）：发起/源侧 + 被拖条目 → 批量传输入队（菜单同契约）。
    if let Some((side, rows)) = st.dnd_transfer.take() {
        begin_transfer_multi(st, side, &rows);
    }

    // 活动 tab 盖章：pane 层请求补上发起 tab id（请求来自发起帧的活动 tab）。
    for (side, path) in list_tmp {
        st.list_reqs
            .push((side, active_ids[side].unwrap_or(0), path));
    }
    for (side, path, is_dir) in new_tmp {
        st.new_reqs
            .push((side, active_ids[side].unwrap_or(0), path, is_dir));
    }
    if let Some((side, path)) = tree_tmp {
        // 树请求盖章即发号（tag 随在途态留存，Ev::Listing/Failed 按 tag 命中）。
        let tag = next_seq(st);
        st.tree_req = Some((side, active_ids[side].unwrap_or(0), tag, path));
    }

    // 窗格层请求的远程列目录（统一从这里发出，按 (侧, tab) 经其挂靠会话发出）；
    // 树懒展开请求随后（互斥时窗格优先）。tab 已关或会话不在池 → 请求丢弃。
    for (side, tab_id, path) in std::mem::take(&mut st.list_reqs) {
        let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) else {
            continue;
        };
        let sid = st.remotes[side].tabs[pos].sid;
        // 先发号（&mut st 独占），再借会话发命令。
        let tag = next_seq(st);
        let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) else {
            continue;
        };
        let tab = &mut st.remotes[side].tabs[pos];
        tab.wait = Some((tag, path.clone()));
        tab.pane.loading = true;
        ss.ssh.list(tag, path);
    }
    // 窗格层请求的远程新建（工具栏/菜单发起；root() 统一经该 tab 挂靠会话发出）。
    // new_pending 保活到 Ev::Created（egui 无事件不重绘，与 del_pending 同理）；
    // tag 记入 inflight，Ev::Created 按其定位重列 tab。
    for (side, tab_id, path, is_dir) in std::mem::take(&mut st.new_reqs) {
        let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) else {
            continue;
        };
        let sid = st.remotes[side].tabs[pos].sid;
        // 先发号记 inflight（&mut st 独占段），再借会话发命令。
        let tag = next_seq(st);
        st.inflight.insert(tag, (side, tab_id));
        let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) else {
            continue;
        };
        if is_dir {
            ss.ssh.mkdir(tag, path);
        } else {
            ss.ssh.create_file(tag, path);
        }
        st.remotes[side].tabs[pos].new_pending += 1;
    }
    // 远程重命名（P0-1 落盘）：同 new_reqs 模式发号入队；完成由 Ev::Created
    // 按 inflight 定位重列（成败细节走终端日志），不再做列表条目的假改名。
    for (side, tab_id, path, new_path) in std::mem::take(&mut st.rename_reqs) {
        let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) else {
            continue;
        };
        let sid = st.remotes[side].tabs[pos].sid;
        // 先发号记 inflight（&mut st 独占段），再借会话发命令。
        let tag = next_seq(st);
        st.inflight.insert(tag, (side, tab_id));
        let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) else {
            continue;
        };
        ss.ssh.rename(tag, path, new_path);
        st.remotes[side].tabs[pos].new_pending += 1;
    }
    if let Some((side, tab_id, tag, path)) = st.tree_req.take()
        && let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id)
    {
        let sid = st.remotes[side].tabs[pos].sid;
        if let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) {
            // 发出后保留在途（含 tag），Ev::Listing / Ev::Failed 按 tag 收账。
            ss.ssh.list(tag, path.clone());
            st.tree_req = Some((side, tab_id, tag, path));
        }
    }

    // FV-01 文件夹选择回执：后台 STA 线程的 IFileDialog 结果。Some(路径) → 跳转
    // 对应侧本地窗格 + 同步联动；None / 通道断开 = 取消或线程异常（清槽复原
    // 按钮）；Empty = 仍在途（回槽，busy 重绘保证下帧再询）。
    if let Some(job) = st.pick.take() {
        match job.rx.try_recv() {
            Ok(Some(dir)) => {
                let side = job.side;
                let pane = pane_view_mut(st, side);
                if crate::local::navigate(pane, &dir) {
                    // P4：导航事件带发起 tab id（pick 回执落位处即当前活动 tab）。
                    let tab_id = active_tab_id(st, side).unwrap_or(0);
                    st.sync_nav = Some((side, tab_id, dir.to_string_lossy().into_owned()));
                }
            }
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => st.pick = Some(job),
        }
        // 回执消费帧里窗格早已按旧目录绘制完毕（本 poll 在窗格绘制之后），
        // 且消费后 pick 清槽、busy 不再保活——必须显式请求下一帧，否则新
        // 目录 / 按钮复原地要等下一次输入事件才上屏（「选中目录后迟迟不
        // 显示」根因）。Empty 已回槽，由 busy 120ms 链驱动，无需重复请求。
        if st.pick.is_none() {
            ui.ctx().request_repaint();
        }
    }

    // 窗格导航驱动：比对重排 + 同步镜像（C# HandleSyncNavigation / ApplySort 挂钩）。
    if let Some((side, tab, path)) = st.sync_nav.take() {
        sync::handle_nav(st, side, tab, path);
    }
    // 比对标记失配（任一侧列表已重载）→ 下帧前重排两侧。
    if st.compare_req {
        st.compare_req = false;
        sync::compare_apply_both(st);
    }

    panes::queue(ui, bottom_left, st);
    panes::terminal(ui, bottom_right, st);

    header::draw(ui, header_rect, st, &p);
    // CR-2：失败条在弹窗之前画（底部独占区域，不与四容器 / 弹窗重叠）。
    if let Some(r) = save_bar {
        draw_save_error_bar(ui, r, &p);
    }
    // 弹窗与右键菜单最后画（最顶层）。
    dialogs::draw(ui, st, &p);

    // CR-1：本地列目录 / 删除的线程驱动与回执入账（本帧窗格已绘制完，
    // 消费后主动约一帧，让新列表 / loading 文案即时上屏）。
    if poll_local_jobs(st) {
        ui.ctx().request_repaint();
    }

    // 分档心跳（项7）：在途异步（任侧连接中 / 等枚举 / 连接对话框等待 / 传输
    // 进行中 / 冲突确认中 / 编辑器回传在途）保持 120ms 快帧；仅外部编辑会话
    // （防抖 / 写锁探测）在途时降到 300ms（对齐 POLL_MS，探测每 300ms 才有
    // 意义，120ms 里过半帧纯空转）；已进入写锁超时兜底再降到 1s；全清不请求
    // ——心跳自停（egui 无事件不重绘，链路完结 CPU 自然回落）。编辑器回传链
    // 的 STAT 回流 → WriteFile → 回执认领与外部编辑的快照防抖 / 写锁探测 /
    // 询问弹窗均靠帧驱动消费。egui 多个 request_repaint_after 取最小，host_panel
    // / import_dialog 既有 120ms 请求不受影响。
    if let Some(d) = repaint_delay(st) {
        ui.ctx().request_repaint_after(d);
    }
}

/// CR-1：帧尾统一驱动本地 I/O。① 取走各本地 tab 登记的 `pending_local` 起线程；
/// ② 收回执按 (侧, tab id) 精确入账——tab 已关只留终端日志、不写窗格，避免
/// 回执落到切换后的新 tab。返回是否消费了回执（供调用方约下一帧）。
pub fn poll_local_jobs(st: &mut State) -> bool {
    for side in 0..2 {
        let pending: Vec<(u32, crate::local::LocalPaneReq, bool)> = st.locals[side]
            .tabs
            .iter_mut()
            .filter_map(|t| {
                let show_hidden = t.pane.show_hidden;
                t.pane
                    .pending_local
                    .take()
                    .map(|req| (t.id, req, show_hidden))
            })
            .collect();
        for (tab_id, req, show_hidden) in pending {
            let (tx, rx) = std::sync::mpsc::channel();
            let job_req = req.clone();
            std::thread::Builder::new()
                .name("local-io".into())
                .spawn(move || {
                    let _ = tx.send(crate::local::run_local(req, show_hidden));
                })
                .ok();
            st.local_jobs.push(LocalJob {
                side,
                tab_id,
                req: job_req,
                rx,
            });
        }
    }

    let mut replies: Vec<(usize, u32, crate::local::LocalPaneReply)> = Vec::new();
    st.local_jobs.retain(|job| match job.rx.try_recv() {
        Ok(reply) => {
            replies.push((job.side, job.tab_id, reply));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            replies.push((
                job.side,
                job.tab_id,
                crate::local::LocalPaneReply::worker_gone(job.req.clone()),
            ));
            false
        }
    });

    let consumed = !replies.is_empty();
    for (side, tab_id, reply) in replies {
        let deleted = if matches!(&reply.req, crate::local::LocalPaneReq::Delete { .. }) {
            Some(crate::local::delete_log_lines(&reply.deleted))
        } else {
            None
        };
        let logs = if let Some(tab) = st
            .locals
            .get_mut(side)
            .and_then(|ls| ls.tabs.iter_mut().find(|t| t.id == tab_id))
        {
            crate::local::apply_local(&mut tab.pane, reply)
        } else {
            // tab 已关：不写窗格，删除成败仍留终端痕迹（绝不写「已删除 N 项」汇总）。
            deleted.unwrap_or_default()
        };
        for line in logs {
            st.terminal.push((crate::local::now_hms(), line));
        }
    }
    if st.terminal.len() > 200 {
        st.terminal.drain(0..st.terminal.len() - 200);
    }
    consumed
}

/// 心跳分档（纯函数，项7，可单测四档）。判定序：在途 > 写锁超时兜底 >
/// 外部编辑会话 > 全清（None）。分档条件包含 ext_write / ext_stat /
/// doc_stat / pending_upload——别只按 ext_edits 判，否则确认框点覆盖后
/// 回执帧会变慢。
fn repaint_delay(st: &State) -> Option<std::time::Duration> {
    const FAST: std::time::Duration = std::time::Duration::from_millis(120);
    const POLL: std::time::Duration = std::time::Duration::from_millis(300);
    const SLOW: std::time::Duration = std::time::Duration::from_millis(1000);
    let in_flight = !st.list_reqs.is_empty()
        // CR-1：本地 I/O 在途（已起线程 / 已登记待起线程）同样要保活，
        // 否则 egui 空闲不重绘，回执永远不到账。
        || !st.local_jobs.is_empty()
        || st
            .locals
            .iter()
            .any(|ls| ls.tabs.iter().any(|t| t.pane.pending_local.is_some()))
        || st.tree_req.is_some()
        || st.pending_tx.is_some()
        || st.chk_pending.is_some()
        || st.chk_computing
        || st.doc_stat.is_some()
        || st
            .editors
            .docs
            .iter()
            .any(|d| d.lock().unwrap().pending_upload)
        || st.remotes.iter().any(|rs| {
            rs.sessions
                .iter()
                .any(|s| matches!(s.state, RemoteState::Connecting) || s.fp_ask.is_some())
                || rs.tabs.iter().any(|t| {
                    t.wait.is_some()
                        || t.del_pending > 0
                        || t.new_pending > 0
                        || t.pending_ed.is_some()
                        || t.ext_stat.is_some()
                        || t.ext_write.is_some()
                })
        })
        || st
            .transfer
            .as_ref()
            .map(|q| {
                q.with_items(|items| {
                    items
                        .iter()
                        .any(|i| matches!(i.status, crate::transfer::Status::Transferring))
                })
            })
            .unwrap_or(false)
        || matches!(st.dialog, dialogs::Dialog::HostPanel { testing: true, .. })
        || matches!(st.dialog, dialogs::Dialog::Import { scanning: true, .. })
        || st.fp_ask_test.is_some()
        || st.pick.is_some();
    if in_flight {
        return Some(FAST);
    }
    // CR-2：设置落盘失败在途（等 2s 退避重试）：1s 慢档兜底保活，不进 FAST
    // ——失败期间每 120ms 空转无意义。maybe_flush 已按 RETRY_BACKOFF 精确
    // 预约重绘，这里只防「预约被其它事件吞掉」的极端情况。
    if st.save_err_pending {
        return Some(SLOW);
    }
    // 写锁超时兜底（任一会话等待 ≥TIMEOUT）：1s 慢档（提醒秒数与 WARN 去重
    // 均靠轮询帧驱动，1s 粒度足够）。
    let lock_wait = st.remotes.iter().any(|rs| {
        rs.tabs.iter().any(|t| {
            t.ext_edits.iter().any(|e| {
                e.lock_wait_since
                    .is_some_and(|s| s.elapsed().as_millis() >= LOCK_TIMEOUT_MS)
            })
        })
    });
    if lock_wait {
        return Some(SLOW);
    }
    // 仅外部编辑会话存在（防抖 / 写锁探测）：300ms（POLL_MS 口径，零功能损失）。
    if st
        .remotes
        .iter()
        .any(|rs| rs.tabs.iter().any(|t| !t.ext_edits.is_empty()))
    {
        return Some(POLL);
    }
    None
}

/// 消费 SSH 工作线程事件（连接成功/失败、目录枚举结果）：按侧遍历会话池逐会话
/// 轮询，事件按 sid 路由——会话级事件（连接/指纹/日志）落会话及其挂靠 tab，数据
/// 级事件（枚举/删除/新建/下载/写回/STAT）落该会话挂靠 tab（P1 每 tab 恰一会话；
/// req 全局唯一，Stat/Exec 回流仍按 req 匹配）。
fn apply_ssh_events(st: &mut State, ctx: &egui::Context) {
    // 预检超时兜底（T20-G③，T20-H② 滑动窗）：每帧先检查在途批次——连续
    // 超窗零回流才强制放行（队列必须形成），未回流项带 probe_incomplete
    // 标记 + 终端对账提示，不静默（F-12）。
    if let Some(ptx) = &mut st.pending_tx {
        let n = force_settle_expired(ptx, STAT_PROBE_TIMEOUT);
        if n > 0 {
            let items = std::mem::take(&mut ptx.items);
            let (side, pane) = (ptx.profile_side, ptx.src_pane);
            settle_batch(st, side, pane, items);
            st.terminal.push((
                crate::local::now_hms(),
                tfmt(
                    "预检超时：{0} 项未完成探测，已按远端不存在放行（传输时按续传决策兜底）",
                    &[&n.to_string()],
                ),
            ));
        }
    }
    for side in 0..2 {
        let sids: Vec<u64> = st.remotes[side].sessions.iter().map(|s| s.sid).collect();
        for sid in sids {
            let evs = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == sid)
                .map(|s| s.ssh.poll())
                .unwrap_or_default();
            for ev in evs {
                apply_ssh_event(st, ctx, side, sid, ev);
            }
            // CR-7：一批事件只裁一次终端（阈值仍 400），不再逐条 drain。
            trim_terminal(&mut st.terminal);
        }
    }
}

/// CR-7 终端批量裁剪：保留末 400 行，由 apply_ssh_events 每批末调用一次。
fn trim_terminal(terminal: &mut Vec<(String, String)>) {
    if terminal.len() > 400 {
        terminal.drain(0..terminal.len() - 400);
    }
}

/// 单个会话事件入账（side = 来源侧；sid = 来源会话，数据级事件经其挂靠 tab）。
fn apply_ssh_event(st: &mut State, ctx: &egui::Context, side: usize, sid: u64, ev: crate::ssh::Ev) {
    // 终端日志来源标签（G1 = [名称 (IP)]，C# HostTabNaming；未留存档案时 [远程]）。
    let tag = match st.remotes[side]
        .sessions
        .iter()
        .find(|s| s.sid == sid)
        .and_then(|s| s.host.as_ref())
    {
        Some(h) => format!("[{}]", dialogs::host_tab_naming(&h.name, &h.host)),
        None => format!("[{}]", t("远程")),
    };
    match ev {
        crate::ssh::Ev::Connected {
            label,
            root,
            confirmed,
        } => {
            // S-8 队列菜单重连回账：在途时记专用成功日志（键 LogReconnected，
            // C# RowReconnectItem_Click 的 Connected 回执同文案）并逐个归账；
            // 普通连接提交不记账，不打这行。
            if st.reconnecting > 0 {
                st.reconnecting -= 1;
                st.terminal
                    .push((crate::local::now_hms(), tfmt("已重新连接：{0}", &[&label])));
            }
            // 会话复位 + 挂靠 tab 初始目录优先级（C# AttachConnectedSession；抽
            // initial_remote_dir 与会话复用落位共用）：同步目录 > 上次浏览目录 >
            // 登录主目录；上次浏览目录失效回退主目录一次，同步目录失效直接报错
            // （显式配置不静默掩盖）。base = 同步/比对自动激活的基准根。
            let hopt = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == sid)
                .and_then(|s| s.host.clone());
            let (init, fallback, remote_base) = match &hopt {
                Some(h) => initial_remote_dir(h, &root),
                None => (root.clone(), None, None),
            };
            // 会话态复位 + 登录主目录存档（P2 会话复用时新 tab 初始目录兜底）。
            if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                ss.state = RemoteState::Off;
                ss.root = Some(root.clone());
            }
            // 该 sid 所有空态等待 tab 逐个落位（P2 会话复用：同主机多 tab 一起
            // 等待连接，各自走初始目录列目录；已有视图的 tab 不重置——先连的
            // tab 浏览态不丢）。G17-ANIM：tab 标题更新为真名。
            let waiting: Vec<u32> = st.remotes[side]
                .tabs
                .iter()
                .filter(|t| t.sid == sid && t.pane.empty)
                .map(|t| t.id)
                .collect();
            for tab_id in waiting {
                let ltag = next_seq(st);
                if let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) {
                    ss.ssh.list(ltag, init.clone());
                }
                if let Some(tab) = st.remotes[side].tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.pane = panes::FilePane {
                        remote_path: Some(init.clone()),
                        ..Default::default()
                    };
                    tab.wait = Some((ltag, init.clone()));
                    tab.pane.loading = true;
                    tab.fallback = fallback.clone();
                    tab.title = label.clone();
                }
            }
            // 活动挂靠 tab 亮相重播（连接提交时已激活；C# 此刻 tab 早已 Add，
            // 等价用户感知的「连接成功后 tab 缓动出现」）。
            if let Some(atid) = st.remotes[side].active
                && st.remotes[side]
                    .tabs
                    .iter()
                    .find(|t| t.id == atid)
                    .is_some_and(|t| t.sid == sid)
            {
                st.tab_born[side] = Some((atid, std::time::Instant::now()));
            }
            st.dialog = dialogs::Dialog::None;
            // 同步/比对自动激活（C# FolderContainer.ConnectAndOpenAsync 规则①②）：
            // 主机配置同步浏览 → 对侧本地 + 本侧远程定基准根自动点亮；配置比对 →
            // 一并自动激活。联对面 = 连接侧远程 + 对侧本地（对称窗格，任一侧均可能）。
            sync::attach_connect(st, side, sid, remote_base);
            // S1.9：指纹落盘（本次确认 ≠ 存档 → 传输/exec 档案回写 + 工作集回写 +
            // 盘上单字段落盘 + 日志；存档匹配路径 confirmed=None 不触发；盘上无
            // 记录 = 指纹仅本会话生效，显式提示而非静默丢弃）。
            if let Some(fp) = &confirmed {
                let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) else {
                    return;
                };
                if ss
                    .host
                    .as_ref()
                    .is_some_and(|h| h.host_key_fingerprint != *fp)
                {
                    // ① 本会话传输/exec 档案同步携带新指纹（H1：后续传输不再按
                    // 旧档案 fail-closed 全拒）。
                    if let Some(p) = &mut ss.profile {
                        p.fingerprint = Some(fp.clone());
                    }
                    let was_empty = ss
                        .host
                        .as_ref()
                        .is_some_and(|h| h.host_key_fingerprint.trim().is_empty());
                    let (host, port, user, last_path) = {
                        let h = ss.host.as_mut().expect("checked above");
                        (
                            h.host.clone(),
                            h.port,
                            h.username.clone(),
                            h.last_remote_path.clone(),
                        )
                    };
                    // ② 工作集回写（显式「保存」不回滚新指纹）+ 盘上单字段落盘。
                    crate::hosts::update_fingerprint(&host, port, &user, fp);
                    let saved =
                        crate::hosts::save_single_session(&host, port, &user, fp, &last_path);
                    if let Some(h) = &mut ss.host {
                        h.host_key_fingerprint = fp.clone();
                    }
                    let msg = match saved {
                        Ok(()) => {
                            if was_empty {
                                t("已记住主机指纹，下次连接自动校验").to_owned()
                            } else {
                                t("已更新主机指纹存档，下次连接自动校验").to_owned()
                            }
                        }
                        // 盘上无此记录（新增未保存）：不谎称「下次自动」，如实提示。
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            t("主机尚未保存进存档，指纹仅本会话生效").to_owned()
                        }
                        Err(e) => tfmt("指纹已在本会话生效，但写入存档失败：{0}", &[&e]),
                    };
                    st.terminal
                        .push((crate::local::now_hms(), format!("[{label}] {msg}")));
                    if st.terminal.len() > 400 {
                        st.terminal.drain(0..st.terminal.len() - 400);
                    }
                }
            }
        }
        crate::ssh::Ev::Failed { what, msg, tag } => {
            // 目录树懒展开失败：tag 命中在途树请求 → 复位各窗格 loading 节点
            // （可再展开）+ 同步收尾（基准入账 + 放弃恢复链，防每帧重发；C# 3 秒
            // 超时同效）+ give_up 该 tab。连接失败（tag=None）时树请求同样不会
            // 回流，一并复位防卡死；其他命令的失败不动树请求。
            let tree_kill = match (&tag, st.tree_req.as_ref()) {
                (Some(tg), Some((_, _, ttag, _))) => *ttag == *tg,
                (None, Some(_)) => what == "连接",
                _ => false,
            };
            if tree_kill {
                let (tside, ttab) = st
                    .tree_req
                    .take()
                    .map(|(s, t, _, _)| (s, t))
                    .unwrap_or((0, 0));
                for s2 in 0..2 {
                    for lt in &mut st.locals[s2].tabs {
                        crate::ui::panes::reset_tree_loading(&mut lt.pane);
                    }
                    for rt in &mut st.remotes[s2].tabs {
                        crate::ui::panes::reset_tree_loading(&mut rt.pane);
                    }
                }
                if let Some(tab) = find_remote_tab_mut(st, tside, ttab) {
                    crate::ui::panes::tree_sync_give_up(&mut tab.pane);
                }
            }
            if what == "连接" {
                if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                    ss.state = RemoteState::Failed(tfmt("连接失败：{0}", &[&msg]));
                }
                // S-8 队列菜单重连回账：在途时记专用失败日志（键 LogReconnectFailed）
                // 并逐个归账（连接失败也是回执，菜单态归零恢复）。
                if st.reconnecting > 0 {
                    st.reconnecting -= 1;
                    st.terminal
                        .push((crate::local::now_hms(), tfmt("重连失败：{0}", &[&msg])));
                }
                // 连接失败：就地关闭当前弹窗（HostPicker/HostPanel 卡片发起的失败
                // 不留门，C# 同语义）；错误文案留在挂靠 tab 空态/状态栏。
                st.dialog = dialogs::Dialog::None;
            } else if st
                .sync_mirror
                .as_ref()
                .is_some_and(|(ms, _, _)| *ms == side)
            {
                // 同步镜像导航失败（镜像请求的响应必带 tag，按其定位发起 tab；
                // mirror 匹配已按 P4 (侧, tab) 口径收窄）：两侧目录不一致
                //（C# MirrorNavigateAsync 失败 → ReportMismatch）→ 关闭联动并弹窗。
                st.sync_mirror = None;
                if let Some(tg) = tag
                    && let Some(tab) = st.remotes[side]
                        .tabs
                        .iter_mut()
                        .find(|t| t.wait.as_ref().is_some_and(|w| w.0 == tg))
                {
                    tab.wait = None;
                    tab.pane.loading = false;
                }
                sync::report_mismatch(st, "SyncMismatchMsg");
            } else {
                // 列目录失败：tag 定位发起 tab（过期 tag = 用户已发起新导航，
                // 静默丢弃——新导航有自己的等待与回退）。上次浏览目录已失效 →
                // 回退登录主目录重试一次（C# 同语义）；无回退资格 → tab 目录
                // 加载失败提示。
                let hit = tag.and_then(|tg| {
                    st.remotes[side]
                        .tabs
                        .iter()
                        .find(|t| t.wait.as_ref().is_some_and(|w| w.0 == tg))
                        .map(|t| t.id)
                });
                let Some(tab_id) = hit else {
                    return;
                };
                let fb = find_remote_tab_mut(st, side, tab_id).and_then(|tab| tab.fallback.take());
                if let Some(home) = fb {
                    let ltag = next_seq(st);
                    if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                        tab.wait = Some((ltag, home.clone()));
                        tab.pane.loading = true;
                        tab.pane.msg = None;
                    }
                    if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                        ss.ssh.list(ltag, home);
                    }
                } else if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                    tab.wait = None;
                    tab.pane.loading = false;
                    tab.pane.msg = Some(format!("{what}失败：{msg}"));
                }
            }
        }
        crate::ssh::Ev::AskFingerprint {
            first,
            host,
            stored,
            presented,
            reply,
        } => {
            // 指纹确认：置该会话请求态，覆盖层本帧绘制；worker 阻塞等按钮回执。
            if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                ss.fp_ask = Some(dialogs::FpAsk {
                    first,
                    host,
                    stored,
                    presented,
                    reply,
                });
            }
        }
        crate::ssh::Ev::Listing { tag, path, entries } => {
            // 目录树懒展开响应优先（不经窗格；tag 命中在途树请求 → 填树清槽；
            // 不命中 = 其他命令的响应，树请求保留在途等真正响应）。树请求只可能
            // 来自远程面板（本地树为静态驱动器）。
            let mut tree_hit = false;
            if let Some((tside, ttab, ttag, _)) = st.tree_req.as_ref()
                && *ttag == tag
            {
                let (tside, ttab) = (*tside, *ttab);
                st.tree_req = None;
                if let Some(tab) = find_remote_tab_mut(st, tside, ttab) {
                    crate::ui::panes::apply_tree_listing(&mut tab.pane, &path, &entries);
                    tree_hit = true;
                }
            }
            // tag 命中 wait = 本响应的发起 tab（tag 全局唯一；同会话多 tab 各自
            // wait 各自等）。不命中 = 过期响应（等待期间用户又点了别的目录）静默
            // 丢弃——不再按路径比对。
            let mirror_hit = {
                let mut hit = None;
                if !tree_hit {
                    let tab = st.remotes[side]
                        .tabs
                        .iter_mut()
                        .find(|t| t.wait.as_ref().is_some_and(|w| w.0 == tag));
                    if let Some(tab) = tab {
                        tab.wait = None;
                        tab.pane.loading = false;
                        tab.pane.msg = None;
                        // 同目录重列（刷新/删除/新建回执）→ 按名保留选中（比 C# 更好：
                        // C# LoadRemoteAsync 一律清空）；跨目录导航 → 清空（C# 同）。
                        // 须在 remote_path 覆写前取旧值判定。
                        let same_dir = tab.pane.remote_path.as_deref() == Some(path.as_str());
                        let old_sel: Vec<String> = if same_dir {
                            tab.pane
                                .selected
                                .iter()
                                .filter_map(|&i| tab.pane.entries.get(i).map(|e| e.name.clone()))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        tab.pane.remote_path = Some(path.clone());
                        // ShowHidden 关时过滤点文件（C# LoadRemoteAsync :583
                        // StartsWith('.') 跳过）。
                        let mut entries = entries;
                        if !tab.pane.show_hidden {
                            entries.retain(|e| !e.name.starts_with('.'));
                        }
                        tab.pane.entries = entries;
                        tab.pane.selected = tab
                            .pane
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| old_sel.iter().any(|n| n == &e.name))
                            .map(|(i, _)| i)
                            .collect();
                        crate::local::apply_sort(&mut tab.pane);
                        // 刷新淡入触发（比 C# 更好的刷新表现）：数据入账即动效；
                        // 上一轮动画未结束则不重启（保刷新钮旋转角度连续，快到账
                        // 不打断首轮，慢到账再触发下一轮）。
                        let now = ctx.time();
                        if tab
                            .pane
                            .refresh_at
                            .map_or(true, |t0| now - t0 >= panes::REFRESH_ANIM)
                        {
                            tab.pane.refresh_at = Some(now);
                        }
                        // 浏览记忆单字段落盘（C# PersistLastRemotePath：仅会话状态字段，
                        // 盘上无此主机记录时静默跳过；落盘失败同静默 best-effort，不搅动
                        // 浏览流程）；回退资格随之耗尽（按挂靠会话的档案落盘）。
                        tab.fallback = None;
                        if let Some(h) = st.remotes[side]
                            .sessions
                            .iter()
                            .find(|s| s.sid == sid)
                            .and_then(|s| s.host.clone())
                        {
                            let _ = crate::hosts::save_single_session(
                                &h.host,
                                h.port,
                                &h.username,
                                &h.host_key_fingerprint,
                                &path,
                            );
                        }
                        // 同步镜像成功落位（P4 按 (侧, tab) 匹配命中即视为成功；
                        // C# 同款隐式约定）——块外统一走 handle_nav。
                        if st
                            .sync_mirror
                            .as_ref()
                            .is_some_and(|(ms, mt, _)| *ms == side && *mt == tab.id)
                        {
                            hit = Some(tab.id);
                        }
                    }
                }
                hit
            };
            if let Some(nav_tab) = mirror_hit {
                st.sync_mirror = None;
                sync::handle_nav(st, side, nav_tab, path);
            }
        }
        crate::ssh::Ev::Disconnected => {
            // 主动断开已即时复位 UI；被动掉线检测随 keepalive 接入。
        }
        crate::ssh::Ev::Log { msg } => {
            // CR-7：不在每条事件裁剪；由 apply_ssh_events 每批末统一 trim 一次。
            // 递归删除日志洪泛时避免每条事件都 Vec::drain 造成的重复搬移。
            st.terminal.push((crate::local::now_hms(), msg));
        }
        crate::ssh::Ev::Deleted { tag } => {
            // tag → 归属 tab（inflight 记录发号映射；未知 tag = tab 已关，静默
            // 丢弃）。删除在途回零 → 刷新该 tab 面板与树。
            let owner = st.inflight.remove(&tag).filter(|&(bs, _)| bs == side);
            let act = owner.and_then(|(_, tab_id)| {
                st.remotes[side]
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == tab_id)
                    .map(|tab| {
                        tab.del_pending = tab.del_pending.saturating_sub(1);
                        if tab.del_pending == 0 && tab.pane.loading {
                            tab.pane.loading = false;
                            // 删除后重建远程树（C# RefreshListKeepingPath 首行
                            // RefreshTreeIfVisible）。
                            crate::ui::panes::tree_mark_dirty(&mut tab.pane);
                            tab.pane.remote_path.clone().map(|rp| (tab.id, rp))
                        } else {
                            None
                        }
                    })
            });
            if let Some(Some((tab_id, rp))) = act {
                st.list_reqs.push((side, tab_id, rp));
            }
        }
        crate::ssh::Ev::Created { tag } => {
            // 新建条目完成（成败均有日志）：tag → 归属 tab（未知 tag 静默丢弃），
            // 统一重建树 + 重列该 tab 当前目录，对齐 C# CreateRemoteEntryAsync
            // 末尾无条件 RefreshListKeepingPath。
            let owner = st.inflight.remove(&tag).filter(|&(bs, _)| bs == side);
            let act = owner.and_then(|(_, tab_id)| {
                st.remotes[side]
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == tab_id)
                    .map(|tab| {
                        tab.new_pending = tab.new_pending.saturating_sub(1);
                        crate::ui::panes::tree_mark_dirty(&mut tab.pane);
                        tab.pane.remote_path.clone().map(|rp| (tab.id, rp))
                    })
            });
            if let Some(Some((tab_id, rp))) = act {
                st.list_reqs.push((side, tab_id, rp));
            }
        }
        crate::ssh::Ev::Exec {
            req,
            exit,
            out,
            err,
        } => {
            // 校验码命令回流：sha256sum/sha512sum 输出首段 hex；SM3 走
            // openssl dgst -sm3，输出 "SM3(路径)= hex"，取 = 号后段（G9）。
            if let Some((want, algo)) = st.chk_pending
                && want == req
            {
                st.chk_pending = None;
                st.chk_local = Some(if exit != 0 {
                    let e = err.trim();
                    if e.is_empty() {
                        Err(tfmt("命令退出码 {0}", &[&exit]))
                    } else {
                        Err(e.to_owned())
                    }
                } else {
                    let val = if algo == 2 {
                        out.rsplit('=').next().unwrap_or("").trim()
                    } else {
                        out.split_whitespace().next().unwrap_or("")
                    };
                    if val.is_empty() {
                        Err(t("命令无输出").to_owned())
                    } else {
                        Ok(val.to_owned())
                    }
                });
            }
        }
        crate::ssh::Ev::FileData { req, result } => {
            // 远程编辑下载回流：写临时副本 → 开编辑器窗口（副本已就绪）。
            // 按请求号匹配发起 tab 的 pending_ed（req 全局唯一，下载由挂靠会话执行）。
            let hit = st.remotes[side]
                .tabs
                .iter_mut()
                .find(|t| t.sid == sid)
                .filter(|t| t.pending_ed.as_ref().is_some_and(|(w, _, _)| *w == req))
                .map(|t| (t.id, t.pending_ed.take()));
            if let Some((tab_id, Some((_, remote_path, name)))) = hit {
                match result {
                    Ok(fd) => {
                        // 同路径旧外部编辑会话先作废（C# EndSession：任何形式
                        // 重编辑都先结束旧会话）。
                        if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                            tab.ext_edits.retain(|e| e.remote_path != remote_path);
                        }
                        let host = st.remotes[side]
                            .sessions
                            .iter()
                            .find(|s| s.sid == sid)
                            .and_then(|s| s.host.as_ref().map(|h| h.host.clone()));
                        if let Some(host) = host {
                            // 白名单扩展名免探，其余按内容探测（C# 白名单免探 /
                            // ReadHeadAsync 头探回退同语义；数据已在内存，无需
                            // 额外往返）。
                            if !crate::editor::is_remote_text_ext(&name)
                                && !crate::editor::is_text_data(&fd.data)
                            {
                                st.terminal.push((
                                    crate::local::now_hms(),
                                    format!("{tag} {}", t("非文本文件，已取消打开")),
                                ));
                            } else if let Ok(copy) =
                                crate::editor::write_remote_copy(&host, &remote_path, &fd.data)
                            {
                                st.terminal.push((
                                    crate::local::now_hms(),
                                    tfmt(
                                        "{0} 已下载 {1}（{2} 字节）到临时副本",
                                        &[&tag, &name, &fd.data.len()],
                                    ),
                                ));
                                let (external, editor_path) = crate::settings::with(|s| {
                                    (!s.editor_builtin, s.editor_path.trim().to_owned())
                                });
                                if external && !editor_path.is_empty() {
                                    // 注册外部编辑会话（C# ExternalEditSession：
                                    // watcher 轮询检测保存 → 询问回传），并启动
                                    // 外部编辑器打开临时副本。
                                    let baseline = ext_snapshot(&copy);
                                    if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                                        tab.ext_edits.push(ExtEdit {
                                            host_tag: tag.clone(),
                                            remote_path: remote_path.clone(),
                                            name: name.clone(),
                                            local: copy.clone(),
                                            base_mtime: fd.mtime,
                                            baseline,
                                            quiet_since: None,
                                            quiet_snap: None,
                                            lock_wait_since: None,
                                            lock_warned: false,
                                            lock_dialog_done: false,
                                        });
                                    }
                                    if let Err(e) =
                                        std::process::Command::new(&editor_path).arg(&copy).spawn()
                                    {
                                        st.terminal.push((
                                            crate::local::now_hms(),
                                            tfmt("外部编辑器启动失败：{0}", &[&e.to_string()]),
                                        ));
                                    }
                                } else {
                                    let dark = ctx.theme() == egui::Theme::Dark;
                                    st.editors.open_remote(
                                        &host,
                                        &copy,
                                        remote_path,
                                        fd.mtime,
                                        dark,
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt("{0} 下载 {1} 失败：{2}", &[&tag, &name, &e]),
                        ));
                    }
                }
            }
        }
        crate::ssh::Ev::FileWritten { req, path, result } => {
            // 外部编辑回传写回回流：按挂靠 tab 的 ext_write 请求号分流（先于内置
            // 编辑器 doc.id 匹配——两套计数器独立可能数值撞号，ext 优先消费）。
            let ext_hit = st.remotes[side]
                .tabs
                .iter_mut()
                .find(|t| t.sid == sid)
                .filter(|t| t.ext_write.is_some_and(|w| w == req))
                .map(|t| {
                    t.ext_write = None;
                    t.id
                });
            if let Some(tab_id) = ext_hit {
                match result {
                    Ok(mtime) => {
                        // 会话回填（C# SetBase + 复位 watcher）：写后远端
                        // mtime 作新基准、快照复位（防自触发）。
                        if let Some(tab) = find_remote_tab_mut(st, side, tab_id)
                            && let Some(e) =
                                tab.ext_edits.iter_mut().find(|e| e.remote_path == path)
                        {
                            e.base_mtime = mtime;
                            e.baseline = ext_snapshot(&e.local);
                            e.quiet_since = None;
                            e.quiet_snap = None;
                        }
                        // C# UpdateStatus($"\"{item.Name}\" {RemoteEditUploaded}")。
                        let fname = std::path::Path::new(&path)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt("\"{0}\" 已回传到远端", &[&fname]),
                        ));
                        // 回传落盘 → 即时重列该 tab 当前目录（视图反映新 mtime）。
                        if let Some(rp) = st.remotes[side]
                            .tabs
                            .iter()
                            .find(|t| t.id == tab_id)
                            .and_then(|t| t.pane.remote_path.clone())
                        {
                            st.list_reqs.push((side, tab_id, rp));
                        }
                    }
                    Err(e) => {
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt("{0} 上传 {1} 失败：{2}", &[&tag, &path, &e]),
                        ));
                    }
                }
            } else {
                // 内置编辑器回传回流（CR-6⑥）：请求号 = doc_id。Ok → mtime 回填
                // base + 清 pending/dirty + 写回执（字节数 · 远端路径）+ 终端日志带
                // 三实数；Err → 失败态（副本保留，再按 Ctrl+S 可重试）。
                match result {
                    Ok(mtime) => {
                        let len = st
                            .editors
                            .docs
                            .iter()
                            .find(|doc| doc.lock().unwrap().id == req)
                            .map(|doc| {
                                let mut d = doc.lock().unwrap();
                                if let Some(r) = &mut d.remote {
                                    r.base_mtime = mtime;
                                }
                                let n = std::fs::metadata(&d.path).map(|m| m.len()).unwrap_or(0);
                                d.pending_upload = false;
                                d.dirty = false;
                                d.save_err = None;
                                d.last_receipt = Some(tfmt("已回传 {0} 字节 · {1}", &[&n, &path]));
                                n
                            })
                            .unwrap_or(0);
                        // 回执唤醒编辑器子视口（项2）：状态栏「正在回传…」
                        // → 回执文案自动流转，不等用户碰编辑器窗口。
                        ctx.request_repaint_of(editor_viewport(req));
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt(
                                "{0} 已回传 {1} 字节，远端 mtime {2}：{3}",
                                &[&tag, &len, &mtime.unwrap_or(0), &path],
                            ),
                        ));
                        // 上传落盘 → 即时重列挂靠 tab 当前目录（比 C# 更好的即时反馈：
                        // 浏览视图直接反映新大小/修改时间，无需手动刷新）。
                        if let Some((tid, rp)) = st.remotes[side]
                            .tabs
                            .iter()
                            .find(|t| t.sid == sid)
                            .and_then(|t| t.pane.remote_path.clone().map(|rp| (t.id, rp)))
                        {
                            st.list_reqs.push((side, tid, rp));
                        }
                    }
                    Err(e) => {
                        for doc in &st.editors.docs {
                            let mut d = doc.lock().unwrap();
                            if d.id == req {
                                d.pending_upload = false;
                                d.dirty = true;
                                d.save_err = Some(tfmt("回传未完成：{0}", &[&e]));
                            }
                        }
                        // 回执唤醒编辑器子视口（项2）：失败红字立即上状态栏。
                        ctx.request_repaint_of(editor_viewport(req));
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt("{0} 上传 {1} 失败：{2}", &[&tag, &path, &e]),
                        ));
                    }
                }
            }
        }
        crate::ssh::Ev::Stat {
            req,
            path,
            exists,
            size,
            mtime,
        } => {
            // 外部编辑回传前冲突探测回流：远端 mtime 与下载基准差 ≥1s →
            // 弹覆盖确认（C# GetLastWriteTime 比对 + ShowRemoteConflictAsync）；
            // 无冲突 / 远端未报 mtime / 文件已不存在 → 直接回传。按挂靠 tab 的
            // ext_stat 请求号分流。
            let mut ext_handled = false;
            let ext_tab = st.remotes[side]
                .tabs
                .iter_mut()
                .find(|t| t.sid == sid && t.ext_stat.as_ref().is_some_and(|(w, _)| *w == req))
                .map(|t| {
                    t.ext_stat = None;
                    t.id
                });
            if let Some(tab_id) = ext_tab {
                ext_handled = true;
                let session = st.remotes[side]
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .and_then(|t| {
                        t.ext_edits
                            .iter()
                            .find(|e| e.remote_path == path)
                            .map(|e| (e.name.clone(), e.base_mtime))
                    });
                if let Some((name, base)) = session {
                    let conflict = match (base, mtime) {
                        (Some(a), Some(b)) => exists && a.abs_diff(b) >= 1,
                        _ => false,
                    };
                    if conflict {
                        st.dialog = dialogs::Dialog::ExtEditConflict {
                            side,
                            tab: tab_id,
                            name,
                            remote_path: path.clone(),
                            src: dialogs::ExtConflictSrc::External,
                        };
                    } else {
                        ext_upload(st, side, tab_id, &path);
                    }
                }
            }
            if !ext_handled {
                // 内置编辑器回传探测回流（CR-6④）：±1s 判据同外部链路。无冲突 /
                // 远端未报 mtime / 文件不存在 → 读副本写回（请求号 = doc_id，内置
                // FileWritten 分支按 doc.id 认领）；有冲突 → 覆盖确认弹窗（内置，
                // 「覆盖」走 doc_upload）。不匹配的回流把槽放回（别侧事件不吃）。
                let mut doc_handled = false;
                if let Some((req_no, doc_id, rpath, lpath, data, base)) = st.doc_stat.take() {
                    if req_no == req {
                        doc_handled = true;
                        let doc = st
                            .editors
                            .docs
                            .iter()
                            .find(|doc| doc.lock().unwrap().id == doc_id)
                            .cloned();
                        let conflict = mtime_conflict(base, mtime, exists);
                        if conflict {
                            let name = doc
                                .as_ref()
                                .map(|doc| doc.lock().unwrap().title.clone())
                                .unwrap_or_default();
                            st.dialog = dialogs::Dialog::ExtEditConflict {
                                side,
                                tab: 0,
                                name,
                                remote_path: rpath,
                                src: dialogs::ExtConflictSrc::Internal {
                                    doc_id,
                                    local_path: lpath,
                                },
                            };
                        } else {
                            // 副本内容复用分派时的读取（槽内 data，项3）：无冲突
                            // 直通不再二次 fs::read。
                            if let Some(ss) =
                                st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid)
                            {
                                ss.ssh.write_file(rpath, doc_id, data);
                            } else {
                                doc_upload_failed(st, ctx, doc_id, t("未找到主机会话"));
                            }
                        }
                    } else {
                        st.doc_stat = Some((req_no, doc_id, rpath, lpath, data, base));
                    }
                }
                if !doc_handled {
                    // 冲突探测回流（T20-E 流式落位）：冲突只标记不中止——冲突项
                    // 留置到集齐统一进弹窗，非冲突项每满阈值即入队一批（队列逐批
                    // 增长，流式全程不停）；全量回流结算统一收尾。
                    let mut step = StreamStep::None;
                    if let Some(ptx) = &mut st.pending_tx
                        && ptx.seq == req
                    {
                        // 滑动窗基线（T20-H②）：任一回流（含重复/落空路径）即刷新
                        // ——60s 零回流才断流，分轮回流的自然间歇不误判超窗。
                        ptx.at = std::time::Instant::now();
                        if let Some(pi) = ptx
                            .items
                            .iter_mut()
                            .find(|pi| pi.stat_path.as_deref() == Some(path.as_str()))
                        {
                            pi.stat_path = None;
                            pi.conflict = exists;
                            if exists {
                                pi.dst_size = size;
                                pi.dst_mtime = mtime;
                            }
                            ptx.resolved += 1;
                            if !exists {
                                ptx.ready += 1; // 冲突项不计入落位余量
                            }
                            step = stream_step(ptx);
                        }
                    }
                    match step {
                        StreamStep::Place => stream_place(st),
                        StreamStep::Settle => stream_settle(st),
                        StreamStep::None => {}
                    }
                }
            }
        }
    }
}

// ── 外部编辑器远程会话（对齐 C# ExternalEditSession 全套）──────

/// 写锁等待软提醒阈值（项6）：等待超过 3s 窗格状态栏出「等待编辑器完成保存…」。
const LOCK_SOFT_MS: u128 = 3000;
/// 写锁等待超时兜底阈值（项6）：等待超过 10s 终端恰 1 条 WARN、状态栏持久
/// 提示、弹退出对话框（立即重试探测 / 取消本次回传）。超时 ≠ 放行上传。
const LOCK_TIMEOUT_MS: u128 = 10000;

/// doc_stat 槽类型：(请求号, doc_id, 远端路径, 副本路径, 副本内容, 基准
/// mtime)。独立别名（type_complexity 收口）；形状见 State.doc_stat 文档。
pub(crate) type DocStatSlot = (u64, u64, String, std::path::PathBuf, Vec<u8>, Option<i64>);

/// 本地临时副本快照（长度 + mtime 秒；C# (Length, LastWriteTimeUtc.Ticks)
/// 等价，秒级粒度足够——编辑器保存间隔远大于 1s）。读不到 → (0, 0)。
fn ext_snapshot(path: &std::path::Path) -> (u64, i64) {
    match std::fs::metadata(path) {
        Ok(m) => (
            m.len(),
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        ),
        Err(_) => (0, 0),
    }
}

/// 写锁检测（C# File.Open Read/FileShare.Read → IOException 推迟等价）：
/// 以 GENERIC_READ + 仅 FILE_SHARE_READ 打开文件——编辑器仍在保存（持有
/// 写句柄）时得到共享冲突即「锁未释放」；其余情况（成功 / 不存在 / 权限）
/// 视为锁已释放。
#[cfg(windows)]
fn ext_write_lock_released(path: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    #[allow(non_snake_case)]
    mod extlock {
        // FILE_SHARE_READ / GENERIC_READ / OPEN_EXISTING（CreateFileW 常量）。
        pub const FILE_SHARE_READ: u32 = 0x1;
        pub const GENERIC_READ: u32 = 0x8000_0000;
        pub const OPEN_EXISTING: u32 = 3;
        pub const INVALID_HANDLE_VALUE: isize = -1;
        /// 共享冲突错误码（ERROR_SHARING_VIOLATION）。
        pub const ERROR_SHARING_VIOLATION: u32 = 32;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            pub fn CreateFileW(
                lpfilename: *const u16,
                dwdesiredaccess: u32,
                dwsharemode: u32,
                lpsecurityattributes: *const core::ffi::c_void,
                dwcreationdisposition: u32,
                dwflagsandattributes: u32,
                htemplatefile: isize,
            ) -> isize;
            pub fn CloseHandle(hobject: isize) -> i32;
            pub fn GetLastError() -> u32;
        }
    }
    use extlock::*;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    // SAFETY：CreateFileW 仅按传入缓冲以只读+共享读打开句柄，成功即关；
    // 失败时取 GetLastError 判共享冲突，无其他副作用。
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            INVALID_HANDLE_VALUE,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        // 共享冲突 = 编辑器仍持写句柄（锁未释放）→ 继续等；其余失败
        //（不存在 / 权限）按已释放放行（C# 只对 IOException 推迟）。
        unsafe { GetLastError() != ERROR_SHARING_VIOLATION }
    } else {
        unsafe { CloseHandle(h) };
        true
    }
}

#[cfg(not(windows))]
fn ext_write_lock_released(_path: &std::path::Path) -> bool {
    true
}

// ── 内置编辑器保存回传（CR-6：副本 → 会话 → STAT 冲突探测 → WriteFile）──

/// 回传冲突判定（CR-6④）：与外部 STAT 分支同款判据——远端已存在且回流
/// mtime 与下载基准差 ≥1s（秒级粒度）；任一侧未报 mtime / 文件不存在不判。
/// 纯函数便于钉子做「同款夹具内外同判定」比对。
fn mtime_conflict(base_mtime: Option<i64>, remote_mtime: Option<i64>, exists: bool) -> bool {
    match (base_mtime, remote_mtime) {
        (Some(a), Some(b)) => exists && a.abs_diff(b) >= 1,
        _ => false,
    }
}

/// 内置编辑器回传失败态回写（CR-6）：副本保留、pending 清零、dirty 回 true、
/// save_err 记原因——状态栏红字提示，再按 Ctrl+S 可重试。回执唤醒编辑器
/// 子视口（项2）：编辑器独立视口不自重绘，不唤醒则状态栏红字挂到用户碰它。
fn doc_upload_failed(st: &mut State, ctx: &egui::Context, doc_id: u64, reason: &str) {
    for doc in &st.editors.docs {
        let mut d = doc.lock().unwrap();
        if d.id == doc_id {
            d.pending_upload = false;
            d.dirty = true;
            d.save_err = Some(tfmt("回传未完成：{0}", &[&reason.to_string()]));
        }
    }
    ctx.request_repaint_of(editor_viewport(doc_id));
}

/// 编辑器文档视口 id（与 editor_window::show_all 声明口径一致，确定性重建）。
fn editor_viewport(doc_id: u64) -> egui::ViewportId {
    egui::ViewportId(egui::Id::unique(("editor", doc_id)))
}

/// 会话池匹配（P1 首个命中侧，行为同现状）：遍历两侧会话找持有该主机的会话。
fn session_for_host(st: &State, host: &str) -> Option<(usize, u64)> {
    st.remotes.iter().enumerate().find_map(|(side, rs)| {
        rs.sessions
            .iter()
            .find(|s| s.host.as_ref().is_some_and(|x| x.host == host))
            .map(|s| (side, s.sid))
    })
}

/// 内置编辑器保存回传分派（CR-6③）：副本读出（失败即失败态、不发任何 ssh
/// 请求）→ 会话匹配（失配同失败态，禁止 drain 后吞掉）→ STAT 冲突探测挂
/// doc_stat 槽（请求号沿用 stat_seq 发号，Ev::Stat 按 req 认领）。副本全文
/// 随槽带入（项3：单次保存只发生一次 fs::read；无冲突直通复用，冲突「覆盖」
/// 走 doc_upload 重读最新）。槽被占（上一回传探测在途）时事件重新入队下一帧
/// 再处理，防单槽覆盖丢事件。
fn saved_upload_dispatch(st: &mut State, ctx: &egui::Context, ev: crate::editor::SavedEvent) {
    if st.doc_stat.is_some() {
        crate::editor::EditorHub::push_saved(ev);
        return;
    }
    st.terminal.push((
        crate::local::now_hms(),
        tfmt("[{0}] 上传编辑副本…", &[&ev.remote_path]),
    ));
    // 副本读失败即刻失败态（对齐外部链路：绝不以空字节写远端）。
    let data = match std::fs::read(&ev.local_path) {
        Ok(d) => d,
        Err(e) => {
            let msg = e.to_string();
            st.terminal.push((
                crate::local::now_hms(),
                tfmt("[{0}] 回传未完成：{1}", &[&ev.remote_path, &msg]),
            ));
            doc_upload_failed(st, ctx, ev.doc_id, &msg);
            return;
        }
    };
    let Some((side, sid)) = session_for_host(st, &ev.host) else {
        let msg = t("未找到主机会话").to_owned();
        st.terminal.push((
            crate::local::now_hms(),
            tfmt("[{0}] 回传未完成：{1}", &[&ev.remote_path, &msg]),
        ));
        doc_upload_failed(st, ctx, ev.doc_id, &msg);
        return;
    };
    st.stat_seq += 1;
    let req = st.stat_seq;
    st.doc_stat = Some((
        req,
        ev.doc_id,
        ev.remote_path.clone(),
        ev.local_path.clone(),
        data,
        ev.base_mtime,
    ));
    if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
        ss.ssh.stat(ev.remote_path.clone(), req);
    } else {
        st.doc_stat = None;
        doc_upload_failed(st, ctx, ev.doc_id, t("未找到主机会话"));
    }
}

/// 内置编辑器「覆盖」回传（CR-6⑤ 冲突弹窗分派）：读副本 → 会话匹配 → 直接
/// WriteFile（请求号 = doc_id，Ev::FileWritten 内置分支按 doc.id 认领）。
pub(super) fn doc_upload(
    st: &mut State,
    ctx: &egui::Context,
    doc_id: u64,
    local_path: &std::path::Path,
    remote_path: &str,
) {
    let data = match std::fs::read(local_path) {
        Ok(d) => d,
        Err(e) => {
            let msg = e.to_string();
            doc_upload_failed(st, ctx, doc_id, &msg);
            return;
        }
    };
    let host = st.editors.docs.iter().find_map(|doc| {
        let d = doc.lock().unwrap();
        (d.id == doc_id)
            .then(|| d.remote.as_ref().map(|r| r.host.clone()))
            .flatten()
    });
    let Some((side, sid)) = host.as_deref().and_then(|h| session_for_host(st, h)) else {
        doc_upload_failed(st, ctx, doc_id, t("未找到主机会话"));
        return;
    };
    if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
        ss.ssh.write_file(remote_path.to_owned(), doc_id, data);
    } else {
        doc_upload_failed(st, ctx, doc_id, t("未找到主机会话"));
    }
}

/// 外部编辑回传：读临时副本全文 → WriteFile（请求号从 stat_seq 分配并占用
/// 挂靠 tab 的 ext_write 槽，Ev::FileWritten 按此分流；借用错开：先取副本
/// 路径与会话 id，再动槽位与会话）。
fn ext_upload(st: &mut State, side: usize, tab_id: u32, remote_path: &str) {
    let pre = find_remote_tab_mut(st, side, tab_id).and_then(|t| {
        t.ext_edits
            .iter()
            .find(|e| e.remote_path == remote_path)
            .map(|e| (e.local.clone(), t.sid))
    });
    let Some((local, sid)) = pre else {
        return;
    };
    if let Ok(data) = std::fs::read(&local) {
        st.stat_seq += 1;
        let req = st.stat_seq;
        if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
            tab.ext_write = Some(req);
        }
        if let Some(ssh) = session_mut(st, side, sid).map(|s| &s.ssh) {
            ssh.write_file(remote_path.to_owned(), req, data);
        } else {
            // 会话已断：清槽防悬挂（正常路径 Stat 刚有响应，几乎不可达）。
            if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                tab.ext_write = None;
            }
        }
    }
}

/// 外部编辑会话轮询（egui 帧驱动，C# FileSystemWatcher + 防抖 + 写锁检测
/// 等价实现——watcher 仅作「有动静」信号源，最终判定同为快照比对）：
/// - 每 300ms 对各会话本地副本取快照（长度+mtime）；
/// - 快照 ≠ 基准 → 起算静默期（期内再变重新计时）；
/// - 静默期满且快照稳定且写锁已释放 → 触发一次询问（触发即更新基准：
///   同一次保存只问一次，「暂不保存」后再保存会再问）；
/// - 静默期满且快照稳定但写锁未释放 → 记等待起点、分级提醒（项6）：
///   0–SOFT 无提醒；SOFT–TIMEOUT 窗格状态栏软提示；≥TIMEOUT 进兜底——
///   终端恰 1 条 WARN + 状态栏持久提示 + 弹 ExtEditLockWait 退出对话框。
///   超时 ≠ 放行上传：暂缓等待、锁释放后自动继续走原确认框，全程不上传。
/// - 弹窗占用时不触发也不动基准（下轮再问），防弹窗套弹窗。
fn poll_ext_edits(st: &mut State) {
    // 快照稳定一次即触发（= 一个 POLL_MS；C# DebounceMs 800ms 缩短——外部
    // Ctrl+S 到弹确认框不再白等 1.1s；防抖语义不变：期内再变重新起算）。
    const QUIET_MS: u128 = POLL_MS;
    const POLL_MS: u128 = 300;
    let now = std::time::Instant::now();
    for side in 0..2 {
        for ti in 0..st.remotes[side].tabs.len() {
            // 主机标签失配（重连/换主机）→ 旧会话静默作废（≈C# 会话随视图销毁）。
            // 标签按该 tab 挂靠会话的档案取（P1 每 tab 一会话）。
            let tab_sid = st.remotes[side].tabs[ti].sid;
            let host_tag = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == tab_sid)
                .and_then(|s| s.host.clone())
                .map(|h| format!("[{}]", dialogs::host_tab_naming(&h.name, &h.host)))
                .unwrap_or_else(|| format!("[{}]", t("远程")));
            let tab_id = st.remotes[side].tabs[ti].id;
            let tab = &mut st.remotes[side].tabs[ti];
            tab.ext_edits.retain(|e| e.host_tag == host_tag);
            if now.duration_since(tab.ext_poll).as_millis() < POLL_MS {
                continue;
            }
            tab.ext_poll = now;
            for e in tab.ext_edits.iter_mut() {
                let snap = ext_snapshot(&e.local);
                if snap == e.baseline {
                    // 快照回基线：等待结束（锁释放后被编辑器改回 / 会话复位），
                    // 清锁等待状态与窗格提示（独立标记成对清，防英文残留）。
                    if e.lock_wait_since.take().is_some() {
                        e.lock_warned = false;
                        e.lock_dialog_done = false; // 下次等锁超时可再弹（CR-10）
                        if tab.lock_msg_on {
                            tab.pane.msg = None;
                            tab.lock_msg_on = false;
                        }
                    }
                    e.quiet_since = None;
                    e.quiet_snap = None;
                    continue;
                }
                match e.quiet_since {
                    // 快照稳定满静默期：写锁已释放才触发（C# 独占写探测）。
                    Some(since)
                        if e.quiet_snap == Some(snap)
                            && since.elapsed().as_millis() >= QUIET_MS
                            && ext_write_lock_released(&e.local) =>
                    {
                        // 锁等待结束：恰 1 条 INFO + 清窗格提示（项6 去重要求）。
                        if e.lock_wait_since.take().is_some() {
                            e.lock_warned = false;
                            e.lock_dialog_done = false; // 下次等锁超时可再弹（CR-10）
                            st.terminal.push((
                                crate::local::now_hms(),
                                t("写锁已释放，继续回传").to_owned(),
                            ));
                            if tab.lock_msg_on {
                                tab.pane.msg = None;
                                tab.lock_msg_on = false;
                            }
                        }
                        // 弹窗占用 → 基准不动，下轮再问。
                        if let dialogs::Dialog::None = st.dialog {
                            // 触发即更新基准：同一次保存只问一次。
                            e.baseline = snap;
                            e.quiet_since = None;
                            e.quiet_snap = None;
                            st.dialog = dialogs::Dialog::ExtEditChanged {
                                side,
                                tab: tab_id,
                                name: e.name.clone(),
                                remote_path: e.remote_path.clone(),
                            };
                        }
                    }
                    // 快照稳定但写锁未释放（编辑器仍在保存）→ 记等待起点 +
                    // 分级提醒（软提示 → 超时兜底 WARN + 退出对话框，项6）。
                    Some(since)
                        if e.quiet_snap == Some(snap)
                            && since.elapsed().as_millis() >= QUIET_MS =>
                    {
                        let started = e.lock_wait_since.get_or_insert(now);
                        let wait = started.elapsed().as_millis();
                        if wait >= LOCK_TIMEOUT_MS {
                            if !e.lock_warned {
                                e.lock_warned = true;
                                st.terminal.push((
                                    crate::local::now_hms(),
                                    tfmt(
                                        "写锁未释放，已暂缓回传：{0}（{1}s）——不会上传半写文件",
                                        &[&e.name, &wait],
                                    ),
                                ));
                            }
                            // 超时框去重（CR-10，与 lock_warned 日志去重分离）：
                            // 本次等锁只弹一次；被别的框占用时本轮不弹不置位，
                            // 下轮补弹（挪进 !lock_warned 块会在占用时永久不弹）。
                            if !e.lock_dialog_done && matches!(st.dialog, dialogs::Dialog::None) {
                                st.dialog = dialogs::Dialog::ExtEditLockWait {
                                    side,
                                    tab: tab_id,
                                    name: e.name.clone(),
                                    remote_path: e.remote_path.clone(),
                                };
                                e.lock_dialog_done = true;
                            }
                        }
                    }
                    // 首次变化 / 期内又变 → 重新起算静默期。
                    _ => {
                        e.quiet_since = Some(now);
                        e.quiet_snap = Some(snap);
                    }
                }
            }
            // 窗格状态栏提示（项6，每轮 poll 重算、无状态残留）：≥SOFT 软
            // 提示 / ≥TIMEOUT 持久提示；清零走独立标记 lock_msg_on（成对置位）。
            let lockmsg = tab.ext_edits.iter().find_map(|e| {
                let s = e.lock_wait_since.as_ref()?;
                let wait = s.elapsed().as_millis();
                (wait >= LOCK_SOFT_MS).then(|| {
                    if wait >= LOCK_TIMEOUT_MS {
                        tfmt("写锁未释放，已暂缓回传：{0}（{1}s）", &[&e.name, &wait])
                    } else {
                        t("等待编辑器完成保存…").to_owned()
                    }
                })
            });
            match lockmsg {
                Some(m) => {
                    tab.pane.msg = Some(m);
                    tab.lock_msg_on = true;
                }
                None if tab.lock_msg_on => {
                    tab.pane.msg = None;
                    tab.lock_msg_on = false;
                }
                None => {}
            }
        }
    }
}

// ── 传输发起（T4）──────────────────────────────────────────

/// 单条「传输到另一侧」任务构造（C# TransferToOppositeAsync 循环体）：
/// 返回 (任务, 目标侧, 远端待探测路径)。源/目标四组合（pane = 发起窗格，
/// dst = 1 - pane；drop 目标语义 = 对侧窗格当前可见视图）：
/// - 本地源（活动本地 tab）+ 对侧远程 → 上传（异步 STAT 探冲突）；
/// - 远程源 + 对侧远程 → 远程↔远程中转（Relay，经本地 temp 两阶段，对齐
///   C# RelayRemoteToRemoteAsync）；左右可打开同一主机——两份档案取各自侧
///   会话（独立连接互不干扰）；
/// - 远程源 + 对侧本地（活动本地 tab）→ 下载（本地目标同步探测冲突）；
/// - 本地源 + 对侧本地 → 本地复制（C# 本地→本地=复制，CopyLocal/CopyLocalDir）。
fn build_transfer(
    st: &State,
    pane: usize,
    row: usize,
) -> Option<(crate::transfer::NewTask, usize, Option<String>, i64)> {
    let dst = 1 - pane;
    // 源/目标窗格类型均按活动 tab 自驱（本地 tab = 本地 pane / 远程 tab = 该侧
    // 远程 pane，两侧对称）。
    // 任务归属（P3）：src/dst 各为 (侧, 发起/目标活动 tab id)——关闭确认计数、
    // 定向刷新、按归属取消都按此定位；无活动 tab（不可能，发起必有视图）兜底 0。
    let owner = crate::transfer::TaskOwner {
        src: (pane as u8, active_tab_id(st, pane).unwrap_or(0)),
        dst: (dst as u8, active_tab_id(st, dst).unwrap_or(0)),
    };
    let src_remote = !pane_is_local(st, pane);
    let dst_remote = !pane_is_local(st, dst) && pane_view(st, dst).remote_path.is_some();
    let (src_name, src_is_dir, size, src_mtime) = match pane_view(st, pane).entries.get(row) {
        Some(e) => (e.name.clone(), e.is_dir, e.size_bytes, e.mtime_secs),
        None => return None,
    };
    match (src_remote, dst_remote) {
        // 本地 → 远程：上传（冲突在远端，异步 STAT）。
        (false, true) => {
            let ldir = pane_view(st, pane).path.clone()?; // 根视图（驱动器列表）不支持整盘传输
            let src = ldir.join(&src_name);
            let rdir = pane_view(st, dst).remote_path.clone()?;
            let remote = join_remote_path(&rdir, &src_name);
            let kind = if src_is_dir {
                crate::transfer::Kind::UploadDir {
                    local: src.clone(),
                    remote: remote.clone(),
                }
            } else {
                crate::transfer::Kind::UploadFile {
                    local: src.clone(),
                    remote: remote.clone(),
                }
            };
            Some((
                crate::transfer::NewTask {
                    source: src.to_string_lossy().into_owned(),
                    dest: remote.clone(),
                    size,
                    force: false,
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                dst,
                Some(remote),
                src_mtime,
            ))
        }
        // 远程 → 远程：Relay 两阶段（源 → 本地 temp → 目标），冲突在目标端（异步 STAT）。
        (true, true) => {
            let rdir = pane_view(st, pane).remote_path.clone()?;
            let remote_src = join_remote_path(&rdir, &src_name);
            let rdir_d = pane_view(st, dst).remote_path.clone()?;
            let remote_dst = join_remote_path(&rdir_d, &src_name);
            let temp = relay_temp_dir(&remote_src, &remote_dst);
            // 两份档案取各自侧活动远程 tab 的会话（左右可打开同一主机——独立
            // 连接互不干扰；活动 tab 即源/目标视图）。
            let (src_prof, dst_prof) = (
                active_remote_session(st, pane).and_then(|s| s.profile.clone())?,
                active_remote_session(st, dst).and_then(|s| s.profile.clone())?,
            );
            let kind = if src_is_dir {
                crate::transfer::Kind::RelayDir {
                    src: src_prof,
                    dst: dst_prof,
                    remote_src: remote_src.clone(),
                    remote_dst: remote_dst.clone(),
                    temp: temp.clone(),
                }
            } else {
                crate::transfer::Kind::RelayFile {
                    src: src_prof,
                    dst: dst_prof,
                    remote_src: remote_src.clone(),
                    remote_dst: remote_dst.clone(),
                    temp: temp.clone(),
                }
            };
            Some((
                crate::transfer::NewTask {
                    source: remote_src.clone(),
                    dest: remote_dst.clone(),
                    size,
                    force: false,
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                dst,
                Some(remote_dst),
                src_mtime,
            ))
        }
        // 远程 → 本地：下载（冲突在本地目标，同步探测）。
        (true, false) => {
            let ldir = pane_view(st, dst).path.clone()?; // 根视图（驱动器列表）不支持整盘传输
            let rdir = pane_view(st, pane).remote_path.clone()?;
            let remote = join_remote_path(&rdir, &src_name);
            let local = ldir.join(&src_name);
            let kind = if src_is_dir {
                crate::transfer::Kind::DownloadDir {
                    remote: remote.clone(),
                    local: local.clone(),
                }
            } else {
                crate::transfer::Kind::DownloadFile {
                    remote: remote.clone(),
                    local: local.clone(),
                }
            };
            Some((
                crate::transfer::NewTask {
                    source: remote.clone(),
                    dest: local.to_string_lossy().into_owned(),
                    size,
                    force: false,
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                pane,
                None,
                src_mtime,
            ))
        }
        // 本地 → 本地：复制（C# FileView 本地→本地=复制；冲突在本地目标，同步探测）。
        (false, false) => {
            let ldir = pane_view(st, pane).path.clone()?;
            let ddir = pane_view(st, dst).path.clone()?;
            let from = ldir.join(&src_name);
            let to = ddir.join(&src_name);
            let kind = if src_is_dir {
                crate::transfer::Kind::CopyLocalDir {
                    from: from.clone(),
                    to: to.clone(),
                }
            } else {
                crate::transfer::Kind::CopyLocal {
                    from: from.clone(),
                    to: to.clone(),
                }
            };
            Some((
                crate::transfer::NewTask {
                    source: from.to_string_lossy().into_owned(),
                    dest: to.to_string_lossy().into_owned(),
                    size,
                    force: false,
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                pane,
                None,
                src_mtime,
            ))
        }
    }
}

/// 「传输到另一侧」批量入口（C# TransferToOppositeAsync(items) :3034：菜单 =
/// SelectedItems、拖拽 Drop = 被拖集合）：逐条构造、整批一次冲突确认
/// （远程目标经目标侧会话异步 STAT；本地目标同步探测冲突）。同批同源同
/// 目标侧，传输档案侧取首条。
pub(crate) fn begin_transfer_multi(st: &mut State, pane: usize, rows: &[usize]) {
    let built: Vec<(crate::transfer::NewTask, usize, Option<String>, i64)> = rows
        .iter()
        .filter_map(|&row| build_transfer(st, pane, row))
        .collect();
    if built.is_empty() {
        return;
    }
    // F1③：上批尚在暂存（确认弹窗待决策 / 预检回流中）时新拖拽显式接管——
    // 未落位暂存部分不再无提示覆盖丢弃，终端落对账日志并关旧确认弹窗；
    // 已落位行不可逆，继续传输（batches 不回溯清行）。
    if let Some(old) = st.pending_tx.take() {
        let n = old.items.len();
        st.terminal.push((
            crate::local::now_hms(),
            tfmt("已放弃上一批待确认的 {0} 项（未入队）", &[&n.to_string()]),
        ));
        if matches!(st.dialog, dialogs::Dialog::TransferConfirm { .. }) {
            st.dialog = dialogs::Dialog::None;
            st.conflict_sel = None; // 旧弹窗勾选态随弹窗一并作废
        }
    }
    let profile_side = built[0].1;
    let mut items: Vec<PendingItem> = built
        .into_iter()
        .map(|(task, _, stat_path, src_mtime)| PendingItem {
            task,
            stat_path,
            conflict: false,
            src_mtime,
            dst_size: None,
            dst_mtime: None,
            probe_incomplete: false,
        })
        .collect();
    if items[0].stat_path.is_some() {
        // 远程目标：整批共用一个请求号批量并发 STAT（T20-G③：一次 StatBatch
        // 替代逐条——此前逐条串行 = O(n×RTT)，万级批次分钟级才回流，队列长期
        // 无法形成；Ev::Stat 回流按 profile_side 匹配会话聚合不变）。
        // T20-H 流式落位：回流结果每满 dnd_stream_batch 个无冲突条目即入队
        // 一批（队列逐批长出，估算与传输并行；冲突即中止转确认弹窗）。
        // S-1：探测期间源面板状态栏挂瞬态提示（C# :3069「点了没反应」对策）。
        st.stat_seq += 1;
        let seq = st.stat_seq;
        if let Some(ss) = active_remote_session(st, profile_side) {
            let paths: Vec<String> = items.iter().filter_map(|pi| pi.stat_path.clone()).collect();
            ss.ssh.stat_batch(paths, seq);
        }
        pane_view_mut(st, pane).msg = Some(preparing_msg());
        st.pending_tx = Some(PendingTx {
            seq,
            profile_side,
            src_pane: pane,
            at: std::time::Instant::now(),
            resolved: 0,
            ready: 0,
            batch: dnd_stream_batch(),
            batches: Vec::new(),
            items,
            placed: 0,
        });
    } else {
        // 本地目标同步探测：逐条探测，仅真实冲突项进确认清单（C# 批量一次弹窗）。
        let mut any = false;
        for pi in &mut items {
            if let Ok(m) = std::fs::metadata(&pi.task.dest) {
                pi.conflict = true;
                pi.dst_size = Some(m.len());
                pi.dst_mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                any = true;
            }
        }
        if any {
            let rows = confirm_data(&items);
            // F-10 方案A：本地批次弹窗勾选态默认值（目标较新默认不勾）。
            st.conflict_sel = Some(conflict_defaults(&rows));
            st.pending_tx = Some(PendingTx {
                seq: 0,
                profile_side,
                src_pane: pane,
                at: std::time::Instant::now(),
                // 本地批次无回流（seq=0）：流式禁用，走确认弹窗批级决策。
                resolved: 0,
                ready: 0,
                batch: usize::MAX,
                batches: Vec::new(),
                items,
                placed: 0,
            });
            st.dialog = dialogs::Dialog::TransferConfirm { rows, note: None };
        } else {
            enqueue_tasks(
                st,
                profile_side,
                items.into_iter().map(|pi| pi.task).collect(),
                0,
            );
        }
    }
}

/// 覆盖确认弹窗数据：冲突清单行（C# 冲突对话框对比表逐项事实，批级一次决策）。
fn confirm_data(items: &[PendingItem]) -> Vec<dialogs::ConflictRow> {
    items
        .iter()
        .filter(|pi| pi.conflict)
        .map(|pi| dialogs::ConflictRow {
            name: pi
                .task
                .source
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&pi.task.source)
                .to_owned(),
            src_size: pi.task.size,
            src_mtime: pi.src_mtime,
            dst_size: pi.dst_size,
            dst_mtime: pi.dst_mtime,
        })
        .collect()
}

/// F-10 方案A：冲突行勾选默认值（纯逻辑便于钉子）——目标较新（dst_mtime >
/// src_mtime）时覆盖会砸掉较新的目标文件 → 默认不勾（跳过）；其余默认勾选
/// （覆盖上传）。目标无 mtime → 勾选。
pub(crate) fn conflict_defaults(rows: &[dialogs::ConflictRow]) -> Vec<bool> {
    rows.iter()
        .map(|r| !r.dst_mtime.is_some_and(|m| m > r.src_mtime))
        .collect()
}

/// PendingItem 版默认勾选（sel 缺项 / None 兜底，与 conflict_defaults 同判据）。
fn item_default(pi: &PendingItem) -> bool {
    !pi.dst_mtime.is_some_and(|m| m > pi.src_mtime)
}

/// PendingItem → NewTask 提取（F-12 三处提取点共用：settle_batch 直入 /
/// stream_place 逐批 / settle_tasks 确认结算）：预检超窗透传位随任务入队
/// ——消费=透传，标记只写不读即失去意义；下游 resume_decision 凭此禁
/// Append（防超窗放行项把远端同名旧目标当断点静默拼接）。
fn into_task(mut pi: PendingItem) -> crate::transfer::NewTask {
    pi.task.probe_incomplete = pi.probe_incomplete;
    pi.task
}

/// 覆盖确认结算映射（纯逻辑便于钉子，F1 + F-10 方案A）：sel = 弹窗逐项
/// 勾选态（与冲突行序对齐；None / 缺项 = 按默认勾选值推断——目标较新跳过、
/// 其余覆盖）。勾选冲突任务置 force（C# forceOverwrite：用户明确覆盖 →
/// 跳过续传决策直接全量，防旧小文件被误判为中断半成品）；未勾选冲突项
/// 剔除并计数 skipped；非冲突项照常入队。返回 (待入队任务, skipped 条数)。
fn settle_tasks(
    items: Vec<PendingItem>,
    sel: Option<&[bool]>,
) -> (Vec<crate::transfer::NewTask>, u64) {
    let mut j = 0usize;
    let mut skipped = 0u64;
    let tasks: Vec<crate::transfer::NewTask> = items
        .into_iter()
        .filter_map(|mut pi| {
            if pi.conflict {
                let upload = sel
                    .and_then(|s| s.get(j).copied())
                    .unwrap_or_else(|| item_default(&pi));
                j += 1;
                if !upload {
                    skipped += 1;
                    return None;
                }
                pi.task.force = true;
            }
            Some(into_task(pi))
        })
        .collect();
    (tasks, skipped)
}

/// 覆盖确认结算：按弹窗勾选态（st.conflict_sel，F-10 方案A）分流——勾选
/// 冲突项置 force 入队、未勾选剔除计数。F1：按下「应用」必有确定结果——
/// 入队，或全不勾时终端落对账日志（skipped 计数可见，不再零入队零提示零日志）。
pub(crate) fn enqueue_pending(st: &mut State) {
    let Some(ptx) = st.pending_tx.take() else {
        return;
    };
    let sel = st.conflict_sel.take();
    let profile_side = ptx.profile_side;
    let (tasks, skipped) = settle_tasks(ptx.items, sel.as_deref());
    if !tasks.is_empty() {
        enqueue_tasks(st, profile_side, tasks, skipped);
    } else if skipped > 0 {
        // F1：全冲突批选「跳过冲突项」→ 零入队但明确放弃，留对账日志。
        st.terminal.push((
            crate::local::now_hms(),
            tfmt(
                "已跳过 {0} 个冲突项，无剩余任务入队",
                &[&skipped.to_string()],
            ),
        ));
    }
}

/// 流式落位阈值快照（settings.dnd_stream_batch；内定值不进设置面板，手编
/// settings.json 可调；下限 1 兜底）。
fn dnd_stream_batch() -> usize {
    crate::settings::with(|s| s.dnd_stream_batch).max(1) as usize
}

/// 入队（惰性建队列线程；远程任务按目标侧活动远程 tab 会话的连接档案建传输
/// 连接）。`skipped` = 覆盖确认「跳过冲突项」条数（无冲突路径恒 0）。
/// 返回批次号（T20-H 流式落位逐批记录，供弹窗取消回溯清行）。
fn enqueue_tasks(
    st: &mut State,
    profile_side: usize,
    tasks: Vec<crate::transfer::NewTask>,
    skipped: u64,
) -> u64 {
    // G12：队列按批持该侧活动远程 tab 会话探测快照实时句柄（本地任务无会话时
    // 用空快照）——双侧会话后传输后校验跟批次侧走；先取句柄再建队列，避免
    // &mut st 借用冲突。
    let (probe, profile) = match active_remote_session(st, profile_side) {
        Some(ss) => (ss.ssh.probe_handle(), ss.profile.clone()),
        None => (
            std::sync::Arc::new(std::sync::Mutex::new((0u64, None))),
            None,
        ),
    };
    let q = st
        .transfer
        .get_or_insert_with(crate::transfer::TransferQueue::new);
    q.enqueue(profile, probe, tasks, skipped)
}

/// G17 关闭确认门（C# FolderContainer.OnTabCloseConfirming）：该 tab 归属的
/// 活动传输任务（owner.src/dst 命中且 Waiting/Transferring，P3 per-tab 判据
/// ——C# LogScope 语义）> 0 → 三键确认；否则直接关。
fn request_close_tab(st: &mut State, side: usize, tab_id: u32) {
    let key = (side as u8, tab_id);
    let n = st
        .transfer
        .as_ref()
        .map(|q| {
            q.with_items(|items| {
                crate::transfer::count_owned_active(items.iter().map(|i| (&i.owner, i.status)), key)
            })
        })
        .unwrap_or(0);
    if n > 0 {
        st.dialog = dialogs::Dialog::TabClose {
            side,
            tab: tab_id,
            n,
        };
    } else {
        finish_close_tab(st, side, tab_id);
    }
}

/// G17 执行关闭（C# OnTabCloseRequested 资源清理）：远程 tab → 挂靠会话 refs-1
/// （归零断开并出池，C# view.Dispose → _sftp.Dispose；worker 断开并发「已断开」
/// 日志 + Ev::Disconnected），tab 移除；本地 tab → 仅移除（tab 持自身 pane，无
/// 共享态可复位）。最后一个 tab 关闭回初始占位（C# AddNewTab(emptyState: true)：
/// 左侧 = 「本地」+ 本地根视图；右侧 = 「远程」+ 空态——空态下标签条隐藏）。
/// Header 按钮状态每帧重算，无需显式通知（C# TabCountChanged /
/// NotifySyncStateChanged 的 Rust 等价物）。
fn finish_close_tab(st: &mut State, side: usize, tab_id: u32) {
    // 该 tab 相关在途树请求一并作废。
    if st
        .tree_req
        .as_ref()
        .is_some_and(|(s, t, _, _)| *s == side && *t == tab_id)
    {
        st.tree_req = None;
    }
    // 该 tab 的在途删除/新建命令 tag 作废：迟到响应落「未知 tag 静默丢弃」。
    st.inflight.retain(|_, v| !(v.0 == side && v.1 == tab_id));
    // 关闭前该 tab 在合并列表中的位置（本地 tab 在前，与绘制层合成顺序一致）；
    // 非活动 tab 的关闭（临时视图差集回流）不动活动项。
    let was_active =
        st.locals[side].active == Some(tab_id) || st.remotes[side].active == Some(tab_id);
    let pos = if let Some(i) = st.locals[side].tabs.iter().position(|t| t.id == tab_id) {
        st.locals[side].tabs.remove(i);
        // 关闭 tab 属挂接对本地端 → 联动拆对（同远程端口径，P4 挂接对粒度）。
        if st
            .sync_base
            .as_ref()
            .is_some_and(|b| b.l_side == side as u8 && b.l_tab == tab_id)
        {
            st.sync_browse = false;
            st.sync_base = None;
            st.sync_mirror = None;
        }
        Some(i)
    } else if let Some(i) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) {
        let sid = st.remotes[side].tabs[i].sid;
        if let Some(si) = st.remotes[side].sessions.iter().position(|s| s.sid == sid) {
            let dead = {
                let ss = &mut st.remotes[side].sessions[si];
                ss.refs = ss.refs.saturating_sub(1);
                ss.refs == 0
            };
            if dead {
                st.remotes[side].sessions[si].ssh.disconnect();
                st.remotes[side].sessions.remove(si);
            }
        }
        let merged_pos = st.locals[side].tabs.len() + i;
        st.remotes[side].tabs.remove(i);
        // 关闭 tab 属挂接对端 → 联动拆对（C# NotifySyncStateChanged 重算联动
        // 按钮）；无关 tab 关闭不影响挂接对（P4 挂接对粒度）。
        if st.sync_base.as_ref().is_some_and(|b| {
            (b.r_side == side as u8 && b.r_tab == tab_id)
                || (b.l_side == side as u8 && b.l_tab == tab_id)
        }) {
            st.sync_browse = false;
            st.sync_base = None;
            st.sync_mirror = None;
        }
        Some(merged_pos)
    } else {
        None
    };
    if was_active {
        st.locals[side].active = None;
        st.remotes[side].active = None;
        // 合并剩余列表（本地在前 + 远程在后）：位置承接（下一 tab 滑入原位），
        // 左侧优先激活「本地」tab（C# FolderContainer 关远程 tab 后激活本地视图）。
        let merged: Vec<(u32, panes::TabKind)> = st.locals[side]
            .tabs
            .iter()
            .map(|t| (t.id, panes::TabKind::Local))
            .chain(
                st.remotes[side]
                    .tabs
                    .iter()
                    .map(|t| (t.id, panes::TabKind::Remote)),
            )
            .collect();
        if merged.is_empty() {
            // 该侧最后一个 tab 关闭回初始占位（C# AddNewTab(emptyState: true)）。
            if side == 0 {
                let mut pane = panes::FilePane::default();
                crate::local::navigate_root(&mut pane);
                let id = st.next_tab_id;
                st.next_tab_id += 1;
                st.locals[side] = LocalSide {
                    tabs: vec![LocalTab { id, pane }],
                    active: Some(id),
                };
            } else {
                let id = st.next_tab_id;
                st.next_tab_id += 1;
                let mut rt = placeholder_remote_tab(id);
                rt.pane.empty_title = t("先选择一个主机").to_owned();
                rt.pane.empty_sub = t("选择要浏览的本地或远端文件系统").to_owned();
                st.remotes[side] = RemoteSide {
                    tabs: vec![rt],
                    active: Some(id),
                    sessions: Vec::new(),
                };
            }
        } else {
            let new_active = match pos {
                Some(p) => {
                    let i = p.min(merged.len() - 1);
                    match side {
                        0 => merged
                            .iter()
                            .find(|(_, k)| *k == panes::TabKind::Local)
                            .map(|(id, _)| *id),
                        _ => merged.get(i).map(|(id, _)| *id),
                    }
                }
                None => st.locals[side].active.or(st.remotes[side].active),
            };
            if let Some(id) = new_active {
                if st.locals[side].tabs.iter().any(|t| t.id == id) {
                    st.locals[side].active = Some(id);
                } else {
                    st.remotes[side].active = Some(id);
                }
            }
        }
    }
}

/// 远程目录 + 名称拼接（原为 transfer::join_remote 的同代码复制，2026-11-06
/// T7 合并：C# RemotePath.Join 有 TrimEnd('/') 规整，复制版原样拼接会产出
/// 三斜杠——统一走 transfer 单实现，本名保留供既有 10 处调用点）。
pub(crate) use crate::transfer::join_remote as join_remote_path;

/// 远程↔远程中转暂存目录（C# RelayTempDirName 同口径）：
/// %TEMP%/iFreeSftp/RemoteTransfer/<sha256("源|目标") 前 24 hex>。
/// 确定性路径：中转失败重试时 temp 残留即续传起点（阶段一/二各自断点续传）。
fn relay_temp_dir(remote_src: &str, remote_dst: &str) -> std::path::PathBuf {
    let key = format!("{remote_src}|{remote_dst}");
    let digest = ring::digest::digest(&ring::digest::SHA256, key.as_bytes());
    let mut hex = String::with_capacity(24);
    for b in digest.as_ref().iter().take(12) {
        hex.push_str(&format!("{b:02x}"));
    }
    std::env::temp_dir()
        .join("iFreeSftp")
        .join("RemoteTransfer")
        .join(hex)
}

/// 远程路径归一化（C# AttachConnectedSession 同规则）：非 / 开头补前导 /；
/// 根保留 /；其余去尾部 /。
fn normalize_remote(p: &str) -> String {
    let p = p.trim();
    if p.is_empty() {
        return "/".to_owned();
    }
    let s = if p.starts_with('/') {
        p.to_owned()
    } else {
        format!("/{p}")
    };
    if s.len() > 1 {
        s.trim_end_matches('/').to_owned()
    } else {
        s
    }
}

/// 分隔条拖拽：竖条调左右列宽，横条调下排行高；hover 只变光标（C# 同款）。
fn splitter(ui: &mut egui::Ui, rect: Rect, vertical: bool, st: &mut State) {
    let resp = ui.allocate_rect(rect, egui::Sense::drag());
    let d = resp.drag_delta();
    if vertical {
        st.left_w += d.x;
    } else {
        st.bottom_h -= d.y;
    }
    if resp.hovered() {
        cursor_intent(
            ui.ctx(),
            CursorLayer::Hover,
            if vertical {
                egui::CursorIcon::ResizeHorizontal
            } else {
                egui::CursorIcon::ResizeVertical
            },
        );
    }
}

/// 无边框窗口边缘缩放（C# ExtendsContentIntoTitleBar 回归修复）：decorations(false)
/// 让系统缩放边随装饰一起消失 → 4 边 + 4 角各挂一层透明拖拽命中区，drag_started
/// 即交给 winit 系统缩放循环（ViewportCommand::BeginResize；Windows 走
/// WM_SYSCOMMAND SC_SIZE，对无边框窗口同样生效；min_inner_size 在模态循环内
/// 仍由 WM_GETMINMAXINFO 钳制）。最大化时整层跳过；须在帧尾调用（注册最晚 =
/// 命中优先级最高，只盖住贴边 6px，不干扰内容与弹层）。
pub fn edge_resize(ui: &mut egui::Ui, modal: bool) {
    // 弹层 / 菜单期间整层跳过（连 BeginResize 一起限）：既不让帧尾注册的贴边
    // 命中区抢弹层边缘，也不让 Edge 意图压过 Dialog 层的 Default。
    // 不前移到 dialogs::draw 之前——注册最晚 = 命中优先级最高是 BeginResize
    // 成立的前提；dialog shell 的抢命中已由 F-9 的 Foreground 分层解决。
    if modal {
        return;
    }
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
        return;
    }
    let r = ctx.input(|i| i.viewport_rect());
    let edge = 6.0; // 边命中厚度（对齐 WinUI 缩放边手感 6-8 逻辑px）
    let corner = 16.0; // 角命中宽度（角部难瞄准，放宽）
    use egui::{CursorIcon, Pos2, ResizeDirection, Sense, Vec2, ViewportCommand};
    // 角区放数组尾部：注册更晚，命中优先于边区。
    let zones: [(Rect, ResizeDirection, CursorIcon); 8] = [
        (
            Rect::from_min_max(r.left_top(), Pos2::new(r.right(), r.top() + edge)),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(Pos2::new(r.left(), r.bottom() - edge), r.right_bottom()),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(r.left_top(), Pos2::new(r.left() + edge, r.bottom())),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(Pos2::new(r.right() - edge, r.top()), r.right_bottom()),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
        (
            Rect::from_min_size(r.min, Vec2::splat(corner)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        (
            Rect::from_min_size(Pos2::new(r.right() - corner, r.top()), Vec2::splat(corner)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            Rect::from_min_size(
                Pos2::new(r.left(), r.bottom() - corner),
                Vec2::splat(corner),
            ),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (
            Rect::from_min_size(r.max - Vec2::splat(corner), Vec2::splat(corner)),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
    ];
    for (rect, dir, icon) in zones {
        let resp = ui.allocate_rect(rect, Sense::drag());
        if resp.hovered() {
            cursor_intent(&ctx, CursorLayer::Edge, icon);
        }
        if resp.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

// ============================================================================
// 远程路径拼接测试（第三批）—— 翻译自 C# RemotePathTests（4 用例）
// 追加到 src/ui/mod.rs 末尾。join_remote_path 是 transfer::join_remote 的
// re-export（本文件底部），全项目唯一实现（panes.rs 树构建已同步收敛）。
// 后两条原为 F4 分歧钉子，2026-11-06 修复（尾斜杠规整）后翻转断言。
// ============================================================================

#[cfg(test)]
mod join_tests {
    use super::join_remote_path;

    #[test]
    fn join_plain() {
        assert_eq!(join_remote_path("/home/user", "a.txt"), "/home/user/a.txt");
    }

    #[test]
    fn join_root_no_double_slash() {
        assert_eq!(join_remote_path("/", "a.txt"), "/a.txt");
    }

    // C#："/home/user/" + "a.txt" → "/home/user/a.txt"（尾斜杠去除）
    // 原为 F4 分歧钉子，2026-11-06 修复后翻转
    #[test]
    fn join_trailing_slash_normalized() {
        assert_eq!(join_remote_path("/home/user/", "a.txt"), "/home/user/a.txt");
    }

    // C#："/home//" + "a.txt" → "/home/a.txt"（多个尾斜杠规整）
    // 原为 F4 分歧钉子，2026-11-06 修复后翻转
    #[test]
    fn join_multiple_trailing_slashes_normalized() {
        assert_eq!(join_remote_path("/home//", "a.txt"), "/home/a.txt");
    }
}

// ============================================================================
// P2 连接落位初始目录优先级测试（initial_remote_dir：Ev::Connected 落位与会话
// 复用落位共用的纯逻辑）—— 三级优先：同步目录 > 上次浏览目录 > 登录主目录。
// ============================================================================

#[cfg(test)]
mod initial_dir_tests {
    use super::initial_remote_dir;
    use crate::hosts::HostProfile;

    #[test]
    fn sync_dir_wins() {
        let h = HostProfile {
            sync_enabled: true,
            remote_sync_path: "/srv/deploy".into(),
            last_remote_path: "/tmp".into(),
            ..Default::default()
        };
        let (init, fallback, base) = initial_remote_dir(&h, "/root");
        assert_eq!(init, "/srv/deploy");
        assert_eq!(fallback, None);
        assert_eq!(base, Some("/srv/deploy".to_owned()));
    }

    #[test]
    fn last_path_second_fallback_root() {
        let h = HostProfile {
            sync_enabled: true,
            remote_sync_path: "  ".into(), // 同步开但路径空白 → 不生效
            last_remote_path: "home/u".into(),
            ..Default::default()
        };
        let (init, fallback, base) = initial_remote_dir(&h, "/root");
        assert_eq!(init, "/home/u"); // 补前导 /（normalize_remote）
        assert_eq!(fallback, Some("/root".to_owned()));
        assert_eq!(base, Some("/home/u".to_owned()));
    }

    #[test]
    fn root_when_nothing_configured() {
        let h = HostProfile {
            last_remote_path: " ".into(),
            ..Default::default()
        };
        let (init, fallback, base) = initial_remote_dir(&h, "/root");
        assert_eq!(init, "/root");
        assert_eq!(fallback, None);
        assert_eq!(base, None);
    }
}

// ============================================================================
// P4 单测：SessionKey 复用匹配（P2 会话池真复用的判定键，PartialEq 口径）。
// ============================================================================

#[cfg(test)]
mod p4_tests {
    use super::SessionKey;

    #[test]
    fn session_key_same_triple_matches() {
        let a = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "root".into(),
        };
        let b = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "root".into(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn session_key_differs_on_port_or_user() {
        let a = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "root".into(),
        };
        let port = SessionKey {
            host: "srv1".into(),
            port: 2222,
            user: "root".into(),
        };
        let user = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "admin".into(),
        };
        assert_ne!(a, port);
        assert_ne!(a, user);
    }
}

// ============================================================================
// T20-G③ 单测：批量 STAT 预检超时兜底（回流丢失批次强制放行）。
// ============================================================================

#[cfg(test)]
mod t20g_tests {
    use super::*;

    /// 批次条目夹具：stat_path 挂起（Some）/ 已回流（None）。
    fn pend_item(stat_path: Option<String>) -> PendingItem {
        PendingItem {
            task: crate::transfer::NewTask {
                source: "/src/a.bin".into(),
                dest: "/dst/a.bin".into(),
                size: 1,
                force: false,
                probe_incomplete: false,
                owner: crate::transfer::TaskOwner {
                    src: (0, 1),
                    dst: (1, 2),
                },
                kind: crate::transfer::Kind::UploadFile {
                    local: "/src/a.bin".into(),
                    remote: "/dst/a.bin".into(),
                },
            },
            stat_path,
            conflict: false,
            src_mtime: 0,
            dst_size: None,
            dst_mtime: None,
            probe_incomplete: false,
        }
    }

    fn pend_tx(seq: u64, items: Vec<PendingItem>) -> PendingTx {
        PendingTx {
            seq,
            profile_side: 1,
            src_pane: 0,
            at: std::time::Instant::now(),
            resolved: 0,
            ready: 0,
            batch: 4,
            batches: Vec::new(),
            placed: 0,
            items,
        }
    }

    /// 钉子（T20-G③ 预检超时兜底）：回流丢失的批次（stat_path 仍挂起）在
    /// 超窗后被强制结算——全部条目 stat_path 清 None 可处置；零窗判定即触发。
    #[test]
    fn stat_probe_timeout_forces_settle() {
        let mut ptx = pend_tx(
            7,
            vec![pend_item(None), pend_item(Some("/dst/a.bin".into()))],
        );
        assert_eq!(
            force_settle_expired(&mut ptx, std::time::Duration::ZERO),
            1,
            "仅未回流条目计数 probe_incomplete"
        );
        assert!(ptx.items.iter().all(|pi| pi.stat_path.is_none()));
        assert!(ptx.items[1].probe_incomplete, "未回流条目带标记");
        assert!(!ptx.items[0].probe_incomplete, "已回流条目不带标记");
    }

    /// 钉子（兜底不误伤本地确认弹窗批次）：seq=0（本地同步探测、无回流等待，
    /// 弹窗可停留任意久）不受超时兜底影响。
    #[test]
    fn stat_probe_timeout_spares_local_confirm_batch() {
        let mut local = pend_tx(0, vec![pend_item(Some("/dst/b.bin".into()))]);
        assert_eq!(
            force_settle_expired(&mut local, std::time::Duration::ZERO),
            0
        );
        assert!(local.items[0].stat_path.is_some());
    }

    // ── T20-H/E 流式落位钉子 ────────────────────────────────────

    /// 钉子（逐批落位边界）：10 条批次阈值 4——第 4/8 条回流触发 Place、
    /// 第 10 条（全量结算）触发 Settle 统一收尾，其余步进 None；stream_take
    /// 摘批后不变式保持（resolved == 余下 stat_path None 条数）。
    #[test]
    fn stream_places_by_threshold_and_settles_remainder() {
        let mut ptx = pend_tx(
            9,
            (0..10)
                .map(|i| pend_item(Some(format!("/dst/f{i:03}.bin"))))
                .collect(),
        );
        let n = ptx.items.len();
        let mut places = 0;
        let mut settled = false;
        let mut placed_total = 0;
        for _ in 0..n {
            // 逐条回流：清 stat_path（事件处理器同款）后判定。
            let pi = ptx
                .items
                .iter_mut()
                .find(|pi| pi.stat_path.is_some())
                .unwrap();
            pi.stat_path = None;
            ptx.resolved += 1;
            ptx.ready += 1;
            match stream_step(&ptx) {
                StreamStep::Place => {
                    places += 1;
                    let need = ptx.ready;
                    let batch = stream_take(&mut ptx, need);
                    assert_eq!(batch.len(), ptx.batch);
                    placed_total += batch.len();
                }
                StreamStep::Settle => {
                    settled = true;
                    assert_eq!(ptx.resolved, ptx.items.len());
                    assert_eq!(
                        ptx.ready,
                        ptx.items.len(),
                        "尾批余量（无冲突测试：全部可落位）"
                    );
                    placed_total += ptx.ready;
                }
                StreamStep::None => {}
            }
            assert_eq!(
                ptx.resolved,
                ptx.items.iter().filter(|p| p.stat_path.is_none()).count(),
                "不变式：resolved == 余下已回流未落位条数"
            );
            assert_eq!(
                ptx.ready,
                ptx.items
                    .iter()
                    .filter(|p| p.stat_path.is_none() && !p.conflict)
                    .count(),
                "不变式：ready == 余下无冲突已回流未落位条数"
            );
        }
        assert_eq!(places, 2, "阈值 4 × 10 条：恰在 4/8 两次落位");
        assert!(settled, "第 10 条全量结算统一收尾");
        assert_eq!(placed_total, 10, "全部条目逐批落位无遗漏");
    }

    /// 钉子（E 策略：冲突不停流）：冲突只标记不中止——非冲突项照常按阈值
    /// 落位（ready 不含冲突项），流式全程不停；冲突项留置到集齐由 Settle
    /// 统一进弹窗（尾批无冲突 + 冲突项一起交 settle_batch 分流）。
    #[test]
    fn stream_conflicts_do_not_stop_placing_and_settle_at_complete() {
        let mut ptx = pend_tx(
            9,
            (0..12)
                .map(|i| pend_item(Some(format!("/dst/g{i}.bin"))))
                .collect(),
        );
        ptx.batch = 4;
        // 回流序：c(冲突) ok ok c ok ok ok ok c ok ok ok——第 5 条回流时
        // ready=4 触发 Place（冲突不占阈值），第 9 条再次 Place。
        let pattern = [
            true, false, false, true, false, false, false, false, true, false, false, false,
        ];
        let mut places = 0;
        let mut placed_total = 0;
        let mut settled = false;
        for (i, conflict) in pattern.iter().enumerate() {
            let pi = ptx
                .items
                .iter_mut()
                .find(|pi| pi.stat_path.is_some())
                .unwrap();
            pi.stat_path = None;
            ptx.resolved += 1;
            if *conflict {
                pi.conflict = true;
            } else {
                ptx.ready += 1;
            }
            match stream_step(&ptx) {
                StreamStep::Place => {
                    places += 1;
                    let need = ptx.ready;
                    let batch = stream_take(&mut ptx, need);
                    assert!(!batch.is_empty() && !batch.iter().any(|p| p.conflict));
                    placed_total += batch.len();
                }
                StreamStep::Settle => {
                    settled = true;
                    assert_eq!(i, pattern.len() - 1, "Settle 仅在集齐（最后一条回流）");
                }
                StreamStep::None => {}
            }
            assert_eq!(ptx.seq, 9, "E 策略无中止，seq 恒保留");
        }
        assert_eq!(
            places, 2,
            "12 条（含 3 冲突）阈值 4：恰两次落位（4+4 无冲突）"
        );
        assert_eq!(placed_total, 8, "仅无冲突项落位，冲突项不占批");
        assert!(settled, "集齐统一 Settle");
        assert_eq!(
            ptx.ready, 1,
            "尾批 1 条无冲突未走 Place（不足阈值，随 Settle 交接）"
        );
        assert_eq!(
            ptx.items.iter().filter(|p| p.conflict).count(),
            3,
            "冲突项全数留置（弹窗数据源）"
        );
        assert_eq!(ptx.items.len(), 4, "尾批 1 无冲突 + 3 冲突 → 统一确认");
    }

    /// 钉子（滑动窗超时，T20-H②）：固定窗在万级批次下误判（分轮回流自然
    /// 间歇 > 剩余窗）——`at` 随任一回流刷新后不再触发；连续零回流超窗才
    /// 强制结算，且仅未回流条目带 probe_incomplete（可见不放行静默，F-12）。
    #[test]
    fn sliding_window_refreshes_on_arrival_and_marks_probe_incomplete() {
        let mut ptx = pend_tx(
            9,
            (0..8)
                .map(|i| pend_item(Some(format!("/dst/s{i}.bin"))))
                .collect(),
        );
        // 分轮回流：每轮回流刷新 at（事件处理器同款），轮间歇超固定窗也不误判。
        for _ in 0..4 {
            let pi = ptx
                .items
                .iter_mut()
                .find(|pi| pi.stat_path.is_some())
                .unwrap();
            pi.stat_path = None;
            ptx.resolved += 1;
            ptx.at = std::time::Instant::now(); // 回流刷新基线
        }
        assert_eq!(
            force_settle_expired(&mut ptx, std::time::Duration::from_secs(60)),
            0,
            "刚回流过（滑动窗重置）不触发兜底"
        );
        // 断流：基线回拨 61s（连续零回流超窗）→ 强制结算，仅未回流 4 条带标记。
        ptx.at = std::time::Instant::now() - std::time::Duration::from_secs(61);
        let n = force_settle_expired(&mut ptx, std::time::Duration::from_secs(60));
        assert_eq!(n, 4, "仅未回流条目计数");
        assert!(ptx.items.iter().all(|pi| pi.stat_path.is_none()));
        assert_eq!(
            ptx.items.iter().filter(|p| p.probe_incomplete).count(),
            4,
            "标记恰落在未回流条目上"
        );
    }

    /// 钉子（弹窗范围行数据源，T20-H；T20-E-R 改真调门控函数）：placed > 0
    /// → 写明「已落位 M 项…只处理剩余 N 项（其中冲突 C 项）」；placed = 0
    /// （无流式前置）→ 不挂范围行。断言走 settle_batch 共用的 scope_note_for，
    /// 不再对拍恒假常量 `(0usize > 0).then(...)`。
    #[test]
    fn scope_note_covers_placed_items_only_when_streaming_placed() {
        assert!(
            scope_note_for(0, 5, 2).is_none(),
            "placed=0 不挂范围行（整批在手，无不可逆前置）"
        );
        let note = scope_note_for(512, 100, 3).expect("placed>0 必挂范围行");
        for frag in ["512", "100", "3"] {
            assert!(
                note.contains(frag),
                "范围行须含 M/N/C 实数：{frag} 缺失于「{note}」"
            );
        }
    }

    /// 钉子（T20-E-R 端到端真调）：逐条回流 → stream_place 逐批入队 →
    /// stream_settle 尾批收尾，队列总数 == 10 且零缺项 / 零重复、零悬挂
    /// pending。既有两枚流式钉只调 stream_step / stream_take 纯逻辑，
    /// 未走真调链（stream_place / stream_settle 测试调用原为零）。
    #[test]
    fn stream_place_then_settle_enqueues_every_item() {
        let mut st = State::default();
        // pend_item 的 NewTask.dest 是固定 /dst/a.bin（只有 stat_path 带序号），
        // 端到端钉要按目标路径查零缺项 → 逐条把 dest / kind.remote 设成唯一值。
        let mut ptx = pend_tx(
            9,
            (0..10)
                .map(|i| {
                    let dest = format!("/dst/e{i:03}.bin");
                    let mut pi = pend_item(Some(dest.clone()));
                    pi.task.dest = dest.clone();
                    pi.task.kind = crate::transfer::Kind::UploadFile {
                        local: format!("/src/e{i:03}.bin").into(),
                        remote: dest,
                    };
                    pi
                })
                .collect(),
        );
        ptx.batch = 4;
        st.pending_tx = Some(ptx);
        let mut places = 0;
        let mut settled = false;
        for _ in 0..10 {
            {
                let ptx = st.pending_tx.as_mut().unwrap();
                let pi = ptx
                    .items
                    .iter_mut()
                    .find(|pi| pi.stat_path.is_some())
                    .unwrap();
                pi.stat_path = None;
                ptx.resolved += 1;
                ptx.ready += 1;
            }
            match stream_step(st.pending_tx.as_ref().unwrap()) {
                StreamStep::Place => {
                    places += 1;
                    stream_place(&mut st);
                }
                StreamStep::Settle => {
                    settled = true;
                    stream_settle(&mut st);
                }
                StreamStep::None => {}
            }
        }
        assert_eq!(places, 2, "阈值 4 × 10 条：4/8 两次 Place");
        assert!(settled, "第 10 条集齐走 stream_settle");
        assert!(st.pending_tx.is_none(), "结算后无悬挂 pending");
        assert!(matches!(st.dialog, dialogs::Dialog::None), "无冲突不出弹窗");
        let q = st.transfer.as_ref().expect("逐批入队");
        assert_eq!(
            q.with_items(|items| items.len()),
            10,
            "落位 8 + 尾批 2 = 10"
        );
        for k in 0..10 {
            let needle = format!("/dst/e{k:03}.bin");
            assert_eq!(
                q.with_items(|items| items
                    .iter()
                    .filter(|i| i.dest.starts_with(needle.as_str()))
                    .count()),
                1,
                "{needle} 恰入队一次（零缺项 / 零重复）"
            );
        }
    }

    /// 钉子（T20-E-R 万级规模）：10000 条 / 单位 256 / 中段冲突 → 落位数
    /// + 剩余入队数 == 10000，零缺项；冲突项全数留置、全数回流完毕。
    /// 纯逻辑对拍 stream_step / stream_take，**不启动真实队列线程**：worker
    /// 会并发领取 Waiting 行，万级下测状态不稳且拖慢门禁。
    #[test]
    fn ten_thousand_stream_batches_keep_every_item() {
        const N: usize = 10_000;
        let mut items: Vec<PendingItem> = (0..N)
            .map(|i| pend_item(Some(format!("/dst/f{i:05}.bin"))))
            .collect();
        // 中段冲突：5000~5002 三条留置，不占 ready、不进 Place。
        for i in [5000usize, 5001, 5002] {
            items[i].conflict = true;
        }
        let mut ptx = pend_tx(9, items);
        ptx.batch = 256;
        let mut placed = 0usize;
        loop {
            let mut newly = 0usize;
            let mut newly_ready = 0usize;
            for pi in ptx.items.iter_mut() {
                if newly == 256 {
                    break;
                }
                if pi.stat_path.take().is_some() {
                    newly += 1;
                    if !pi.conflict {
                        newly_ready += 1;
                    }
                }
            }
            if newly == 0 {
                break;
            }
            ptx.resolved += newly;
            ptx.ready += newly_ready;
            loop {
                match stream_step(&ptx) {
                    StreamStep::Place => {
                        let need = ptx.ready;
                        let batch = stream_take(&mut ptx, need);
                        assert!(!batch.iter().any(|p| p.conflict), "冲突项不进落位批");
                        placed += batch.len();
                    }
                    StreamStep::Settle | StreamStep::None => break,
                }
            }
        }
        let remaining = ptx.items.len();
        assert_eq!(
            placed + remaining,
            N,
            "落位数 {placed} + 剩余入队数 {remaining} == 10000，零缺项"
        );
        assert!(
            placed >= N - 3 - 256,
            "绝大多数按 256 逐批落位，未被卡住（placed={placed}）"
        );
        assert!(
            remaining <= 256 + 3,
            "尾批不超过一个单位 + 冲突留置项（remaining={remaining}）"
        );
        assert_eq!(
            ptx.items.iter().filter(|p| p.conflict).count(),
            3,
            "中段 3 条冲突全数留置到集齐（无提前弹窗）"
        );
        assert_eq!(
            ptx.items.iter().filter(|p| p.stat_path.is_some()).count(),
            0,
            "全数回流完毕，无挂起 STAT"
        );
    }

    /// 钉子（阈值下限兜底）：batch 归一 ≥1——0/负值经 dnd_stream_batch 兜底
    /// 为 1（逐条落位不丢失）；usize::MAX 禁用流式（确认弹窗批次永不 Place）。
    #[test]
    fn stream_batch_threshold_floor_and_disabled() {
        assert_eq!(0i32.max(1) as usize, 1);
        let mut ptx = pend_tx(9, vec![pend_item(Some("/dst/h.bin".into()))]);
        ptx.batch = usize::MAX;
        let pi = ptx
            .items
            .iter_mut()
            .find(|pi| pi.stat_path.is_some())
            .unwrap();
        pi.stat_path = None;
        ptx.resolved += 1;
        ptx.ready += 1;
        // 禁用流式时全量结算仍走 Settle（余量随统一收尾落位，不依赖阈值）。
        assert!(matches!(stream_step(&ptx), StreamStep::Settle));
    }

    /// 钉子（F-12 端到端）：超窗结算走 apply_ssh_events 生产分支——终端落
    /// 「预检超时」对账提示、批次清空直接入队；NewTask 透传位经 into_task
    /// 提取路径为真（消费=透传，resume_decision 禁 Append 的判定层数据源）。
    #[test]
    fn probe_timeout_settle_terminal_notice_and_task_passthrough() {
        // ① 生产结算分支：apply_ssh_events 帧首消费超窗 pending_tx。
        let mut st = State::default();
        let mut ptx = pend_tx(
            9,
            vec![pend_item(None), pend_item(Some("/dst/a.bin".into()))],
        );
        ptx.at = std::time::Instant::now() - std::time::Duration::from_secs(61);
        st.pending_tx = Some(ptx);
        apply_ssh_events(&mut st, &egui::Context::default());
        assert!(st.pending_tx.is_none(), "超窗批结算清空");
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains("预检超时")),
            "终端落对账提示：{:?}",
            st.terminal
        );
        let q = st.transfer.as_ref().expect("放行项直接入队");
        assert_eq!(q.with_items(|items| items.len()), 2, "整批放行入队");
        // ② NewTask 透传位：超时标记项经 into_task 提取后为真（队列行字段
        // transfer 模块私有，此处对拍同一提取路径的纯函数产物）。
        let mut ptx2 = pend_tx(
            9,
            vec![pend_item(None), pend_item(Some("/dst/a.bin".into()))],
        );
        ptx2.at = std::time::Instant::now() - std::time::Duration::from_secs(61);
        force_settle_expired(&mut ptx2, std::time::Duration::from_secs(60));
        let (tasks, _) = settle_tasks(ptx2.items, None);
        assert!(!tasks[0].probe_incomplete, "已回流项不带透传位");
        assert!(tasks[1].probe_incomplete, "超时放行项 NewTask 透传位为真");
    }
}

// ============================================================================
// T20-F1 单测：覆盖确认「应用（逐项勾选，F-10 方案A）/ 取消」与 shell 关闭
// 路径（Esc / 遮罩 / ×）按下必有确定结果——入队或明确放弃整批，不留悬挂
// pending（seq=0 批次超窗兜底不覆盖，残留即 busy 恒真 120ms 重绘）。
// 端到端链全调生产函数（settle_batch / enqueue_pending / settle_tasks /
// cancel_pending_stream / dialogs::shell），不手写重演状态变化。
// ============================================================================

#[cfg(test)]
mod t20f1_tests {
    use super::*;

    /// 批次条目夹具：n_ok 个无冲突（/dst/ok{i}）+ n_conflict 个冲突
    /// （/dst/c{i}），全部已回流（stat_path None，冲突标记就位）——
    /// settle_batch 集齐态直入。
    fn conf_items(n_ok: usize, n_conflict: usize) -> Vec<PendingItem> {
        (0..n_ok + n_conflict)
            .map(|i| {
                let conflict = i >= n_ok;
                let dest = if conflict {
                    format!("/dst/c{i}.bin")
                } else {
                    format!("/dst/ok{i}.bin")
                };
                PendingItem {
                    task: crate::transfer::NewTask {
                        source: format!("/src/f{i}.bin"),
                        dest: dest.clone(),
                        size: 1,
                        force: false,
                        probe_incomplete: false,
                        owner: crate::transfer::TaskOwner {
                            src: (0, 1),
                            dst: (1, 2),
                        },
                        kind: crate::transfer::Kind::UploadFile {
                            local: format!("/src/f{i}.bin").into(),
                            remote: dest,
                        },
                    },
                    stat_path: None,
                    conflict,
                    src_mtime: 0,
                    dst_size: None,
                    dst_mtime: None,
                    probe_incomplete: false,
                }
            })
            .collect()
    }

    /// 钉子（结算映射纯逻辑，F1 要求 2 + F-10 方案A）：勾选 = 冲突任务置
    /// force 入队；不勾 = 剔除冲突项入队其余 + skipped 计数（C# skipped 汇总
    /// {1} 数据源）；混合勾选逐项分流。
    #[test]
    fn settle_tasks_selection_splits_per_item() {
        let (tasks, skipped) = settle_tasks(conf_items(2, 2), Some(&[true, true]));
        assert_eq!(skipped, 0, "全勾不计 skipped");
        assert_eq!(tasks.len(), 4, "全勾 = 全量入队");
        for (i, task) in tasks.iter().enumerate() {
            assert_eq!(
                task.force,
                i >= 2,
                "勾选冲突项 force（C# forceOverwrite），非冲突项不置"
            );
        }
        let (tasks, skipped) = settle_tasks(conf_items(2, 2), Some(&[false, false]));
        assert_eq!(skipped, 2, "全不勾 = 冲突项计数");
        assert_eq!(tasks.len(), 2, "全不勾 = 剔除冲突项后入队其余");
        assert!(tasks.iter().all(|t| !t.force), "非冲突项不带 force");
        // 混合勾选：只勾第 2 个冲突项 → 第 1 个跳过、第 2 个 force 入队。
        let (tasks, skipped) = settle_tasks(conf_items(2, 2), Some(&[false, true]));
        assert_eq!(skipped, 1, "未勾冲突项计入 skipped");
        assert_eq!(tasks.len(), 3, "非冲突 2 项 + 勾选冲突 1 项");
        let forced: Vec<_> = tasks.iter().filter(|t| t.force).collect();
        assert_eq!(forced.len(), 1, "仅勾选的冲突项带 force");
        assert!(
            forced[0].dest.starts_with("/dst/c3"),
            "勾选的是第 2 个冲突项：{:?}",
            forced[0].dest
        );
    }

    /// 钉子（F-10 方案A）：conflict_defaults —— 目标较新默认不勾（覆盖会砸
    /// 掉较新目标），其余默认勾选；目标无 mtime → 勾选。
    #[test]
    fn conflict_defaults_newer_dst_unchecked() {
        let row = |src_mtime: i64, dst_mtime: Option<i64>| dialogs::ConflictRow {
            name: "a.bin".into(),
            src_size: 1,
            src_mtime,
            dst_size: Some(1),
            dst_mtime,
        };
        let rows = vec![
            row(100, Some(200)), // 目标较新 → 不勾
            row(200, Some(100)), // 源较新 → 勾
            row(100, Some(100)), // 相等（差 <1s 视为同批）→ 勾
            row(100, None),      // 无目标 mtime → 勾
        ];
        assert_eq!(
            conflict_defaults(&rows),
            vec![false, true, true, true],
            "目标较新默认不勾，其余默认勾选"
        );
    }

    /// 钉子（端到端「应用」）：远程 STAT 全回流含冲突 → settle_batch 弹
    /// 窗批次暂存（完整 NewTask 在 pending_tx.items，rows 仅展示）→ 勾选态
    /// 默认全勾（夹具无目标 mtime）→ enqueue_pending 全量入队，无悬挂。
    #[test]
    fn conflict_settle_overwrite_enqueues_all() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(3, 2));
        assert!(
            matches!(st.dialog, dialogs::Dialog::TransferConfirm { .. }),
            "冲突批出确认弹窗"
        );
        assert_eq!(
            st.conflict_sel.as_deref(),
            Some([true, true].as_slice()),
            "弹窗创建即填勾选默认值（夹具无目标 mtime → 全勾）"
        );
        let ptx = st.pending_tx.as_ref().expect("弹窗批次暂存");
        assert_eq!(ptx.seq, 0, "弹窗批次 seq 清零（迟到回流落空匹配）");
        assert_eq!(
            ptx.items.len(),
            5,
            "完整任务项保留在 pending_tx（NewTask 数据源）"
        );
        assert_eq!(ptx.items.iter().filter(|p| p.conflict).count(), 2);
        enqueue_pending(&mut st);
        assert!(st.pending_tx.is_none(), "应用后无悬挂 pending");
        assert!(st.conflict_sel.is_none(), "勾选态随应用 take 清空");
        let q = st.transfer.as_ref().expect("应用 = 入队");
        assert_eq!(q.with_items(|items| items.len()), 5, "队列任务数 = 全量");
        assert_eq!(
            q.with_items(|items| items
                .iter()
                .filter(|i| i.dest.starts_with("/dst/c"))
                .count()),
            2,
            "冲突任务照常入队（force 由 settle_tasks 钉子断言）"
        );
    }

    /// 钉子（端到端「全部不勾」）：settle_batch → 勾选态全清 → enqueue_pending
    /// ——只入队非冲突余项，无悬挂。
    #[test]
    fn conflict_settle_skip_enqueues_rest_only() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(3, 2));
        st.conflict_sel = Some(vec![false, false]);
        enqueue_pending(&mut st);
        assert!(st.pending_tx.is_none(), "应用后无悬挂 pending");
        assert!(st.conflict_sel.is_none(), "勾选态随应用 take 清空");
        let q = st.transfer.as_ref().expect("非冲突余项仍入队");
        assert_eq!(
            q.with_items(|items| items.len()),
            3,
            "队列任务数 = 非冲突数"
        );
        assert!(
            q.with_items(|items| items.iter().all(|i| i.dest.starts_with("/dst/ok"))),
            "未勾冲突项被剔除"
        );
    }

    /// 钉子（F1 零结果场景）：全冲突批全不勾 → 零入队但必须有
    /// 确定结果——终端落对账日志（skipped 计数可见），不再零提示零日志。
    #[test]
    fn all_conflict_skip_aborts_with_terminal_log_and_zero_enqueue() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(0, 3));
        st.conflict_sel = Some(vec![false, false, false]);
        enqueue_pending(&mut st);
        assert!(st.pending_tx.is_none());
        assert!(st.transfer.is_none(), "全冲突 + 全不勾 = 零入队");
        assert_eq!(st.terminal.len(), 1, "对账日志可见（skipped 计数不丢）");
        assert!(
            st.terminal[0].1.contains('3'),
            "日志含 skipped 实数：{:?}",
            st.terminal[0].1
        );
    }

    /// 钉子（端到端「取消」）：settle_batch → cancel_pending_stream——
    /// 暂存清空、勾选态清空、队列零新增、终端落对账日志（明确放弃整批）。
    #[test]
    fn cancel_clears_pending_without_enqueue() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(3, 2));
        cancel_pending_stream(&mut st);
        assert!(st.pending_tx.is_none(), "取消后暂存清空");
        assert!(st.conflict_sel.is_none(), "取消后勾选态清空");
        assert!(st.transfer.is_none(), "队列零新增");
        assert_eq!(st.terminal.len(), 1, "明确放弃留对账日志");
        assert!(
            st.terminal[0].1.contains('5'),
            "日志含未入队条数：{:?}",
            st.terminal[0].1
        );
    }

    /// 钉子（本地目标回归）：本地批次（stat_path None）无冲突 → settle_batch
    /// 直接入队收尾（不出弹窗、不留暂存）。
    #[test]
    fn local_settle_without_conflict_enqueues_directly() {
        let mut st = State::default();
        settle_batch(&mut st, 0, 0, conf_items(4, 0));
        assert!(matches!(st.dialog, dialogs::Dialog::None), "无冲突不出弹窗");
        assert!(st.pending_tx.is_none());
        let q = st.transfer.as_ref().expect("无冲突直接入队");
        assert_eq!(q.with_items(|items| items.len()), 4);
    }

    /// 钉子（端到端 shell 关闭路径，F1 要求 2）：确认弹窗 Esc 关闭（遮罩 /
    /// × 与 Esc 共用同一关闭分支）→ 按「取消」结算——暂存清空、busy 判据的
    /// pending_tx 项复位（不再恒真 120ms 重绘）。
    #[test]
    fn shell_close_esc_settles_pending() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(2, 1));
        assert!(st.pending_tx.is_some());
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        let p = crate::theme::palette(&ctx);
        let mut out = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = dialogs::shell(
                    ui,
                    &mut st,
                    &p,
                    640.0,
                    360.0,
                    t("传输确认"),
                    false,
                    0.0,
                    false,
                    None,
                    // C1/F-9：关闭回调收口 shell 层——宿主自声明暂存态处置，
                    // 语义与 transfer_confirm_dialog 内部调用点一致（F1）。
                    dialogs::ShellOpts {
                        on_close: dialogs::CloseAction::CancelPendingStream,
                        ..Default::default()
                    },
                    |_, _, _| {},
                );
                assert!(!r, "Esc 关闭");
            });
        });
        // 无渲染器消费字体纹理增量（epaint Drop debug_assert 会因未应用
        // delta 恐慌）——测试只关心状态链，显式清空。
        out.textures_delta.clear();
        assert!(matches!(st.dialog, dialogs::Dialog::None));
        assert!(
            st.pending_tx.is_none(),
            "关闭路径结算悬挂 pending（busy 判据 pending_tx 项复位）"
        );
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains('3')),
            "关闭 = 明确放弃，终端留对账日志"
        );
    }

    /// 钉子（T20-E-R 英文态假状态）：旧实现硬编码中文前缀，英文
    /// preparing_msg() 结算后永不清除。新判定按当前语言精确比较，中英
    /// 各一钉（语言作参数，不改写全局 language 态 → 并行零串扰）。
    #[test]
    fn preparing_msg_clears_in_both_languages() {
        let zh = "正在准备传输（探测目标/统计文件）…";
        let en = "Preparing transfer (checking target/counting files)…";
        assert_eq!(crate::i18n::t_for(zh, false), zh);
        assert_eq!(crate::i18n::t_for(zh, true), en, "英文键活且逐字一致");
        assert!(is_preparing_msg(zh, false), "中文态识别中文准备提示");
        assert!(!is_preparing_msg(en, false), "中文态不误清英文提示");
        assert!(!is_preparing_msg(zh, true), "英文态不误清中文提示");
        assert!(is_preparing_msg(en, true), "英文态能识别英文准备提示");
        assert!(
            !en.starts_with("正在准备传输"),
            "旧前缀实现在英文态恒不匹配（回归证据）"
        );
        // 真调：当前语言（默认中文）下清准备提示，被错误覆写过的不误清。
        let mut st = State::default();
        pane_view_mut(&mut st, 0).msg = Some(preparing_msg());
        clear_preparing_msg(&mut st, 0);
        assert!(
            pane_view_mut(&mut st, 0).msg.is_none(),
            "准备提示结算后清除"
        );
        pane_view_mut(&mut st, 0).msg = Some("目录读取失败".into());
        clear_preparing_msg(&mut st, 0);
        assert_eq!(
            pane_view_mut(&mut st, 0).msg.as_deref(),
            Some("目录读取失败"),
            "期间被错误覆写则保留"
        );
    }

    /// 钉子（T20-E-R 取消已落位批次）：流式已落位批次号非空时，
    /// cancel_pending_stream 走 q.cancel_batches 回溯；pending 清空、
    /// 勾选态清空、终端留对账日志，且本弹窗零新入队。
    #[test]
    fn cancel_settles_already_placed_batches() {
        let mut st = State::default();
        // 先真入队一批（建立队列 + 首批次号 1），再让确认弹窗挂上该批次。
        let first: Vec<crate::transfer::NewTask> =
            conf_items(2, 0).into_iter().map(into_task).collect();
        enqueue_tasks(&mut st, 1, first, 0);
        let before = st
            .transfer
            .as_ref()
            .unwrap()
            .with_items(|items| items.len());
        assert_eq!(before, 2, "已落位批次在队列里");
        settle_batch(&mut st, 1, 0, conf_items(2, 1));
        if let Some(ptx) = st.pending_tx.as_mut() {
            ptx.batches.push(1);
            ptx.placed = before;
            ptx.resolved = ptx.items.len();
        }
        cancel_pending_stream(&mut st);
        assert!(st.pending_tx.is_none(), "取消后暂存清空");
        assert!(st.conflict_sel.is_none(), "勾选态清空");
        assert_eq!(
            st.transfer
                .as_ref()
                .unwrap()
                .with_items(|items| items.len()),
            before,
            "取消不新增队列行（已落位批次按 cancel_batches 结算，不删行）"
        );
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains('3')),
            "明确放弃留对账日志（未入队条数）"
        );
    }
}

// ── CR-6 内置编辑器保存回传闭环钉子 ─────────────────────────

#[cfg(test)]
mod cr6_tests {
    use super::*;
    use crate::ssh::Ev;

    /// 会话池夹具：sid=1、键 (host, 22, "u")。SSH 句柄离线 spawn（worker 空转；
    /// 任何 write_file 请求都会回一条 Ev::FileWritten——用 poll() 断言「没发」）。
    fn sess(host: &str) -> SideSession {
        SideSession {
            sid: 1,
            key: SessionKey {
                host: host.into(),
                port: 22,
                user: "u".into(),
            },
            ssh: crate::ssh::Ssh::spawn(),
            host: None,
            profile: None,
            state: RemoteState::Off,
            root: None,
            fp_ask: None,
            refs: 1,
        }
    }

    /// 等 worker 回执：无连接下 write_file 必回 FileWritten（Err）——窗口内
    /// 一条都没有 = 请求从未发出（write_file 调用数为 0 的链路级实证）。
    fn has_file_written(ssh: &crate::ssh::Ssh) -> bool {
        for _ in 0..40 {
            if ssh
                .poll()
                .iter()
                .any(|e| matches!(e, Ev::FileWritten { .. }))
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// 钉子（CR-6③ 副本读失败）：不发任何 ssh 请求（STAT 槽不挂 = write_file
    /// 调用数为 0、远端内容不变），doc 置失败态可重试（dirty 回 true、
    /// pending 清零、save_err 记因、副本路径不动）。池内挂会话：读失败先于
    /// 会话匹配短路，无论匹配与否都不发任何 ssh 请求。
    #[test]
    fn saved_upload_read_failure_sends_no_ssh_and_stays_retryable() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        let bad = std::env::temp_dir().join("ifreesftp-cr6-missing-copy.txt");
        let _ = std::fs::remove_file(&bad);
        st.editors
            .open_remote("h1", &bad, "/r.txt".into(), Some(1000), true);
        saved_upload_dispatch(
            &mut st,
            &egui::Context::default(),
            crate::editor::SavedEvent {
                doc_id: 1,
                local_path: bad.clone(),
                remote_path: "/r.txt".into(),
                host: "h1".into(),
                base_mtime: Some(1000),
            },
        );
        assert!(st.doc_stat.is_none(), "读失败不挂 STAT 槽（零 ssh 请求）");
        let d = st.editors.docs[0].lock().unwrap();
        assert!(d.dirty, "失败态 dirty 回 true：再按 Ctrl+S 可重试");
        assert!(!d.pending_upload, "在途态清零");
        assert!(
            d.save_err.as_deref().unwrap_or("").contains("回传未完成"),
            "save_err 记因：{}",
            d.save_err.as_deref().unwrap_or("")
        );
        drop(d);
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains("回传未完成")),
            "终端留失败对账日志"
        );
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "write_file 调用数为 0（远端内容不变）"
        );
    }

    /// 钉子（CR-6③ 会话失配）：drain 后不得吞——doc 失败态回写、副本保留、
    /// 零 ssh 请求（与外部链路「found=None 静默」缺陷对照）。
    #[test]
    fn saved_upload_session_mismatch_keeps_copy_and_marks_retryable() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h9")); // 与 ev.host 不匹配
        let copy = std::env::temp_dir().join("ifreesftp-cr6-mismatch-copy.txt");
        std::fs::write(&copy, b"payload").unwrap();
        st.editors
            .open_remote("h1", &copy, "/r.txt".into(), Some(1000), true);
        saved_upload_dispatch(
            &mut st,
            &egui::Context::default(),
            crate::editor::SavedEvent {
                doc_id: 1,
                local_path: copy.clone(),
                remote_path: "/r.txt".into(),
                host: "h1".into(),
                base_mtime: Some(1000),
            },
        );
        assert!(st.doc_stat.is_none(), "失配不挂 STAT 槽（零 ssh 请求）");
        assert!(copy.exists(), "副本保留");
        let d = st.editors.docs[0].lock().unwrap();
        assert!(d.dirty, "失败态可重试");
        assert!(!d.pending_upload);
        assert!(d.save_err.is_some());
        drop(d);
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "write_file 调用数为 0（远端内容不变）"
        );
    }

    /// 钉子（CR-6④ 内外同判定）：内置与外部共用同款 ±1s 判据——同款
    /// base_mtime 冲突夹具上判定一致（远端存在且差 ≥1s 才算冲突；任一侧
    /// 未报 mtime / 文件不存在不判）。外部分支公式照抄作比对基准。
    #[test]
    fn internal_conflict_verdict_matches_external_predicate() {
        let cases: [(Option<i64>, Option<i64>, bool); 7] = [
            (Some(1000), Some(1000), true),  // 同 mtime：无冲突
            (Some(1000), Some(1001), true),  // 差 1s：冲突
            (Some(1001), Some(1000), true),  // 反向差 1s：冲突
            (Some(1000), Some(1002), true),  // 差 2s：冲突
            (None, Some(1001), true),        // 未报基准：跳过
            (Some(1000), None, true),        // 远端未报 mtime：跳过
            (Some(1000), Some(1001), false), // 文件不存在：不判
        ];
        for (base, remote, exists) in cases {
            // 外部 STAT 分支（ext_edits 路）原样判据。
            let external = match (base, remote) {
                (Some(a), Some(b)) => exists && a.abs_diff(b) >= 1,
                _ => false,
            };
            assert_eq!(
                mtime_conflict(base, remote, exists),
                external,
                "base={base:?} remote={remote:?} exists={exists}"
            );
        }
    }

    /// 钉子（CR-6⑥ 成功回执）：FileWritten::Ok → base mtime 回填（回执值 =
    /// 随后重新 STAT 将看到的 mtime）、dirty/pending 清零、last_receipt 字节数
    /// 与副本实长一致（= 重新 STAT 的 size）、终端日志带三实数。
    #[test]
    fn file_written_ok_receipt_bytes_mtime_match() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        let copy = std::env::temp_dir().join("ifreesftp-cr6-receipt-copy.txt");
        let content = b"hello world"; // 11 字节
        std::fs::write(&copy, content).unwrap();
        st.editors
            .open_remote("h1", &copy, "/r.txt".into(), Some(1000), true);
        st.editors.docs[0].lock().unwrap().pending_upload = true; // save 入队在途态
        apply_ssh_event(
            &mut st,
            &egui::Context::default(),
            0,
            1,
            Ev::FileWritten {
                req: 1,
                path: "/r.txt".into(),
                result: Ok(Some(1720000001)),
            },
        );
        let d = st.editors.docs[0].lock().unwrap();
        assert_eq!(
            d.remote.as_ref().unwrap().base_mtime,
            Some(1720000001),
            "base mtime 回填 = 回执值（重新 STAT 将看到的 mtime）"
        );
        assert!(!d.dirty, "回执后清 dirty");
        assert!(!d.pending_upload, "回执后在途态清零");
        let rct = d.last_receipt.clone().unwrap();
        drop(d);
        assert!(rct.contains("11"), "回执带字节数：{rct}");
        assert!(rct.contains("/r.txt"), "回执带远端路径：{rct}");
        assert_eq!(
            std::fs::metadata(&copy).unwrap().len(),
            11,
            "回执字节数 = 副本实长（= 重新 STAT 的 size）"
        );
        assert!(
            st.terminal
                .iter()
                .any(|(_, m)| m.contains("11") && m.contains("1720000001")),
            "终端日志带字节数/mtime 实数"
        );
    }

    /// 钉子（写锁探测语义，反判回归）：CreateFileW(GENERIC_READ, 仅
    /// FILE_SHARE_READ) 探测——① 无占用 → 已释放；② 他人持写句柄且不授予
    /// 共享读（share_mode = 仅 FILE_SHARE_WRITE）→ 共享冲突 = 锁未释放
    /// → false（此前误返 true，编辑器保存中会放行半写文件上传）；
    /// ③ 句柄释放 → true；④ 文件不存在 → true（不推迟）。
    #[test]
    #[cfg(windows)]
    fn ext_write_lock_released_sharing_violation_waits() {
        let f = std::env::temp_dir().join("ifreesftp-extlock-probe.txt");
        std::fs::write(&f, b"probe").unwrap();
        // ① 无占用：已释放。
        assert!(ext_write_lock_released(&f), "无占用应判已释放");
        // ② 持写句柄（不授予共享读）→ 共享冲突 = 锁未释放，继续等。
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_WRITE: u32 = 0x2;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_WRITE)
            .open(&f)
            .expect("持锁句柄");
        assert!(
            !ext_write_lock_released(&f),
            "共享冲突应判锁未释放（反判回归）"
        );
        // ③ 句柄释放 → 已释放。
        drop(holder);
        assert!(ext_write_lock_released(&f), "释放后应判已释放");
        // ④ 文件不存在 → 已释放（不推迟）。
        assert!(ext_write_lock_released(
            &std::env::temp_dir().join("ifreesftp-extlock-missing.txt")
        ));
        let _ = std::fs::remove_file(&f);
    }

    /// 钉子（写失败回执，项5 收口）：Ev::FileWritten Err → doc 失败态置位
    /// （save_err 记因、dirty 回 true 绝不置 false），在途态清零可重试。
    #[test]
    fn file_written_err_marks_doc_retryable_never_dirty_false() {
        let ctx = egui::Context::default();
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        st.editors.open_remote(
            "h1",
            &std::env::temp_dir().join("ifreesftp-fwerr.txt"),
            "/r.txt".into(),
            Some(1000),
            true,
        );
        {
            let mut d = st.editors.docs[0].lock().unwrap();
            d.pending_upload = true; // save 入队后的在途态
        }
        apply_ssh_event(
            &mut st,
            &ctx,
            0,
            1,
            Ev::FileWritten {
                req: 1,
                path: "/r.txt".into(),
                result: Err("permission denied".into()),
            },
        );
        let d = st.editors.docs[0].lock().unwrap();
        assert!(d.dirty, "写失败 dirty 必须回 true（不得 dirty=false）");
        assert!(!d.pending_upload, "在途态清零：再按 Ctrl+S 可重试");
        assert!(
            d.save_err
                .as_deref()
                .unwrap_or("")
                .contains("permission denied"),
            "save_err 记因：{:?}",
            d.save_err
        );
    }

    /// CR-2 钉子：失败条从底部切出且不覆盖主区；窗口过矮不切（不产生负高度）。
    #[test]
    fn cr2_save_error_bar_geometry_does_not_overlap_panes() {
        let full = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 800.0));
        let (rest, none) = save_error_bar_rect(full, false);
        assert_eq!(rest, full, "无失败态时布局一字不变");
        assert!(none.is_none());
        let (rest, bar) = save_error_bar_rect(full, true);
        let bar = bar.expect("失败态应切出底部条");
        assert_eq!(bar.bottom(), full.bottom());
        assert_eq!(bar.height(), SAVE_BAR_H);
        assert_eq!(rest.bottom(), bar.top(), "主区不得与提示条重叠");
        assert_eq!(rest.top(), full.top());
        let tiny = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 100.0));
        let (rest, none) = save_error_bar_rect(tiny, true);
        assert_eq!(rest, tiny, "过矮窗口不切条");
        assert!(none.is_none());
    }

    /// CR-2 钉子：失败在途走 1s 慢档兜底（不是 120ms 空转），既有快帧优先。
    #[test]
    fn cr2_save_failure_uses_slow_heartbeat_not_fast() {
        let mut st = State::default();
        st.remotes[0].tabs.push(placeholder_remote_tab(1));
        assert_eq!(repaint_delay(&st), None, "无失败态应自停");
        st.save_err_pending = true;
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(1000)),
            "失败在途应慢档兜底"
        );
        st.doc_stat = Some((
            1,
            1,
            "/r.txt".into(),
            std::path::PathBuf::new(),
            vec![],
            None,
        ));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "其它在途工作仍优先快帧"
        );
    }

    /// 钉子（心跳分档四档，项7）：全清 None / 在途 120ms / 仅外部会话 300ms
    /// / 写锁超时兜底 1000ms；在途优先于兜底。
    #[test]
    fn repaint_delay_four_tiers() {
        let mut st = State::default();
        st.remotes[0].tabs.push(placeholder_remote_tab(1));
        // 全清：不请求（自停）。
        assert_eq!(repaint_delay(&st), None, "全清应自停");
        // 在途（doc_stat 槽）→ 120ms。
        st.doc_stat = Some((
            1,
            1,
            "/r.txt".into(),
            std::path::PathBuf::new(),
            vec![],
            None,
        ));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "在途应 120ms"
        );
        st.doc_stat = None;
        // 在途（ext_write 槽）→ 120ms（确认框覆盖后回执不吃慢档）。
        st.remotes[0].tabs[0].ext_write = Some(9);
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "ext_write 在途应 120ms"
        );
        st.remotes[0].tabs[0].ext_write = None;
        // 仅外部编辑会话（防抖/写锁探测）→ 300ms。
        let f = std::env::temp_dir().join("ifreesftp-rd-tier.txt");
        std::fs::write(&f, b"tier").unwrap();
        st.remotes[0].tabs[0].ext_edits.push(ExtEdit {
            host_tag: "[h1]".into(),
            remote_path: "/r.txt".into(),
            name: "r.txt".into(),
            local: f.clone(),
            base_mtime: Some(1000),
            baseline: (5, 1000),
            quiet_since: None,
            quiet_snap: None,
            lock_wait_since: None,
            lock_warned: false,
            lock_dialog_done: false,
        });
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(300)),
            "仅外部会话应 300ms"
        );
        // 写锁等待超 10s（兜底）→ 1000ms。
        st.remotes[0].tabs[0].ext_edits[0].lock_wait_since =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(11000));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(1000)),
            "超时兜底应 1000ms"
        );
        // 在途优先于兜底。
        st.remotes[0].tabs[0].ext_stat = Some((7, "/r.txt".into()));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "在途应优先于兜底"
        );
        let _ = std::fs::remove_file(&f);
    }

    /// 钉子（写锁超时兜底 + 去重，项6，Windows）：持锁 ≥10s —— 终端恰 1 条
    /// WARN（锁 Warned 去重，禁每轮刷屏）、状态栏持久提示、零 ssh 上传、
    /// 心跳 1000ms；锁释放 → 恰 1 条 INFO、基准更新、自动弹确认框；随后
    /// 快照回基线 → 状态栏提示成对清除（独立标记 lock_msg_on）。
    #[test]
    #[cfg(windows)]
    fn lock_wait_timeout_warns_once_recovers_with_single_info() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        st.remotes[0].tabs.push(placeholder_remote_tab(1));
        st.remotes[0].tabs[0].sid = 1;
        let copy = std::env::temp_dir().join("ifreesftp-lockwait-copy.txt");
        std::fs::write(&copy, b"payload").unwrap();
        let host_tag = format!("[{}]", t("远程"));
        let snap0 = ext_snapshot(&copy);
        st.remotes[0].tabs[0].ext_edits.push(ExtEdit {
            host_tag,
            remote_path: "/r.txt".into(),
            name: "r.txt".into(),
            local: copy.clone(),
            base_mtime: Some(1000),
            baseline: (snap0.0 + 1, snap0.1 + 1), // ≠ 当前快照：视为编辑器已保存
            quiet_since: Some(std::time::Instant::now() - std::time::Duration::from_millis(1000)),
            quiet_snap: Some(ext_snapshot(&copy)),
            lock_wait_since: Some(
                std::time::Instant::now() - std::time::Duration::from_millis(11000),
            ), // 已等 11s
            lock_warned: false,
            lock_dialog_done: false,
        });
        // 模拟编辑器持写句柄（不授予共享读 → 探测共享冲突）。
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_WRITE: u32 = 0x2;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_WRITE)
            .open(&copy)
            .expect("持锁句柄");
        let back = |st: &mut State| {
            st.remotes[0].tabs[0].ext_poll =
                std::time::Instant::now() - std::time::Duration::from_millis(400);
        };
        // 超时轮询：WARN 恰 1 条 + 状态栏持久提示 + 零上传 + 心跳 1000ms。
        back(&mut st);
        poll_ext_edits(&mut st);
        let warns = st
            .terminal
            .iter()
            .filter(|(_, m)| m.contains("不会上传半写文件"))
            .count();
        assert_eq!(warns, 1, "超时 WARN 恰 1 条");
        assert!(
            st.remotes[0].tabs[0]
                .pane
                .msg
                .as_deref()
                .unwrap_or("")
                .contains("写锁未释放"),
            "状态栏持久提示：{:?}",
            st.remotes[0].tabs[0].pane.msg
        );
        assert!(st.remotes[0].tabs[0].lock_msg_on, "独立清除标记置位");
        assert!(
            matches!(
                st.dialog,
                dialogs::Dialog::ExtEditLockWait { ref name, .. } if name == "r.txt"
            ),
            "超时应弹退出对话框"
        );
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "超时兜底绝不放行上传（零 write_file）"
        );
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(1000)),
            "兜底期间心跳 1000ms"
        );
        // 再轮询一轮：WARN 不再重复（lock_warned 去重）。
        back(&mut st);
        poll_ext_edits(&mut st);
        assert_eq!(
            st.terminal
                .iter()
                .filter(|(_, m)| m.contains("不会上传半写文件"))
                .count(),
            1,
            "WARN 去重：不得每轮刷屏"
        );
        // 锁释放：恰 1 条 INFO + 等待态清零（弹窗占用时基准不动）。
        drop(holder);
        back(&mut st);
        poll_ext_edits(&mut st);
        let infos = st
            .terminal
            .iter()
            .filter(|(_, m)| m.contains("写锁已释放，继续回传"))
            .count();
        assert_eq!(infos, 1, "锁释放 INFO 恰 1 条");
        assert!(st.remotes[0].tabs[0].ext_edits[0].lock_wait_since.is_none());
        // 关掉兜底对话框：触发已重置静默期（quiet_since=now），回拨满足
        // QUIET_MS 后下轮正常弹确认框（基准更新）。
        st.dialog = dialogs::Dialog::None;
        st.remotes[0].tabs[0].ext_edits[0].quiet_since =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(400));
        back(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditChanged { .. }),
            "锁释放后自动弹确认框"
        );
        // 快照回基线：状态栏提示成对清除（独立标记归零，无英文残留）。
        back(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            st.remotes[0].tabs[0].pane.msg.is_none() && !st.remotes[0].tabs[0].lock_msg_on,
            "回基线后提示清除：{:?}",
            st.remotes[0].tabs[0].pane.msg
        );
        let _ = std::fs::remove_file(&copy);
    }

    /// CR-10 夹具：与 lock_wait_timeout_warns_once_recovers_with_single_info
    /// 同款——持锁 ≥10s 的外部编辑会话（baseline ≠ 快照、静默期已满、等待已
    /// 11s）。返回 (State, 持锁句柄, 副本路径)；句柄存活即写锁未释放。
    /// 副本名 = 进程 id + 进程内计数器：并行档三钉各占一文件互不踩
    ///（共用固定名会在默认并行档撞出 os error 5 / error 32）。
    #[cfg(windows)]
    fn lockwait_fixture() -> (State, std::fs::File, std::path::PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        st.remotes[0].tabs.push(placeholder_remote_tab(1));
        st.remotes[0].tabs[0].sid = 1;
        let copy = std::env::temp_dir().join(format!(
            "ifreesftp-lockwait-dialog-copy-{}-{seq}.txt",
            std::process::id()
        ));
        std::fs::write(&copy, b"payload").unwrap();
        let host_tag = format!("[{}]", t("远程"));
        let snap0 = ext_snapshot(&copy);
        st.remotes[0].tabs[0].ext_edits.push(ExtEdit {
            host_tag,
            remote_path: "/r.txt".into(),
            name: "r.txt".into(),
            local: copy.clone(),
            base_mtime: Some(1000),
            baseline: (snap0.0 + 1, snap0.1 + 1), // ≠ 当前快照：视为编辑器已保存
            quiet_since: Some(std::time::Instant::now() - std::time::Duration::from_millis(1000)),
            quiet_snap: Some(ext_snapshot(&copy)),
            lock_wait_since: Some(
                std::time::Instant::now() - std::time::Duration::from_millis(11000),
            ), // 已等 11s
            lock_warned: false,
            lock_dialog_done: false,
        });
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_WRITE: u32 = 0x2;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_WRITE)
            .open(&copy)
            .expect("持锁句柄");
        (st, holder, copy)
    }

    /// 轮询回拨（tab.ext_poll 过期，绕过 POLL_MS 节流）。
    #[cfg(windows)]
    fn back400(st: &mut State) {
        st.remotes[0].tabs[0].ext_poll =
            std::time::Instant::now() - std::time::Duration::from_millis(400);
    }

    /// 钉子（CR-10）：超时框被 Esc / × / 遮罩关闭（shell 公共出口 = 只关弹窗，
    /// 等待态不动）后不再重现；等待计时、WARN 去重、状态栏持久提示、零上传
    /// 全部照旧。
    #[test]
    #[cfg(windows)]
    fn lock_wait_dialog_dismiss_does_not_reappear() {
        let (mut st, holder, copy) = lockwait_fixture();
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }),
            "首次超时弹框"
        );
        // 模拟 shell 公共关闭出口：st.dialog = None，等待态不动。
        st.dialog = dialogs::Dialog::None;
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(matches!(st.dialog, dialogs::Dialog::None), "关闭后不再重现");
        let e = &st.remotes[0].tabs[0].ext_edits[0];
        assert!(e.lock_wait_since.is_some(), "等待态保留（继续计时）");
        assert!(e.lock_warned, "WARN 去重标记保持");
        assert!(e.lock_dialog_done, "弹框去重标记已置位（本次等锁不再弹）");
        assert!(
            st.remotes[0].tabs[0]
                .pane
                .msg
                .as_deref()
                .unwrap_or("")
                .contains("写锁未释放"),
            "状态栏持久提示仍在：{:?}",
            st.remotes[0].tabs[0].pane.msg
        );
        assert_eq!(
            st.terminal
                .iter()
                .filter(|(_, m)| m.contains("不会上传半写文件"))
                .count(),
            1,
            "WARN 恰 1 条（日志去重不变）"
        );
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "锁未释放期间零 write_file"
        );
        drop(holder);
        let _ = std::fs::remove_file(&copy);
    }

    /// 钉子（CR-10）：首次超时恰逢其它弹窗占用 → 本轮不弹、标记不置位；
    /// 占用解除后下一轮补弹（证明弹框去重必须独立于 lock_warned）。
    #[test]
    #[cfg(windows)]
    fn lock_wait_dialog_defers_when_dialog_occupied() {
        let (mut st, holder, copy) = lockwait_fixture();
        st.dialog = dialogs::Dialog::SyncMismatch { msg: "busy".into() };
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::SyncMismatch { .. }),
            "占用框不被顶掉"
        );
        assert!(
            !st.remotes[0].tabs[0].ext_edits[0].lock_dialog_done,
            "占用轮不置位（否则永久不弹）"
        );
        assert!(
            st.remotes[0].tabs[0].ext_edits[0].lock_warned,
            "WARN 照常打（日志去重与弹框去重分离）"
        );
        // 占用解除 → 下一轮补弹。
        st.dialog = dialogs::Dialog::None;
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }),
            "占用解除后补弹"
        );
        drop(holder);
        let _ = std::fs::remove_file(&copy);
    }

    /// 钉子（CR-10）：等待态复位（「立即重试探测」按钮语义）后重新造一次
    /// 超时 → 超时框能再弹（lock_dialog_done 复位链有效）。
    #[test]
    #[cfg(windows)]
    fn lock_wait_dialog_next_wait_can_reopen() {
        let (mut st, holder, copy) = lockwait_fixture();
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }));
        st.dialog = dialogs::Dialog::None;
        // 「立即重试探测」复位等待态（与 ext_edit_lock_wait_body retry 分支同字段）。
        let e0 = &mut st.remotes[0].tabs[0].ext_edits[0];
        e0.quiet_since = None;
        e0.quiet_snap = None;
        e0.lock_wait_since = None;
        e0.lock_warned = false;
        e0.lock_dialog_done = false;
        st.remotes[0].tabs[0].lock_msg_on = false;
        // 重新造一次超时：静默满 + 等待 ≥TIMEOUT。
        let e1 = &mut st.remotes[0].tabs[0].ext_edits[0];
        e1.quiet_since = Some(std::time::Instant::now() - std::time::Duration::from_millis(1000));
        e1.quiet_snap = Some(ext_snapshot(&e1.local));
        e1.lock_wait_since =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(11000));
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }),
            "复位后下一次等锁超时可再弹"
        );
        drop(holder);
        let _ = std::fs::remove_file(&copy);
    }
}

// ============================================================================
// F-8 光标意图裁决钉子（纯 egui::Context / run_ui；不启 GUI、不连远程主机、
// 不落临时文件——并行档各用例独占 Context，无共享路径）
// ============================================================================
#[cfg(test)]
mod cursor_tests {
    use super::*;

    /// 造两帧真实 Ui：egui 0.37 的 hover 命中基于「上一帧注册的 WidgetRect」，
    /// 第一帧只注册命中区，第二帧才有 hovered 并可断言（check=true）。
    fn frame(pointer: Option<egui::Pos2>, f: impl Fn(&mut egui::Ui, bool)) {
        let ctx = egui::Context::default();
        for pass in 0..2 {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            ));
            if let Some(pos) = pointer {
                input.events.push(egui::Event::PointerMoved(pos));
            }
            let check = pass == 1;
            let mut out = ctx.run_ui(input, |ui| f(ui, check));
            // egui 0.37：丢弃 FullOutput 前须消费纹理增量，否则 drop 会 panic。
            out.textures_delta.clear();
        }
    }

    fn icon(ui: &egui::Ui) -> egui::CursorIcon {
        ui.ctx().output(|o| o.cursor_icon)
    }

    /// 只在第二帧（命中区已注册）比对。
    fn expect(check: bool, ui: &egui::Ui, want: egui::CursorIcon, msg: &str) {
        if check {
            assert_eq!(icon(ui), want, "{}", msg);
        }
    }

    /// 逐层累加 + 四层正序 / 逆序 + 同层先写者：裁决结果恒为最高优先层。
    #[test]
    fn cursor_decide_priority() {
        use egui::CursorIcon as I;
        frame(None, |ui, check| {
            let ctx = ui.ctx();
            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_decide(ctx);
            expect(check, ui, I::PointingHand, "仅 Hover");

            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_intent(ctx, CursorLayer::Edge, I::ResizeEast);
            cursor_decide(ctx);
            expect(check, ui, I::ResizeEast, "Edge 压过 Hover");

            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_intent(ctx, CursorLayer::Edge, I::ResizeEast);
            cursor_intent(ctx, CursorLayer::Drag, I::Grabbing);
            cursor_decide(ctx);
            expect(check, ui, I::Grabbing, "Drag 压过 Edge / Hover");

            for (layer, ic) in [
                (CursorLayer::Hover, I::PointingHand),
                (CursorLayer::Edge, I::ResizeEast),
                (CursorLayer::Drag, I::Grabbing),
                (CursorLayer::Dialog, I::Default),
            ] {
                cursor_intent(ctx, layer, ic);
            }
            cursor_decide(ctx);
            expect(check, ui, I::Default, "四层正序：Dialog 最高");

            for (layer, ic) in [
                (CursorLayer::Dialog, I::Default),
                (CursorLayer::Drag, I::Grabbing),
                (CursorLayer::Edge, I::ResizeEast),
                (CursorLayer::Hover, I::PointingHand),
            ] {
                cursor_intent(ctx, layer, ic);
            }
            cursor_decide(ctx);
            expect(check, ui, I::Default, "四层逆序：低层不得反超");

            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_intent(ctx, CursorLayer::Hover, I::Grabbing);
            cursor_decide(ctx);
            expect(check, ui, I::PointingHand, "同层保留先写者");
        });
    }

    /// 终局兜底：意图槽 take 语义，无意图帧裁决后 Default，不粘滞 Grabbing。
    #[test]
    fn cursor_decide_empty_defaults() {
        frame(None, |ui, check| {
            let ctx = ui.ctx();
            cursor_intent(ctx, CursorLayer::Drag, egui::CursorIcon::Grabbing);
            cursor_decide(ctx);
            expect(check, ui, egui::CursorIcon::Grabbing, "有 Drag 意图");
            // released / pointer gone / dialog 切换 → 本帧不再上报 Drag。
            cursor_decide(ctx);
            expect(
                check,
                ui,
                egui::CursorIcon::Default,
                "无意图帧不得残留 Grabbing",
            );
            cursor_decide(ctx);
            expect(check, ui, egui::CursorIcon::Default, "连续空帧仍为 Default");
        });
    }

    /// modal=true 整层跳过（零 Edge 意图）；modal=false 贴边 hover 时 Edge 生效。
    #[test]
    fn edge_resize_modal_skips() {
        let top_edge = egui::Pos2::new(400.0, 2.0);
        frame(Some(top_edge), |ui, check| {
            edge_resize(ui, true);
            cursor_decide(ui.ctx());
            expect(
                check,
                ui,
                egui::CursorIcon::Default,
                "弹层期间贴边也不得写 Edge",
            );
        });
        frame(Some(top_edge), |ui, check| {
            edge_resize(ui, false);
            cursor_decide(ui.ctx());
            expect(
                check,
                ui,
                egui::CursorIcon::ResizeNorth,
                "非弹层贴边应上报 Edge",
            );
        });
    }

    /// 自绘按钮 hover 手型；禁用态不上报（std_button 系全委托 std_button_en）。
    #[test]
    fn std_button_hover_points() {
        let rect = egui::Rect::from_min_size(egui::Pos2::new(10.0, 10.0), egui::vec2(80.0, 28.0));
        let hover = egui::Pos2::new(50.0, 20.0);
        frame(Some(hover), |ui, check| {
            let p = theme::palette(ui.ctx());
            let _ = dialogs::std_button_en(
                ui,
                rect,
                "OK",
                egui::Color32::WHITE,
                &p,
                egui::FontId::default(),
                true,
            );
            cursor_decide(ui.ctx());
            expect(
                check,
                ui,
                egui::CursorIcon::PointingHand,
                "enabled + hover 应手型",
            );
        });
        frame(Some(hover), |ui, check| {
            let p = theme::palette(ui.ctx());
            let _ = dialogs::std_button_en(
                ui,
                rect,
                "OK",
                egui::Color32::WHITE,
                &p,
                egui::FontId::default(),
                false,
            );
            cursor_decide(ui.ctx());
            expect(check, ui, egui::CursorIcon::Default, "disabled 不得报手型");
        });
    }

    /// 合法放置不再上报 Copy 而是 Default；DropHint 绘制一行不动。
    #[test]
    fn panes_copy_is_default_no_drop_hint_change() {
        frame(None, |ui, check| {
            cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::Default);
            cursor_decide(ui.ctx());
            expect(check, ui, egui::CursorIcon::Default, "合法放置为默认箭头");
        });
        assert_ne!(egui::CursorIcon::Copy, egui::CursorIcon::Default);
        let src = include_str!("panes.rs");
        assert!(
            src.contains("cursor_intent(ui.ctx(), CursorLayer::Hover, CursorIcon::Default)"),
            "file_rows 合法放置分支须上报 Default 而非 Copy"
        );
        assert!(
            src.contains("p.accent.gamma_multiply(0.08)"),
            "DropHint 高亮填充不得随光标改动删除"
        );
    }
}

#[cfg(test)]
mod cr1_jobs_tests {
    use super::*;
    use std::path::PathBuf;

    fn req_delete() -> crate::local::LocalPaneReq {
        crate::local::LocalPaneReq::Delete {
            dir: PathBuf::from("C:/cr1"),
            targets: vec!["a.txt".to_owned()],
            keep: Vec::new(),
        }
    }

    /// 钉子④：删除中 tab 被关 → 回执不落到别的 tab，删除成败仍留终端痕迹。
    #[test]
    fn delete_reply_after_tab_close_keeps_log_and_drops_pane_update() {
        let mut st = State::default();
        let req = req_delete();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(crate::local::LocalPaneReply {
            req: req.clone(),
            entries: None,
            deleted: vec![("C:/cr1/a.txt".to_owned(), Ok(()))],
        })
        .unwrap();
        st.local_jobs.push(LocalJob {
            side: 0,
            tab_id: 999_999,
            req,
            rx,
        });
        assert!(poll_local_jobs(&mut st), "回执被消费");
        assert!(st.local_jobs.is_empty(), "在途槽清空");
        assert_eq!(st.terminal.len(), 1, "tab 已关仍留删除日志");
        assert!(st.terminal[0].1.contains("C:/cr1/a.txt"));
        assert!(!st.terminal[0].1.contains("已删除 1 项"));
    }

    /// 钉子⑤：工作线程异常退出（channel 断开）→ 清 loading，不留「加载中…」死态。
    #[test]
    fn worker_disconnect_clears_loading_and_reports_failure() {
        let mut st = State::default();
        let tab_id = st.locals[0].tabs[0].id;
        // 真实请求以 pane.path 为枚举目标；夹具须对齐，否则回执按过期丢弃。
        st.locals[0].tabs[0].pane.path = Some(PathBuf::from("C:/cr1"));
        st.locals[0].tabs[0].pane.loading = true;
        let (tx, rx) = std::sync::mpsc::channel::<crate::local::LocalPaneReply>();
        drop(tx);
        let req = crate::local::LocalPaneReq::Refresh {
            path: PathBuf::from("C:/cr1"),
            keep: Vec::new(),
        };
        st.local_jobs.push(LocalJob {
            side: 0,
            tab_id,
            req,
            rx,
        });
        assert!(poll_local_jobs(&mut st));
        assert!(!st.locals[0].tabs[0].pane.loading, "断开也须清 loading");
        assert!(
            st.locals[0].tabs[0].pane.msg.is_some(),
            "失败须显式提示，不静默"
        );
    }

    /// 钉子⑥：本地 I/O 在途要保活，否则 egui 空闲不重绘、回执永远不到账。
    #[test]
    fn local_io_in_flight_keeps_heartbeat_busy() {
        let mut st = State::default();
        assert_eq!(repaint_delay(&st), None, "全清应自停");
        st.locals[0].tabs[0].pane.pending_local = Some(crate::local::LocalPaneReq::Refresh {
            path: PathBuf::from("C:/cr1"),
            keep: Vec::new(),
        });
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "登记待起线程即进 120ms 快帧"
        );
    }
}

// ============================================================================
// CR-7：单批删除回执归零 + 终端批末裁剪（纯状态夹具，无需远程主机 / GUI）。
// ============================================================================
#[cfg(test)]
mod cr7_terminal_tests {
    use super::*;
    use crate::ssh::Ev;

    /// 200 项一个 tag：一条 Ev::Deleted 就把 del_pending 归零并触发重列，
    /// 不会卡在「正在删除…」（按项计数的旧语义会需要 200 条回执）。
    #[test]
    fn single_deleted_reply_zeroes_pending_and_relists() {
        let mut st = State::default();
        let tab_id = st.remotes[1].tabs[0].id;
        st.remotes[1].tabs[0].pane.remote_path = Some("/r".to_owned());
        st.remotes[1].tabs[0].pane.loading = true;
        st.remotes[1].tabs[0].del_pending = 1;
        let tag = next_seq(&mut st);
        st.inflight.insert(tag, (1, tab_id));
        apply_ssh_event(
            &mut st,
            &egui::Context::default(),
            1,
            1,
            Ev::Deleted { tag },
        );
        assert_eq!(st.remotes[1].tabs[0].del_pending, 0, "批计数归零");
        assert!(st.inflight.is_empty(), "在途 tag 清空");
        assert!(!st.remotes[1].tabs[0].pane.loading, "busy 链解除");
        assert_eq!(st.list_reqs.len(), 1, "回零触发重列");
    }

    /// 批末一次裁剪即收敛：1000 行 → 保留末 400 行，再裁长度不变。
    #[test]
    fn trim_terminal_keeps_last_400_in_one_pass() {
        let mut terminal: Vec<(String, String)> = (0..1000)
            .map(|i| (i.to_string(), format!("line-{i}")))
            .collect();
        trim_terminal(&mut terminal);
        assert_eq!(terminal.len(), 400);
        assert_eq!(terminal[0].1, "line-600");
        assert_eq!(terminal[399].1, "line-999");
        trim_terminal(&mut terminal);
        assert_eq!(terminal.len(), 400, "一次裁剪已收敛");
    }
}
