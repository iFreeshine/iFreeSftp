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
    pub items: Vec<PendingItem>,
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
    }
}

/// 主窗口 UI 状态。
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
    /// 拖拽传输请求（FV-25：目标侧, 被拖条目行号；窗格层 Drop 置位）。
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
    /// 打开的编辑器文档 + 保存回传事件队列。
    pub editors: crate::editor::EditorHub,
    /// 主机指纹确认请求——HostPanel「测试连接」专用槽（无侧归属；窗格会话的
    /// 指纹请求在各会话 SideSession.fp_ask，覆盖层按 会话池 → 测试槽 顺序查找）。
    pub fp_ask_test: Option<dialogs::FpAsk>,
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

/// 本地 tab 标题（P4 §6 现算）：当前目录名；根视图/驱动器列表 = 此电脑
///（盘根 file_name 为 None 同口径，C# 同以根列表页标题显示）。
fn local_tab_title(p: &panes::FilePane) -> String {
    p.path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| t("此电脑").to_owned())
}

/// 远程目录末段（P4 §6 现算）："/" → "/"；"/home/user" → "user"；空态 None → ""。
fn remote_dir_label(rp: Option<&str>) -> String {
    match rp {
        None => String::new(),
        // 尾斜杠（理论上不会出现）跳过空段取真末段；全空段（根）→ "/"。
        Some(p) => p
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("/")
            .to_owned(),
    }
}

