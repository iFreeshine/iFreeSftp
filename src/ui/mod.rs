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

/// 单侧远程会话上下文（对齐 C# 每窗格独立 FileView + SftpSessionService）：
/// 左右各持一份独立 SSH 会话与远程面板——左侧 + 号连接落侧 0、右侧落侧 1，
/// 两侧可打开同一主机（两份独立会话互不干扰）。
pub struct RemoteCtx {
    /// 该侧 SSH 会话（首次连接时惰性启动工作线程）。
    pub ssh: Option<crate::ssh::Ssh>,
    /// 连接状态（驱动该侧远程面板空态文案）。
    pub state: RemoteState,
    /// 等待枚举结果的远程目录（丢弃过期 Listing）。
    pub wait: Option<String>,
    /// 初始目录列举失败时的一次性回退目标（登录主目录；C# LastRemotePath 失效回退）。
    pub fallback: Option<String>,
    /// 该侧会话的主机档案（连接提交时留存；初始目录/浏览记忆/终端标签用）。
    pub host: Option<crate::hosts::HostProfile>,
    /// 该侧传输连接档案（入队远程任务用；Relay 任务两档案均入 Kind）。
    pub profile: Option<crate::transfer::Profile>,
    /// 该侧远程删除在途计数（回零后刷新该侧面板）。
    pub del_pending: u64,
    /// 该侧远程新建在途计数（Mkdir/CreateFile 已发未回；busy 链保活到
    /// Ev::Created，防 egui 空闲不重绘 —— 与 del_pending 同模式）。
    pub new_pending: u64,
    /// 该侧远程编辑下载在途：(请求号, 远程路径, 文件名)。
    pub pending_ed: Option<(u64, String, String)>,
    /// 外部编辑器远程编辑会话（按远程路径唯一；C# _editSessions 同口径）。
    pub ext_edits: Vec<ExtEdit>,
    /// 外部编辑回传前远端 mtime 冲突探测在途：(请求号, 远程路径)。
    pub ext_stat: Option<(u64, String)>,
    /// 外部编辑回传写回在途（请求号；Ev::FileWritten 按此分流）。
    pub ext_write: Option<u64>,
    /// 外部编辑会话轮询节流点。
    pub ext_poll: std::time::Instant,
    /// 该侧主机指纹确认请求（worker 阻塞等回复；Some = 本帧绘制确认覆盖层）。
    pub fp_ask: Option<dialogs::FpAsk>,
    /// 该侧远程文件面板。
    pub pane: panes::FilePane,
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

impl Default for RemoteCtx {
    fn default() -> Self {
        Self {
            ssh: None,
            state: RemoteState::Off,
            wait: None,
            fallback: None,
            host: None,
            profile: None,
            del_pending: 0,
            new_pending: 0,
            pending_ed: None,
            ext_edits: Vec::new(),
            ext_stat: None,
            ext_write: None,
            ext_poll: std::time::Instant::now(),
            fp_ask: None,
            pane: panes::FilePane {
                empty: true,
                ..Default::default()
            },
        }
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
    /// 左窗格本地视图（左侧「本地」tab 的显示内容）。
    pub left: panes::FilePane,
    /// 右窗格本地视图（右侧「本地」tab 的显示内容；对称窗格，C# 同款）。
    pub right: panes::FilePane,
    /// 左侧 tab 条（「本地」/ 远程 tab 并存；active_local() 决定左窗格映射）。
    pub left_tabs: panes::Tabs,
    /// 右侧 tab 条（「本地」/ 远程主机 tab 并存，C# 对称窗格同款）。
    pub right_tabs: panes::Tabs,
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
    /// 双窗格远程会话上下文（0=左 1=右；左右可连同一主机——两份独立会话）。
    pub remotes: [RemoteCtx; 2],
    /// 窗格层发来的远程列目录请求（(侧, 路径)；root() 统一经该侧 ssh 发出）。
    pub list_reqs: Vec<(usize, String)>,
    /// 窗格层发来的远程新建请求（(侧, 全路径, 是否目录)；root() 统一经该侧
    /// ssh 发出；完成走 Ev::Created 重列）。
    pub new_reqs: Vec<(usize, String, bool)>,
    /// 目录树懒展开请求（侧, 路径）；Listing 命中填树、Failed 复位。
    pub tree_req: Option<(usize, String)>,
    /// 同步浏览基准根（本地, 远程）：连接同步主机落位时设置；None = 未配置。
    pub sync_base: Option<(String, String)>,
    /// 同步镜像在途的 (远程侧, 目标)（Ev::Failed 命中即报失配，Ev::Listing 命中
    /// 即成功；对称窗格后镜像请求可能发往任一侧远程会话，按侧匹配）。
    pub sync_mirror: Option<(usize, String)>,
    /// 窗格导航事件（窗格层发来；root() 消费驱动镜像/比对）。
    pub sync_nav: Option<(usize, String)>,
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
    /// STAT 请求序号。
    pub stat_seq: u64,
    /// 上一帧队列是否有活动任务（忙→静沿触发两侧刷新）。
    pub q_active: bool,
    /// G17「取消队列并关闭」：(侧, 起始时刻) 等待块边界退出；退出或 5s 超时兜底后执行关闭。
    pub tab_close_wait: Option<(usize, std::time::Instant)>,
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
    /// 指纹请求在各侧 RemoteCtx.fp_ask，覆盖层按 侧 → 测试槽 顺序查找）。
    pub fp_ask_test: Option<dialogs::FpAsk>,
}

/// 远程连接状态。
pub enum RemoteState {
    Off,
    Connecting,
    Failed(String),
}

/// 侧 → 活动窗格是否本地视图（本地 tab = 是；远程 tab = 否）。两侧对称窗格的
/// 自驱判定（弹窗/传输/同步共用；C# 每 tab 独立 FileView 的等价物）。
pub(crate) fn pane_is_local(st: &State, side: usize) -> bool {
    match side {
        0 => st.left_tabs.active_local(),
        _ => st.right_tabs.active_local(),
    }
}

/// 侧 → 活动窗格视图：本地 tab = 该侧本地 pane（0 = st.left / 1 = st.right）；
/// 远程 tab = 该侧远程 pane。
pub(crate) fn pane_view(st: &State, side: usize) -> &panes::FilePane {
    match pane_is_local(st, side) {
        true => {
            if side == 0 {
                &st.left
            } else {
                &st.right
            }
        }
        false => &st.remotes[side].pane,
    }
}

pub(crate) fn pane_view_mut(st: &mut State, side: usize) -> &mut panes::FilePane {
    match pane_is_local(st, side) {
        true => {
            if side == 0 {
                &mut st.left
            } else {
                &mut st.right
            }
        }
        false => &mut st.remotes[side].pane,
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            icons: crate::icons::Cache::default(),
            sys_icons: crate::icons_sys::Cache::default(),
            left_w: 0.0,
            bottom_h: theme::BOTTOM_ROW_MIN,
            sync_browse: false,
            compare: false,
            left: {
                // 本地根视图：「此电脑」驱动器列表（C# NavigateLocalRoot 初始态）。
                let mut pane = panes::FilePane::default();
                crate::local::navigate_root(&mut pane);
                pane
            },
            left_tabs: panes::Tabs::default(),
            // 右窗格初始空态引导（C# FolderContainer.OnLoaded 右侧同款）。
            right: panes::FilePane {
                empty: true,
                ..Default::default()
            },
            right_tabs: panes::Tabs::single(t("远程")),
            dialog: dialogs::Dialog::None,
            menu: None,
            ctx_menu: ctx_menu::CtxMenu::default(),
            crumb_edit: None,
            // 设置弹窗初值来自 settings.json（C# AppSettingsService 同款）。
            settings: dialogs::Settings::from_store(),
            remotes: [RemoteCtx::default(), RemoteCtx::default()],
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
        let active = q.with_items(|items| {
            items.iter().any(|i| {
                matches!(
                    i.status,
                    crate::transfer::Status::Waiting
                        | crate::transfer::Status::Transferring
                        | crate::transfer::Status::Verifying
                )
            })
        });
        if st.q_active && !active {
            // 对称窗格：两侧本地视图一并刷新（传输结果立即可见）。
            let now = ui.ctx().time();
            crate::local::refresh(&mut st.left);
            crate::local::refresh(&mut st.right);
            st.left.refresh_at = Some(now);
            st.right.refresh_at = Some(now);
            // 任一侧远程面板正在浏览 → 重列该侧目录（传输结果立即可见）。
            for (side, r) in st.remotes.iter_mut().enumerate() {
                if let Some(rp) = r.pane.remote_path.clone() {
                    st.list_reqs.push((side, rp));
                }
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

    // 编辑器保存回传（T7）：远程文档写回远端；本地文档写盘已在编辑器内完成。
    // 左右可连同一主机（两份独立会话）：按 doc_id 反查文档 RemoteRef.host →
    // 匹配持有该主机的侧，写回走该侧会话。
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
        let side = host.and_then(|h| {
            st.remotes
                .iter()
                .position(|r| r.host.as_ref().is_some_and(|x| x.host == h))
        });
        if let Some(side) = side
            && let Some(ssh) = &st.remotes[side].ssh
        {
            ssh.write_file(
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

    // 各侧远程面板空态文案随该侧连接状态每帧刷新（启动时的 t() 缓存串不随
    // 语言切换；C# EmptyStateTitle / EmptyStateMessage）。
    for r in &mut st.remotes {
        match &r.state {
            RemoteState::Connecting => {
                r.pane.empty = true;
                r.pane.empty_title = t("正在连接…").to_owned();
                r.pane.empty_sub = t("正在建立 SSH 连接").to_owned();
            }
            RemoteState::Failed(msg) => {
                r.pane.empty = true;
                r.pane.empty_title = t("连接失败").to_owned();
                r.pane.empty_sub = msg.chars().take(60).collect();
            }
            RemoteState::Off => {
                if r.pane.empty {
                    r.pane.empty_title = t("先选择一个主机").to_owned();
                    r.pane.empty_sub = t("选择要浏览的本地或远端文件系统").to_owned();
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

    let mut hooks = dialogs::Hooks {
        menu: &mut st.menu,
        ctx_menu: &mut st.ctx_menu,
        dialog: &mut st.dialog,
        crumb_edit: &mut st.crumb_edit,
        list_req: &mut st.list_reqs,
        new_req: &mut st.new_reqs,
        tree_req: &mut st.tree_req,
        sync_nav: &mut st.sync_nav,
        compare_req: &mut st.compare_req,
        transfer: &mut st.dnd_transfer,
        pick: &mut st.pick,
        tab_close: None,
    };
    // 左窗格：活动 tab 是「本地」→ 本地视图；远程 tab → 该侧远程面板。
    // 右窗格恒远程面板。内联分支保 place 级借用检查（pane 与 tabs 独立借用）。
    if st.left_tabs.active_local() {
        panes::folder(
            ui,
            left_top,
            &mut st.left,
            &mut st.left_tabs,
            &mut st.icons,
            &mut st.sys_icons,
            &mut hooks,
            0,
        );
    } else {
        panes::folder(
            ui,
            left_top,
            &mut st.remotes[0].pane,
            &mut st.left_tabs,
            &mut st.icons,
            &mut st.sys_icons,
            &mut hooks,
            0,
        );
    }
    // 右窗格：与左对称（活动 tab 本地 → 右本地视图；远程 tab → 该侧远程面板）。
    if st.right_tabs.active_local() {
        panes::folder(
            ui,
            right_top,
            &mut st.right,
            &mut st.right_tabs,
            &mut st.icons,
            &mut st.sys_icons,
            &mut hooks,
            1,
        );
    } else {
        panes::folder(
            ui,
            right_top,
            &mut st.remotes[1].pane,
            &mut st.right_tabs,
            &mut st.icons,
            &mut st.sys_icons,
            &mut hooks,
            1,
        );
    }
    // G17：活动标签关闭请求（C# Tabs_TabCloseRequested → TabCloseRequested 事件）
    // → 确认门；hooks 借用先取值再释放。
    let tab_close = hooks.tab_close.take();
    drop(hooks);
    if let Some(side) = tab_close {
        request_close_tab(st, side);
    }

    // G17「取消队列并关闭」：等活动任务到块边界退出再关 tab（C# 轮询
    // 50×100ms；超时兜底放行——传输连接独立存活，迟到结算照常落库）。
    if let Some((side, t0)) = st.tab_close_wait {
        let active = st.transfer.as_ref().is_some_and(|q| {
            q.with_items(|items| {
                items.iter().any(|i| {
                    matches!(
                        i.status,
                        crate::transfer::Status::Waiting | crate::transfer::Status::Transferring
                    )
                })
            })
        });
        if !active || t0.elapsed() >= std::time::Duration::from_secs(5) {
            st.tab_close_wait = None;
            finish_close_tab(st, side);
        }
    }

    // 拖拽 Drop（FV-25）：目标侧 + 被拖条目 → 批量传输入队。
    if let Some((side, rows)) = st.dnd_transfer.take() {
        begin_transfer_multi(st, side, &rows);
    }

    // 窗格层请求的远程列目录（统一从这里发出，按侧经该侧 ssh）；树懒展开请求
    // 随后（互斥时窗格优先）。
    for (side, req) in std::mem::take(&mut st.list_reqs) {
        let r = &mut st.remotes[side];
        r.wait = Some(req.clone());
        r.pane.loading = true;
        if let Some(ssh) = &r.ssh {
            ssh.list(req);
        }
    }
    // 窗格层请求的远程新建（工具栏/菜单发起；root() 统一经该侧 ssh 发出）。
    // new_pending 保活到 Ev::Created（egui 无事件不重绘，与 del_pending 同理）。
    for (side, path, is_dir) in std::mem::take(&mut st.new_reqs) {
        let r = &mut st.remotes[side];
        if let Some(ssh) = &r.ssh {
            if is_dir {
                ssh.mkdir(path);
            } else {
                ssh.create_file(path);
            }
            r.new_pending += 1;
        }
    }
    if let Some((side, req)) = st.tree_req.take()
        && let Some(ssh) = &st.remotes[side].ssh
    {
        ssh.list(req);
    }

    // FV-01 文件夹选择回执：后台 STA 线程的 IFileDialog 结果。Some(路径) → 跳转
    // 对应侧本地窗格 + 同步联动；None / 通道断开 = 取消或线程异常（清槽复原
    // 按钮）；Empty = 仍在途（回槽，busy 重绘保证下帧再询）。
    if let Some(job) = st.pick.take() {
        match job.rx.try_recv() {
            Ok(Some(dir)) => {
                let side = job.side;
                let pane = match side {
                    0 => &mut st.left,
                    _ => &mut st.right,
                };
                if crate::local::navigate(pane, &dir) {
                    st.sync_nav = Some((side, dir.to_string_lossy().into_owned()));
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
    if let Some((side, path)) = st.sync_nav.take() {
        sync::handle_nav(st, side, path);
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
        || st.remotes.iter().any(|r| {
            matches!(r.state, RemoteState::Connecting)
                || r.wait.is_some()
                || r.del_pending > 0
                || r.new_pending > 0
                || r.pending_ed.is_some()
                || r.fp_ask.is_some()
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

/// 消费 SSH 工作线程事件（连接成功/失败、目录枚举结果）：按侧轮询各自会话，
/// 事件带侧分发（左右独立会话互不串扰；req 全局唯一，Stat/Exec 回流仍按 req 匹配）。
fn apply_ssh_events(st: &mut State, ctx: &egui::Context) {
    for side in 0..2 {
        let evs = st.remotes[side]
            .ssh
            .as_ref()
            .map(|s| s.poll())
            .unwrap_or_default();
        for ev in evs {
            apply_ssh_event(st, ctx, side, ev);
        }
    }
}

/// 单个会话事件入账（side = 事件来源侧）。
fn apply_ssh_event(st: &mut State, ctx: &egui::Context, side: usize, ev: crate::ssh::Ev) {
    // 终端日志来源标签（G1 = [名称 (IP)]，C# HostTabNaming；未连接时 [远程]）。
    let tag = match &st.remotes[side].host {
        Some(h) => format!("[{}]", dialogs::host_tab_naming(&h.name, &h.host)),
        None => format!("[{}]", t("远程")),
    };
    match ev {
        crate::ssh::Ev::Connected {
            label,
            root,
            confirmed,
        } => {
            // 该侧复位 + 初始目录优先级（C# AttachConnectedSession）：同步目录 >
            // 上次浏览目录 > 登录主目录；上次浏览目录失效回退主目录一次，同步目录
            // 失效直接报错（显式配置不静默掩盖）。base = 同步/比对自动激活的基准根
            // （同步目录 > 上次浏览目录；仅登录主目录 → 未配置口径）。
            let remote_base = {
                let r = &mut st.remotes[side];
                r.state = RemoteState::Off;
                let init = match &r.host {
                    Some(h) if h.sync_enabled && !h.remote_sync_path.trim().is_empty() => {
                        normalize_remote(&h.remote_sync_path)
                    }
                    Some(h) if !h.last_remote_path.trim().is_empty() => {
                        normalize_remote(&h.last_remote_path)
                    }
                    _ => root.clone(),
                };
                r.fallback = match &r.host {
                    Some(h) if h.sync_enabled && !h.remote_sync_path.trim().is_empty() => None,
                    Some(h) if !h.last_remote_path.trim().is_empty() => Some(root.clone()),
                    _ => None,
                };
                let base = match &r.host {
                    Some(h) if h.sync_enabled && !h.remote_sync_path.trim().is_empty() => {
                        Some(normalize_remote(&h.remote_sync_path))
                    }
                    Some(h) if !h.last_remote_path.trim().is_empty() => {
                        Some(normalize_remote(&h.last_remote_path))
                    }
                    _ => None,
                };
                r.pane = panes::FilePane {
                    remote_path: Some(init.clone()),
                    ..Default::default()
                };
                r.wait = Some(init.clone());
                r.pane.loading = true;
                if let Some(ssh) = &r.ssh {
                    ssh.list(init);
                }
                base
            };
            st.dialog = dialogs::Dialog::None;
            // tab 落定（G17-ANIM）：两侧同款——连接期占位远程 tab 标题更新为真名 +
            // 亮相重播（对称窗格；右侧空态占位 tab 是 local:false，命中就地改名分支）。
            // C# 此刻 tab 早已 Add（动画在标签先行时播完），此处等价用户感知的
            // 「连接成功后 tab 缓动出现」。
            let tabs = if side == 0 {
                &mut st.left_tabs
            } else {
                &mut st.right_tabs
            };
            if let Some(idx) = tabs.items.iter().position(|i| !i.local) {
                tabs.items[idx].title = label.clone();
                tabs.active = idx;
                tabs.born = Some((idx, std::time::Instant::now()));
            }
            // 同步/比对自动激活（C# FolderContainer.ConnectAndOpenAsync 规则①②）：
            // 主机配置同步浏览 → 对侧本地 + 本侧远程定基准根自动点亮；配置比对 →
            // 一并自动激活。联对面 = 连接侧远程 + 对侧本地（对称窗格，任一侧均可能）。
            sync::attach_connect(st, side, remote_base);
            // S1.9：指纹落盘（本次确认 ≠ 存档 → 传输/exec 档案回写 + 工作集回写 +
            // 盘上单字段落盘 + 日志；存档匹配路径 confirmed=None 不触发；盘上无
            // 记录 = 指纹仅本会话生效，显式提示而非静默丢弃）。
            if let Some(fp) = &confirmed
                && st.remotes[side]
                    .host
                    .as_ref()
                    .is_some_and(|h| h.host_key_fingerprint != *fp)
            {
                // ① 本会话传输/exec 档案同步携带新指纹（H1：后续传输不再按
                // 旧档案 fail-closed 全拒）。
                if let Some(p) = &mut st.remotes[side].profile {
                    p.fingerprint = Some(fp.clone());
                }
                let was_empty = st.remotes[side]
                    .host
                    .as_ref()
                    .is_some_and(|h| h.host_key_fingerprint.trim().is_empty());
                let (host, port, user, last_path) = {
                    let h = st.remotes[side].host.as_mut().expect("checked above");
                    (
                        h.host.clone(),
                        h.port,
                        h.username.clone(),
                        h.last_remote_path.clone(),
                    )
                };
                // ② 工作集回写（显式「保存」不回滚新指纹）+ 盘上单字段落盘。
                crate::hosts::update_fingerprint(&host, port, &user, fp);
                let saved = crate::hosts::save_single_session(&host, port, &user, fp, &last_path);
                if let Some(h) = &mut st.remotes[side].host {
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
        crate::ssh::Ev::Failed { what, msg } => {
            // 目录树懒展开失败：复位 loading 节点（可再展开）；同步步收尾
            // （基准入账 + 放弃恢复链，防每帧重发；C# 3 秒超时同效）。树请求只
            // 可能来自远程面板，give_up 落到发起侧面板。
            if let Some((tside, _)) = st.tree_req.take() {
                crate::ui::panes::reset_tree_loading(&mut st.left);
                crate::ui::panes::reset_tree_loading(&mut st.remotes[0].pane);
                crate::ui::panes::reset_tree_loading(&mut st.remotes[1].pane);
                let pane = if tside == 0 {
                    &mut st.remotes[0].pane
                } else {
                    &mut st.remotes[1].pane
                };
                crate::ui::panes::tree_sync_give_up(pane);
            }
            if what == "连接" {
                st.remotes[side].state = RemoteState::Failed(tfmt("连接失败：{0}", &[&msg]));
                // 连接失败：就地关闭当前弹窗（HostPicker/HostPanel 卡片发起的失败
                // 不留门，C# 同语义）；错误文案留在该侧空态/状态栏。
                st.dialog = dialogs::Dialog::None;
            } else if st.sync_mirror.as_ref().is_some_and(|(ms, _)| *ms == side) {
                // 同步镜像导航失败（按侧匹配在途镜像请求）：两侧目录不一致（C#
                // MirrorNavigateAsync 失败 → ReportMismatch）→ 关闭联动并弹窗。
                st.sync_mirror = None;
                st.remotes[side].wait = None;
                st.remotes[side].pane.loading = false;
                sync::report_mismatch(st, "SyncMismatchMsg");
            } else {
                // 上次浏览目录已失效 → 回退登录主目录重试一次（C# 同语义）；
                // 无回退资格 → 该侧目录加载失败提示。
                let r = &mut st.remotes[side];
                if let Some(home) = r.fallback.take() {
                    r.wait = Some(home.clone());
                    r.pane.loading = true;
                    r.pane.msg = None;
                    if let Some(ssh) = &r.ssh {
                        ssh.list(home);
                    }
                } else {
                    r.wait = None;
                    r.pane.loading = false;
                    r.pane.msg = Some(format!("{what}失败：{msg}"));
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
            // 指纹确认：置该侧请求态，覆盖层本帧绘制；worker 阻塞等按钮回执。
            st.remotes[side].fp_ask = Some(dialogs::FpAsk {
                first,
                host,
                stored,
                presented,
                reply,
            });
        }
        crate::ssh::Ev::Listing { path, entries } => {
            // 目录树懒展开响应优先（不经窗格；过期响应丢弃）。树请求只可能来自
            // 远程面板（本地树为静态驱动器）。
            let mut tree_hit = false;
            if let Some((tside, tp)) = st.tree_req.take() {
                if tp == path {
                    let pane = if tside == 0 {
                        &mut st.remotes[0].pane
                    } else {
                        &mut st.remotes[1].pane
                    };
                    crate::ui::panes::apply_tree_listing(pane, &path, &entries);
                    tree_hit = true;
                } else {
                    st.tree_req = Some((tside, tp));
                }
            }
            // 过期响应丢弃（等待期间用户又点了别的目录）。
            let mirror_hit = {
                let r = &mut st.remotes[side];
                let mut hit = false;
                if !tree_hit && r.wait.as_deref() == Some(path.as_str()) {
                    r.wait = None;
                    r.pane.loading = false;
                    r.pane.msg = None;
                    // 同目录重列（刷新/删除/新建回执）→ 按名保留选中（比 C# 更好：
                    // C# LoadRemoteAsync 一律清空）；跨目录导航 → 清空（C# 同）。
                    // 须在 remote_path 覆写前取旧值判定。
                    let same_dir = r.pane.remote_path.as_deref() == Some(path.as_str());
                    let old_sel: Vec<String> = if same_dir {
                        r.pane
                            .selected
                            .iter()
                            .filter_map(|&i| r.pane.entries.get(i).map(|e| e.name.clone()))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    r.pane.remote_path = Some(path.clone());
                    // ShowHidden 关时过滤点文件（C# LoadRemoteAsync :583
                    // StartsWith('.') 跳过）。
                    let mut entries = entries;
                    if !r.pane.show_hidden {
                        entries.retain(|e| !e.name.starts_with('.'));
                    }
                    r.pane.entries = entries;
                    r.pane.selected = r
                        .pane
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| old_sel.iter().any(|n| n == &e.name))
                        .map(|(i, _)| i)
                        .collect();
                    crate::local::apply_sort(&mut r.pane);
                    // 刷新淡入触发（比 C# 更好的刷新表现）：数据入账即动效；
                    // 上一轮动画未结束则不重启（保刷新钮旋转角度连续，快到账
                    // 不打断首轮，慢到账再触发下一轮）。
                    let now = ctx.time();
                    if r.pane
                        .refresh_at
                        .map_or(true, |t0| now - t0 >= panes::REFRESH_ANIM)
                    {
                        r.pane.refresh_at = Some(now);
                    }
                    // 浏览记忆单字段落盘（C# PersistLastRemotePath：仅会话状态字段，
                    // 盘上无此主机记录时静默跳过；落盘失败同静默 best-effort，不搅动
                    // 浏览流程）；回退资格随之耗尽。
                    r.fallback = None;
                    if let Some(h) = r.host.clone() {
                        let _ = crate::hosts::save_single_session(
                            &h.host,
                            h.port,
                            &h.username,
                            &h.host_key_fingerprint,
                            &path,
                        );
                    }
                    // 同步镜像成功落位（sync_mirror 按侧匹配命中即视为成功；C# 同款
                    // 隐式约定）——块外统一走 handle_nav。
                    hit = st.sync_mirror.as_ref().is_some_and(|(ms, _)| *ms == side);
                }
                hit
            };
            if mirror_hit {
                st.sync_mirror = None;
                sync::handle_nav(st, side, path);
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
        crate::ssh::Ev::Deleted => {
            // 该侧删除在途回零 → 刷新该侧面板与树。
            let r = &mut st.remotes[side];
            r.del_pending = r.del_pending.saturating_sub(1);
            if r.del_pending == 0 && r.pane.loading {
                r.pane.loading = false;
                // 删除后重建远程树（C# RefreshListKeepingPath 首行
                // RefreshTreeIfVisible）。
                crate::ui::panes::tree_mark_dirty(&mut r.pane);
                if let Some(rp) = r.pane.remote_path.clone() {
                    st.list_reqs.push((side, rp));
                }
            }
        }
        crate::ssh::Ev::Created => {
            // 新建条目完成（成败均有日志）：统一重建树 + 重列当前目录，对齐
            // C# CreateRemoteEntryAsync 末尾无条件 RefreshListKeepingPath。
            let r = &mut st.remotes[side];
            r.new_pending = r.new_pending.saturating_sub(1);
            crate::ui::panes::tree_mark_dirty(&mut r.pane);
            if let Some(rp) = r.pane.remote_path.clone() {
                st.list_reqs.push((side, rp));
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
            // 按请求号匹配发起侧（req 全局唯一，下载由该侧会话执行）。
            if st.remotes[side]
                .pending_ed
                .as_ref()
                .is_some_and(|(w, _, _)| *w == req)
            {
                let (_, remote_path, name) = st.remotes[side].pending_ed.take().unwrap();
                match result {
                    Ok(fd) => {
                        // 同路径旧外部编辑会话先作废（C# EndSession：任何形式
                        // 重编辑都先结束旧会话）。
                        st.remotes[side]
                            .ext_edits
                            .retain(|e| e.remote_path != remote_path);
                        let host = st.remotes[side].host.as_ref().map(|h| h.host.clone());
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
                                    st.remotes[side].ext_edits.push(ExtEdit {
                                        host_tag: tag.clone(),
                                        remote_path: remote_path.clone(),
                                        name: name.clone(),
                                        local: copy.clone(),
                                        base_mtime: fd.mtime,
                                        baseline,
                                        quiet_since: None,
                                        quiet_snap: None,
                                    });
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
            // 外部编辑回传写回回流：按 ext_write 请求号分流（先于内置编辑器
            // doc.id 匹配——两套计数器独立可能数值撞号，ext 优先消费）。
            if st.remotes[side].ext_write.is_some_and(|w| w == req) {
                st.remotes[side].ext_write = None;
                match result {
                    Ok(mtime) => {
                        // 会话回填（C# SetBase + 复位 watcher）：写后远端
                        // mtime 作新基准、快照复位（防自触发）。
                        if let Some(e) = st.remotes[side]
                            .ext_edits
                            .iter_mut()
                            .find(|e| e.remote_path == path)
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
                        // 回传落盘 → 即时重列该侧当前目录（视图反映新 mtime）。
                        if let Some(rp) = st.remotes[side].pane.remote_path.clone() {
                            st.list_reqs.push((side, rp));
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
                        // 上传落盘 → 即时重列该侧当前目录（比 C# 更好的即时反馈：
                        // 浏览视图直接反映新大小/修改时间，无需手动刷新）。
                        if let Some(rp) = st.remotes[side].pane.remote_path.clone() {
                            st.list_reqs.push((side, rp));
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
            // 无冲突 / 远端未报 mtime / 文件已不存在 → 直接回传。
            let mut ext_handled = false;
            if st.remotes[side]
                .ext_stat
                .as_ref()
                .is_some_and(|(w, _)| *w == req)
            {
                st.remotes[side].ext_stat = None;
                ext_handled = true;
                let session = st.remotes[side]
                    .ext_edits
                    .iter()
                    .find(|e| e.remote_path == path)
                    .map(|e| (e.name.clone(), e.base_mtime));
                if let Some((name, base)) = session {
                    let conflict = match (base, mtime) {
                        (Some(a), Some(b)) => exists && a.abs_diff(b) >= 1,
                        _ => false,
                    };
                    if conflict {
                        st.dialog = dialogs::Dialog::ExtEditConflict {
                            side,
                            name,
                            remote_path: path.clone(),
                        };
                    } else {
                        ext_upload(st, side, &path);
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
/// ext_write 槽，Ev::FileWritten 按此分流；借用错开：先取副本路径再动槽位）。
fn ext_upload(st: &mut State, side: usize, remote_path: &str) {
    let local = st.remotes[side]
        .ext_edits
        .iter()
        .find(|e| e.remote_path == remote_path)
        .map(|e| e.local.clone());
    if let Some(local) = local
        && let Ok(data) = std::fs::read(&local)
    {
        st.stat_seq += 1;
        let req = st.stat_seq;
        st.remotes[side].ext_write = Some(req);
        if let Some(ssh) = &st.remotes[side].ssh {
            ssh.write_file(remote_path.to_owned(), req, data);
        } else {
            // 会话已断：清槽防悬挂（正常路径 Stat 刚有响应，几乎不可达）。
            st.remotes[side].ext_write = None;
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
    for (side, r) in st.remotes.iter_mut().enumerate() {
        // 主机标签失配（重连/换主机）→ 旧会话静默作废（≈C# 会话随视图销毁）。
        let host_tag = match &r.host {
            Some(h) => format!("[{}]", dialogs::host_tab_naming(&h.name, &h.host)),
            None => format!("[{}]", t("远程")),
        };
        r.ext_edits.retain(|e| e.host_tag == host_tag);
        if now.duration_since(r.ext_poll).as_millis() < POLL_MS {
            continue;
        }
        r.ext_poll = now;
        for e in r.ext_edits.iter_mut() {
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
    let src_remote = !pane_is_local(st, pane);
    let dst_remote = !pane_is_local(st, dst) && st.remotes[dst].pane.remote_path.is_some();
    let (src_name, src_is_dir, size, src_mtime) = match pane_view(st, pane).entries.get(row) {
        Some(e) => (e.name.clone(), e.is_dir, e.size_bytes, e.mtime_secs),
        None => return None,
    };
    match (src_remote, dst_remote) {
        // 本地 → 远程：上传（冲突在远端，异步 STAT）。
        (false, true) => {
            let ldir = pane_view(st, pane).path.clone()?; // 根视图（驱动器列表）不支持整盘传输
            let src = ldir.join(&src_name);
            let rdir = st.remotes[dst].pane.remote_path.clone()?;
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
                },
                dst,
                Some(remote),
                src_mtime,
            ))
        }
        // 远程 → 远程：Relay 两阶段（源 → 本地 temp → 目标），冲突在目标端（异步 STAT）。
        (true, true) => {
            let rdir = st.remotes[pane].pane.remote_path.clone()?;
            let remote_src = join_remote_path(&rdir, &src_name);
            let rdir_d = st.remotes[dst].pane.remote_path.clone()?;
            let remote_dst = join_remote_path(&rdir_d, &src_name);
            let temp = relay_temp_dir(&remote_src, &remote_dst);
            let (src_prof, dst_prof) = (
                st.remotes[pane].profile.clone()?,
                st.remotes[dst].profile.clone()?,
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
                },
                dst,
                Some(remote_dst),
                src_mtime,
            ))
        }
        // 远程 → 本地：下载（冲突在本地目标，同步探测）。
        (true, false) => {
            let ldir = pane_view(st, dst).path.clone()?; // 根视图（驱动器列表）不支持整盘传输
            let rdir = st.remotes[pane].pane.remote_path.clone()?;
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
        // 远程目标：整批共用一个请求号逐条异步 STAT（发往目标侧会话）；
        // Ev::Stat 回流按 profile_side 匹配会话聚合。
        st.stat_seq += 1;
        let seq = st.stat_seq;
        if let Some(ssh) = &st.remotes[profile_side].ssh {
            for pi in &items {
                if let Some(path) = &pi.stat_path {
                    ssh.stat(path.clone(), seq);
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

/// 入队（惰性建队列线程；远程任务按目标侧会话的连接档案建传输连接）。
fn enqueue_tasks(st: &mut State, profile_side: usize, tasks: Vec<crate::transfer::NewTask>) {
    // G12：队列按批持该侧会话探测快照实时句柄（本地任务无 ssh 时用空快照）
    // ——双侧会话后传输后校验跟批次侧走；先取句柄再建队列，避免 &mut st 借用冲突。
    let probe = st.remotes[profile_side]
        .ssh
        .as_ref()
        .map(|s| s.probe_handle())
        .unwrap_or_else(|| std::sync::Arc::new(std::sync::Mutex::new((0u64, None))));
    let q = st
        .transfer
        .get_or_insert_with(crate::transfer::TransferQueue::new);
    q.enqueue(st.remotes[profile_side].profile.clone(), probe, tasks);
}

/// G17 关闭确认门（C# FolderContainer.OnTabCloseConfirming）：会话队列有活动
/// 传输任务 → 三键确认；否则直接关。Rust 队列无 per-tab 日志源（C# LogScope），
/// 单队列即全局判据。
fn request_close_tab(st: &mut State, side: usize) {
    let has_active = st.transfer.as_ref().is_some_and(|q| {
        q.with_items(|items| {
            items.iter().any(|i| {
                matches!(
                    i.status,
                    crate::transfer::Status::Waiting | crate::transfer::Status::Transferring
                )
            })
        })
    });
    if has_active {
        st.dialog = dialogs::Dialog::TabClose { side };
    } else {
        finish_close_tab(st, side);
    }
}

/// G17 执行关闭（C# OnTabCloseRequested 资源清理）：断该侧会话（C# view.Dispose
/// → _sftp.Dispose；worker 断开并发「已断开」日志 + Ev::Disconnected），远程 tab
/// 整侧上下文复位（含指纹确认/删除在途/编辑下载在途），本地 tab 回空态引导页。
/// 最后一个 tab 关闭回初始单 tab（C# AddNewTab(emptyState: true)——空态标签条
/// 隐藏，Rust 同款）。Header 按钮状态每帧重算，无需显式通知
/// （C# TabCountChanged / NotifySyncStateChanged 的 Rust 等价物）。
fn finish_close_tab(st: &mut State, side: usize) {
    // 关的是远程 tab 还是本地 tab：两侧对称窗格，看活动 tab 类型
    // （C# 每 tab 独立 FileView，关闭即断该 tab 会话，另一 tab 不受影响）。
    let remote_tab = !pane_is_local(st, side);
    if remote_tab {
        // 断该侧会话（take 后 disconnect；指纹确认 reply 发送端随复位失效，
        // worker 阻塞发送失败即按拒绝处理）+ 整侧复位。
        if let Some(ssh) = st.remotes[side].ssh.take() {
            ssh.disconnect();
        }
        st.remotes[side] = RemoteCtx::default();
        // 该侧树在途请求一并作废。
        if st.tree_req.as_ref().is_some_and(|(s, _)| *s == side) {
            st.tree_req = None;
        }
        // 活动视图换人（C# NotifySyncStateChanged 重算联动按钮）：联对面 =
        // 任一侧本地 + 另一侧远程，任一侧远程 tab 关闭均拆对 → 联动失效。
        st.sync_browse = false;
        st.sync_base = None;
        st.sync_mirror = None;
    } else {
        let pane = if side == 0 {
            &mut st.left
        } else {
            &mut st.right
        };
        *pane = panes::FilePane {
            empty: true,
            ..Default::default()
        };
        pane.empty_title = t("先选择一个主机").to_owned();
        pane.empty_sub = t("选择要浏览的本地或远端文件系统").to_owned();
    }
    // 标签处理：非最后一个移除活动标签；最后一个回初始单 tab（空态下标签条
    // 隐藏，占位标题不可见）。左侧关远程 tab 后优先激活「本地」tab。
    let tabs = if side == 0 {
        &mut st.left_tabs
    } else {
        &mut st.right_tabs
    };
    if tabs.items.len() > 1 {
        tabs.items.remove(tabs.active);
        if tabs.active >= tabs.items.len() {
            tabs.active = tabs.items.len() - 1;
        }
        if side == 0
            && let Some(i) = tabs.items.iter().position(|i| i.local)
        {
            tabs.active = i;
        }
    } else if side == 0 {
        *tabs = panes::Tabs::default();
    } else {
        *tabs = panes::Tabs::single(t("远程"));
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