/// 侧 tab 条合成视图：本地 tab 在前（「本地」固定第一，对齐现状）+ 远程 tab 在
/// 后；active 下标 = 活动项在扁平列表中的位置；born 由 tab id 反查下标。绘制层
/// （tab_strip/folder）零改动——每帧合成临时 Tabs 传入，回写经 id 映射。
fn build_tabs_view(st: &State, side: usize) -> panes::Tabs {
    let mut items = Vec::new();
    for lt in &st.locals[side].tabs {
        // P4 §6：本地 tab 标题现算（当前目录名；根视图/驱动器列表 = 此电脑），
        // 避免漏更新——导航落位即换标题，无需各写点回写。
        items.push(panes::TabItem {
            id: lt.id,
            title: local_tab_title(&lt.pane),
            kind: panes::TabKind::Local,
        });
    }
    for rt in &st.remotes[side].tabs {
        // P4 §6：远程 tab 标题 = {label} {目录末段}；label 存 tab.title（连接
        // 落位时写），目录末段由 pane.remote_path 现算；空态只显 label。
        let dir = remote_dir_label(rt.pane.remote_path.as_deref());
        items.push(panes::TabItem {
            id: rt.id,
            title: if dir.is_empty() {
                rt.title.clone()
            } else {
                format!("{} {dir}", rt.title)
            },
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
            ctx_menu: ctx_menu::CtxMenu::default(),
            crumb_edit: None,
            // 设置弹窗初值来自 settings.json（C# AppSettingsService 同款）。
            settings: dialogs::Settings::from_store(),
            list_reqs: Vec::new(),
            new_reqs: Vec::new(),
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
            stat_seq: 0,
            inflight: std::collections::HashMap::new(),
            q_owner_tabs: Vec::new(),
            q_active: false,
            tab_close_wait: None,
            chk_local: None,
            chk_pending: None,
            chk_computing: false,
            chk_rx: None,
            editors: crate::editor::EditorHub::new(),
            fp_ask_test: None,
        }
    }
}

/// 每帧入口。
pub fn root(ui: &mut egui::Ui, st: &mut State) {
    // 光标每帧复位（egui 0.37 光标跨帧粘滞：output.rs take() 保留旧值，框架不兜底；
    // 分隔条/列宽 thumb 等「hover 才设形」的光标在指针离开后无人覆盖，会卡在
    // resize 形态带到其他部件上。C# Cursor 为元素属性、离开自动恢复——对齐之：
    // 帧首先归默认箭头，随后各 hover 部件的 set_cursor_icon 顺序覆盖）。
    ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    let p = theme::palette(ui.ctx());

    // 设置防抖落盘（500ms；未到期预约重绘）。
    crate::settings::maybe_flush(ui.ctx());
    // 设置落盘失败显式上报（H4：take 语义，一次失败只报一条，不每帧刷屏）。
    if let Some(e) = crate::settings::take_save_error() {
        st.terminal.push((crate::local::now_hms(), e));
    }

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

    // 传输队列：worker 日志入终端面板；忙→静沿刷新两侧（传输结果立即可见）。
    if let Some(q) = &st.transfer {
        for line in q.drain_log() {
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
        } else {
            // 忙期间每帧重算归属集合（新入队任务即时纳入定向刷新范围）。
            st.q_owner_tabs = owners;
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

    // 编辑器保存回传（T7）：远程文档写回远端；本地文档写盘已在编辑器内完成。
    // 左右可连同一主机（两份独立会话）：按 doc_id 反查文档 RemoteRef.host →
    // 遍历两侧会话池匹配持有该主机的会话（P1 首个命中侧，行为同现状），写回
    // 走该会话。
    for ev in crate::editor::EditorHub::drain_saved() {
        st.terminal.push((
            crate::local::now_hms(),
            tfmt("[{0}] 上传编辑副本…", &[&ev.remote_path]),
        ));
        let host = st.editors.docs.iter().find_map(|doc| {
            let d = doc.lock().unwrap();
            (d.id == ev.doc_id)
                .then(|| d.remote.as_ref().map(|r| r.host.clone()))
                .flatten()
        });
        let mut found: Option<(usize, u64)> = None;
        if let Some(h) = host {
            'outer: for (side, rs) in st.remotes.iter().enumerate() {
                for s in &rs.sessions {
                    if s.host.as_ref().is_some_and(|x| x.host == h) {
                        found = Some((side, s.sid));
                        break 'outer;
                    }
                }
            }
        }
        if let Some((side, sid)) = found
            && let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid)
        {
            ss.ssh.write_file(
                ev.remote_path.clone(),
                ev.doc_id,
                std::fs::read(&ev.local_path).unwrap_or_default(),
            );
        }
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

    // 拖拽 Drop（FV-25）：目标侧 + 被拖条目 → 批量传输入队。
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
    // 弹窗与右键菜单最后画（最顶层）。
    dialogs::draw(ui, st, &p);

    // 有在途异步（任侧连接中 / 等枚举 / 连接对话框等待 / 传输进行中 / 冲突确认中）
    // 时保持重绘，及时收到工作线程事件（egui 无事件不重绘）。
    let busy = !st.list_reqs.is_empty()
        || st.tree_req.is_some()
        || st.pending_tx.is_some()
        || st.chk_pending.is_some()
        || st.chk_computing
        || st.remotes.iter().any(|rs| {
            rs.sessions
                .iter()
                .any(|s| matches!(s.state, RemoteState::Connecting) || s.fp_ask.is_some())
                || rs.tabs.iter().any(|t| {
                    t.wait.is_some()
                        || t.del_pending > 0
                        || t.new_pending > 0
                        || t.pending_ed.is_some()
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
    if busy {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }
}

/// 消费 SSH 工作线程事件（连接成功/失败、目录枚举结果）：按侧遍历会话池逐会话
/// 轮询，事件按 sid 路由——会话级事件（连接/指纹/日志）落会话及其挂靠 tab，数据
/// 级事件（枚举/删除/新建/下载/写回/STAT）落该会话挂靠 tab（P1 每 tab 恰一会话；
/// req 全局唯一，Stat/Exec 回流仍按 req 匹配）。
fn apply_ssh_events(st: &mut State, ctx: &egui::Context) {
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
        }
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
            st.terminal.push((crate::local::now_hms(), msg));
            if st.terminal.len() > 400 {
                st.terminal.drain(0..st.terminal.len() - 400);
            }
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
                // 远程编辑回传完成：mtime 回填 base（避免自冲突）+ 终端日志。
                match result {
                    Ok(mtime) => {
                        for doc in &st.editors.docs {
                            let mut d = doc.lock().unwrap();
                            if d.id == req
                                && let Some(r) = &mut d.remote
                            {
                                r.base_mtime = mtime;
                            }
                        }
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt("{0} 已上传编辑副本 {1}", &[&tag, &path]),
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
                        };
                    } else {
                        ext_upload(st, side, tab_id, &path);
                    }
                }
            }
            if !ext_handled {
                // 冲突探测回流：批次全部确认 → 弹覆盖确认或直接入队。
                let mut done: Option<(usize, Vec<PendingItem>)> = None;
                if let Some(ptx) = &mut st.pending_tx
                    && ptx.seq == req
                {
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
                    }
                    if ptx.items.iter().all(|pi| pi.stat_path.is_none()) {
                        done = Some((ptx.profile_side, std::mem::take(&mut ptx.items)));
                    }
                }
                if let Some((profile_side, items)) = done {
                    st.pending_tx = None;
                    if items.iter().any(|pi| pi.conflict) {
                        st.dialog = dialogs::Dialog::TransferConfirm {
                            rows: confirm_data(&items),
                        };
                    } else {
                        enqueue_tasks(
                            st,
                            profile_side,
                            items.into_iter().map(|pi| pi.task).collect(),
                        );
                    }
                }
            }
        }
    }
}

// ── 外部编辑器远程会话（对齐 C# ExternalEditSession 全套）──────

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
        unsafe { GetLastError() == ERROR_SHARING_VIOLATION }
    } else {
        unsafe { CloseHandle(h) };
        true
    }
}

#[cfg(not(windows))]
fn ext_write_lock_released(_path: &std::path::Path) -> bool {
    true
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
/// - 快照 ≠ 基准 → 起算 800ms 静默期（C# DebounceMs；期内再变重新计时）；
/// - 静默期满且快照稳定且写锁已释放 → 触发一次询问（触发即更新基准：
///   同一次保存只问一次，「暂不保存」后再保存会再问）；
/// - 弹窗占用时不触发也不动基准（下轮再问），防弹窗套弹窗。
fn poll_ext_edits(st: &mut State) {
    const QUIET_MS: u128 = 800; // C# ExternalEditSession.DebounceMs
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
                    // 快照稳定但写锁未释放（编辑器仍在保存）→ 继续等待。
                    Some(_) if e.quiet_snap == Some(snap) => {}
                    // 首次变化 / 期内又变 → 重新起算静默期。
                    _ => {
                        e.quiet_since = Some(now);
                        e.quiet_snap = Some(snap);
                    }
                }
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
        })
        .collect();
    if items[0].stat_path.is_some() {
        // 远程目标：整批共用一个请求号逐条异步 STAT（发往目标侧活动远程 tab
        // 的会话）；Ev::Stat 回流按 profile_side 匹配会话聚合。
        st.stat_seq += 1;
        let seq = st.stat_seq;
        if let Some(ss) = active_remote_session(st, profile_side) {
            for pi in &items {
                if let Some(path) = &pi.stat_path {
                    ss.ssh.stat(path.clone(), seq);
                }
            }
        }
        st.pending_tx = Some(PendingTx {
            seq,
            profile_side,
            items,
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
            st.pending_tx = Some(PendingTx {
                seq: 0,
                profile_side,
                items,
            });
            st.dialog = dialogs::Dialog::TransferConfirm { rows };
        } else {
            enqueue_tasks(
                st,
                profile_side,
                items.into_iter().map(|pi| pi.task).collect(),
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

/// 覆盖确认结算：skip=true 丢弃冲突条目；冲突任务置 force（C# forceOverwrite：
/// 用户明确覆盖 → 跳过续传决策直接全量，防旧小文件被误判为中断半成品）。
pub(crate) fn enqueue_pending(st: &mut State, skip_conflicts: bool) {
    let Some(ptx) = st.pending_tx.take() else {
        return;
    };
    let profile_side = ptx.profile_side;
    let tasks: Vec<crate::transfer::NewTask> = ptx
        .items
        .into_iter()
        .filter(|pi| !pi.conflict || !skip_conflicts)
        .map(|mut pi| {
            if pi.conflict {
                pi.task.force = true;
            }
            pi.task
        })
        .collect();
    if !tasks.is_empty() {
        enqueue_tasks(st, profile_side, tasks);
    }
}

/// 入队（惰性建队列线程；远程任务按目标侧活动远程 tab 会话的连接档案建传输
/// 连接）。
fn enqueue_tasks(st: &mut State, profile_side: usize, tasks: Vec<crate::transfer::NewTask>) {
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
    q.enqueue(profile, probe, tasks);
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
        ui.ctx().set_cursor_icon(if vertical {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        });
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
// P4 单测：SessionKey 复用匹配（P2 会话池真复用的判定键，PartialEq 口径）+
// tab 标题现算纯函数（§6：本地目录名 / 远程目录末段）。
// ============================================================================

#[cfg(test)]
mod p4_tests {
    use super::{SessionKey, remote_dir_label};

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

    #[test]
    fn remote_dir_label_last_segment() {
        assert_eq!(remote_dir_label(Some("/")), "/");
        assert_eq!(remote_dir_label(Some("/home/user")), "user");
        assert_eq!(remote_dir_label(Some("/etc/")), "etc"); // 尾斜杠兜底取真末段
        assert_eq!(remote_dir_label(None), "");
    }
}
