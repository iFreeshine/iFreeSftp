//! 弹窗与浮层（对照 C# RenameInputDialog / DeleteConfirmDialog / ChecksumDialog /
//! PermissionDialog / HostPickerPanel / SettingsPanel + FileView 右键菜单）。
//!
//! 统一外壳（README 3.9 弹框统一原则，令牌清单 §7.2）：
//! 遮罩 #80000000（点击关闭）→ 居中卡片（DialogTitleBarBrush 一体底 + 1px 描边 +
//! 圆角 8 + 阴影）→ 标题行（padding 14,5,5,2 / 关闭钮 28×28 / 标题 14 或 16）→
//! 内容圆弧容器（距边 5 / 圆角 6 / CardBackgroundFill 底）。
//! ESC / 遮罩 / 关闭钮 均关闭；标题行拖动窗口（C# TitleBar_PointerMoved 同效果）：
//! 主机/设置面板在主窗口内平移（C# 无边框窗口语义），小弹窗保持视口拖动。
//! 设置面板值为演示态内存值，落盘随 T3+ 的 AppSettingsService 接入。

use egui::{
    Align, Align2, Area, Color32, FontFamily, FontId, Id, Layout, Margin, Order, Pos2, Rect,
    RichText, Sense, Stroke, Vec2,
};

use crate::i18n::{t, t_ctx, tfmt};
use crate::theme;

use super::State;

// ── 外壳令牌（theme-tokens-csharp.md §7.2）──────────────────
/// 标题行 padding（左,上,右,下）。
const TITLE_PAD: (f32, f32, f32, f32) = (14.0, 5.0, 5.0, 2.0);
/// 关闭钮边长。
const CLOSE_SIZE: f32 = 28.0;
/// 内容区 padding（横,纵）。
const CONTENT_PAD: (f32, f32) = (16.0, 12.0);
/// 组间距。
const GROUP_GAP: f32 = 14.0;
/// 标题行总高（上 5 + 关闭钮 28 + 下 2）。
const TITLE_BAR_H: f32 = 35.0;
/// 标准钮 / 输入框高（WinUI 控件默认 32）。
const BTN_H: f32 = 32.0;
/// 右键菜单宽（令牌清单 §7.3）。
pub(super) const MENU_W: f32 = 200.0;
/// 右键菜单项高（MenuFlyoutItem：Padding `11,8,11,9` + 14px 字行高 ≈ 19，
/// MinHeight 32 被内容撑过，实测 ≈ 36）。
pub(super) const MENU_ITEM_H: f32 = 36.0;
/// 菜单分隔线槽高（MenuFlyoutSeparator：Height 1 + Margin 上下 1）。
const MENU_SEP_H: f32 = 3.0;
/// 菜单体上下留白（MenuFlyoutPresenterThemePadding `0,2,0,2`）。
pub(super) const MENU_BODY_PAD: f32 = 2.0;

// ── 主题相关取值 ────────────────────────────────────────────
/// CardBackgroundFillColorDefault（弹窗内容圆弧容器底；host_panel、import_dialog 消费）。
/// 精确值：亮 = 30% 白叠层 / 暗 = 5.1% 白。
pub(crate) fn card_bg(t: egui::Theme) -> Color32 {
    match t {
        egui::Theme::Light => Color32::from_rgba_unmultiplied(255, 255, 255, 0xB3),
        egui::Theme::Dark => Color32::from_white_alpha(0x0D),
    }
}

// ── 弹窗状态 ────────────────────────────────────────────────
/// 当前弹窗。`None` = 无弹窗。
pub enum Dialog {
    None,
    /// 重命名：窗格 + 原名 + 输入中名字。
    Rename {
        pane: usize,
        orig: String,
        name: String,
    },
    /// 删除确认：窗格 + 待删行号。
    Delete {
        pane: usize,
        indices: Vec<usize>,
    },
    /// 校验码：完整路径（本地绝对路径或远程路径）+ 算法下标 + 计算态。
    /// side = Some(侧) 为远程路径（经该侧 Ssh exec）；None = 本地（ring 直算）。
    Checksum {
        name: String,
        path: String,
        side: Option<usize>,
        algo: usize,
        copied: Option<f64>,
    },
    /// 权限九宫格：bits = 属主×3 + 属组×3 + 其他×3（rwx）。
    Permission {
        pane: usize,
        name: String,
        is_dir: bool,
        bits: [bool; 9],
    },
    /// 打开内容选择器：目标窗格（0=左本地 / 1=右远程）。new_tab = tab 栏加号
    /// 入口（C# NewTabRequested → HostPickerOverlay.OpenFor：HostPicked 强制
    /// TabIntent="new" 新标签页 / LocalPicked 非空态 AddNewTab）；false = 空态
    /// 引导按钮（就地打开，C# 空态判定 mode=current 同语义）。
    HostPicker {
        target: usize,
        new_tab: bool,
    },
    /// 传输覆盖确认（C# 冲突确认对话框）：冲突清单对比表（名称/源/目标的
    /// 大小+mtime），批级一次决策（全部覆盖/跳过冲突项/取消）。
    TransferConfirm {
        /// 冲突清单行（目标较新行目标列标警示色）。
        rows: Vec<ConflictRow>,
    },
    /// 设置面板（920×640）。win_drag = 标题栏面板拖动偏移（C# 无边框窗口）。
    Settings {
        win_drag: Vec2,
    },
    /// 主机设置面板（C# HostPanel 920×640 1:1，实现见 host_panel.rs）：
    /// 左列卡片网格 / 右列三页签；显式保存语义（打开快照、保存整表落盘）。
    HostPanel {
        /// 内存工作集（打开时盘上快照）。
        list: Vec<crate::hosts::HostProfile>,
        /// 选中卡片下标（None=未选中，右列页签禁用）。
        sel: Option<usize>,
        /// 选中载入时的主机地址（保存时比对，变更清指纹）。
        orig_host: String,
        /// 右列页签（0 常规 / 1 同步 / 2 传输）。
        tab: usize,
        /// 端口文本框（数字过滤，保存时解析钳制）。
        port_text: String,
        /// keep-alive 间隔文本框（数字过滤，保存时解析钳制）。
        ka_text: String,
        /// 卡片右键菜单（下标 + 弹出位置）。
        menu: Option<(usize, Pos2)>,
        /// InfoBar 提醒（成功?, 标题, 详情, 打开时刻, 手动关）。
        alert: Option<(bool, String, String, f64, bool)>,
        /// 测试连接在途回执（G15：结果 + 主机条目快照；成功带本次确认指纹，
        /// None = 存档精确匹配未弹框）。
        test_rx: Option<
            std::sync::mpsc::Receiver<(Result<Option<String>, String>, crate::hosts::HostProfile)>,
        >,
        /// 测试连接 Interactive 指纹门事件（AskFingerprint / Log，G15）。
        fp_rx: Option<std::sync::mpsc::Receiver<crate::ssh::Ev>>,
        /// 测试连接进行中。
        testing: bool,
        /// 卡片拖拽排序来源下标。
        drag: Option<usize>,
        /// 标题栏面板拖动偏移（C# HostPanel 无边框窗口拖动语义）。
        win_drag: Vec2,
    },
    /// 会话导入向导（C# ImportDialog 三态，实现见 import_dialog.rs）：
    /// 同意页（隐私说明+来源勾选）→ 预览表（判重+警告标记）→ 结果页。
    Import {
        /// 当前态：0 同意 / 1 预览 / 2 结果。
        stage: u8,
        /// 显式同意勾选（未勾选不可扫描）。
        consent: bool,
        /// 来源勾选集合（下标 = SourceKind::index()）。
        sources: [bool; 6],
        /// 扫描进行中（后台线程）。
        scanning: bool,
        /// 扫描回执。
        scan_rx: Option<std::sync::mpsc::Receiver<crate::import::ScanResult>>,
        /// 扫描到的会话（与 rows 平行同序，导入时取判重定位）。
        sessions: Vec<crate::import::ImportedSession>,
        /// 分来源统计（结果页展示）。
        stats: Vec<crate::import::SourceStat>,
        /// 预览行（列文本预构建）。
        rows: Vec<super::import_dialog::ImportRow>,
        /// 预览页概要。
        summary: String,
        /// 本机未发现的来源。
        missing: Vec<crate::import::SourceKind>,
        /// 单来源失败记录（来源 + 机器可读 code）。
        errors: Vec<(crate::import::SourceKind, &'static str)>,
        /// 超出全局总量上限被丢弃的条数。
        truncated: usize,
        /// 本次已导入条数。
        imported: usize,
    },
    /// 同步浏览失配提示（C# ReportMismatch ContentDialog）：说明 + 关闭。
    SyncMismatch {
        msg: String,
    },
    /// 关闭标签页确认（G17，C# OnTabCloseConfirming ContentDialog）：关哪侧哪个 tab。
    TabClose {
        side: usize,
        /// 目标 tab id（State 派发；确认/取消/等待路径都以它定位）。
        tab: u32,
        /// 该 tab 归属的活动传输任务数（P3 per-tab 计数，弹窗正文显示）。
        n: usize,
    },
    /// 外部编辑器缓存变化询问（C# ShowRemoteEditChangedAsync）：编辑器保存
    /// 落盘静默期满触发；「保存编辑结果」→ 先 STAT 探远端 mtime 冲突再回传。
    ExtEditChanged {
        side: usize,
        /// 发起 tab id（ext_edits/ext_stat/ext_write 都挂靠在该 tab 上）。
        tab: u32,
        name: String,
        remote_path: String,
    },
    /// 外部编辑器远端冲突确认（C# ShowRemoteConflictAsync）：回传前探测到
    /// 远端 mtime 与下载基准差 ≥1s；「覆盖」→ 直接回传。
    ExtEditConflict {
        side: usize,
        /// 发起 tab id（ext_upload 定位该 tab 的会话与槽位）。
        tab: u32,
        name: String,
        remote_path: String,
    },
}

/// 冲突清单行（传输覆盖确认弹窗对比表数据；C# 冲突对话框逐项事实的批级聚合）。
pub struct ConflictRow {
    pub name: String,
    /// 源大小（字节）/ mtime（Unix 秒）。
    pub src_size: u64,
    pub src_mtime: i64,
    /// 目标大小 / mtime（远程 STAT 或本地 metadata；未报属性时 None）。
    pub dst_size: Option<u64>,
    pub dst_mtime: Option<i64>,
}

/// 队列面板右键菜单来源。
#[derive(Clone, Copy)]
pub enum QueueMenu {
    /// 队列空白区：暂停/继续、停止、清除（C# Root_RightTapped）。
    Batch,
    /// 队列行：取消/重试/重连/移除（按行状态显隐，C# Row_RightTapped）。
    Item(u64),
}

/// 右键菜单状态（C# MenuFlyout 轻量镜像）：弹出位置 + 来源窗格 + 行号（None=空白区）+ 队列菜单 + 终端菜单。
pub struct MenuState {
    pub pos: Pos2,
    pub pane: usize,
    pub row: Option<usize>,
    pub queue: Option<QueueMenu>,
    pub terminal: bool,
}

/// 主机指纹确认请求（C# HostKeyConfirmDialog；worker 阻塞等 reply 回 bool，
/// true = 信任；false / 通道断 = 拒绝）。
pub struct FpAsk {
    /// true = 首次连接（无存档）→ 默认信任；false = 存档失配 → 默认取消（防中间人）。
    pub first: bool,
    pub host: String,
    /// 此前存档指纹（SHA256 base64 无前缀）。
    pub stored: Option<String>,
    /// 本次出示指纹（同格式）。
    pub presented: String,
    pub reply: std::sync::mpsc::Sender<bool>,
}

/// C# PureHelpers.HostTabNaming.Build：名称空或与地址相同 → 只显地址；
/// 地址空 → 只显名称；否则 "{名称} ({地址})"。
pub(crate) fn host_tab_naming(name: &str, host: &str) -> String {
    let name = name.trim();
    let host = host.trim();
    if name.is_empty() || name == host {
        host.to_owned()
    } else if host.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({host})")
    }
}

/// 设置面板值（AppSettingsService 演示镜像；落盘随 T3+ 接入）。
pub struct Settings {
    /// 当前大类页下标。
    pub category: usize,
    pub theme_dark: bool,
    pub lang_en: bool,
    pub timeout: f32,
    pub max_retries: f32,
    pub retry_delay: f32,
    pub op_timeout: f32,
    pub open_mode: usize,
    pub keepalive: bool,
    pub ka_interval: f32,
    pub concurrent: f32,
    pub del_server_cmd: bool,
    pub resume: bool,
    pub resume_threshold: f32,
    pub verify: bool,
    pub verify_algo: usize,
    pub tar_accel: bool,
    pub tar_threshold: f32,
    pub preserve_mtime: bool,
    pub editor_builtin: bool,
    /// 外部编辑器路径输入框（编辑方式子卡右侧，C# EditorPathBox）。
    pub editor_path: String,
    /// 注册码输入框内容（注册页激活卡右侧，C# RegCodeBox）。
    pub reg_code: String,
    /// 关于页「有新版本时提醒我」开关（C# UpdateCheckToggle）。
    pub update_check: bool,
    /// 注册页激活结果提示（Some = 激活过一次后：(文案, 是否成功色)）；
    /// None = 显示「粘贴收到的注册码…」引导（C# RegCodeHint 初始文案）。
    pub reg_hint: Option<(String, bool)>,
    /// 更新检查进行中（按钮禁用，C# _checking 防重入）。
    pub update_checking: bool,
    /// 更新卡结果文案（C# UpdateCheckResult；空 = 未检查）。
    pub update_check_result: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            category: 0,
            theme_dark: false,
            lang_en: false,
            timeout: 20.0,
            max_retries: 2.0,
            retry_delay: 5.0,
            op_timeout: 15.0,
            open_mode: 0,
            keepalive: true,
            ka_interval: 30.0,
            concurrent: 2.0,
            del_server_cmd: true,
            resume: true,
            resume_threshold: 1024.0,
            verify: false,
            verify_algo: 0,
            tar_accel: false, // 与 settings.rs::default 一致（端到端用例补齐前不默认启用）
            tar_threshold: 50.0,
            preserve_mtime: true,
            editor_builtin: true,
            editor_path: String::new(),
            reg_code: String::new(),
            update_check: true,
            reg_hint: None,
            update_checking: false,
            update_check_result: String::new(),
        }
    }
}

impl Settings {
    /// 启动初值：读 settings::AppSettings（settings.json）。
    pub fn from_store() -> Self {
        crate::settings::with(|s| Self {
            category: 0,
            theme_dark: s.theme == "Dark",
            lang_en: s.language == "en-US",
            timeout: s.connect_timeout as f32,
            max_retries: s.max_retries as f32,
            retry_delay: s.retry_delay as f32,
            op_timeout: s.operation_timeout as f32,
            open_mode: s.connect_open_mode,
            keepalive: s.keep_alive,
            ka_interval: s.keep_alive_interval as f32,
            concurrent: s.concurrent_transfers as f32,
            del_server_cmd: s.remote_delete_exec,
            resume: s.resume,
            resume_threshold: s.resume_threshold_kb as f32,
            verify: s.verify,
            verify_algo: s.verify_algo,
            tar_accel: s.tar_accel,
            tar_threshold: s.tar_threshold as f32,
            preserve_mtime: s.preserve_mtime,
            editor_builtin: s.editor_builtin,
            editor_path: s.editor_path.clone(),
            reg_code: String::new(),
            update_check: s.update_check,
            reg_hint: None,
            update_checking: false,
            update_check_result: String::new(),
        })
    }

    /// 每帧回写 store（内部整包比对置脏，防抖落盘）。主题沿用 window.ini 不回写；
    /// 语言回写（下拉改动经此生效并随 settings.json 持久化）。
    fn sync_to_store(&self) {
        crate::settings::sync_from_dialog(crate::settings::AppSettings {
            theme: String::new(),
            language: if self.lang_en {
                "en-US".into()
            } else {
                "zh-CN".into()
            },
            connect_timeout: self.timeout as i32,
            max_retries: self.max_retries as i32,
            retry_delay: self.retry_delay as i32,
            operation_timeout: self.op_timeout as i32,
            connect_open_mode: self.open_mode,
            concurrent_transfers: self.concurrent as i32,
            editor_builtin: self.editor_builtin,
            editor_path: self.editor_path.clone(),
            remote_delete_exec: self.del_server_cmd,
            keep_alive: self.keepalive,
            keep_alive_interval: self.ka_interval as i32,
            update_check: self.update_check,
            resume: self.resume,
            resume_threshold_kb: self.resume_threshold as i32,
            tar_accel: self.tar_accel,
            tar_threshold: self.tar_threshold as i32,
            verify: self.verify,
            verify_algo: self.verify_algo,
            preserve_mtime: self.preserve_mtime,
            dirty: false,
            last_change: None,
        });
    }
}

/// 传给窗格层的交互钩子：右键菜单 / 弹窗打开 / 面包屑编辑。
pub struct Hooks<'a> {
    pub menu: &'a mut Option<MenuState>,
    /// 编辑框全局右键菜单（面包屑编辑框 show 后注册，main.rs 帧首/帧尾驱动）。
    pub ctx_menu: &'a mut super::ctx_menu::CtxMenu,
    pub dialog: &'a mut Dialog,
    pub crumb_edit: &'a mut Option<(usize, String)>,
    /// 远程列目录请求（(侧, 路径)；root() 统一经对应侧 Ssh 发出）。
    pub list_req: &'a mut Vec<(usize, String)>,
    /// 远程新建请求（(侧, 全路径, 是否目录)；root() 统一经对应侧 Ssh 发出，
    /// 完成走 Ev::Created 重列）。
    pub new_req: &'a mut Vec<(usize, String, bool)>,
    /// 目录树懒展开请求（侧, 路径）；响应经 Ev::Listing 填树不经窗格。
    pub tree_req: &'a mut Option<(usize, String)>,
    /// 窗格导航事件（侧, 新路径）：同步浏览镜像 + 比对重排驱动（root() 消费）。
    pub sync_nav: &'a mut Option<(usize, String)>,
    /// 比对标记与 entries 长度失配（任一侧列表重载后需重排）。
    pub compare_req: &'a mut bool,
    /// 拖拽传输请求（FV-25：目标侧, 被拖条目行号；root() 走批量传输入队）。
    pub transfer: &'a mut Option<(usize, Vec<usize>)>,
    /// 文件夹选择在途槽（FV-01：open_dir 发起后台 STA 线程弹框；root() 轮询
    /// 回执跳转。Some = 在途，工具栏按钮此间禁用防重复弹框）。
    pub pick: &'a mut Option<crate::local::PickJob>,
    /// G17 活动标签关闭请求（(侧, tab id)；root() 走确认门与资源清理，C#
    /// Tabs_TabCloseRequested → TabCloseRequested 事件）。
    pub tab_close: Option<(usize, u32)>,
}

/// 任一侧会话池存在待决指纹确认（含测试连接槽）——覆盖层挂载 / Esc 抑制判定共用。
fn fp_ask_pending(st: &State) -> bool {
    st.remotes
        .iter()
        .any(|rs| rs.sessions.iter().any(|s| s.fp_ask.is_some()))
        || st.fp_ask_test.is_some()
}

// ── 每帧入口 ────────────────────────────────────────────────
pub fn draw(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    draw_menu(ui, st, p);
    match st.dialog {
        Dialog::None => {}
        Dialog::Rename { .. } => rename_dialog(ui, st, p),
        Dialog::Delete { .. } => delete_dialog(ui, st, p),
        Dialog::Checksum { .. } => checksum_dialog(ui, st, p),
        Dialog::Permission { .. } => permission_dialog(ui, st, p),
        Dialog::HostPicker { .. } => host_picker(ui, st, p),
        Dialog::TransferConfirm { .. } => transfer_confirm_dialog(ui, st, p),
        Dialog::Settings { mut win_drag } => {
            if settings_dialog(ui, st, p, &mut win_drag) {
                // 写回拖拽偏移（partial move 字段须回填，否则下一帧偏移丢失）；
                // false = 已关闭（shell 已置 None，不再复活）。
                st.dialog = Dialog::Settings { win_drag };
            }
        }
        Dialog::HostPanel { .. } => super::host_panel::draw(ui, st, p),
        Dialog::Import { .. } => super::import_dialog::draw(ui, st, p),
        Dialog::SyncMismatch { .. } => sync_mismatch_dialog(ui, st, p),
        Dialog::TabClose { .. } => tab_close_dialog(ui, st, p),
        Dialog::ExtEditChanged { .. } => ext_edit_changed_dialog(ui, st, p),
        Dialog::ExtEditConflict { .. } => ext_edit_conflict_dialog(ui, st, p),
    }
    // 指纹确认覆盖层（最顶层；期间底下弹窗的 Esc 已被 shell 抑制）。
    if fp_ask_pending(st) {
        fp_confirm_overlay(ui, st, p);
    }
}

/// 统一外壳。返回内容圆弧容器矩形；None = 弹窗已关闭（ESC/遮罩/关闭钮）。
pub(crate) fn shell(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    w: f32,
    h: f32,
    title: &str,
    large: bool,
    // 标题行下间距（C# 弹窗壳内容 Grid RowSpacing：HostPanel / SettingsPanel 4，其余 0）。
    row_gap: f32,
    // 是否画统一内容圆弧容器底。C# 内容层两模式（§7.2）：
    // ① 统一容器——四个小弹窗（Margin 5,0,5,5）/ ImportDialog / HostPickerPanel 自绘
    //   CardBackgroundFillColorDefault Border（圆角 6），此处统一镜像，传 true；
    // ② 无容器——HostPanel / SettingsPanel 主体 Grid（Margin 5）上直接挂左右子卡
    //   Border（两 XAML 同构），ContentDialog（传输确认 / 同步失配）走系统模板内容
    //   直放，均无容器 Border，传 false，否则多出一层父容器色。
    content_card: bool,
    // Some = 面板可拖（HostPanel / Settings：对齐 C# 无边框窗口标题栏拖动，
    // 面板整体在主窗口内平移）；None = 保持视口拖动（小弹窗既有行为）。
    mut win_drag: Option<&mut Vec2>,
) -> Option<Rect> {
    // 弹窗层清一次光标：egui 0.37 同层 hover 无 z 序排除，被弹窗遮住的
    // splitter / 列宽 thumb 仍会 hovered 并把 ↔ 写进 output（发生在帧首
    // 复位之后）；shell 是弹窗最前置绘制点，此处后写胜出，弹窗内部控件的
    // 光标设置发生在其后不受影响。
    ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    let full = ui.max_rect();
    // 指纹确认覆盖层打开时 Esc 归覆盖层（防一击双关：底弹窗不应同时响应）。
    let esc = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) && !fp_ask_pending(st);

    // 遮罩：先挂（z 最底），点击即关。
    let mask = ui.allocate_rect(full, Sense::click());
    ui.painter().rect_filled(full, 0.0, theme::mask());

    // 卡片：软投影（C# ThemeShadow Z=32 观感近似，统一走 theme::draw_shadow）
    // + 一体底 + 描边。
    // 面板偏移仅 HostPanel / Settings 启用（小弹窗零偏移 = 居中）。
    let card = Rect::from_center_size(
        full.center() + *win_drag.as_deref().unwrap_or(&Vec2::ZERO),
        egui::vec2(w, h),
    );
    theme::draw_shadow(ui, card, theme::CONTAINER_RADIUS, theme::ShadowKind::Dialog);
    ui.painter()
        .rect_filled(card, theme::CONTAINER_RADIUS, p.dialog_title_bar);
    ui.painter().rect_stroke(
        card,
        theme::CONTAINER_RADIUS,
        Stroke::new(1.0, p.card_stroke),
        egui::StrokeKind::Inside,
    );
    // 卡片命中面：吸收内部点击，避免穿透到遮罩造成误关。
    ui.allocate_rect(card, Sense::click());

    // 子元素排在描边内缩 1px 的 inner 区（C# Card BorderThickness=1）。
    let inner = card.shrink(1.0);
    // 标题行：窄标题栏通栏贴边。面板模式累计指针位移整体平移（阴影/标题/
    // 内容全由 card 派生，自动跟随）；视口模式拖动 OS 主窗口。
    let title_bar = Rect::from_min_max(
        inner.min,
        Pos2::new(inner.right(), inner.top() + TITLE_BAR_H),
    );
    let drag = ui.allocate_rect(title_bar, Sense::click_and_drag());
    if let Some(off) = win_drag.as_deref_mut() {
        let delta = drag.drag_delta();
        if delta != Vec2::ZERO {
            *off += delta;
        }
    } else if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    let title_size = if large { 16.0 } else { 14.0 };
    let icon_size = if large { 11.0 } else { 10.0 };
    ui.painter().text(
        Pos2::new(inner.left() + TITLE_PAD.0, title_bar.center().y),
        Align2::LEFT_CENTER,
        title,
        // FontWeight=SemiBold → brand 字族（theme.rs 注册的 SemiBold 品牌面）。
        FontId::new(title_size, FontFamily::Name("brand".into())),
        p.text_primary,
    );

    // 关闭钮 28×28（hover Subtle 二级 / 按下三级）。
    let close_r = Rect::from_center_size(
        Pos2::new(
            title_bar.right() - TITLE_PAD.2 - CLOSE_SIZE / 2.0,
            title_bar.center().y,
        ),
        egui::vec2(CLOSE_SIZE, CLOSE_SIZE),
    );
    let close = ui.allocate_rect(close_r, Sense::click());
    if close.hovered() {
        let bg = if close.is_pointer_button_down_on() {
            p.row_pressed
        } else {
            p.row_hover
        };
        ui.painter().rect_filled(close_r, 4.0, bg);
    }
    if let Some(img) = st.icons.get(ui.ctx(), "x", p.text_primary, icon_size) {
        img.paint_at(
            ui,
            Rect::from_center_size(close_r.center(), egui::vec2(icon_size, icon_size)),
        );
    }

    // 内容区几何：排在 inner 内、距边 5px、圆角 6；标题行下间距 = row_gap
    // （C# 弹窗壳内容 Grid RowSpacing：HostPanel / SettingsPanel 4，其余 0）。
    // 统一圆弧容器底仅在 content_card 时绘制（card_bg 为半透明白叠层，
    // 与内部自绘子卡片叠加会变浅，C# HostPanel 无此层）。
    let content = Rect::from_min_max(
        Pos2::new(inner.left() + 5.0, title_bar.bottom() + row_gap),
        Pos2::new(inner.right() - 5.0, inner.bottom() - 5.0),
    );
    if content_card {
        ui.painter().rect_filled(content, 6.0, p.card_bg);
    }

    if esc || close.clicked() || mask.clicked() {
        st.dialog = Dialog::None;
        return None;
    }
    Some(content)
}

/// 在内容容器里建一个自顶向下布局的子 UI。
fn content_ui(ui: &mut egui::Ui, content: Rect) -> egui::Ui {
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content)
            .layout(Layout::top_down(Align::Min)),
    )
}

// ── 通用控件（WinUI 手绘口径）──────────────────────────────
/// 标准按钮（默认字号 14）：底=ControlFillColorDefault，hover=Secondary / 按下=Tertiary，描边 ControlStrokeColorSecondary。
pub(crate) fn std_button(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &str,
    fg: Color32,
    p: &theme::Palette,
) -> bool {
    std_button_fs(ui, rect, label, fg, p, 14.0)
}

/// 标准按钮（可指定字号；C# 控件级 FontSize 覆写场景：设置卡 13 / 主机面板 12）。
pub(crate) fn std_button_fs(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &str,
    fg: Color32,
    p: &theme::Palette,
    font: f32,
) -> bool {
    std_button_en(ui, rect, label, fg, p, font, true)
}

/// 标准按钮（含禁用态；C# IsEnabled=false：ControlFillColorDisabled 底 +
/// ControlStrokeColorDisabled 描边 + TextFillColorDisabled 字，点击不响应）。
pub(crate) fn std_button_en(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &str,
    fg: Color32,
    p: &theme::Palette,
    font: f32,
    enabled: bool,
) -> bool {
    // 描边像素对齐（theme::snap_rect）：半像素坐标会让 1px 边线被抗锯齿摊薄。
    let rect = theme::snap_rect(rect, ui.ctx().pixels_per_point());
    let resp = ui.allocate_rect(rect, Sense::click());
    let (bg, stroke_c, text_c) = if !enabled {
        // Fluent 默认禁用三件套（C# 未覆写，v11 补录令牌文档）。
        (
            p.control_disabled,
            p.control_stroke_disabled,
            p.text_disabled,
        )
    } else if resp.is_pointer_button_down_on() {
        (p.control_pressed, p.control_stroke, fg)
    } else if resp.hovered() {
        (p.control_hover, p.control_stroke, fg)
    } else {
        (p.control_bg, p.control_stroke, fg)
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    ui.painter().rect_stroke(
        rect,
        4.0,
        theme::control_border(ui.ctx().pixels_per_point(), stroke_c),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(font),
        text_c,
    );
    enabled && resp.clicked()
}

/// 危险按钮：同标准按钮，字色 SystemFillColorCritical（删除钮）。
fn danger_button(ui: &mut egui::Ui, rect: Rect, label: &str, p: &theme::Palette) -> bool {
    std_button(ui, rect, label, p.critical, p)
}

/// 按钮宽（文字 + 左右 11）。
fn btn_w(ui: &egui::Ui, label: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(14.0), Color32::WHITE)
        .size()
        .x
        + 22.0
}

/// 右对齐按钮组（间距 8），返回是否有按钮被点击。
fn button_row(
    ui: &mut egui::Ui,
    right_x: f32,
    y: f32,
    labels: &[&str],
    p: &theme::Palette,
) -> bool {
    let mut x = right_x;
    let mut clicked = false;
    for label in labels.iter().rev() {
        let w = btn_w(ui, label);
        x -= w;
        if std_button(
            ui,
            Rect::from_min_size(Pos2::new(x, y), egui::vec2(w, BTN_H)),
            label,
            p.text_primary,
            p,
        ) {
            clicked = true;
        }
        x -= 8.0;
    }
    clicked
}

/// 预乘分量线性混色（开关滑动过渡的颜色插值）。
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        f(a.r(), b.r()),
        f(a.g(), b.g()),
        f(a.b(), b.b()),
        f(a.a(), b.a()),
    )
}

/// 开关（WinUI ToggleSwitch 46 口径 → 轨道 40×20 / 拇指 12 / 轨道内边距 4）。
/// 滑动动画：`animate_value_with_time`（0.15s，ControlFastAnimation 档手感），
/// 轨道底 / 描边 / 拇指色随进度插值，off 描边随滑动渐隐（WinUI on 态无描边）。
pub(crate) fn toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    on: bool,
    enabled: bool,
    p: &theme::Palette,
) -> bool {
    let resp = ui.allocate_rect(rect, Sense::click());
    let t = ui
        .ctx()
        .animate_value_with_time(resp.id, f32::from(on), 0.15);
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    // 轨道底：off = ControlAltFillColorSecondary（悬停 Tertiary / 按下 Quarternary），
    // on = AccentFillColorDefault（悬停 Secondary = ACCENT_HOVER / 按下 Tertiary = ACCENT_PRESSED）。
    let off_fill = if !enabled {
        Color32::TRANSPARENT // ControlAltFillColorDisabled = 透明
    } else if pressed {
        p.toggle_off_fill_pressed
    } else if hovered {
        p.toggle_off_fill_hover
    } else {
        p.toggle_off_fill
    };
    let on_fill = if !enabled {
        p.accent_disabled // AccentFillColorDisabled
    } else if pressed {
        theme::ACCENT_PRESSED
    } else if hovered {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    ui.painter()
        .rect_filled(rect, rect.height() / 2.0, mix(off_fill, on_fill, t));
    // off 描边 = ControlStrongStrokeColorDefault（禁用 ControlStrongStrokeColorDisabled）。
    let stroke_c = if enabled {
        p.control_strong_fill
    } else {
        p.control_strong_fill_disabled
    };
    ui.painter().rect_stroke(
        rect,
        rect.height() / 2.0,
        Stroke::new(1.0, mix(stroke_c, Color32::TRANSPARENT, t)),
        egui::StrokeKind::Inside,
    );
    // 拇指 = KnobFillOff（TextFillColorSecondary，禁用 TextFillColorDisabled）→
    // KnobFillOn（TextOnAccentFillColorPrimary：亮白 / 暗黑）。
    let knob_d = 12.0;
    let kx = rect.left() + 4.0 + knob_d / 2.0 + t * (rect.width() - 8.0 - knob_d);
    let knob_off = if enabled {
        p.text_secondary
    } else {
        p.text_disabled
    };
    ui.painter().circle_filled(
        Pos2::new(kx, rect.center().y),
        knob_d / 2.0,
        mix(knob_off, p.on_accent_fg, t),
    );
    enabled && resp.clicked()
}

/// 单选钮（WinUI RadioButton 自绘）：egui RadioButton 选中内点与文字共用
/// fg_stroke 令牌、Selection 样式无分色字段，无法点变蓝而文字不变 → 自绘。
/// 图标 20 圈：未选 = 1px ControlStrongStroke 透明底（悬停 ControlHover）；
/// 选中 = 外圈 + 内点标志蓝（2026-10-31 定案：选中更醒目，悬停/按下用
/// ACCENT_HOVER / ACCENT_PRESSED），文字距图标 8。
fn radio(
    ui: &mut egui::Ui,
    rect: Rect,
    checked: bool,
    label: &str,
    fs: f32,
    p: &theme::Palette,
) -> bool {
    let icon_d = 20.0;
    let icon = Rect::from_center_size(
        Pos2::new(rect.left() + icon_d / 2.0, rect.center().y),
        egui::vec2(icon_d, icon_d),
    );
    let resp = ui.allocate_rect(rect, Sense::click());
    let pt = ui.painter_at(rect);
    let accent = if resp.is_pointer_button_down_on() {
        theme::ACCENT_PRESSED
    } else if resp.hovered() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    // 外圈：选中 accent / 未选 ControlStrongStroke（悬停 ControlHover）。
    let ring_c = if checked {
        accent
    } else if resp.hovered() {
        p.control_hover
    } else {
        p.control_strong_fill
    };
    pt.circle_stroke(icon.center(), 9.5, Stroke::new(1.0, ring_c));
    if checked {
        pt.circle_filled(icon.center(), 4.0, accent);
    }
    pt.text(
        Pos2::new(rect.left() + icon_d + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(fs),
        p.text_primary,
    );
    resp.clicked()
}

/// 复选框 18×18（WinUI CheckBox：选中品牌蓝底白对勾，未选 1px 描边）。
fn checkbox(ui: &mut egui::Ui, center: Pos2, checked: bool, p: &theme::Palette) -> bool {
    let rect = Rect::from_center_size(center, egui::vec2(18.0, 18.0));
    let resp = ui.allocate_rect(rect, Sense::click());
    if checked {
        ui.painter().rect_filled(rect, 3.0, theme::ACCENT);
        let c = rect.center();
        let g = 4.5;
        let s = Stroke::new(1.6, Color32::WHITE);
        ui.painter().line_segment(
            [
                Pos2::new(c.x - g, c.y),
                Pos2::new(c.x - g * 0.3, c.y + g * 0.7),
            ],
            s,
        );
        ui.painter().line_segment(
            [
                Pos2::new(c.x - g * 0.3, c.y + g * 0.7),
                Pos2::new(c.x + g, c.y - g * 0.7),
            ],
            s,
        );
    } else {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 3.0, p.hover_bg);
        }
        ui.painter().rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, p.text_tertiary),
            egui::StrokeKind::Inside,
        );
    }
    resp.clicked()
}

// ── 重命名（320）────────────────────────────────────────────
fn rename_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (pane, orig) = match &st.dialog {
        Dialog::Rename { pane, orig, .. } => (*pane, orig.clone()),
        _ => return,
    };
    let mut name = match &st.dialog {
        Dialog::Rename { name, .. } => name.clone(),
        _ => return,
    };
    // 实时校验：Windows 非法字符 / 空 / 与原名相同（远程条目不限非法字符，T3 接入）。
    let illegal = name.chars().any(|c| "\\/:*?\"<>|".contains(c));
    let ok = !name.is_empty() && name != orig && !illegal;

    let Some(content) = shell(ui, st, p, 320.0, 144.0, t("重命名"), false, 0.0, true, None)
    else {
        return;
    };
    let mut cui = content_ui(ui, content);
    let w = content.width() - CONTENT_PAD.0 * 2.0;

    let te = egui::TextEdit::singleline(&mut name)
        .font(egui::TextStyle::Body)
        .desired_width(w)
        .min_size(egui::vec2(w, BTN_H));
    cui.put(
        Rect::from_min_size(
            Pos2::new(
                content.left() + CONTENT_PAD.0,
                content.top() + CONTENT_PAD.1,
            ),
            egui::vec2(w, BTN_H),
        ),
        te,
    );

    let btn_y = content.top() + CONTENT_PAD.1 + BTN_H + GROUP_GAP;
    // 确认钮：IsValidNewName 才启用（C# RenameInputDialog.xaml.cs:62 初始
    // IsEnabled=false、:99 校验通过启用）；禁用态点击不响应。
    let confirm_w = btn_w(ui, t("重命名"));
    let cancel_w = btn_w(ui, t("取消"));
    let bx = content.right() - CONTENT_PAD.0;
    let confirm = std_button_en(
        ui,
        Rect::from_min_size(
            Pos2::new(bx - confirm_w, btn_y),
            egui::vec2(confirm_w, BTN_H),
        ),
        t("重命名"),
        p.text_primary,
        p,
        14.0,
        ok,
    );
    let cancel = std_button(
        ui,
        Rect::from_min_size(
            Pos2::new(bx - confirm_w - 8.0 - cancel_w, btn_y),
            egui::vec2(cancel_w, BTN_H),
        ),
        t("取消"),
        p.text_primary,
        p,
    );
    let enter = ok && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if confirm || enter {
        apply_rename(st, pane, &name);
        st.dialog = Dialog::None;
        // 重命名帧末同步改 pane 条目（本帧已画完）→ 主动约一帧防断档。
        ui.ctx().request_repaint();
    } else if cancel {
        st.dialog = Dialog::None;
    } else if let Dialog::Rename { name: n, .. } = &mut st.dialog {
        *n = name;
    }
}

// ── pane → 会话侧映射（对称窗格，对齐 C# 每 tab 独立 FileView）──────────
/// 弹窗/菜单 pane（= 侧号）→ 会话侧：活动视图为远程 tab 时 Some(侧)，
/// 本地 tab 时 None（无会话语义）。两侧同款判定。
fn menu_side(st: &State, pane: usize) -> Option<usize> {
    (!super::pane_is_local(st, pane)).then_some(pane)
}

/// pane（= 侧号）当前视图：本地 tab = 该侧本地 pane；远程 tab = 该侧远程面板。
fn pane_view(st: &State, pane: usize) -> &super::panes::FilePane {
    super::pane_view(st, pane)
}

fn pane_view_mut(st: &mut State, pane: usize) -> &mut super::panes::FilePane {
    super::pane_view_mut(st, pane)
}

/// 对侧视图（传输目标）：pane（= 侧号）→ 另一侧当前视图。
fn other_view(st: &State, pane: usize) -> &super::panes::FilePane {
    super::pane_view(st, 1 - pane)
}

/// 传输就绪（C# CanTransferToOpposite 单侧口径）：本地视图要求路径非空；远程
/// 视图要求该侧活动远程 tab 已挂会话。
fn view_ready(st: &State, pane: usize) -> bool {
    match menu_side(st, pane) {
        Some(side) => super::active_remote_session(st, side).is_some(),
        None => pane_view(st, pane).path.is_some(),
    }
}

fn apply_rename(st: &mut State, pane: usize, name: &str) {
    let entries = pane_view_mut(st, pane);
    if let Some(&i) = entries.selected.first()
        && let Some(e) = entries.entries.get_mut(i)
    {
        e.name = name.to_owned();
    }
    // 重命名影响树中目录名 → 重建路径树（C# :1613-1617 → RefreshListKeepingPath
    // 首行 RefreshTreeIfVisible 同走）。
    crate::ui::panes::tree_mark_dirty(entries);
}

// ── 删除确认（320）──────────────────────────────────────────
fn delete_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Dialog::Delete { pane, indices } = &st.dialog else {
        return;
    };
    let entries = pane_view(st, *pane);
    let first = indices
        .first()
        .and_then(|i| entries.entries.get(*i))
        .map(|e| e.name.clone())
        .unwrap_or_default();
    let count = indices.len();

    let shown = if count > 1 {
        format!("{first} {}", tfmt("等 {0} 项", &[&count]))
    } else {
        first
    };
    let Some(content) = shell(ui, st, p, 320.0, 166.0, t("删除"), false, 0.0, true, None) else {
        return;
    };
    let cui = content_ui(ui, content);
    let x = content.left() + CONTENT_PAD.0;

    // 条目行：危险红删除图标 + 名称 50% 淡化。
    if let Some(img) = st.icons.get(ui.ctx(), "trash", p.critical, 16.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(
                Pos2::new(x + 8.0, content.top() + CONTENT_PAD.1 + 10.0),
                egui::vec2(16.0, 16.0),
            ),
        );
    }
    cui.painter().text(
        Pos2::new(x + 26.0, content.top() + CONTENT_PAD.1 + 10.0),
        Align2::LEFT_CENTER,
        shown,
        FontId::proportional(14.0),
        // Opacity=0.5 叠 TextFillColorPrimary（C# EntryName）。
        theme::dim50(p.text_primary),
    );

    let body_y = content.top() + CONTENT_PAD.1 + 20.0 + GROUP_GAP;
    cui.painter().text(
        Pos2::new(x, body_y + 10.0),
        Align2::LEFT_CENTER,
        t("所选内容将被永久删除，此操作无法恢复。"),
        FontId::proportional(14.0),
        p.text_primary,
    );

    // 删除（危险红字）+ 取消。
    let btn_y = body_y + 20.0 + GROUP_GAP;
    let del_w = btn_w(ui, t("删除"));
    let cancel_w = btn_w(ui, t("取消"));
    let del = danger_button(
        ui,
        Rect::from_min_size(
            Pos2::new(content.right() - CONTENT_PAD.0 - del_w, btn_y),
            egui::vec2(del_w, BTN_H),
        ),
        t("删除"),
        p,
    );
    let cancel = std_button(
        ui,
        Rect::from_min_size(
            Pos2::new(
                content.right() - CONTENT_PAD.0 - del_w - 8.0 - cancel_w,
                btn_y,
            ),
            egui::vec2(cancel_w, BTN_H),
        ),
        t("取消"),
        p.text_primary,
        p,
    );
    if del {
        apply_delete(st);
        // 弹窗绘制于帧末：本地删除同步改 pane（本帧已画完）→ 主动约一帧防断档；
        // 远程侧 busy 链（del_pending）本就保活，重复请求无害。
        ui.ctx().request_repaint();
    }
    if del || cancel {
        st.dialog = Dialog::None;
    }
}

fn apply_delete(st: &mut State) {
    let Dialog::Delete { pane, indices } = &st.dialog else {
        return;
    };
    let mut indices = indices.clone();
    indices.sort_unstable();
    indices.dedup();
    // 弹窗 pane（= 侧号）→ 会话侧：活动视图为远程 tab 时该侧远程会话删除，
    // 本地 tab 走本地删除分支。
    let Some(side) = menu_side(st, *pane) else {
        // 本地：真实删除（C5：逐项留终端日志，失败记录可定位路径）。
        let view = pane_view(st, *pane);
        let Some(dir) = view.path.clone() else {
            return; // 根视图（驱动器列表）不可删
        };
        let targets: Vec<String> = indices
            .iter()
            .filter_map(|i| view.entries.get(*i).map(|e| e.name.clone()))
            .collect();
        for name in targets {
            let p = dir.join(&name);
            let shown = p.to_string_lossy().into_owned();
            // Shell API 删除进回收站可还原（C# FileView.xaml.cs:4578「用 Shell
            // 而非 File.Delete」同语义；目录由 SHFileOperation 整树递归）。
            let r = crate::local::delete_to_bin(&p);
            match r {
                Ok(_) => st.terminal.push((
                    crate::local::now_hms(),
                    tfmt("[本地] 已删除 {0}", &[&shown]),
                )),
                Err(e) => st.terminal.push((
                    crate::local::now_hms(),
                    tfmt("[本地] 删除失败 {0}：{1}", &[&shown, &e]),
                )),
            }
        }
        // C# 删除后 RefreshListKeepingPath + RefreshTreeIfVisible → 重建路径树
        //（保留展开链，内部标记失效后由同步步恢复）。
        let view = pane_view_mut(st, *pane);
        crate::local::refresh(view);
        crate::ui::panes::tree_mark_dirty(view);
        return;
    };
    // 远程：Cmd::Delete（目录 SFTP 递归），逐项日志走 Ev::Log，完成计数回零后刷新。
    // 顺序借用：先收齐待删条目并定路径，再发号记 inflight（Ev::Deleted 按其定位
    // 重列 tab），最后借该 tab 挂靠会话发 delete。
    let Some(tab) = super::active_remote_tab(st, side) else {
        return;
    };
    let dir = tab.pane.remote_path.clone();
    let tab_id = tab.id;
    let targets: Vec<(String, bool)> = if dir.is_some() {
        indices
            .iter()
            .filter_map(|i| tab.pane.entries.get(*i).map(|e| (e.name.clone(), e.is_dir)))
            .collect()
    } else {
        Vec::new()
    };
    let sid = st.remotes[side]
        .tabs
        .iter()
        .find(|t| t.id == tab_id)
        .map(|t| t.sid);
    let has_session = sid.is_some_and(|sid| st.remotes[side].sessions.iter().any(|s| s.sid == sid));
    let paths: Vec<(String, bool)> = if has_session {
        targets
            .iter()
            .filter_map(|(name, is_dir)| {
                dir.as_deref()
                    .map(|d| (super::join_remote_path(d, name), *is_dir))
            })
            .collect()
    } else {
        Vec::new()
    };
    // 发号 + 在途记录（&mut st 独占段；tab 关闭时随 finish_close_tab 清理）。
    let tags: Vec<u64> = paths
        .iter()
        .map(|_| {
            let tag = super::next_seq(st);
            st.inflight.insert(tag, (side, tab_id));
            tag
        })
        .collect();
    let mut sent = 0usize;
    if let Some(ss) = super::session_of_tab(st, side, tab_id) {
        for ((path, is_dir), tag) in paths.iter().zip(tags.iter()) {
            ss.ssh.delete(*tag, path.clone(), *is_dir);
            sent += 1;
        }
    }
    if let Some(tab) = super::active_remote_tab_mut(st, side) {
        tab.pane.loading = sent > 0;
        tab.del_pending += sent as u64;
    }
}

// ── 校验码（420）────────────────────────────────────────────
const ALGOS: [&str; 3] = ["SHA-256", "SHA-512", "SM3"];

/// 本地文件哈希（ring，256KB 分块流式；SM3 暂不支持 —— C# 自带 SM3 实现随后续任务移植）。
fn hash_local(path: &str, algo: usize) -> Result<String, String> {
    use ring::digest as dg;
    let alg = match algo {
        0 => &dg::SHA256,
        1 => &dg::SHA512,
        _ => return Err(t("SM3 暂不支持").to_owned()),
    };
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut ctx = dg::Context::new(alg);
    let mut buf = vec![0u8; 256 * 1024];
    use std::io::Read;
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(ctx
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// 启动校验计算：本地开线程 ring 直算；远程走该侧 exec 通道
/// （sha256sum / sha512sum / SM3 经 openssl dgst，G9 S6.5）。
fn checksum_start(st: &mut State, algo: usize, path: &str, remote_side: Option<usize>) {
    use crate::ssh::shell_quote;
    if let Some(side) = remote_side {
        let cmd = match algo {
            0 => format!("sha256sum -- {}", shell_quote(path)),
            1 => format!("sha512sum -- {}", shell_quote(path)),
            // SM3：非 sm3sum（非通用命令），统一走 openssl（C# 同口径）。
            _ => format!("openssl dgst -sm3 {}", shell_quote(path)),
        };
        st.stat_seq += 1;
        let req = st.stat_seq;
        st.chk_pending = Some((req, algo));
        if let Some(ss) = super::active_remote_session(st, side) {
            ss.ssh.exec(req, cmd);
        }
        st.chk_local = None;
    } else {
        st.chk_pending = None;
        st.chk_local = None;
        st.chk_computing = true;
        let p = path.to_owned();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("checksum".into())
            .spawn(move || {
                let _ = tx.send(hash_local(&p, algo));
            })
            .ok();
        st.chk_rx = Some(rx);
    }
}

fn checksum_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (name, path, side, mut algo, mut copied) = match &st.dialog {
        Dialog::Checksum {
            name,
            path,
            side,
            algo,
            copied,
            ..
        } => (name.clone(), path.clone(), *side, *algo, *copied),
        _ => return,
    };
    let Some(content) = shell(ui, st, p, 420.0, 242.0, t("校验码"), false, 0.0, true, None)
    else {
        return;
    };
    let mut cui = content_ui(ui, content);
    let x = content.left() + CONTENT_PAD.0;
    let w = content.width() - CONTENT_PAD.0 * 2.0;
    let mut y = content.top() + CONTENT_PAD.1;

    // 条目行：文件图标 + 名称 50% 淡化。
    if let Some(img) = st.icons.get(ui.ctx(), "file", p.text_secondary, 16.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(Pos2::new(x + 8.0, y + 10.0), egui::vec2(16.0, 16.0)),
        );
    }
    cui.painter().text(
        Pos2::new(x + 26.0, y + 10.0),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(14.0),
        // Opacity=0.5 叠 TextFillColorPrimary（C# EntryName）。
        theme::dim50(p.text_primary),
    );
    y += 20.0 + GROUP_GAP;

    // 算法行：标签 + 下拉（选中即计算）。
    cui.painter().text(
        Pos2::new(x, y + 15.0),
        Align2::LEFT_CENTER,
        t("算法"),
        FontId::proportional(14.0),
        p.text_primary,
    );
    // 标签 Auto 宽 + ColumnSpacing 10（C# 算法行 Grid ColumnSpacing=10）。
    let algo_w = cui
        .painter()
        .layout_no_wrap(
            t("算法").to_owned(),
            FontId::proportional(14.0),
            Color32::WHITE,
        )
        .size()
        .x;
    let combo_r = Rect::from_min_size(Pos2::new(x + algo_w + 10.0, y), egui::vec2(160.0, 32.0));
    let mut combo_ui = cui.new_child(
        egui::UiBuilder::new()
            .max_rect(combo_r)
            .layout(Layout::top_down(Align::Min)),
    );
    // G9：远程按该侧服务端探测快照置灰不可用算法；本地 / 未探测全可用。
    let hp = side
        .and_then(|s| super::active_remote_session(st, s))
        .and_then(|s| s.ssh.hash_probe());
    let combo = egui::ComboBox::from_id_salt("chk-algo")
        .width(160.0 - 8.0)
        .selected_text(ALGOS.get(algo).copied().unwrap_or("SHA-256"));
    let resp = combo.show_ui(&mut combo_ui, |c| {
        for (i, label) in ALGOS.iter().enumerate() {
            let ok = hp.map_or(true, |h| match i {
                0 => h.sha256,
                1 => h.sha512,
                _ => h.sm3,
            });
            if ok {
                c.selectable_value(&mut algo, i, *label);
            } else {
                c.add_enabled(false, egui::Button::selectable(false, *label));
            }
        }
    });
    if resp.response.changed() {
        copied = None;
        checksum_start(st, algo, &path, side);
    }
    y += 32.0 + GROUP_GAP;

    // 码值行：只读等宽框（高 50 折行）+ 复制钮。
    // 计算态优先级：exec 在途 > 本地线程在途 > 结果（chk_local）> 空串。
    let value = if st.chk_pending.is_some() || st.chk_computing {
        String::new()
    } else {
        match &st.chk_local {
            Some(Ok(h)) => h.clone(),
            Some(Err(e)) => tfmt("计算失败：{0}", &[&e]),
            None => String::new(),
        }
    };
    let computing = st.chk_pending.is_some() || st.chk_computing;
    let copy_label = match copied {
        Some(t0) if ui.ctx().time() - t0 < 1.5 => t("已复制"),
        // 校验和复制钮对齐 C# ChkCopy="Copy"；主机菜单"复制"（Duplicate）仍走字典。
        _ => t_ctx("复制", "Copy"),
    };
    let copy_w = btn_w(ui, copy_label);
    // 码值框：TextBox 高 50 只读折行（ControlFill/Stroke 默认底描边），右侧复制钮间距 8。
    let value_r = Rect::from_min_size(Pos2::new(x, y), egui::vec2(w - copy_w - 8.0, 50.0));
    cui.painter().rect_filled(value_r, 4.0, p.control_bg);
    theme::elevation_border(
        cui.painter(),
        value_r,
        4.0,
        p.control_stroke_default,
        p.control_stroke,
    );
    let galley = cui.painter().layout(
        if computing {
            t("计算中…").to_owned()
        } else {
            value.clone()
        },
        egui::FontId::monospace(12.0),
        p.text_primary,
        value_r.width() - 16.0,
    );
    cui.painter().galley(
        Pos2::new(value_r.left() + 8.0, value_r.top() + 8.0),
        galley,
        p.text_primary,
    );
    // 复制钮（高 50）：算完才启用（C# ChecksumDialog.xaml.cs:91/103）。
    let copy_ok = !computing && matches!(&st.chk_local, Some(Ok(_)));
    if std_button_en(
        ui,
        Rect::from_min_size(Pos2::new(x + w - copy_w, y), egui::vec2(copy_w, 50.0)),
        copy_label,
        p.text_primary,
        p,
        14.0,
        copy_ok,
    ) {
        ui.ctx().copy_text(value.to_owned());
        copied = Some(ui.ctx().time());
    }
    y += 50.0 + GROUP_GAP;

    // 底部操作：关闭。
    if button_row(ui, content.right() - CONTENT_PAD.0, y, &[t("关闭")], p) {
        st.dialog = Dialog::None;
        return;
    }
    if let Dialog::Checksum {
        algo: a, copied: c, ..
    } = &mut st.dialog
    {
        *a = algo;
        *c = copied;
    }
}

// ── 权限（384 九宫格）───────────────────────────────────────
fn permission_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (pane, name, is_dir, mut bits) = match &st.dialog {
        Dialog::Permission {
            pane,
            name,
            is_dir,
            bits,
            ..
        } => (*pane, name.clone(), *is_dir, *bits),
        _ => return,
    };
    let Some(content) = shell(ui, st, p, 384.0, 303.0, t("权限"), false, 0.0, true, None) else {
        return;
    };
    let cui = content_ui(ui, content);
    let x = content.left() + CONTENT_PAD.0;
    let mut y = content.top() + CONTENT_PAD.1;

    // 条目行：类型图标 + 名称 50% 淡化。
    let icon = if is_dir { "folder" } else { "file" };
    if let Some(img) = st.icons.get(ui.ctx(), icon, p.text_secondary, 16.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(Pos2::new(x + 8.0, y + 10.0), egui::vec2(16.0, 16.0)),
        );
    }
    cui.painter().text(
        Pos2::new(x + 26.0, y + 10.0),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(14.0),
        // Opacity=0.5 叠 TextFillColorPrimary（C# EntryName）。
        theme::dim50(p.text_primary),
    );
    y += 20.0 + GROUP_GAP;

    // 九宫格：行=属主/属组/其他，列=读/写/执行（C# Grid 列 */72/72/72、RowSpacing=4：
    // 表头 14pt 行高 19，行块 32 高 + 4 间距 → 行距 36）。
    let rows = [t("所有者"), t("群组"), t("其他")];
    let cols = [t("读"), t("写"), t("执行")];
    let grid_w = content.width() - CONTENT_PAD.0 * 2.0;
    let cb_cx0 = x + grid_w - 180.0; // 三个 72 列的左缘（* 列填充余宽）
    for (ci, label) in cols.iter().enumerate() {
        let cx = cb_cx0 + 36.0 + ci as f32 * 72.0;
        cui.painter().text(
            Pos2::new(cx, y + 9.5),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(14.0),
            p.text_secondary,
        );
    }
    for (ri, row) in rows.iter().enumerate() {
        let row_y = y + 19.0 + 4.0 + 16.0 + ri as f32 * 36.0;
        cui.painter().text(
            Pos2::new(x, row_y),
            Align2::LEFT_CENTER,
            *row,
            FontId::proportional(14.0),
            p.text_primary,
        );
        for (ci, _) in cols.iter().enumerate() {
            let cx = cb_cx0 + 36.0 + ci as f32 * 72.0;
            let idx = ri * 3 + ci;
            if checkbox(ui, Pos2::new(cx, row_y), bits[idx], p) {
                bits[idx] = !bits[idx];
            }
        }
    }
    y += 19.0 + 4.0 + 3.0 * 36.0 - 4.0 + GROUP_GAP;

    // 当前值：八进制 + 符号串（实时重算）。
    let value = perm_value(&bits, is_dir);
    cui.painter().text(
        Pos2::new(content.center().x, y + 8.0),
        Align2::CENTER_CENTER,
        value.as_str(),
        FontId::proportional(12.0),
        p.text_secondary,
    );
    y += 16.0 + GROUP_GAP;

    // 操作按钮：应用 / 取消。
    let apply_y = y;
    let apply_w = btn_w(ui, t("应用"));
    let cancel_w = btn_w(ui, t("取消"));
    let apply = std_button(
        ui,
        Rect::from_min_size(
            Pos2::new(content.right() - CONTENT_PAD.0 - apply_w, apply_y),
            egui::vec2(apply_w, BTN_H),
        ),
        t("应用"),
        p.text_primary,
        p,
    );
    let cancel = std_button(
        ui,
        Rect::from_min_size(
            Pos2::new(
                content.right() - CONTENT_PAD.0 - apply_w - 8.0 - cancel_w,
                apply_y,
            ),
            egui::vec2(cancel_w, BTN_H),
        ),
        t("取消"),
        p.text_primary,
        p,
    );
    if apply {
        apply_perm(st, pane, &value);
    }
    if apply || cancel {
        st.dialog = Dialog::None;
        return;
    }
    if let Dialog::Permission { bits: b, .. } = &mut st.dialog {
        *b = bits;
    }
}

fn perm_value(bits: &[bool; 9], is_dir: bool) -> String {
    let mut oct = String::new();
    let mut sym = String::new();
    sym.push(if is_dir { 'd' } else { '-' });
    for grp in bits.chunks(3) {
        let mut n = 0;
        for (i, on) in grp.iter().enumerate() {
            if *on {
                n |= 4 >> i;
            }
        }
        oct.push_str(&n.to_string());
        sym.push(if grp[0] { 'r' } else { '-' });
        sym.push(if grp[1] { 'w' } else { '-' });
        sym.push(if grp[2] { 'x' } else { '-' });
    }
    format!("{oct} ({sym})")
}

fn apply_perm(st: &mut State, pane: usize, value: &str) {
    let sym = value
        .split('(')
        .nth(1)
        .unwrap_or("")
        .trim_end_matches(')')
        .to_owned();
    let entries = pane_view_mut(st, pane);
    if let Some(&i) = entries.selected.first()
        && let Some(e) = entries.entries.get_mut(i)
    {
        e.perms = sym;
    }
}

// ── 打开内容选择器（520×613）────────────────────────────────

/// 主机区快照（HostStoreService.Hosts 同口径：名称 + user@host 副标题）。
fn host_list_snapshot() -> Vec<crate::hosts::HostProfile> {
    crate::hosts::with(|hs| hs.to_vec())
}

fn host_picker(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (target, new_tab) = match &st.dialog {
        Dialog::HostPicker { target, new_tab } => (*target, *new_tab),
        _ => return,
    };
    let hosts = host_list_snapshot();
    // 滚动区高：C# ScrollViewer MinHeight 440 / MaxHeight 540，行高 48；
    // 卡高 = 壳 42 + 区头 30 + 本地行 48 + 分隔 13 + 区头 32 + list_h
    // （C# HostPickerPanel 无「新建连接」行，对齐后 Rust 同构）。
    // 两入口（空态按钮 / 两侧 + 号）恒绘本地区段（C# HostPickerPanel.OpenFor
    // 与来源窗格无关，恒含 LocalSection；右侧「本地」落位 = 对称窗格本地 tab）。
    let list_h = if hosts.is_empty() {
        440.0
    } else {
        (hosts.len() as f32 * 48.0).clamp(440.0, 540.0)
    };
    let shell_h = 165.0 + list_h;
    let Some(content) = shell(
        ui,
        st,
        p,
        520.0,
        shell_h,
        t("打开目标"),
        true,
        0.0,
        true,
        None,
    ) else {
        return;
    };
    let cui = content_ui(ui, content);
    let pad_x = 16.0;
    let mut y = content.top();

    // 本地区段（C# LocalSection：区头 + 本地单行 + 「浏览本地文件」尾注）。
    let local_clicked = {
        // 本地区头（C# 区头 Margin 16,10,16,4，12pt 行高 16 → 占 30）。
        cui.painter().text(
            Pos2::new(content.left() + pad_x, y + 10.0 + 8.0),
            Align2::LEFT_CENTER,
            t("本地"),
            FontId::proportional(12.0),
            p.text_secondary,
        );
        y += 30.0;

        // 本地单行（ListViewItem 全宽、行高 48；hover = ListViewItemBackgroundPointerOver
        // = SubtleFillColorSecondary）。
        let local_row = Rect::from_min_max(
            Pos2::new(content.left(), y),
            Pos2::new(content.right(), y + 48.0),
        );
        let local_resp = ui.allocate_rect(local_row, Sense::click());
        if local_resp.hovered() {
            ui.painter().rect_filled(local_row, 4.0, p.row_hover);
        }
        if let Some(img) = st.icons.get(ui.ctx(), "desktop", p.text_primary, 18.0) {
            img.paint_at(
                ui,
                Rect::from_center_size(
                    Pos2::new(local_row.left() + 19.0, local_row.center().y),
                    egui::vec2(18.0, 18.0),
                ),
            );
        }
        cui.painter().text(
            Pos2::new(local_row.left() + 40.0, local_row.center().y),
            Align2::LEFT_CENTER,
            t("本地文件系统"),
            FontId::proportional(14.0),
            p.text_primary,
        );
        cui.painter().text(
            Pos2::new(local_row.right() - 10.0, local_row.center().y),
            Align2::RIGHT_CENTER,
            t("浏览本地文件"),
            FontId::proportional(12.0),
            p.text_secondary,
        );
        y += 48.0;
        local_resp.clicked()
    };

    // 分隔线 1px（C# Rectangle Height=1 Margin 16,6：线上下各 6）。
    y += 6.0;
    ui.painter().line_segment(
        [
            Pos2::new(content.left() + pad_x, y),
            Pos2::new(content.right() - pad_x, y),
        ],
        Stroke::new(1.0, p.row_separator),
    );
    y += 6.0;

    // 主机区头（C# 区头 Margin 16,12,16,4 → 占 32）。
    cui.painter().text(
        Pos2::new(content.left() + pad_x, y + 12.0 + 8.0),
        Align2::LEFT_CENTER,
        t_ctx("主机", "Hosts"),
        FontId::proportional(12.0),
        p.text_secondary,
    );
    y += 32.0;

    // 主机列表（440~540 高，超出滚动；空档提示 —— C# EmptyHint Margin 16,4,16,14）。
    let list = Rect::from_min_max(
        Pos2::new(content.left(), y),
        Pos2::new(content.right(), y + list_h),
    );
    let mut picked: Option<usize> = None;
    let mut list_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(list)
            .layout(Layout::top_down(Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("host-picker-list")
        .show(&mut list_ui, |s| {
            if hosts.is_empty() {
                s.painter().text(
                    Pos2::new(content.left() + 16.0, y + 4.0 + 8.0),
                    Align2::LEFT_CENTER,
                    t("暂无主机，点 + 新增"),
                    FontId::proportional(12.0),
                    p.text_secondary,
                );
                return;
            }
            for (i, h) in hosts.iter().enumerate() {
                // ListViewItem 全宽、行高 48（Padding 10,10 → 内容高 28）。
                let row = Rect::from_min_max(
                    Pos2::new(content.left(), y + i as f32 * 48.0),
                    Pos2::new(content.right(), y + (i + 1) as f32 * 48.0),
                );
                let resp = s.allocate_rect(row, Sense::click());
                if resp.hovered() {
                    s.painter().rect_filled(row, 4.0, p.row_hover);
                }
                // 橙色圆角图标块 28（Ubuntu 橙 + 白图标，theme::UBUNTU）。
                let logo = Rect::from_center_size(
                    Pos2::new(row.left() + 24.0, row.center().y),
                    egui::vec2(28.0, 28.0),
                );
                s.painter().rect_filled(logo, 6.0, theme::UBUNTU);
                if let Some(img) = st.icons.get(s.ctx(), "desktop-tower", Color32::WHITE, 13.0) {
                    img.paint_at(
                        s,
                        Rect::from_center_size(logo.center(), egui::vec2(13.0, 13.0)),
                    );
                }
                let title = if h.name.is_empty() {
                    h.host.clone()
                } else {
                    h.name.clone()
                };
                s.painter().text(
                    Pos2::new(row.left() + 50.0, row.center().y),
                    Align2::LEFT_CENTER,
                    title,
                    FontId::proportional(14.0),
                    p.text_primary,
                );
                s.painter().text(
                    Pos2::new(row.right() - 10.0, row.center().y),
                    Align2::RIGHT_CENTER,
                    h.detail(),
                    FontId::proportional(12.0),
                    p.text_secondary,
                );
                if resp.clicked() {
                    picked = Some(i);
                }
            }
        });

    if local_clicked {
        open_target(st, target, new_tab);
        st.dialog = Dialog::None;
    } else if let Some(i) = picked {
        // 选主机即连（C# HostPicked → ConnectToHostAsync，不经编辑弹窗）；
        // 加号入口 new_tab 强制新标签页（C# TabIntent="new"），空态按钮就地。
        // 连接落发起侧窗格（左 + 号 → 侧 0，右 + 号 → 侧 1，可同主机双会话）。
        let h = hosts[i].clone();
        st.dialog = Dialog::None;
        connect_profile(st, &h, target, new_tab);
    }
}

/// 连接提交共用入口（HostPicker 选主机 / HostPanel 卡片 / 菜单重连）：
/// tab 落定（追加 / 就地激活）→ 为目标 tab 建新会话并绑定（bind_session 发 Connect）。
/// 连接结果经 Ev::Connected / Ev::Failed 回流（root() apply_ssh_events 按 sid 路由到
/// 挂靠 tab）。`side` = 连接落位侧（0 左 / 1 右；两侧可各自连接同一主机，独立会话）。
/// `new_tab`（tab 栏加号入口）：追加 tab 而非就地激活（C# HostPicked 强制
/// TabIntent="new" 新标签页）。P1 不做同主机共享会话（P2 池复用）：同主机第二
/// tab 也 spawn 新 Ssh。
pub(crate) fn connect_profile(
    st: &mut State,
    h: &crate::hosts::HostProfile,
    side: usize,
    new_tab: bool,
) {
    let label = host_tab_naming(&h.name, &h.host);
    // L4：端口显式校验（不再静默钳成 0 —— 0 端口必连不上且无任何提示）。
    if !(1..=65535).contains(&h.port) {
        st.terminal.push((
            crate::local::now_hms(),
            format!(
                "[{label}] {}",
                tfmt("端口无效：{0}（需 1-65535），连接未发起", &[&h.port])
            ),
        ));
        return;
    }
    // G17-ANIM：C# 标签先行即 AddNewTabGetView → TabItems.Add 播系统亮相动画。
    // 加号入口（new_tab）或该侧尚无远程 tab → 追加远程 tab（亮相恒播）；否则就地
    // 激活既有远程 tab 原位换标题（活动远程 tab 优先，无则首个——现状
    // position(!local) 口径；右侧重连播亮相，左侧 tab 常显不重播）。
    let tab_id = if new_tab || st.remotes[side].tabs.is_empty() {
        let id = st.next_tab_id;
        st.next_tab_id += 1;
        st.remotes[side].tabs.push(super::RemoteTab {
            id,
            sid: 0,
            title: label.clone(),
            pane: super::panes::FilePane {
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
        });
        activate_remote_tab(st, side, id);
        st.tab_born[side] = Some((id, std::time::Instant::now()));
        id
    } else {
        let tid = match st.remotes[side].active {
            Some(a) if st.remotes[side].tabs.iter().any(|t| t.id == a) => a,
            _ => st.remotes[side].tabs[0].id,
        };
        activate_remote_tab(st, side, tid);
        // 右侧空态占位 tab 就地改名 = 空态引导消失、标签条首现，补亮相动画
        //（左侧重连场景 tab 常显，不重播）。
        if side == 1 {
            st.tab_born[side] = Some((tid, std::time::Instant::now()));
        }
        tid
    };
    // P2 会话池复用（SPEC §3.1）：追加 tab 分支先按 SessionKey{host,port,user}
    // 查该侧会话池——命中即复用既有会话（refs+=1，不 spawn / 不重发 Connect，
    // 指纹确认等随之天然单次）；未命中走「断旧建新」。就地重连（new_tab=false
    // 且该侧已有 tab）维持重连语义，不复用。
    if new_tab || st.remotes[side].tabs.is_empty() {
        let key = super::SessionKey {
            host: h.host.clone(),
            port: h.port as u16,
            user: h.username.clone(),
        };
        if let Some(sid) = st.remotes[side]
            .sessions
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.sid)
        {
            reuse_session(st, side, tab_id, sid, h, &label);
            return;
        }
    }
    bind_session(st, side, tab_id, h, &label);
}

/// P2 会话复用落位：新 tab 指向既有会话（refs+=1，不 spawn / 不重发 Connect）。
/// 会话已连接 → 直接走「初始目录列目录」（对照 Ev::Connected 优先级，抽
/// initial_remote_dir 与其共用）+ 同步挂接重挂；会话仍在连接中（含指纹待确认）
/// → 空态等待，由 Ev::Connected 按 sid 统一落位——复用会话不会二次触发
/// AskFingerprint（覆盖层遍历会话池绘制既有请求）。
fn reuse_session(
    st: &mut State,
    side: usize,
    tab_id: u32,
    sid: u64,
    h: &crate::hosts::HostProfile,
    label: &str,
) {
    let Some(si) = st.remotes[side].sessions.iter().position(|s| s.sid == sid) else {
        return;
    };
    st.remotes[side].sessions[si].refs += 1;
    // host/profile 不覆盖（同 key 语义相同，保留首个）；原会话无档案则补上
    //（初始目录优先级 / 浏览记忆落盘需要）。
    if st.remotes[side].sessions[si].host.is_none() {
        st.remotes[side].sessions[si].host = Some(h.clone());
    }
    let connected = matches!(st.remotes[side].sessions[si].state, super::RemoteState::Off);
    // tab 绑定 + 浏览态复位（与 bind_session 同口径）。
    if let Some(tab) = st.remotes[side].tabs.iter_mut().find(|t| t.id == tab_id) {
        tab.sid = sid;
        tab.title = label.to_owned();
        tab.fallback = None;
        tab.del_pending = 0;
        tab.new_pending = 0;
        tab.pending_ed = None;
        tab.ext_edits.clear();
        tab.ext_stat = None;
        tab.ext_write = None;
        tab.ext_poll = std::time::Instant::now();
        tab.wait = None;
        tab.pane = if connected {
            super::panes::FilePane::default()
        } else {
            // 会话仍在连接中：等待 Ev::Connected 按 sid 统一落位。
            super::panes::FilePane {
                empty: true,
                ..Default::default()
            }
        };
    }
    if connected {
        // 已连接会话：直接初始列目录（复用路径没有 Connected 事件）。
        let root = st.remotes[side].sessions[si]
            .root
            .clone()
            .unwrap_or_else(|| "/".to_owned());
        let (init, fallback, remote_base) = super::initial_remote_dir(h, &root);
        let ltag = super::next_seq(st);
        if let Some(tab) = st.remotes[side].tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.pane = super::panes::FilePane {
                remote_path: Some(init.clone()),
                ..Default::default()
            };
            tab.wait = Some((ltag, init.clone()));
            tab.pane.loading = true;
            tab.fallback = fallback;
        }
        if let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) {
            ss.ssh.list(ltag, init);
        }
        // 同步挂接重挂（对照 Ev::Connected 尾部；C# 每次连接提交重组联对面）。
        super::sync::attach_connect(st, side, sid, remote_base);
    }
}

/// 为既有远程 tab 建新会话并绑定：旧会话 refs-1（归零断开 + 出池，C# 重连 =
/// 换会话；两侧可同主机——独立实例），新会话注入传输/exec 档案发 Connect；
/// tab 浏览态复位（原 RemoteCtx::default() 平移到 tab 字段）+ 换标题 + 激活；
/// 会话重建即脱离联动面（同步浏览挂接由 Ev::Connected 重挂）。
fn bind_session(
    st: &mut State,
    side: usize,
    tab_id: u32,
    h: &crate::hosts::HostProfile,
    label: &str,
) {
    let port = h.port as u16;
    let auth = match h.auth_mode {
        crate::hosts::AuthMode::KeyFile => crate::ssh::Auth::KeyFile(h.key_file_path.clone()),
        crate::hosts::AuthMode::Password => crate::ssh::Auth::Password(h.password.clone()),
    };
    // 存档指纹（C# SetTrustedFingerprint 时机；空串归 None）。
    let fp = (!h.host_key_fingerprint.trim().is_empty()).then(|| h.host_key_fingerprint.clone());
    // 旧会话释放（P1 每 tab 一会话：refs 1→0 即断开移除）。
    let old_sid = st.remotes[side]
        .tabs
        .iter()
        .find(|t| t.id == tab_id)
        .map(|t| t.sid);
    if let Some(osid) = old_sid
        && let Some(si) = st.remotes[side].sessions.iter().position(|s| s.sid == osid)
    {
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
    // 新会话（P1 每 tab 一会话，行为等同现状；P2 同主机共享复用）。
    let sid = st.next_sid;
    st.next_sid += 1;
    let ssh = crate::ssh::Ssh::spawn();
    // exec 通道（校验码等）独立连接用同一档案（实例级档案，随该会话）。
    ssh.set_exec_profile(Some(crate::ssh::ExecSpec {
        host: h.host.clone(),
        port,
        user: h.username.clone(),
        auth: auth.clone(),
        fingerprint: fp.clone(),
    }));
    ssh.connect(
        h.host.clone(),
        port,
        h.username.clone(),
        auth.clone(),
        label.to_owned(),
        fp.clone(),
    );
    let key = super::SessionKey {
        host: h.host.clone(),
        port,
        user: h.username.clone(),
    };
    st.remotes[side].sessions.push(super::SideSession {
        sid,
        key,
        ssh,
        // 会话主机档案：初始目录优先级 / 浏览记忆落盘（LastRemotePath）用。
        host: Some(h.clone()),
        // 传输档案：G16 静默门用存档指纹（G12 校验改读该 tab 会话探测快照实时
        // 句柄，见 ui/mod.rs enqueue_tasks；连接提交时快照会错过首次探测）。
        profile: Some(crate::transfer::Profile {
            host: h.host.clone(),
            port,
            user: h.username.clone(),
            auth,
            fingerprint: fp,
        }),
        state: super::RemoteState::Connecting,
        root: None,
        fp_ask: None,
        refs: 1,
    });
    // tab 绑定 + 浏览态复位 + 该侧远程面板切「正在连接」空态。
    if let Some(tab) = st.remotes[side].tabs.iter_mut().find(|t| t.id == tab_id) {
        tab.sid = sid;
        tab.pane = super::panes::FilePane {
            empty: true,
            ..Default::default()
        };
        tab.wait = None;
        tab.fallback = None;
        tab.del_pending = 0;
        tab.new_pending = 0;
        tab.pending_ed = None;
        tab.ext_edits.clear();
        tab.ext_stat = None;
        tab.ext_write = None;
        tab.ext_poll = std::time::Instant::now();
        tab.title = label.to_owned();
    }
    // 激活该 tab + 会话重建即脱离联动面（同步浏览挂接由 Ev::Connected 重挂；
    // 对称窗格任一侧连接均可能重组联对面）。
    activate_remote_tab(st, side, tab_id);
    st.sync_browse = false;
    st.sync_base = None;
    st.sync_mirror = None;
}

/// 激活某侧指定远程 tab（本地活动让位；侧内唯一活动）。
fn activate_remote_tab(st: &mut State, side: usize, tab_id: u32) {
    st.locals[side].active = None;
    st.remotes[side].active = Some(tab_id);
}

/// 激活某侧指定本地 tab。
fn activate_local_tab(st: &mut State, side: usize, tab_id: u32) {
    st.remotes[side].active = None;
    st.locals[side].active = Some(tab_id);
}

/// 队列菜单「重连」：断开会话所属远程 tab 原位重建会话（connect_profile 就地
/// 路径同款：标题刷新 + 激活 + 右侧重播亮相）。
pub(crate) fn reconnect_tab(
    st: &mut State,
    side: usize,
    tab_id: u32,
    h: &crate::hosts::HostProfile,
) {
    if st.remotes[side].tabs.iter().any(|t| t.id == tab_id) {
        let label = host_tab_naming(&h.name, &h.host);
        activate_remote_tab(st, side, tab_id);
        if side == 1 {
            st.tab_born[side] = Some((tab_id, std::time::Instant::now()));
        }
        bind_session(st, side, tab_id, h, &label);
    }
}

/// 选择结果落到发起侧窗格（本地 → 此电脑根视图）。`target` = 落位侧（0 左 /
/// 1 右）。`new_tab`（C# OpenLocalView 非空态 → AddNewTab）追加本地 tab；false =
/// 空态引导就地打开（C# DismissEmptyState）：该侧本地 tabs 重置为单 tab，被
/// discard 的远程 tab 走会话清理（refs 归零断开 + 出池）。
/// （P4：本地 tab 标题由 tab 条合成处按当前目录现算，此函数不再落标题。）
fn open_target(st: &mut State, target: usize, new_tab: bool) {
    let mut pane = super::panes::FilePane::default();
    crate::local::navigate_root(&mut pane);
    if new_tab {
        // C# OpenLocalView 非空态 AddNewTab：追加本地 tab + 亮相。
        let id = st.next_tab_id;
        st.next_tab_id += 1;
        st.locals[target].tabs.push(super::LocalTab { id, pane });
        activate_local_tab(st, target, id);
        st.tab_born[target] = Some((id, std::time::Instant::now()));
    } else {
        // C# DismissEmptyState 就地打开：被 discard 的远程 tab 走 finish_close_tab
        // 同款会话清理（无确认门——空态引导覆盖时无可见 tab 条）。
        let rids: Vec<u32> = st.remotes[target].tabs.iter().map(|t| t.id).collect();
        for rid in rids {
            discard_remote_tab(st, target, rid);
        }
        let id = st.next_tab_id;
        st.next_tab_id += 1;
        st.locals[target].tabs = vec![super::LocalTab { id, pane }];
        activate_local_tab(st, target, id);
        // G17-ANIM：C# 承载此目标的 tab 同走 TabItems.Add → 系统亮相动画。
        st.tab_born[target] = Some((id, std::time::Instant::now()));
    }
}

/// 无确认门直接弃置一个远程 tab（open_target 就地路径专用）：挂靠会话 refs-1
/// 归零断开 + 出池（C# view.Dispose），tab 移除、活动项让位。
fn discard_remote_tab(st: &mut State, side: usize, tab_id: u32) {
    let sid = st.remotes[side]
        .tabs
        .iter()
        .find(|t| t.id == tab_id)
        .map(|t| t.sid);
    if let Some(sid) = sid
        && let Some(si) = st.remotes[side].sessions.iter().position(|s| s.sid == sid)
    {
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
    st.remotes[side].tabs.retain(|t| t.id != tab_id);
    if st.remotes[side].active == Some(tab_id) {
        st.remotes[side].active = None;
    }
}

// ── WinUI 输入框（textbox.xaml 默认模板口径）───────────────
/// WinUI TextBox 自绘容器（textbox.xaml 默认模板口径）：圆角 4（ControlCornerRadius）、
/// 边 1px ControlStrokeColorDefault（经 theme::control_border——2026-10-31 定案
/// 输入框/按钮全局去边框）、
/// 悬停底 ControlFillColorSecondary、聚焦底 ControlFillColorInputActive + 底部 2px
/// accent 下划线（TextControlElevationBorderFocusedBrush 渐变的单色近似）、
/// 禁用 ControlFillColorDisabled。文字区组件内显式缩进左右 10
/// （TextControlThemePadding），内嵌无边框 TextEdit，文字垂直居中；
/// 聚焦且非空时右侧显示 DeleteButton 清除钮（点击清空）。
/// 焦点/悬停取上一帧响应态（首帧按静默态渲染，无感知）。
pub(crate) fn winui_text_box(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    val: &mut String,
    secret: bool,
    fs: f32,
    enabled: bool,
    p: &theme::Palette,
) {
    // 描边像素对齐（theme::snap_rect）：半像素坐标会让 1px 边线被抗锯齿摊薄。
    let rect = theme::snap_rect(rect, ui.ctx().pixels_per_point());
    let (focus, hover) = ui
        .ctx()
        .read_response(id)
        .map_or((false, false), |r| (r.has_focus(), r.hovered()));
    let bg = if !enabled {
        p.control_disabled
    } else if focus {
        p.control_bg_focused
    } else if hover {
        p.control_hover
    } else {
        p.control_bg
    };
    let pt = ui.painter_at(rect);
    pt.rect_filled(rect, 4.0, bg);
    // 描边（C# ControlElevationBorderBrush 渐变简化：顶/左/右淡线 + 底重线；
    // 聚焦底边由 2px accent 下划线接管 → 全圈淡线；禁用单色 disabled）。
    // 线宽 1 物理像素（暗主题淡白线被抗锯齿摊薄会整条不可见）。
    let bw = theme::border_width(ui.ctx().pixels_per_point());
    if !enabled {
        pt.rect_stroke(
            rect,
            4.0,
            Stroke::new(bw, p.control_stroke_disabled),
            egui::StrokeKind::Inside,
        );
    } else if focus {
        pt.rect_stroke(
            rect,
            4.0,
            Stroke::new(bw, p.control_stroke_default),
            egui::StrokeKind::Inside,
        );
    } else {
        theme::elevation_border(&pt, rect, 4.0, p.control_stroke_default, p.control_stroke);
    }
    if enabled && focus {
        pt.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left() + 1.0, rect.bottom() - 2.0),
                Pos2::new(rect.right() - 1.0, rect.bottom()),
            ),
            0.0,
            theme::ACCENT,
        );
    }
    // 清除按钮（WinUI TextBox DeleteButton 口径：聚焦且非空时右侧 20×20 圆钮、
    // 右缩进 4，✗ 两段 1.5px 线；点击清空内容）。出现时文字区右侧让位 24。
    let has_clear = enabled && focus && !val.is_empty();
    if has_clear {
        let clear_r = Rect::from_center_size(
            Pos2::new(rect.right() - 14.0, rect.center().y),
            egui::vec2(20.0, 20.0),
        );
        let cr = ui.interact(clear_r, id.with("clear"), Sense::click());
        let cx = clear_r.center();
        if cr.hovered() {
            pt.circle_filled(cx, 10.0, p.control_hover);
        }
        let g = 3.5;
        let s = Stroke::new(1.5, p.text_secondary);
        pt.line_segment(
            [Pos2::new(cx.x - g, cx.y - g), Pos2::new(cx.x + g, cx.y + g)],
            s,
        );
        pt.line_segment(
            [Pos2::new(cx.x - g, cx.y + g), Pos2::new(cx.x + g, cx.y - g)],
            s,
        );
        if cr.clicked() {
            val.clear();
        }
    }
    // 文字区由组件显式缩进左右各 10（TextControlThemePadding，不依赖 TextEdit
    // margin 的 frame 叠加语义），清除钮出现时右侧再让 24。
    let text_r = Rect::from_min_max(
        Pos2::new(rect.left() + 10.0, rect.top()),
        Pos2::new(
            rect.right() - 10.0 - if has_clear { 24.0 } else { 0.0 },
            rect.bottom(),
        ),
    );
    let te = egui::TextEdit::singleline(val)
        .id(id)
        .password(secret)
        .frame(egui::Frame::NONE)
        .font(FontId::proportional(fs))
        .margin(Margin::same(0))
        .align(Align2::LEFT_CENTER)
        .desired_width(text_r.width())
        .min_size(text_r.size());
    let mut c = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_r)
            .layout(Layout::top_down(Align::Min)),
    );
    c.add_enabled(enabled, te);
}

// ── 设置（920×640，侧栏 150 + 分区卡）───────────────────────
const CATS: [&str; 6] = ["外观", "连接", "文件编辑", "传输", "注册", "关于"];

fn settings_dialog(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    win_drag: &mut Vec2,
) -> bool {
    let Some(content) = shell(
        ui,
        st,
        p,
        920.0,
        640.0,
        t("设置"),
        true,
        4.0,
        false,
        Some(win_drag),
    ) else {
        return false;
    };
    let th = ui.ctx().theme();
    // C# SettingsPanel 主体 Grid Margin 5 已含在 shell 的 content 几何（与 HostPanel
    // 同构、无统一容器），body 不再二次缩边，左右子卡直接挂。
    let body = content;

    // 左卡片：大类导航（C# 左列宽 150；钮 Padding 14,9 / FontSize 13 / 圆角 4 → 高≈35）。
    // SetCategoryState（SettingsPanel.xaml.cs）：选中 = nav_selected_bg 底 + tab_active_fg 字，
    // 不响应 hover；未选中 = nav_rest_bg 块底（亮主题透明）+ text_primary，按下走 pressed_bg。
    let nav = Rect::from_min_max(body.min, Pos2::new(body.left() + 150.0, body.bottom()));
    ui.painter().rect_filled(nav, 6.0, card_bg(th));
    let mut y = nav.top() + 8.0;
    for (i, cat) in CATS.iter().enumerate() {
        let item = Rect::from_min_size(
            Pos2::new(nav.left() + 8.0, y),
            egui::vec2(nav.width() - 16.0, 35.0),
        );
        let resp = ui.allocate_rect(item, Sense::click());
        let active = i == st.settings.category;
        if active {
            ui.painter().rect_filled(item, 4.0, p.nav_selected_bg);
        } else {
            let bg = if resp.is_pointer_button_down_on() {
                p.pressed_bg
            } else if resp.hovered() {
                p.hover_bg
            } else {
                p.nav_rest_bg
            };
            ui.painter().rect_filled(item, 4.0, bg);
        }
        ui.painter().text(
            Pos2::new(item.left() + 14.0, item.center().y),
            Align2::LEFT_CENTER,
            t(*cat),
            FontId::proportional(13.0),
            if active {
                p.tab_active_fg
            } else {
                p.text_primary
            },
        );
        if resp.clicked() {
            st.settings.category = i;
        }
        y += item.height() + 4.0;
    }

    // 右卡片：设置页（内容边距 20 / 滚动）。
    let page = Rect::from_min_max(Pos2::new(nav.right() + 5.0, body.top()), body.max);
    ui.painter().rect_filled(page, 6.0, card_bg(th));
    let mut page_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(page)
            .layout(Layout::top_down(Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("settings-page")
        .show(&mut page_ui, |s| {
            let inner = Rect::from_min_size(
                Pos2::new(page.left() + 20.0, s.cursor().top()),
                egui::vec2(page.width() - 40.0, s.max_rect().height()),
            );
            let mut card_ui = s.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::Min)),
            );
            // 右页内容 Grid Margin="20,16"：左右 20 已由 inner 内缩，上下 16 在此补齐。
            card_ui.add_space(16.0);
            match st.settings.category {
                0 => appearance_page(&mut card_ui, st, p),
                1 => connection_page(&mut card_ui, st, p),
                2 => file_edit_page(&mut card_ui, st, p),
                3 => transfer_page(&mut card_ui, st, p),
                4 => registration_page(&mut card_ui, st, p),
                _ => about_page(&mut card_ui, st, p),
            }
            // C# Grid Margin 下边 16：滚到底时内容不贴视口底。
            card_ui.add_space(16.0);
            // egui 只统计 Ui 上的直接分配：孙级 card_ui 的布局不会回传 s 的
            // min_rect（s 测得内容高恒 0 → ScrollArea 判定无溢出、滚动禁用）。
            // 必须显式回传测量矩形，滚动条与滚轮才会生效。
            let measured = card_ui.min_rect();
            s.expand_to_include_rect(measured);
        });

    // 弹窗开着每帧整包回写 store（内部比对置脏 → 500ms 防抖落盘）。
    st.settings.sync_to_store();
    true
}

/// 页标题（C# 页首 TextBlock：FontSize 20 / SemiBold / Margin 0,0,0,12；
/// brand 字族即注册的 Inter-SemiBold，行高按实测排版取值）。
fn page_title(ui: &mut egui::Ui, title: &str, p: &theme::Palette) {
    let font = FontId::new(20.0, FontFamily::Name("brand".into()));
    let h = ui
        .painter()
        .layout_no_wrap(title.to_owned(), font.clone(), p.text_primary)
        .size()
        .y;
    ui.painter().text(
        Pos2::new(ui.cursor().left(), ui.cursor().top() + h / 2.0),
        Align2::LEFT_CENTER,
        title,
        font,
        p.text_primary,
    );
    ui.add_space(h + 12.0);
}

/// SettingsCard 卡片行（C# SettingsCardStyle：CardBackgroundFill 底 + CardStroke 描边 +
/// 圆角 4 + Padding 16,10）：左「13px 名称 + 11px 描述（StackPanel Spacing 2）」，右控件整卡垂直居中。
/// `gap_after` = 卡后间距（页 StackPanel Spacing 8 / 母子组 Spacing 1）；
/// `indent` = 子卡整卡左缩进（C# 子卡 Margin 24,0,0,0：容器左缘右移、右缘与
/// 母卡对齐），文字与卡体一起缩进。
/// 右控件由 `ctrl` 闭包绘制并返回结果（闭包直接可变借用设置字段，无需临时桥）。
#[expect(clippy::too_many_arguments)]
fn settings_card<T>(
    ui: &mut egui::Ui,
    w: f32,
    name: &str,
    desc: &str,
    ctrl_w: f32,
    enabled: bool,
    gap_after: f32,
    indent: f32,
    ctrl: impl FnOnce(&mut egui::Ui, Rect) -> T,
) -> T {
    settings_card_fs(
        ui,
        w,
        name,
        desc,
        ctrl_w,
        32.0,
        enabled,
        gap_after,
        indent,
        FontId::proportional(13.0),
        ctrl,
    )
}

/// settings_card 控件超高版：`ctrl_h` = 右控件实际所需高（竖向单选组 64），
/// 卡高随之撑大，上下仍保 Padding 10（编辑方式行专用，其余行走 settings_card）。
#[expect(clippy::too_many_arguments)]
fn settings_card_h<T>(
    ui: &mut egui::Ui,
    w: f32,
    name: &str,
    desc: &str,
    ctrl_w: f32,
    ctrl_h: f32,
    enabled: bool,
    gap_after: f32,
    indent: f32,
    ctrl: impl FnOnce(&mut egui::Ui, Rect) -> T,
) -> T {
    settings_card_fs(
        ui,
        w,
        name,
        desc,
        ctrl_w,
        ctrl_h,
        enabled,
        gap_after,
        indent,
        FontId::proportional(13.0),
        ctrl,
    )
}

/// settings_card 全参版：`name_font` 覆写名称字体（关于页应用卡 C# FontWeight=SemiBold → brand 字族）；
/// `ctrl_h` = 右控件所需最小高（WinUI 控件默认 32，竖向单选组 64），卡高随之撑大。
#[expect(clippy::too_many_arguments)]
fn settings_card_fs<T>(
    ui: &mut egui::Ui,
    w: f32,
    name: &str,
    desc: &str,
    ctrl_w: f32,
    ctrl_h: f32,
    enabled: bool,
    gap_after: f32,
    indent: f32,
    name_font: FontId,
    ctrl: impl FnOnce(&mut egui::Ui, Rect) -> T,
) -> T {
    let p = theme::palette(ui.ctx());
    settings_card_body(
        ui,
        w,
        name,
        desc,
        ctrl_w,
        ctrl_h,
        enabled,
        gap_after,
        indent,
        name_font,
        if enabled {
            p.text_secondary
        } else {
            p.text_tertiary
        },
        ctrl,
    )
}

/// settings_card 激活卡变体：`hint` = Some(是否成功) 时描述按 C# RegCodeHint
/// 动态前景着色（成功绿 0x2E9960 / 失败红 0xE05353），None = 常规描述色（引导
/// 文案）；控件字号 13（RegCodeBox / RegActivateBtn FontSize 13）。
#[expect(clippy::too_many_arguments)]
fn settings_card_hint<T>(
    ui: &mut egui::Ui,
    w: f32,
    name: &str,
    desc: &str,
    hint: Option<bool>,
    ctrl_w: f32,
    ctrl_h: f32,
    ctrl: impl FnOnce(&mut egui::Ui, Rect) -> T,
) -> T {
    let desc_c = match hint {
        Some(true) => Color32::from_rgb(0x2E, 0x99, 0x60),
        Some(false) => Color32::from_rgb(0xE0, 0x53, 0x53),
        None => theme::palette(ui.ctx()).text_secondary,
    };
    settings_card_body(
        ui,
        w,
        name,
        desc,
        ctrl_w,
        ctrl_h,
        true,
        8.0,
        0.0,
        FontId::proportional(13.0),
        desc_c,
        ctrl,
    )
}

/// settings_card 家族公共主体（描述色显式传入，settings_card/_h/_fs/_hint 薄壳转发）。
#[expect(clippy::too_many_arguments)]
fn settings_card_body<T>(
    ui: &mut egui::Ui,
    w: f32,
    name: &str,
    desc: &str,
    ctrl_w: f32,
    ctrl_h: f32,
    enabled: bool,
    gap_after: f32,
    indent: f32,
    name_font: FontId,
    desc_c: Color32,
    ctrl: impl FnOnce(&mut egui::Ui, Rect) -> T,
) -> T {
    let t = ui.ctx().theme();
    let p = theme::palette(ui.ctx());
    // 卡体宽 = 行宽 - 缩进（子卡整卡缩进：容器左缘右移 indent、右缘与母卡对齐，
    // C# 子卡 Margin 24,0,0,0 语义）；卡内文字左 padding 固定 16。
    let card_w = w - indent;
    let pad_l = 16.0;
    // 文本可用宽（名称裁剪区/描述换行共用；控件区左缘再留 8 间距）。
    let text_w = card_w - pad_l - 16.0 - if ctrl_w > 0.0 { ctrl_w + 8.0 } else { 0.0 };
    let name_h = ui
        .painter()
        .layout_no_wrap(name.to_owned(), name_font.clone(), p.text_primary)
        .size()
        .y;
    // 描述按可用宽预排成 galley（原来只取换行测高、绘制却走 painter.text 的单行
    // no-wrap，长描述整行画进右侧控件区）。
    let desc_galley = if desc.is_empty() {
        None
    } else {
        Some(
            ui.painter()
                .layout(desc.to_owned(), FontId::proportional(11.0), desc_c, text_w),
        )
    };
    let desc_h = desc_galley.as_ref().map_or(0.0, |g| g.size().y);
    // 文本块 = 名称 +（7 间距 + 描述；2026-10-31 定案：大字小字上下间隔加大 5px，
    // C# StackPanel Spacing 2 → 7）；与右控件区取大（ctrl_h = 控件实际所需高，
    // 如竖向单选组 64 会撑高卡片），再加 Padding 16,10 的上下 10。
    let text_h = name_h + if desc_h > 0.0 { 7.0 + desc_h } else { 0.0 };
    let h = 10.0 + text_h.max(if ctrl_w > 0.0 { ctrl_h } else { 0.0 }) + 10.0;
    // 布局流按整行宽 w 分配，卡体视觉左缘右移 indent（名称/描述/控件区均相对
    // 卡体定位，自动随缩进；右缘 = alloc 右缘与母卡对齐）。
    let (alloc, _) = ui.allocate_exact_size(egui::vec2(w, h), Sense::hover());
    let rect = if indent > 0.0 {
        Rect::from_min_size(
            Pos2::new(alloc.left() + indent, alloc.top()),
            egui::vec2(card_w, h),
        )
    } else {
        alloc
    };
    ui.painter().rect_filled(rect, 4.0, card_bg(t));
    ui.painter().rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, p.card_stroke),
        egui::StrokeKind::Inside,
    );
    let name_c = if enabled {
        p.text_primary
    } else {
        p.text_tertiary
    };
    // 名称裁剪到文本区（右界 = 控件区左缘 - 8）：painter.text 无裁剪，英文长名称
    // 会画进右侧控件区（对齐 C# TextBlock 列内裁剪语义）。
    let name_right = rect.left() + pad_l + text_w;
    ui.painter()
        .with_clip_rect(Rect::from_min_max(
            Pos2::new(rect.left() + pad_l, rect.top()),
            Pos2::new(name_right, rect.bottom()),
        ))
        .text(
            Pos2::new(rect.left() + pad_l, rect.top() + 10.0 + name_h / 2.0),
            Align2::LEFT_CENTER,
            name,
            name_font,
            name_c,
        );
    if let Some(g) = desc_galley {
        // galley 左上角定位（预排时已按 text_w 换行，多行不再溢出控件区）。
        ui.painter().galley(
            Pos2::new(rect.left() + pad_l, rect.top() + 10.0 + name_h + 7.0),
            g,
            desc_c,
        );
    }
    let ctrl_r = Rect::from_min_max(
        Pos2::new(rect.right() - 16.0 - ctrl_w, rect.top()),
        Pos2::new(rect.right() - 16.0, rect.bottom()),
    );
    let mut c = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(ctrl_r)
            .layout(Layout::top_down(Align::Min)),
    );
    let out = ctrl(&mut c, ctrl_r);
    ui.add_space(gap_after);
    out
}

/// 开关贴右（轨道 40×20 垂直居中）：右缘贴齐控件区右缘（各行右缘统一 =
/// 卡片右 padding，C# 行 Grid Auto 列右贴语义）。
fn card_toggle(ui: &mut egui::Ui, r: Rect, enabled: bool, val: &mut bool) {
    let rect = Rect::from_min_size(
        Pos2::new(r.right() - 40.0, r.center().y - 10.0),
        egui::vec2(40.0, 20.0),
    );
    if toggle(ui, rect, *val, enabled, &theme::palette(ui.ctx())) {
        *val = !*val;
    }
}

/// 滑条（WinUI Slider 自绘）：轨道 SliderTrackThemeHeight 4、拇指 18×18 accent 实心圆
/// （slider.xaml SliderInnerThumb Fill=TemplateBinding Background → accent）、
/// trailing fill = AccentFillColorDefault；悬停 / 按下换 AccentFillColorSecondary /
/// Tertiary 变体，禁用全灰。egui Slider 的轨道与拇指同用 widgets.inactive.bg_fill
/// 无法分色，故完全自绘交互：allocate_rect click_and_drag + 指针 x → 值映射，
/// step 对齐取整。
#[expect(clippy::too_many_arguments)]
pub(crate) fn winui_slider(
    ui: &mut egui::Ui,
    rect: Rect,
    val: &mut f32,
    min: f32,
    max: f32,
    step: f64,
    enabled: bool,
    p: &theme::Palette,
) {
    let resp = ui.allocate_rect(
        rect,
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    // 拇指活动范围 = 轨道内缩半径 9（下同 thumb_x）：点击/拖动映射与显示一致。
    let span = (rect.width() - 18.0).max(1.0);
    if enabled
        && (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let t = ((pos.x - rect.left() - 9.0) / span).clamp(0.0, 1.0);
        let raw = f64::from(min) + f64::from(t) * f64::from(max - min);
        *val = ((raw / step).round() * step).clamp(f64::from(min), f64::from(max)) as f32;
    }
    let cy = rect.center().y;
    let t = if max > min {
        ((*val - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // 拇指（18 直径）中心内缩半径：t=1 时圆心在 right()-9，整圆不被
    // painter_at(rect) 裁成半圆（WinUI Slider 拇指活动范围同样内缩轨道两端）。
    let thumb_x = rect.left() + 9.0 + t * span;
    let (rail_c, fill_c, thumb_c) = if !enabled {
        (
            p.control_strong_fill_disabled,
            p.accent_disabled,
            p.accent_disabled,
        )
    } else if resp.is_pointer_button_down_on() {
        (
            p.control_strong_fill,
            theme::ACCENT_PRESSED,
            theme::ACCENT_PRESSED,
        )
    } else if resp.hovered() {
        (
            p.control_strong_fill,
            theme::ACCENT_HOVER,
            theme::ACCENT_HOVER,
        )
    } else {
        (p.control_strong_fill, theme::ACCENT, theme::ACCENT)
    };
    let pt = ui.painter_at(rect);
    // 轨道（4px 高，垂直居中）。
    let rail = Rect::from_min_max(
        Pos2::new(rect.left(), cy - 2.0),
        Pos2::new(rect.right(), cy + 2.0),
    );
    pt.rect_filled(rail, 2.0, rail_c);
    // trailing 填充（拇指左侧已走过部分）。
    if t > 0.0 {
        pt.rect_filled(
            Rect::from_min_max(rail.min, Pos2::new(thumb_x, rail.max.y)),
            2.0,
            fill_c,
        );
    }
    // 拇指（18×18 accent 实心圆）。
    pt.circle_filled(Pos2::new(thumb_x, cy), 9.0, thumb_c);
}

/// 滑条行（宽 140）+ 值文字；禁用时仅绘制值文字。
#[expect(clippy::too_many_arguments)]
fn card_slider(
    ui: &mut egui::Ui,
    r: Rect,
    val: &mut f32,
    min: f32,
    max: f32,
    step: f64,
    suffix: &'static str,
    enabled: bool,
) {
    let p = theme::palette(ui.ctx());
    // 高 22.5 容纳 18 拇指（SliderHorizontalThumbHeight）。
    let rect = Rect::from_min_size(
        Pos2::new(r.left(), r.center().y - 11.25),
        egui::vec2(140.0, 22.5),
    );
    if enabled {
        winui_slider(ui, rect, val, min, max, step, true, &p);
    }
    // 值文字右对齐到控件区右缘（各行右缘统一 = 卡片右 padding 16；原左对齐画在
    // r.left()+150 处无宽度约束，"1024 KB" 溢出控件区顶到卡片边框）。
    ui.painter().text(
        Pos2::new(r.right(), r.center().y),
        Align2::RIGHT_CENTER,
        tfmt(suffix, &[&format!("{:.0}", *val)]),
        FontId::proportional(13.0),
        p.text_primary,
    );
}

/// 下拉框（WinUI ComboBox 自绘）：容器 32 高圆角 4 + 1px ControlStrokeColorDefault
/// 淡边，底色三态（rest ControlFillColorDefault / hover Secondary / 按下·展开
/// Tertiary，展开时描边换 text_primary 近似 FocusStrokeColorOuter）；展开浮层
/// AcrylicInAppFillColorDefault 底 + SurfaceStrokeColorFlyout 描边（圆角 8），
/// 条目高 40（ComboBoxItem 默认），悬停 / 选中 Subtle 系底色，文字左缩进 12
/// （ComboBox Padding 12,4）。`fs` 为文字字号。
pub(crate) fn card_combo(
    ui: &mut egui::Ui,
    r: Rect,
    id_str: &str,
    val: &mut usize,
    options: &[&str],
    w: f32,
    fs: f32,
) {
    let ctx = ui.ctx().clone();
    let p = theme::palette(ui.ctx());
    let id = Id::unique(id_str);
    let open_id = id.with("open");
    let open = ctx.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    let rect = Rect::from_center_size(r.center(), egui::vec2(w, 32.0));
    // 描边像素对齐（theme::snap_rect）：半像素坐标会让 1px 边线被抗锯齿摊薄。
    let rect = theme::snap_rect(rect, ctx.pixels_per_point());
    let resp = ui.allocate_rect(rect, Sense::click());
    let (bg, stroke_c) = if open || resp.is_pointer_button_down_on() {
        (p.control_pressed, p.text_primary)
    } else if resp.hovered() {
        (p.control_hover, p.control_stroke_default)
    } else {
        (p.control_bg, p.control_stroke_default)
    };
    let pt = ui.painter_at(rect);
    pt.rect_filled(rect, theme::CONTROL_RADIUS, bg);
    // 描边：展开/按下单色深线，静默/悬停走 elevation 渐变近似（顶淡底重）。
    if open || resp.is_pointer_button_down_on() {
        pt.rect_stroke(
            rect,
            theme::CONTROL_RADIUS,
            Stroke::new(theme::border_width(ctx.pixels_per_point()), stroke_c),
            egui::StrokeKind::Inside,
        );
    } else {
        theme::elevation_border(
            &pt,
            rect,
            theme::CONTROL_RADIUS,
            p.control_stroke_default,
            p.control_stroke,
        );
    }
    pt.text(
        Pos2::new(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        options.get(*val).copied().unwrap_or(""),
        FontId::proportional(fs),
        p.text_primary,
    );
    // 展开箭头（右缩进 17，两段 1.5px 线画 ˅）。
    let cx = rect.right() - 17.0;
    let cy = rect.center().y;
    let ch = Stroke::new(1.5, p.text_secondary);
    pt.line_segment([Pos2::new(cx - 4.0, cy - 2.0), Pos2::new(cx, cy + 2.0)], ch);
    pt.line_segment([Pos2::new(cx, cy + 2.0), Pos2::new(cx + 4.0, cy - 2.0)], ch);
    if resp.clicked() {
        ctx.data_mut(|d| d.insert_temp(open_id, !open));
    }

    let mut close = false;
    if open {
        // 浮层：向下 4 间距，下方放不下翻上方（WinUI ComboBox 下拉方向逻辑）。
        let item_h = 40.0;
        let pop_h = options.len() as f32 * item_h + 2.0;
        let pop_w = rect.width();
        // raw.screen_rect 是 Option（fallback 到控件 rect，每帧实际必有值）。
        let screen = ctx.input(|i| i.raw.screen_rect).unwrap_or(rect);
        let top = if rect.bottom() + 4.0 + pop_h <= screen.bottom() - 8.0 {
            rect.bottom() + 4.0
        } else {
            rect.top() - 4.0 - pop_h
        };
        let pop_rect = Rect::from_min_size(Pos2::new(rect.left(), top), egui::vec2(pop_w, pop_h));
        // 遮罩先建（点空白关闭），浮层后建（同 Order 后者在上）。
        Area::new(id.with("blocker"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(&ctx, |ui| {
                let b = ui.allocate_rect(screen, Sense::click());
                if b.clicked() {
                    close = true;
                }
            });
        Area::new(id.with("popup"))
            .order(Order::Foreground)
            .fixed_pos(pop_rect.min)
            .show(&ctx, |ui| {
                let pt = ui.painter().with_clip_rect(pop_rect);
                pt.rect_filled(pop_rect, 8.0, p.combo_popup_bg);
                pt.rect_stroke(
                    pop_rect,
                    8.0,
                    Stroke::new(1.0, p.combo_popup_stroke),
                    egui::StrokeKind::Outside,
                );
                for (i, label) in options.iter().enumerate() {
                    let ir = Rect::from_min_size(
                        Pos2::new(
                            pop_rect.left() + 1.0,
                            pop_rect.top() + 1.0 + i as f32 * item_h,
                        ),
                        egui::vec2(pop_w - 2.0, item_h),
                    );
                    let item = ui.allocate_rect(ir, Sense::click());
                    let fill = if i == *val {
                        if item.hovered() {
                            p.tab_rest
                        } else {
                            p.row_hover
                        }
                    } else if item.hovered() {
                        p.row_hover
                    } else {
                        Color32::TRANSPARENT
                    };
                    if fill != Color32::TRANSPARENT {
                        pt.rect_filled(ir, 0.0, fill);
                    }
                    pt.text(
                        Pos2::new(ir.left() + 12.0, ir.center().y),
                        Align2::LEFT_CENTER,
                        *label,
                        FontId::proportional(fs),
                        p.text_primary,
                    );
                    if item.clicked() {
                        *val = i;
                        close = true;
                    }
                }
            });
    }
    if close {
        ctx.data_mut(|d| d.insert_temp(open_id, false));
    }
}

fn appearance_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("外观"), p);
    let w = ui.max_rect().width();

    // 主题：切换即时生效。
    let dark_now = st.settings.theme_dark;
    let picked = settings_card(
        ui,
        w,
        t("主题"),
        t("界面配色跟随系统，或手动指定深浅"),
        150.0,
        true,
        8.0,
        0.0,
        |c, r| {
            // 下标映射：0=深色 1=浅色。原写法 usize::from(dark_now) 颠倒，
            // 弹窗一打开就判定"要翻主题"逐帧 install → 屏幕狂闪。
            let mut i = usize::from(!dark_now);
            card_combo(
                c,
                r,
                "set-theme",
                &mut i,
                &[t("深色"), t("浅色")],
                150.0,
                13.0,
            );
            i
        },
    );
    let want_dark = picked == 0;
    if want_dark != dark_now {
        st.settings.theme_dark = want_dark;
        crate::theme::install(
            ui.ctx(),
            if want_dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            },
        );
    }

    // 语言。
    let en_now = st.settings.lang_en;
    let picked = settings_card(
        ui,
        w,
        t("语言"),
        t("界面显示语言"),
        150.0,
        true,
        8.0,
        0.0,
        |c, r| {
            let mut i = usize::from(en_now);
            card_combo(
                c,
                r,
                "set-lang",
                &mut i,
                &[t("中文"), t("English")],
                150.0,
                13.0,
            );
            i
        },
    );
    st.settings.lang_en = picked == 1;
}

fn connection_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("连接"), p);
    let w = ui.max_rect().width();
    let mode = st.settings.open_mode;
    let ka = st.settings.keepalive;
    let s = &mut st.settings;

    settings_card(
        ui,
        w,
        t("连接超时（秒）"),
        t("建立 SSH 连接的等待上限（秒），超时后按重试设置自动重连"),
        186.0,
        true,
        8.0,
        0.0,
        |c, r| card_slider(c, r, &mut s.timeout, 15.0, 30.0, 5.0, "{0} 秒", true),
    );
    settings_card(
        ui,
        w,
        t("最大重试次数"),
        t("连接失败后的自动重试次数"),
        186.0,
        true,
        8.0,
        0.0,
        |c, r| card_slider(c, r, &mut s.max_retries, 1.0, 3.0, 1.0, "{0} 次", true),
    );
    settings_card(
        ui,
        w,
        t("登录重试延迟（秒）"),
        t("两次重试之间的等待时间（秒）"),
        186.0,
        true,
        8.0,
        0.0,
        |c, r| card_slider(c, r, &mut s.retry_delay, 3.0, 10.0, 1.0, "{0} 秒", true),
    );
    settings_card(
        ui,
        w,
        t("操作超时（秒）"),
        t("单次 SFTP 请求等待响应的上限（秒），超时按错误处理不再挂起"),
        186.0,
        true,
        8.0,
        0.0,
        |c, r| card_slider(c, r, &mut s.op_timeout, 5.0, 15.0, 5.0, "{0} 秒", true),
    );

    // 打开方式（C# ComboBox Width 150）。
    let picked = settings_card(
        ui,
        w,
        t("连接打开方式"),
        t("点击主机卡片连接时，右侧窗口的打开方式"),
        150.0,
        true,
        8.0,
        0.0,
        |c, r| {
            let mut i = mode;
            card_combo(
                c,
                r,
                "set-openmode",
                &mut i,
                &[t("每次询问"), t("总是当前标签页"), t("总是新标签页")],
                150.0,
                13.0,
            );
            i
        },
    );
    s.open_mode = picked;

    // 保活母子组（C#：母卡 + 子卡 Margin 24,0,0,0 / 组 StackPanel Spacing 1）。
    settings_card(
        ui,
        w,
        t("keep-alive 连接"),
        t("周期发送心跳包，防止长连接被路由器或防火墙断开"),
        46.0,
        true,
        1.0,
        0.0,
        |c, r| card_toggle(c, r, true, &mut s.keepalive),
    );
    settings_card(
        ui,
        w,
        t("keep-alive 间隔"),
        t("心跳包发送周期（秒），需开启 keep-alive 连接"),
        186.0,
        ka,
        8.0,
        24.0,
        |c, r| card_slider(c, r, &mut s.ka_interval, 15.0, 60.0, 15.0, "{0} 秒", ka),
    );
}

/// 外部编辑器可执行文件选择框（C# EditorBrowseBtn_Click：FileTypeFilter 仅 .exe）；
/// 取消或失败返回 None。非 Windows 无 comdlg32，恒 None（按钮可见但不弹窗）。
#[cfg(windows)]
fn pick_exe_file() -> Option<String> {
    // OpenFileNameW / GetOpenFileNameW（comdlg32）——与 host_panel 密钥文件选择
    // 同款零依赖实现（结构体为 comdlg32 ABI 原样副本，避免跨模块 pub 暴露）。
    #[repr(C)]
    #[allow(non_snake_case)] // comdlg32 原字段名
    struct OpenFileNameW {
        lStructSize: u32,
        hwndOwner: *mut core::ffi::c_void,
        hInstance: *mut core::ffi::c_void,
        lpstrFilter: *const u16,
        lpstrCustomFilter: *mut u16,
        nMaxCustFilter: u32,
        nFilterIndex: u32,
        lpstrFile: *mut u16,
        nMaxFile: u32,
        lpstrFileTitle: *mut u16,
        nMaxFileTitle: u32,
        lpstrInitialDir: *const u16,
        lpstrTitle: *const u16,
        Flags: u32,
        nFileOffset: u16,
        nFileExtension: u16,
        lpstrDefExt: *const u16,
        lCustData: usize,
        lpfnHook: *mut core::ffi::c_void,
        lpTemplateName: *const u16,
        pvReserved: *mut core::ffi::c_void,
        dwReserved: u32,
        FlagsEx: u32,
    }
    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    // 双 NUL 分隔的过滤器对（名称\0模式\0）：与 C# picker 仅 .exe 过滤对齐，
    // 补「所有文件」便于调试期选脚本（C# 无此项；模式仍默认 .exe）。
    let filter = wide(&format!(
        "{}\0*.exe\0{}\0*.*\0",
        t("程序 (*.exe)"),
        t("所有文件 (*.*)")
    ));
    let title = wide(t("选择外部编辑器"));
    let mut buf = [0u16; 1024];
    let mut ofn = OpenFileNameW {
        lStructSize: std::mem::size_of::<OpenFileNameW>() as u32,
        hwndOwner: std::ptr::null_mut(),
        hInstance: std::ptr::null_mut(),
        lpstrFilter: filter.as_ptr(),
        lpstrCustomFilter: std::ptr::null_mut(),
        nMaxCustFilter: 0,
        nFilterIndex: 1,
        lpstrFile: buf.as_mut_ptr(),
        nMaxFile: buf.len() as u32,
        lpstrFileTitle: std::ptr::null_mut(),
        nMaxFileTitle: 0,
        lpstrInitialDir: std::ptr::null(),
        lpstrTitle: title.as_ptr(),
        Flags: 0x1804, // OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST | OFN_HIDEREADONLY
        nFileOffset: 0,
        nFileExtension: 0,
        lpstrDefExt: std::ptr::null(),
        lCustData: 0,
        lpfnHook: std::ptr::null_mut(),
        lpTemplateName: std::ptr::null(),
        pvReserved: std::ptr::null_mut(),
        dwReserved: 0,
        FlagsEx: 0,
    };
    // SAFETY: ofn/buf/filter/title 均为本函数栈变量，调用期间合法；GetOpenFileNameW
    // 自带模态消息泵（winit 主线程直调与密钥文件选择同先例，已验证无冲突）。
    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(not(windows))]
fn pick_exe_file() -> Option<String> {
    None
}

fn file_edit_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("文件编辑"), p);
    let w = ui.max_rect().width();
    let builtin = st.settings.editor_builtin;
    let s = &mut st.settings;

    // 母子组（C#：组 StackPanel Spacing 1 / 子卡 Margin 24,0,0,0）。
    // 母卡 编辑方式 + 右侧单选组（内置/外部，自绘 radio 选中标志蓝，FontSize 13，
    // 垂直一列整体居中，两项各 32 → 组高 64，卡高按 ctrl_h 撑大保上下 Padding 10）。
    settings_card_h(
        ui,
        w,
        t("编辑方式"),
        t("选择打开文件使用的编辑器"),
        110.0,
        64.0,
        true,
        1.0,
        0.0,
        |c, r| {
            let mut sel = usize::from(!s.editor_builtin); // 0 内置 / 1 外部
            let pc = theme::palette(c.ctx());
            // 两项各占 32（WinUI RadioButton 默认高），组整体垂直居中。
            let top = r.center().y - 32.0;
            for (i, label) in [t("内置编辑器"), t("外部编辑器")].into_iter().enumerate() {
                let rr = Rect::from_min_size(
                    Pos2::new(r.left(), top + i as f32 * 32.0),
                    egui::vec2(r.width(), 32.0),
                );
                if radio(c, rr, sel == i, label, 13.0, &pc) {
                    sel = i;
                }
            }
            s.editor_builtin = sel == 0;
        },
    );
    // 子卡 编辑器路径（C#：内置时整卡禁用变灰）：
    // 右侧 TextBox 220×32（FontSize 13）+ ColumnSpacing 6 + 浏览钮（Padding 12,5 / FontSize 13）。
    settings_card(
        ui,
        w,
        t("编辑器路径"),
        t("选择外部编辑器的可执行文件（如 notepad++、VS Code）。路径留空时回退使用内置编辑器。"),
        300.0,
        !builtin,
        8.0,
        24.0,
        |c, r| {
            let pc = theme::palette(c.ctx());
            c.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, FontId::proportional(13.0));
            let bw = c
                .painter()
                .layout_no_wrap(
                    t("浏览…").to_owned(),
                    FontId::proportional(13.0),
                    pc.text_primary,
                )
                .size()
                .x
                + 24.0;
            let total = 220.0 + 6.0 + bw;
            let box_r = Rect::from_min_size(
                Pos2::new(r.right() - total, r.center().y - 16.0),
                egui::vec2(220.0, 32.0),
            );
            let te = egui::TextEdit::singleline(&mut s.editor_path).font(egui::TextStyle::Body);
            c.put(box_r, te);
            let btn_r = Rect::from_min_size(
                Pos2::new(box_r.right() + 6.0, r.center().y - 16.0),
                egui::vec2(bw, 32.0),
            );
            // C# EditorBrowseBtn_Click：FileOpenPicker 仅 .exe 过滤，选中即填入
            // 路径框（TextChanged 即时落盘；此处 sync_to_store 每帧回写等价）。
            if std_button_fs(c, btn_r, t("浏览…"), pc.text_primary, &pc, 13.0)
                && let Some(path) = pick_exe_file()
            {
                s.editor_path = path;
            }
        },
    );
}

fn transfer_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("传输"), p);
    let w = ui.max_rect().width();
    let resume_on = st.settings.resume;
    let verify_on = st.settings.verify;
    let verify_algo = st.settings.verify_algo;
    let tar_on = st.settings.tar_accel;
    let s = &mut st.settings;

    // 卡组顺序对齐 C# SettingsPanel.xaml 传输页：并发 → 远程删除 →
    // 组[断点续传+续传阈值] → 组[传输后校验+校验算法] → 组[tar加速+tar阈值]；
    // （C# 的「保留修改时间」卡已摘除：Rust 未实现，见下方注释与 README 能力矩阵）
    // 母卡 gap 1（卡组 StackPanel Spacing 1）+ 子卡 indent 24（Margin 24,0,0,0），组尾 gap 8。
    settings_card(
        ui,
        w,
        t("并发传输数"),
        t(
            "同时进行的传输任务数。1 = 串行并复用当前连接；大于 1 时每个任务使用独立连接，多文件传输更快，部分服务器可能限制连接数。",
        ),
        186.0,
        true,
        8.0,
        0.0,
        |c, r| card_slider(c, r, &mut s.concurrent, 1.0, 3.0, 1.0, "{0} 个", true),
    );
    let del_desc = if s.del_server_cmd {
        t(
            "删除远程目录时优先使用服务端命令（rm -rf，海量文件秒级）；命令通道不可用或未能删净时自动回退 SFTP 递归。",
        )
    } else {
        t("不使用服务端命令，通过 SFTP 协议逐条递归删除，兼容所有服务器；大目录耗时较长。")
    };
    settings_card(
        ui,
        w,
        t("远程删除方式"),
        del_desc,
        46.0,
        true,
        8.0,
        0.0,
        |c, r| card_toggle(c, r, true, &mut s.del_server_cmd),
    );

    // 断点续传母子组。
    settings_card(
        ui,
        w,
        t("断点续传"),
        t(
            "开启后，大文件传输中断可从断点续传：目标小于源时追加，大小相等或目标更大一律全量重传。同名旧文件会先弹窗确认，选择\"覆盖\"即全量重传，不会误追加。适合大文件经不稳定链路传输，默认开启。",
        ),
        46.0,
        true,
        1.0,
        0.0,
        |c, r| card_toggle(c, r, true, &mut s.resume),
    );
    settings_card(
        ui,
        w,
        t("续传阈值"),
        t("小于该值的文件一律全量重传（重传代价可忽略，阈值宜取大不取小）"),
        // 控件区预算 = 滑条 140 + 间距 10 + 最宽值文字 "1024 KB"（值文字右对齐后
        // 与滑条间留出 ≥10 间隙，不再溢出）。
        210.0,
        resume_on,
        8.0,
        24.0,
        |c, r| {
            card_slider(
                c,
                r,
                &mut s.resume_threshold,
                200.0,
                1024.0,
                8.0,
                "{0} KB",
                resume_on,
            )
        },
    );

    // 传输后校验母子组（校验算法 ComboBox 宽 110）。
    settings_card(
        ui,
        w,
        t("传输后校验"),
        t(
            "传输完成后自动比对本地与远端文件杂凑（经 exec 通道，不占传输链路），不一致的任务标记失败，尽早暴露损坏文件；命令通道不可用时跳过校验，不影响传输结果。默认关闭。",
        ),
        46.0,
        true,
        1.0,
        0.0,
        |c, r| card_toggle(c, r, true, &mut s.verify),
    );
    let picked = settings_card(
        ui,
        w,
        t("校验算法"),
        t("自动：按服务端支持情况选最强算法（SHA256 → SHA512 → SM3）；固定算法不可用时校验跳过"),
        110.0,
        verify_on,
        8.0,
        24.0,
        |c, r| {
            let mut i = verify_algo;
            card_combo(
                c,
                r,
                "set-verify",
                &mut i,
                &[t("自动"), "SHA-256", "SHA-512", "SM3"],
                110.0,
                13.0,
            );
            i
        },
    );
    s.verify_algo = picked;

    // 保留修改时间（C# 位于校验组与 tar 组之间）。
    // T6 摘除：Rust 版逐文件传输未实现 mtime 保留（需 filetime，R5 未批准），
    // 开关挂着不生效属于「文案承诺了代码没做的事」；tar 模式天然保留 mtime
    // 不受影响。字段与存档键保留（C# 存档互读），见 README 能力矩阵。

    // tar 打包加速母子组。
    settings_card(
        ui,
        w,
        t("tar 打包加速"),
        t(
            "目录传输文件数较多时用单流打包替代逐文件往返（海量小文件数量级提速）；本地或服务端没有 tar、命令失败时自动回退逐文件，结果不变。tar 模式为整目录单任务，失败只能整体重试。",
        ),
        46.0,
        true,
        1.0,
        0.0,
        |c, r| card_toggle(c, r, true, &mut s.tar_accel),
    );
    settings_card(
        ui,
        w,
        t("tar 文件数阈值"),
        t("目录内文件数达到该值才改用单流打包，少量文件逐文件直传更快"),
        // 同续传阈值：预算按最宽值文字 "10000 KB" 计。
        210.0,
        tar_on,
        8.0,
        24.0,
        |c, r| {
            card_slider(
                c,
                r,
                &mut s.tar_threshold,
                10.0,
                10000.0,
                10.0,
                "{0} KB",
                tar_on,
            )
        },
    );
}

/// 注册页（C# SettingsPanel.xaml 注册大类 4 卡：免费承诺 / 注册状态 / 激活 / 公告；
/// 动态刷新对照 C# UpdateRegistrationUi：状态行分支文案 + 勋章胶囊 + 购买入口显隐）。
fn registration_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("注册"), p);
    let w = ui.max_rect().width();
    let s = crate::registration::service::global().state();

    // 免费承诺（RegFreeTitle 13 + RegFreeDesc 11，无右控件）。
    settings_card(
        ui,
        w,
        t("免费承诺"),
        t("全部功能对所有用户永久免费开放。注册勋章纯属自愿赞助，不解锁、不限制任何功能。"),
        0.0,
        true,
        8.0,
        0.0,
        |_, _| {},
    );

    // 非 Windows：无凭据加密能力（NoopStore 设计性不落盘）—— 静态可见提示
    // （红线：不静默明文落盘，激活态仅本会话有效）。
    #[cfg(not(windows))]
    settings_card(
        ui,
        w,
        t("平台提示"),
        t("当前平台暂不支持凭据加密存储，注册码仅本次会话有效，重启后需重新激活。"),
        0.0,
        true,
        8.0,
        0.0,
        |_, _| {},
    );

    // 注册状态卡：desc 按 C# UpdateRegistrationUi 分支（吊销 / 已注册（永久|到期
    // yyyy-MM-dd 本地时区）/ 未注册引导）；badge_on 时右侧勋章胶囊。
    let badge_on = s.registered && !s.revoked && s.badge_enabled;
    let badge_text = if s.badge_text.trim().is_empty() {
        t("支持者")
    } else {
        s.badge_text.as_str()
    };
    let status_desc = if s.revoked {
        t("注册已被吊销，请重新激活").to_owned()
    } else if s.registered {
        // ExpiresAt==0 → 永久；否则本地时区日期段（fmt_mtime_unix 前 10 字符）。
        let expires = if s.expires_at == 0 {
            t("永久有效").to_owned()
        } else {
            let date = crate::local::fmt_mtime_unix(s.expires_at as i64);
            crate::i18n::tfmt("有效期至 {0}", &[&&date[..10]])
        };
        crate::i18n::tfmt("已注册：{0}（{1}）", &[&s.user_name, &expires])
    } else {
        t("免费使用 · 注册后显示支持者勋章").to_owned()
    };
    settings_card(
        ui,
        w,
        t("注册状态"),
        &status_desc,
        if badge_on {
            badge_pill_w(ui, badge_text)
        } else {
            0.0
        },
        true,
        8.0,
        0.0,
        |c, r| {
            if badge_on {
                badge_pill(c, r, badge_text);
            }
        },
    );

    // 激活卡：右列两行（Spacing 8）= 码框 250 + 激活钮 / 购买双渠道钮行
    // （C# RegBuyCnBtn/RegBuyIntlBtn URL 非空才显示；点击经 UrlGuard 白名单启动）。
    let f13 = FontId::proportional(13.0);
    let act_bw = ui
        .painter()
        .layout_no_wrap(t("激活").to_owned(), f13.clone(), Color32::WHITE)
        .size()
        .x
        + 24.0; // Padding 12,5 左右
    let buy_cn_w = if s.purchase_cn.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(
                t("购买注册码 (爱发电)").to_owned(),
                f13.clone(),
                Color32::WHITE,
            )
            .size()
            .x
            + 24.0
    };
    let buy_intl_w = if s.purchase_intl.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(
                t("Get License (Ko-fi)").to_owned(),
                f13.clone(),
                Color32::WHITE,
            )
            .size()
            .x
            + 24.0
    };
    let buy_w = buy_cn_w
        + if buy_cn_w > 0.0 && buy_intl_w > 0.0 {
            8.0
        } else {
            0.0
        }
        + buy_intl_w;
    let ctrl_w = (250.0 + 8.0 + act_bw).max(buy_w);
    // hint = Some((文案, 是否成功色))；None = 引导文案（C# RegCodeHint 初始）。
    let (hint_desc, hint_ok) = match &st.settings.reg_hint {
        Some((txt, ok)) => (txt.clone(), Some(*ok)),
        None => (t("粘贴收到的注册码，点击激活").to_owned(), None),
    };
    settings_card_hint(
        ui,
        w,
        t("注册码"),
        &hint_desc,
        hint_ok,
        ctrl_w,
        32.0 + 8.0 + 32.0,
        |c, r| {
            // 控件字号 13（RegCodeBox / RegActivateBtn FontSize 13）。
            c.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, FontId::proportional(13.0));
            // 右列垂直居中（C# StackPanel VerticalAlignment=Center，两行共 72）。
            let row_top = r.center().y - 36.0;
            let box_r = Rect::from_min_size(
                Pos2::new(r.right() - 250.0 - 8.0 - act_bw, row_top),
                egui::vec2(250.0, 32.0),
            );
            let te = egui::TextEdit::singleline(&mut st.settings.reg_code)
                .font(egui::TextStyle::Body)
                .desired_width(250.0);
            c.put(box_r, te);
            let btn_r = Rect::from_min_size(
                Pos2::new(box_r.right() + 8.0, row_top),
                egui::vec2(act_bw, 32.0),
            );
            // C# RegActivateBtn_Click：err = Activate(...)；hint = err ?? 激活成功，
            // 成功绿 / 失败红；输入框不清空（C# RegCodeBox.Text 无清空）。
            if std_button_fs(c, btn_r, t("激活"), p.text_primary, p, 13.0) {
                st.settings.reg_hint = Some(
                    match crate::registration::service::global().activate(&st.settings.reg_code) {
                        crate::registration::service::ActivateResult::Success(_) => {
                            (t("激活成功，感谢支持！").to_owned(), true)
                        }
                        crate::registration::service::ActivateResult::OfficialOnly => (
                            t("激活需官方构建：社区构建不含注册模块，请从官网下载正式版本。")
                                .to_owned(),
                            false,
                        ),
                        crate::registration::service::ActivateResult::VerifyFail(kind) => (
                            match kind {
                                crate::registration::core::ErrorKind::Format => {
                                    t("注册码格式无效").to_owned()
                                }
                                crate::registration::core::ErrorKind::Signature => {
                                    t("注册码校验失败").to_owned()
                                }
                                crate::registration::core::ErrorKind::Expired => {
                                    t("注册码已过期").to_owned()
                                }
                                // M-7：None = 验签失败但未带错误种类，映射为格式类
                                //错误提示（不再 unreachable! 崩溃；社区构建/异常
                                //载荷路径可达）。
                                crate::registration::core::ErrorKind::None => {
                                    t("注册码格式错误").to_owned()
                                }
                            },
                            false,
                        ),
                        crate::registration::service::ActivateResult::SaveFailed(e) => (
                            // 落盘失败 = 激活未完成（内存态未置注册，与 C# 落盘
                            // 失败即激活失败同语义）：红字，reg_code 未清空可重试。
                            crate::i18n::tfmt("激活未完成：{0}（注册码未能保存，请重试）", &[&e]),
                            false,
                        ),
                    },
                );
            }
            // 行2：购买双渠道（右贴齐，Spacing 8；C# 行序 Cn 左 Intl 右）。
            if buy_w > 0.0 {
                let row2 = row_top + 40.0;
                let mut x = r.right();
                if buy_intl_w > 0.0 {
                    x -= buy_intl_w;
                    if std_button_fs(
                        c,
                        Rect::from_min_size(Pos2::new(x, row2), egui::vec2(buy_intl_w, 32.0)),
                        t("Get License (Ko-fi)"),
                        p.text_primary,
                        p,
                        13.0,
                    ) {
                        crate::registration::guard::launch_url(&s.purchase_intl);
                    }
                    x -= 8.0;
                }
                if buy_cn_w > 0.0 {
                    x -= buy_cn_w;
                    if std_button_fs(
                        c,
                        Rect::from_min_size(Pos2::new(x, row2), egui::vec2(buy_cn_w, 32.0)),
                        t("购买注册码 (爱发电)"),
                        p.text_primary,
                        p,
                        13.0,
                    ) {
                        crate::registration::guard::launch_url(&s.purchase_cn);
                    }
                }
            }
        },
    );

    // 公告卡：远程下发（C# RegNoticeCard 无公告整卡 Collapsed）。
    if !s.remote_notice.is_empty() {
        settings_card(
            ui,
            w,
            t("公告"),
            &s.remote_notice,
            0.0,
            true,
            8.0,
            0.0,
            |_, _| {},
        );
    }
}

/// 勋章胶囊宽（Padding 12,5 + ★ 13 + Spacing 6 + 文字 12）。
fn badge_pill_w(ui: &egui::Ui, text: &str) -> f32 {
    let tw = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(12.0), Color32::WHITE)
        .size()
        .x;
    12.0 + 13.0 + 6.0 + tw + 12.0
}

/// 勋章胶囊（C# RegBadgePill：Padding 12,5 / 圆角 12 / bg #26D4A017 边 #66D4A017，
/// 内金色 #D4A017 图标 + 文字 12；C# E734 徽章星形 icons-src 无对应资产 → 文本 ★
/// 近似）。右贴齐控件区右缘，垂直居中 32 高。
fn badge_pill(ui: &mut egui::Ui, r: Rect, text: &str) {
    let gold = Color32::from_rgb(0xD4, 0xA0, 0x17);
    let pill_w = badge_pill_w(ui, text);
    let pill = Rect::from_min_size(
        Pos2::new(r.right() - pill_w, r.center().y - 16.0),
        egui::vec2(pill_w, 32.0),
    );
    ui.painter().rect_filled(
        pill,
        12.0,
        Color32::from_rgba_unmultiplied(0xD4, 0xA0, 0x17, 0x26),
    );
    ui.painter().rect_stroke(
        pill,
        12.0,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xD4, 0xA0, 0x17, 0x66)),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        Pos2::new(pill.left() + 12.0 + 6.5, pill.center().y),
        Align2::CENTER_CENTER,
        "★",
        FontId::proportional(13.0),
        gold,
    );
    ui.painter().text(
        Pos2::new(pill.left() + 12.0 + 13.0 + 6.0, pill.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.0),
        gold,
    );
}

/// 超链接按钮镜像（C# HyperlinkButton：FontSize 13 / accent 前景 / hover 下划线，新标签页打开）。
fn card_link(ui: &mut egui::Ui, r: Rect, label: &str, url: &str, p: &theme::Palette) {
    let font = FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), p.accent);
    // 贴控件区右缘（各行右缘统一）。
    let rect = Rect::from_min_size(
        Pos2::new(
            r.right() - galley.size().x,
            r.center().y - galley.size().y / 2.0,
        ),
        galley.size(),
    );
    let resp = ui.allocate_rect(rect, Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(
        Pos2::new(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        p.accent,
    );
    // PointerOver 下划线（HyperlinkButton hover 语义）。
    if resp.hovered() {
        let y = rect.bottom() + 1.0;
        ui.painter().line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(1.0, p.accent),
        );
    }
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// 关于页（C# SettingsPanel.xaml 关于大类 6 卡：应用 / 发布渠道 / 更新 / 许可证 / 支持开发 / 版权）。
fn about_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("关于 iFreeSftp"), p);
    let w = ui.max_rect().width();

    // 应用卡：名称 SemiBold（brand 字族）+ 版本描述 + 右「访问官网」链接。
    settings_card_fs(
        ui,
        w,
        "iFreeSftp",
        t("版本 0.9.0（Rust 迁移版）"),
        56.0,
        32.0,
        true,
        8.0,
        0.0,
        FontId::new(13.0, FontFamily::Name("brand".into())),
        |c, r| card_link(c, r, t("访问官网"), "https://ifrees.top", p),
    );
    // 发布渠道卡：右「GitHub」链接。
    settings_card(
        ui,
        w,
        t("发布渠道"),
        t("最新版本与发布动态见官网 / GitHub 仓库"),
        48.0,
        true,
        8.0,
        0.0,
        |c, r| card_link(c, r, "GitHub", "https://github.com/iFreeshine", p),
    );
    // 更新卡：desc = 检查结果文案（C# UpdateCheckResult 在左列描述行，不弹窗）；
    // 右按钮组 Spacing 12 = 检查更新钮 + 提醒开关（C# UpdateCheckBtn + UpdateCheckToggle）。
    let update_on = st.settings.update_check;
    let checking = st.settings.update_checking;
    let result_text = st.settings.update_check_result.clone();
    let update_flip = settings_card(
        ui,
        w,
        t("有新版本时提醒我"),
        &result_text,
        132.0,
        true,
        8.0,
        0.0,
        |c, r| {
            // 检查更新钮（FontSize 13，宽 74 预算 = 文字 + Padding；检查中禁用）。
            let btn_r = Rect::from_min_size(
                Pos2::new(r.right() - 132.0, r.center().y - 16.0),
                egui::vec2(74.0, 32.0),
            );
            if std_button_en(c, btn_r, t("检查更新"), p.text_primary, p, 13.0, !checking) {
                // C# UpdateCheckBtn_Click：置检查中 → 强刷远程；结果文案由
                // RemoteRefreshDone 事件回填（ui::root 事件消费块）。
                st.settings.update_checking = true;
                st.settings.update_check_result = t("正在检查…").to_owned();
                crate::registration::service::global().refresh_remote(true, c.ctx().clone());
            }
            // 提醒开关（Width 46 / Margin 0,0,-2,0 → 右缘距卡右 23 ≈ 46/2）。
            let tg_r = Rect::from_center_size(
                Pos2::new(r.right() - 23.0, r.center().y),
                egui::vec2(46.0, 20.0),
            );
            toggle(c, tg_r, update_on, true, p)
        },
    );
    if update_flip {
        st.settings.update_check = !update_on;
    }
    // 许可证卡。
    settings_card(
        ui,
        w,
        t("许可证"),
        t(
            "本应用以 GNU AGPL-3.0 许可证开源发布。官方构建版额外集成了独立分发的增值组件；社区构建完全基于开源代码。",
        ),
        0.0,
        true,
        8.0,
        0.0,
        |_, _| {},
    );
    // 支持开发卡。
    settings_card(
        ui,
        w,
        t("支持开发"),
        t(
            "iFreeSftp 永久免费。如果它对你有帮助，欢迎注册成为支持者——这是对开发的支持，不与任何功能绑定。",
        ),
        0.0,
        true,
        8.0,
        0.0,
        |_, _| {},
    );
    // 版权卡。
    settings_card(
        ui,
        w,
        t("版权"),
        t("Copyright © 2026 ifrees.top"),
        0.0,
        true,
        8.0,
        0.0,
        |_, _| {},
    );
}

// ── 右键菜单（宽 200）───────────────────────────────────────
fn draw_menu(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(m) = st.menu.as_ref() else {
        return;
    };
    // 菜单打开期间同样清一次光标（同 shell：被菜单遮住的窗格层控件 hover 泄漏）。
    ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    let pos = m.pos;
    let pane = m.pane;
    let row = m.row;
    let queue = m.queue;
    let term_menu = m.terminal;
    let ppp = ui.ctx().pixels_per_point();

    // 菜单模型：行菜单（本地/远程两套）/ 空白区菜单 / 队列菜单（C# BuildItemMenu / BuildBlankMenu）/
    // 终端菜单（C# TerminalMenuFlyout）。
    enum Item {
        /// 文本, 可用, 危险红。
        Label(&'static str, bool, bool),
        Sep,
    }
    let items: Vec<Item> = if term_menu {
        let has_log = !st.terminal.is_empty();
        vec![
            Item::Label("清除全部", has_log, false),
            Item::Label("复制全部", has_log, false),
        ]
    } else if let Some(qm) = queue {
        match qm {
            QueueMenu::Batch => {
                // C# Root_RightTapped 弹出规则：传输中=暂停+停止+清除 / 暂停中=继续+停止+清除 /
                // 静止且有已完结=清除+重试全部。
                let (transferring, finished) = match &st.transfer {
                    Some(q) => (
                        q.with_items(|items| {
                            items
                                .iter()
                                .any(|i| matches!(i.status, crate::transfer::Status::Transferring))
                        }),
                        q.with_items(|items| {
                            items.iter().any(|i| {
                                matches!(
                                    i.status,
                                    crate::transfer::Status::Done
                                        | crate::transfer::Status::Failed
                                        | crate::transfer::Status::Cancelled
                                )
                            })
                        }),
                    ),
                    None => (false, false),
                };
                let paused = st.transfer.as_ref().map(|q| q.paused()) == Some(true);
                let mut v = vec![];
                if transferring && !paused {
                    v.push(Item::Label("暂停", true, false));
                    v.push(Item::Label("停止", true, false));
                    v.push(Item::Label("清除已完成", true, false));
                    v.push(Item::Label("清除所有", true, false));
                } else if paused {
                    v.push(Item::Label("继续传输", true, false));
                    v.push(Item::Label("停止", true, false));
                    v.push(Item::Label("清除已完成", true, false));
                    v.push(Item::Label("清除所有", true, false));
                } else if finished {
                    v.push(Item::Label("清除已完成", true, false));
                    v.push(Item::Label("清除所有", true, false));
                    v.push(Item::Label("重置并重新请求所有", true, false));
                }
                v
            }
            QueueMenu::Item(id) => {
                // C# Row_RightTapped（QueueContainer.xaml.cs:200-223）按行状态决定
                // 显隐（Collapsed 同义 = 不进菜单）：取消 = 等待/传输中；重试 =
                // 失败/已取消；重连 = 失败/已取消且任务涉及远端侧（C# AttachReconnect
                // remotes 非空同口径，本地↔本地无此入口），**仅会话断开时可点**
                // （探针 C# IsConnectedProbe：已连接置灰，无会话按已连接口径置灰）；
                // 移除 = 任意状态。行菜单无分隔线（QC-02 修复：原缺「重连」且多一分隔）。
                let (can_cancel, can_retry, remote) = match &st.transfer {
                    Some(q) => q.with_items(|items| match items.iter().find(|i| i.id == id) {
                        Some(i) => (
                            matches!(
                                i.status,
                                crate::transfer::Status::Waiting
                                    | crate::transfer::Status::Transferring
                            ),
                            matches!(
                                i.status,
                                crate::transfer::Status::Failed
                                    | crate::transfer::Status::Cancelled
                            ),
                            i.is_remote(),
                        ),
                        None => (false, false, false),
                    }),
                    None => (false, false, false),
                };
                let mut v = vec![];
                if can_cancel {
                    v.push(Item::Label("取消", true, false));
                }
                if can_retry {
                    v.push(Item::Label("重试", true, false));
                    // 重连目标 = 任务涉及远端侧中**已断开**的会话（任一断开即可点；
                    // 实际重建见命令区遍历）。
                    let can_reconnect = remote
                        && st
                            .remotes
                            .iter()
                            .any(|rs| rs.sessions.iter().any(|s| !s.ssh.is_connected()));
                    v.push(Item::Label("重连", can_reconnect, false));
                }
                v.push(Item::Label("移除", true, false));
                v
            }
        }
    } else {
        match row {
            Some(_) => {
                let remote = menu_side(st, pane).is_some();
                let p = pane_view(st, pane);
                let is_dir = row
                    .and_then(|i| p.entries.get(i))
                    .map(|e| e.is_dir)
                    .unwrap_or(true);
                // 「传输到另一侧」启用条件（C# CanTransferToOpposite）：对侧视图非空
                // && 两侧当前路径非空 && 两侧视图各自就绪（本地有路径 / 远程有会话）。
                let other = other_view(st, pane);
                let path_ok =
                    |pp: &super::panes::FilePane| pp.path.is_some() || pp.remote_path.is_some();
                let can_transfer = !other.empty
                    && path_ok(p)
                    && path_ok(other)
                    && view_ready(st, pane)
                    && view_ready(st, 1 - pane);
                if remote {
                    // 「编辑」启用条件（C# RemoteEditMenuEnabled）：非目录 + ≤10MB
                    // + 扩展名不在二进制黑名单（白名单/未知扩展名可点，打开后按内容
                    // 探测回退）。「校验码」启用条件（G9 S6.5）：非目录 + 服务端哈希
                    // 探测任一可用（ServerHashAny，未探测/不可用即置灰）。
                    let can_edit = row
                        .and_then(|i| p.entries.get(i))
                        .map(|e| {
                            !e.is_dir
                                && e.size_bytes <= crate::editor::MAX_FILE_SIZE
                                && !crate::editor::is_remote_binary_ext(&e.name)
                        })
                        .unwrap_or(false);
                    let hash_any = menu_side(st, pane)
                        .and_then(|s| super::active_remote_session(st, s))
                        .and_then(|s| s.ssh.hash_probe())
                        .is_some_and(|h| h.any());
                    // C# BuildItemMenu 远程分支顺序（FileView.xaml.cs:2415-2452）：
                    // 传输到另一侧 / 编辑 ║ 刷新 / 重命名 / 删除(红) ║ 权限 / 校验码
                    // = 7 项 + 2 分隔，首个分隔在「编辑」之后（FV-15 修复：原误置
                    // 「传输到另一侧」后）。
                    vec![
                        Item::Label("传输到另一侧", can_transfer, false),
                        Item::Label("编辑", can_edit, false),
                        Item::Sep,
                        Item::Label("刷新", true, false),
                        Item::Label("重命名", true, false),
                        Item::Label("删除", true, true),
                        Item::Sep,
                        Item::Label("权限", true, false),
                        Item::Label("校验码", !is_dir && hash_any, false),
                    ]
                } else {
                    // 「编辑」启用条件：非目录 + 文本探测 + ≤10MB（C# IsTextFile(item)）。
                    let can_edit = !is_dir
                        && row
                            .and_then(|i| p.entries.get(i))
                            .map(|e| {
                                e.size_bytes <= crate::editor::MAX_FILE_SIZE
                                    && p.path
                                        .as_ref()
                                        .map(|d| crate::editor::is_text_file(&d.join(&e.name)))
                                        .unwrap_or(false)
                            })
                            .unwrap_or(false);
                    // 「打开」恒可用（C# MenuOpen 无 isEnabled）：目录进入下一层 /
                    // 文件用系统默认关联程序打开（OpenEntry）。「新建文件夹/新建文件」
                    // = 当前路径非空（C# _currentPath.Length>0，根视图禁用）。
                    let can_new = p.path.is_some();
                    vec![
                        Item::Label("传输到另一侧", can_transfer, false),
                        Item::Label("编辑", can_edit, false),
                        Item::Label("打开", true, false),
                        Item::Sep,
                        Item::Label("新建文件夹", can_new, false),
                        Item::Label("新建文件", can_new, false),
                        Item::Sep,
                        Item::Label("刷新", true, false),
                        Item::Label("重命名", true, false),
                        Item::Label("删除", true, true),
                    ]
                }
            }
            None => {
                // 空白区菜单（C# BuildBlankMenu）：本地/远程同一份四项——新建文件夹 /
                // 新建文件 / ─ / 刷新；启用 = 远程该侧已连（IsConnected）/ 本地当前
                // 路径非空（_currentPath.Length>0）。C# 无「断开」项。
                let p = pane_view(st, pane);
                // 新建可用 = 本地有路径 / 远程已浏览到目录（与工具栏 can_new 同口径）。
                let can_new = match menu_side(st, pane) {
                    Some(side) => super::pane_view(st, side).remote_path.is_some(),
                    None => p.path.is_some(),
                };
                vec![
                    Item::Label("新建文件夹", can_new, false),
                    Item::Label("新建文件", can_new, false),
                    Item::Sep,
                    Item::Label("刷新", true, false),
                ]
            }
        }
    };
    let sep_n = items.iter().filter(|i| matches!(i, Item::Sep)).count();
    if items.is_empty() {
        // 队列菜单无适用动作（C# 队列空不弹的等价处理）。
        st.menu = None;
        return;
    }
    // 高度只计文本项：渲染循环里分隔线只画 3px 槽（FV-26 修复——原先 items.len()
    // 把 Sep 也按 36px 计入，每条分隔线虚高 33px）。C# MenuFlyout 高度由内容自适应
    // （令牌清单 §7.3：项 36 / 分隔槽 3 / 体上下垫 2）。
    let label_n = items.len() - sep_n;
    let h = label_n as f32 * MENU_ITEM_H + sep_n as f32 * MENU_SEP_H + MENU_BODY_PAD * 2.0;

    // 位置钳制到窗口内。
    let full = ui.max_rect();
    let x = pos.x.min(full.right() - MENU_W - 4.0);
    let y = if pos.y + h > full.bottom() - 4.0 {
        full.bottom() - 4.0 - h
    } else {
        pos.y
    };
    let rect = Rect::from_min_size(Pos2::new(x, y), egui::vec2(MENU_W, h));

    // 轻量关闭：菜单外任何点击（遮罩先挂，菜单体后挂于其上）。
    let catcher = ui.allocate_rect(full, Sense::click());
    if catcher.clicked() {
        st.menu = None;
        return;
    }
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        st.menu = None;
        return;
    }

    // 菜单体：软投影 + 不透明底 + 描边（底 = PaneMenuPresenterStyle 覆写的不透明色，
    // 描边 = SurfaceStrokeColorFlyout，1px 对齐设备像素）。
    // 投影对照 C# MenuFlyout 系统 Fluent 阴影，统一走 theme::draw_shadow。
    theme::draw_shadow(ui, rect, 8.0, theme::ShadowKind::Flyout);
    ui.painter().rect_filled(rect, 8.0, p.menu_bg);
    ui.painter().rect_stroke(
        theme::snap_rect(rect, ppp),
        8.0,
        Stroke::new(theme::border_width(ppp), p.combo_popup_stroke),
        egui::StrokeKind::Inside,
    );

    let mut action: Option<&'static str> = None;
    let mut iy = rect.top() + MENU_BODY_PAD;
    for item in &items {
        match item {
            Item::Sep => {
                // 分隔线（MenuFlyoutSeparator）：1px 近全宽（Margin -4 抵消项外边距），
                // 色 DividerStrokeColorDefault；槽高 3（上下 Margin 1）。
                let w = theme::border_width(ppp);
                let y = ((iy + 1.0 + w * 0.5) * ppp).round() / ppp;
                ui.painter().line_segment(
                    [
                        Pos2::new(rect.left() + 4.0, y),
                        Pos2::new(rect.right() - 4.0, y),
                    ],
                    Stroke::new(w, p.divider_stroke),
                );
                iy += MENU_SEP_H;
            }
            Item::Label(label, enabled, danger) => {
                let r = Rect::from_min_size(
                    Pos2::new(rect.left() + 4.0, iy),
                    egui::vec2(MENU_W - 8.0, MENU_ITEM_H),
                );
                let resp = ui.allocate_rect(r, Sense::click());
                // hover 底 = SubtleFillColorSecondary；按下底 = SubtleFillColorTertiary。
                if *enabled && resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(r, 4.0, p.row_pressed);
                } else if *enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }
                // 禁用 = TextFillColorDisabled；危险红 hover/按下翻回正文色
                // （C# 模板 VSM：PointerOver 设 TextBlock.Foreground = 正文色）。
                let fg = if !*enabled {
                    p.text_disabled
                } else if *danger && !resp.hovered() {
                    theme::MENU_DANGER
                } else {
                    p.text_primary
                };
                ui.painter().text(
                    // C# Padding 上 8 下 9 → 文字中心较几何中心偏上 0.5px。
                    Pos2::new(r.left() + 11.0, r.center().y - 0.5),
                    Align2::LEFT_CENTER,
                    t(*label),
                    FontId::proportional(14.0),
                    fg,
                );
                if *enabled && resp.clicked() {
                    action = Some(label);
                }
                iy += MENU_ITEM_H;
            }
        }
    }

    // 菜单命令（演示态可落地：新建/重命名/删除/权限/校验码；其余仅关闭菜单）。
    if let Some(label) = action {
        st.menu = None;
        // 菜单绘制于帧末（窗格/终端本帧已画完）：本地新建/刷新/删除等同步改
        // pane 的动作要下一帧才可见 —— egui 无事件不重绘，必须主动约一帧，
        // 否则画面停在旧列表直到下一次鼠标事件（「刷新不起作用」断档根因）。
        ui.ctx().request_repaint();
        // 终端菜单（C# ClearAll_Click / CopyAll_Click）。
        if term_menu {
            match label {
                "清除全部" => st.terminal.clear(),
                "复制全部" => {
                    let text = st
                        .terminal
                        .iter()
                        .map(|(t, s)| format!("{t}  {s}"))
                        .collect::<Vec<_>>()
                        .join("\r\n");
                    ui.ctx().copy_text(text);
                }
                _ => {}
            }
            return;
        }
        // 批量动作目标集（C# :2405-2407 右键先切选中）：命中已选集 → 整组选中
        // 一起执行（:2579 删除 / :3044 传输）；否则仅右键行。
        let rows: Vec<usize> = match row {
            Some(i) => {
                let pane_d = pane_view(st, pane);
                if pane_d.selected.contains(&i) {
                    pane_d.selected.clone()
                } else {
                    vec![i]
                }
            }
            None => Vec::new(),
        };
        match label {
            "编辑" => {
                // 顺序借用：先快照条目名与当前目录（pane_view 借 st），结束后再写状态。
                let snap = {
                    let p = pane_view(st, pane);
                    let side = menu_side(st, pane);
                    row.and_then(|i| p.entries.get(i))
                        .map(|e| (side, e.name.clone(), p.path.clone(), p.remote_path.clone()))
                };
                if let Some((side, name, local_dir, remote_dir)) = snap {
                    match side {
                        None => {
                            // 本地：文本探测已在菜单启用条件做过。外部编辑器模式
                            // （C# OpenWithEditor）→ 独立进程启动；失败记终端
                            // （C# UpdateStatus(ex.Message)）。否则内置编辑器。
                            if let Some(dir) = local_dir {
                                let path = dir.join(&name);
                                let (external, editor_path) = crate::settings::with(|s| {
                                    (!s.editor_builtin, s.editor_path.trim().to_owned())
                                });
                                if external && !editor_path.is_empty() {
                                    if let Err(e) =
                                        std::process::Command::new(&editor_path).arg(&path).spawn()
                                    {
                                        st.terminal.push((
                                            crate::local::now_hms(),
                                            tfmt("外部编辑器启动失败：{0}", &[&e.to_string()]),
                                        ));
                                    }
                                } else {
                                    let dark = ui.ctx().theme() == egui::Theme::Dark;
                                    st.editors.open_local(&path, dark);
                                }
                            }
                        }
                        Some(side) => {
                            if let Some(rp) = remote_dir {
                                // 远程：下载到临时副本（回流失配丢弃）→ 开编辑器窗口。
                                // 请求挂发起帧活动远程 tab（P1 每 tab 一会话）。
                                let remote = super::join_remote_path(&rp, &name);
                                st.stat_seq += 1;
                                let req = st.stat_seq;
                                if let Some(tab) = super::active_remote_tab_mut(st, side) {
                                    tab.pending_ed = Some((req, remote.clone(), name.clone()));
                                }
                                if let Some(ss) = super::active_remote_session(st, side) {
                                    ss.ssh.read_file(remote, req, crate::editor::MAX_FILE_SIZE);
                                }
                                let tag = super::active_remote_session(st, side)
                                    .and_then(|s| s.host.clone())
                                    .map(|h| format!("[{}@{}]", h.username, h.host))
                                    .unwrap_or_else(|| t("[远程]").to_owned());
                                st.terminal.push((
                                    crate::local::now_hms(),
                                    format!("{tag} {}", tfmt("下载 {0}…", &[&name])),
                                ));
                            }
                        }
                    }
                }
            }
            "打开" => {
                // C# OpenEntry：目录进入下一层；文件用系统默认关联程序打开。
                // 顺序借用：先快照条目与当前目录，再写 st（list_reqs / sync_nav）。
                if let Some(i) = row {
                    let snap = {
                        let p = pane_view(st, pane);
                        p.entries.get(i).map(|e| {
                            (
                                e.is_dir,
                                e.name.clone(),
                                p.path.clone(),
                                p.remote_path.clone(),
                            )
                        })
                    };
                    if let Some((is_dir, name, local_dir, remote_dir)) = snap {
                        if is_dir {
                            // 目录进入（file_rows 双击同款三分支）。
                            if let Some(rp) = remote_dir {
                                // 远程视图（pane 即侧号）：列目录请求带 (侧, tab id) 入队。
                                let target = if rp == "/" {
                                    format!("/{name}")
                                } else {
                                    format!("{rp}/{name}")
                                };
                                st.list_reqs.push((
                                    pane,
                                    super::active_tab_id(st, pane).unwrap_or(0),
                                    target,
                                ));
                            } else if local_dir.is_none() {
                                // 本地根视图（此电脑）：条目即盘符，直接按盘符导航。
                                let target = std::path::PathBuf::from(name.as_str());
                                if crate::local::navigate(pane_view_mut(st, pane), &target) {
                                    let tab_id = super::active_tab_id(st, pane).unwrap_or(0);
                                    st.sync_nav =
                                        Some((pane, tab_id, target.to_string_lossy().into_owned()));
                                }
                            } else if let Some(base) = local_dir {
                                let target = base.join(&name);
                                if crate::local::navigate(pane_view_mut(st, pane), &target) {
                                    let tab_id = super::active_tab_id(st, pane).unwrap_or(0);
                                    st.sync_nav =
                                        Some((pane, tab_id, target.to_string_lossy().into_owned()));
                                }
                            }
                        } else if let Some(base) = local_dir {
                            // 本地文件：系统默认关联程序（远程条目无「打开」项，C# 同）。
                            crate::local::open_with_default(&base.join(&name));
                        }
                    }
                }
            }
            "重命名" => {
                if let Some(i) = row {
                    let orig = pane_view(st, pane).entries.get(i).map(|e| e.name.clone());
                    if let Some(orig) = orig {
                        st.dialog = Dialog::Rename {
                            pane,
                            orig: orig.clone(),
                            name: orig,
                        };
                    }
                }
            }
            "删除" => {
                // 多选删除（C# :2579 SelectedItems 全量，占位行已在选中链路排除）。
                if !rows.is_empty() {
                    st.dialog = Dialog::Delete {
                        pane,
                        indices: rows,
                    };
                }
            }
            "权限" => {
                if let Some(i) = row {
                    // 顺序借用：快照条目（perms/name/is_dir），结束后再写 st.dialog。
                    let snap = pane_view(st, pane)
                        .entries
                        .get(i)
                        .map(|e| (e.name.clone(), e.is_dir, e.perms.clone()));
                    if let Some((name, is_dir, perms)) = snap {
                        let mut bits = [false; 9];
                        for (k, c) in perms.chars().skip(1).take(9).enumerate() {
                            bits[k] = c != '-';
                        }
                        st.dialog = Dialog::Permission {
                            pane,
                            name,
                            is_dir,
                            bits,
                        };
                    }
                }
            }
            "校验码" => {
                if let Some(i) = row {
                    // 顺序借用：快照条目名与当前目录，再写 st.dialog / 发起计算。
                    let snap = {
                        let p = pane_view(st, pane);
                        let side = menu_side(st, pane);
                        p.entries
                            .get(i)
                            .map(|e| (e.name.clone(), side, p.remote_path.clone(), p.path.clone()))
                    };
                    let Some((name, side, remote_dir, local_dir)) = snap else {
                        return;
                    };
                    let path = match side {
                        Some(_) => remote_dir
                            .as_ref()
                            .map(|d| super::join_remote_path(d, &name)),
                        None => local_dir
                            .as_ref()
                            .map(|d| d.join(&name).to_string_lossy().into_owned()),
                    };
                    let Some(path) = path else {
                        return;
                    };
                    // G9：默认算法按探测快照取首个可用（SHA256 → SHA512 → SM3）；
                    // 本地恒 SHA-256（ring 自算不依赖探测）。
                    let algo = match side
                        .and_then(|s| super::active_remote_session(st, s))
                        .and_then(|s| s.ssh.hash_probe())
                    {
                        Some(h) if h.sha256 => 0,
                        Some(h) if h.sha512 => 1,
                        Some(h) if h.sm3 => 2,
                        _ => 0,
                    };
                    st.dialog = Dialog::Checksum {
                        name,
                        path: path.clone(),
                        side,
                        algo,
                        copied: None,
                    };
                    checksum_start(st, algo, &path, side);
                }
            }
            "新建文件夹" | "新建文件" => {
                let is_dir = label == "新建文件夹";
                let base = if is_dir {
                    t("新建文件夹")
                } else {
                    t("新建文本文档.txt")
                };
                // 远程 tab：经该 tab 挂靠会话新建（重名规避按当前列表，Ev::Created
                // 回执后重列）；本地 tab：同步创建 + 重列。
                if let Some(side) = menu_side(st, pane) {
                    if let Some(rp) = pane_view(st, pane).remote_path.clone() {
                        let name = super::panes::dedupe_remote(pane_view(st, pane), base);
                        let target = super::join_remote_path(&rp, &name);
                        st.new_reqs.push((
                            side,
                            super::active_tab_id(st, side).unwrap_or(0),
                            target,
                            is_dir,
                        ));
                    }
                } else if is_dir {
                    crate::local::create_dir(pane_view_mut(st, pane), base);
                } else {
                    crate::local::create_file(pane_view_mut(st, pane), base);
                }
            }
            "刷新" => {
                // 触发刷新动效（列表淡入 + 刷新钮旋转，与工具栏刷新同口径）。
                pane_view_mut(st, pane).refresh_at = Some(ui.ctx().input(|i| i.time));
                if let Some(rp) = pane_view(st, pane).remote_path.clone() {
                    // 远程视图（pane 即侧号）：重列该活动 tab 目录。
                    st.list_reqs
                        .push((pane, super::active_tab_id(st, pane).unwrap_or(0), rp));
                } else {
                    crate::local::refresh(pane_view_mut(st, pane));
                }
            }
            "传输到另一侧" => {
                // 批量传输（C# :3044 SelectedItems；单条即单元素批次）。
                if !rows.is_empty() {
                    super::begin_transfer_multi(st, pane, &rows);
                }
            }
            // ── 队列菜单 ──
            "暂停" => {
                if let Some(q) = &st.transfer {
                    q.pause();
                }
            }
            "继续传输" => {
                if let Some(q) = &st.transfer {
                    q.resume();
                }
            }
            "停止" => {
                if let Some(q) = &st.transfer {
                    q.stop_all();
                }
            }
            "清除已完成" => {
                if let Some(q) = &st.transfer {
                    q.clear_done();
                }
            }
            "清除所有" => {
                if let Some(q) = &st.transfer {
                    q.clear_all();
                }
            }
            "重置并重新请求所有" => {
                if let Some(q) = &st.transfer {
                    let ids = q.with_items(|items| {
                        items
                            .iter()
                            .filter(|i| {
                                matches!(
                                    i.status,
                                    crate::transfer::Status::Failed
                                        | crate::transfer::Status::Cancelled
                                )
                            })
                            .map(|i| i.id)
                            .collect::<Vec<_>>()
                    });
                    for id in ids {
                        q.retry(id);
                    }
                }
            }
            "重试" => {
                if let Some(QueueMenu::Item(id)) = queue
                    && let Some(q) = &st.transfer
                {
                    q.retry(id);
                }
            }
            "取消" => {
                if let Some(QueueMenu::Item(id)) = queue
                    && let Some(q) = &st.transfer
                {
                    q.cancel(id);
                }
            }
            "重连" => {
                // C# RowReconnectItem_Click：只重建 SSH 会话，不重跑任务、不改队列
                // 状态（行保持失败态，用户自行决定是否再点「重试」）。Rust 复用连接
                // 提交入口按上次连接参数重建（结果经 Ev::Connected / Ev::Failed 回流）；
                // 遍历两侧会话池重建所有断开的会话所属 tab（行不记录侧号，按
                // 「断开即重连」口径）。
                let mut downs: Vec<(usize, u32, crate::hosts::HostProfile)> = Vec::new();
                for side in 0..2 {
                    for tab in &st.remotes[side].tabs {
                        if let Some(ss) =
                            st.remotes[side].sessions.iter().find(|s| s.sid == tab.sid)
                            && !ss.ssh.is_connected()
                            && let Some(h) = ss.host.clone()
                        {
                            downs.push((side, tab.id, h));
                        }
                    }
                }
                for (side, tab_id, h) in downs {
                    reconnect_tab(st, side, tab_id, &h);
                }
            }
            "移除" => {
                if let Some(QueueMenu::Item(id)) = queue
                    && let Some(q) = &st.transfer
                {
                    q.remove(id);
                }
            }
            _ => {}
        }
    }
}

// ── 传输覆盖确认（C# 冲突确认对话框）────────────────────────
fn transfer_confirm_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(content) = shell(
        ui,
        st,
        p,
        640.0,
        360.0,
        t("传输确认"),
        false,
        0.0,
        false,
        None,
    ) else {
        return;
    };
    // 行数据先拷为展示串（按钮区需要可变借用 st，先结束对 st.dialog 的共享借用）。
    let rows: Vec<(String, (String, String), (String, String), bool)> = match &st.dialog {
        Dialog::TransferConfirm { rows } => rows
            .iter()
            .map(|r| {
                // 名称超长省略（列宽固定，Grid 不裁剪 Label）。
                let shown: String = r.name.chars().take(22).collect();
                let name = if r.name.chars().count() > 22 {
                    format!("{shown}…")
                } else {
                    shown
                };
                let src = (
                    crate::local::format_size(r.src_size),
                    crate::local::fmt_mtime_unix(r.src_mtime),
                );
                let dst = match (r.dst_size, r.dst_mtime) {
                    (Some(s), Some(m)) => (
                        crate::local::format_size(s),
                        crate::local::fmt_mtime_unix(m),
                    ),
                    (Some(s), None) => (crate::local::format_size(s), String::new()),
                    (None, Some(m)) => (String::new(), crate::local::fmt_mtime_unix(m)),
                    (None, None) => (String::new(), String::new()),
                };
                // 目标较源新 → 覆盖将砸掉较新的目标文件，目标列警示色。
                let newer = r.dst_mtime.is_some_and(|m| m > r.src_mtime);
                (name, src, dst, newer)
            })
            .collect(),
        _ => return,
    };

    let mut cui = content_ui(ui, content);
    cui.add_space(10.0);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(RichText::new(tfmt("目标已存在 {0} 个同名项目：", &[&rows.len()])).size(13.0));
    });
    cui.add_space(6.0);

    // 冲突清单对比表（名称 / 源 / 目标，大小+mtime 两行；超出可视高滚动）。
    let list_h = (content.bottom() - CONTENT_PAD.1 - BTN_H - 8.0 - cui.cursor().top()).max(40.0);
    egui::ScrollArea::vertical()
        .id_salt("tx-conflict-list")
        .max_height(list_h)
        .show(&mut cui, |s| {
            egui::Grid::new("tx-conflict-grid")
                .num_columns(3)
                .spacing([16.0, 5.0])
                .show(s, |g| {
                    for head in [t("名称"), t("源"), t("目标")] {
                        g.label(RichText::new(head).size(12.0).color(p.text_secondary));
                    }
                    g.end_row();
                    for (name, src, dst, newer) in &rows {
                        let d_col = if *newer {
                            theme::WARNING
                        } else {
                            p.text_secondary
                        };
                        g.label(RichText::new(name.as_str()).size(13.0));
                        g.vertical(|v| {
                            v.label(
                                RichText::new(src.0.as_str())
                                    .size(12.0)
                                    .color(p.text_secondary),
                            );
                            v.label(
                                RichText::new(src.1.as_str())
                                    .size(12.0)
                                    .color(p.text_secondary),
                            );
                        });
                        g.vertical(|v| {
                            if dst.0.is_empty() && dst.1.is_empty() {
                                v.label(RichText::new("—").size(12.0).color(p.text_secondary));
                            } else {
                                if !dst.0.is_empty() {
                                    v.label(RichText::new(dst.0.as_str()).size(12.0).color(d_col));
                                }
                                if !dst.1.is_empty() {
                                    v.label(RichText::new(dst.1.as_str()).size(12.0).color(d_col));
                                }
                            }
                        });
                        g.end_row();
                    }
                });
        });

    // 右对齐按钮组：跳过冲突项 / 全部覆盖 / 取消。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_over = btn_w(ui, t("全部覆盖"));
    let w_skip = btn_w(ui, t("跳过冲突项"));
    let w_cancel = btn_w(ui, t("取消"));
    let x0 = content.right() - CONTENT_PAD.0;
    let r_cancel = Rect::from_min_size(Pos2::new(x0 - w_cancel, y), egui::vec2(w_cancel, BTN_H));
    let r_over = Rect::from_min_size(
        Pos2::new(x0 - w_cancel - 8.0 - w_over, y),
        egui::vec2(w_over, BTN_H),
    );
    let r_skip = Rect::from_min_size(
        Pos2::new(x0 - w_cancel - 8.0 - w_over - 8.0 - w_skip, y),
        egui::vec2(w_skip, BTN_H),
    );

    let over = std_button(ui, r_over, t("全部覆盖"), p.text_primary, p);
    let skip = std_button(ui, r_skip, t("跳过冲突项"), p.text_primary, p);
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {
        // 取消整批（丢弃 pending）。
        st.pending_tx = None;
        st.dialog = Dialog::None;
    } else if skip {
        super::enqueue_pending(st, true);
        st.dialog = Dialog::None;
    } else if over {
        super::enqueue_pending(st, false);
        st.dialog = Dialog::None;
    }
}

// ── 同步失配提示（C# ReportMismatch ContentDialog）────────────
fn sync_mismatch_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(content) = shell(
        ui,
        st,
        p,
        400.0,
        172.0,
        t("同步浏览"),
        false,
        0.0,
        false,
        None,
    ) else {
        return;
    };
    let msg = match &st.dialog {
        Dialog::SyncMismatch { msg } => msg.clone(),
        _ => return,
    };

    let mut cui = content_ui(ui, content);
    cui.add_space(16.0);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(RichText::new(&msg).size(13.0));
    });
    cui.add_space(4.0);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(
            RichText::new(t("已关闭同步联动。可重新定位两侧目录后手动建立。"))
                .size(12.0)
                .color(p.text_secondary),
        );
    });
    cui.add_space(12.0);

    // 右对齐按钮组：重新建立 / 关闭。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_retry = btn_w(ui, t("重新建立"));
    let w_close = btn_w(ui, t("关闭"));
    let x0 = content.right() - CONTENT_PAD.0;
    let r_close = Rect::from_min_size(Pos2::new(x0 - w_close, y), egui::vec2(w_close, BTN_H));
    let r_retry = Rect::from_min_size(
        Pos2::new(x0 - w_close - 8.0 - w_retry, y),
        egui::vec2(w_retry, BTN_H),
    );

    let retry = std_button(ui, r_retry, t("重新建立"), p.text_primary, p);
    let close = std_button(ui, r_close, t("关闭"), p.text_primary, p);
    if retry {
        // 重新评估当前两侧目录（C# RestartSyncBrowse：目录已对齐即重新点亮）。
        let ev = super::sync::evaluate(st);
        st.dialog = Dialog::None;
        if ev.is_ok() {
            st.sync_browse = true;
        } else {
            // 仍失配：终端提示，不重复弹窗。
            st.terminal.push((
                crate::local::now_hms(),
                t("同步浏览: 两侧目录仍不一致").to_owned(),
            ));
        }
    } else if close {
        st.dialog = Dialog::None;
    }
}

// ── 外部编辑器：缓存变化询问 / 远端冲突确认（C# ShowRemoteEditChangedAsync /
// ShowRemoteConflictAsync 紧凑对话框，标题 MenuEdit=编辑）──────────────

/// 编辑器保存落盘（快照静默期满）→ 询问是否回传。
fn ext_edit_changed_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(content) = shell(ui, st, p, 380.0, 150.0, t("编辑"), false, 0.0, false, None) else {
        return;
    };
    let (side, tab, name, remote_path) = match &st.dialog {
        Dialog::ExtEditChanged {
            side,
            tab,
            name,
            remote_path,
        } => (*side, *tab, name.clone(), remote_path.clone()),
        _ => return,
    };

    let mut cui = content_ui(ui, content);
    cui.add_space(16.0);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(
            RichText::new(tfmt(
                "\"{0}\" 的文件缓存已经改变，是否保存编辑结果？",
                &[&name],
            ))
            .size(13.0),
        );
    });

    // 右对齐按钮组（C# ContentDialog：Primary 左 / Close 右）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_save = btn_w(ui, t("保存编辑结果"));
    let w_cancel = btn_w(ui, t("取消"));
    let x0 = content.right() - CONTENT_PAD.0;
    let r_cancel = Rect::from_min_size(Pos2::new(x0 - w_cancel, y), egui::vec2(w_cancel, BTN_H));
    let r_save = Rect::from_min_size(
        Pos2::new(x0 - w_cancel - 8.0 - w_save, y),
        egui::vec2(w_save, BTN_H),
    );

    let save = std_button(ui, r_save, t("保存编辑结果"), p.text_primary, p);
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {
        // 暂不保存：会话保留（基准已在触发时更新），下次保存快照再变会再问。
        st.dialog = Dialog::None;
    } else if save {
        // 回传前先探测远端 mtime（C# GetLastWriteTime 冲突比对）：探测在途
        // 挂靠发起 tab，STAT 经其挂靠会话发出。
        st.stat_seq += 1;
        let req = st.stat_seq;
        if let Some(t) = super::find_remote_tab_mut(st, side, tab) {
            t.ext_stat = Some((req, remote_path.clone()));
        }
        if let Some(ssh) = super::session_of_tab(st, side, tab).map(|s| &s.ssh) {
            ssh.stat(remote_path.clone(), req);
        }
        st.dialog = Dialog::None;
    }
}

/// 回传前探测到远端已变更（mtime 与下载基准差 ≥1s）→ 覆盖确认。
fn ext_edit_conflict_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(content) = shell(ui, st, p, 380.0, 150.0, t("编辑"), false, 0.0, false, None) else {
        return;
    };
    let (side, tab, remote_path) = match &st.dialog {
        Dialog::ExtEditConflict {
            side,
            tab,
            remote_path,
            ..
        } => (*side, *tab, remote_path.clone()),
        _ => return,
    };

    let mut cui = content_ui(ui, content);
    cui.add_space(16.0);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(RichText::new(t("远端文件已被修改，仍要覆盖吗？")).size(13.0));
    });

    // 右对齐按钮组（C# ContentDialog：Primary 左 / Close 右）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_over = btn_w(ui, t("覆盖"));
    let w_cancel = btn_w(ui, t("取消"));
    let x0 = content.right() - CONTENT_PAD.0;
    let r_cancel = Rect::from_min_size(Pos2::new(x0 - w_cancel, y), egui::vec2(w_cancel, BTN_H));
    let r_over = Rect::from_min_size(
        Pos2::new(x0 - w_cancel - 8.0 - w_over, y),
        egui::vec2(w_over, BTN_H),
    );

    let overwrite = std_button(ui, r_over, t("覆盖"), p.text_primary, p);
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {
        // 不覆盖：会话保留（基准已在触发时更新），下次保存快照再变会再问。
        st.dialog = Dialog::None;
    } else if overwrite {
        super::ext_upload(st, side, tab, &remote_path);
        st.dialog = Dialog::None;
    }
}

// ── 关闭标签页确认（G17，C# FolderContainer.OnTabCloseConfirming ContentDialog）──
/// 三键（Primary 后台继续 / Secondary 取消队列并关闭 / Close 不关了，
/// DefaultButton=Close）：确认判据为该 tab 归属的活动任务数（P3 per-tab，
/// C# LogScope 语义）；「取消队列并关闭」也只取消归属该 tab 的活动任务。
fn tab_close_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(content) = shell(
        ui,
        st,
        p,
        360.0,
        160.0,
        t("关闭标签页"),
        false,
        0.0,
        false,
        None,
    ) else {
        return;
    };
    let (side, tab, n) = match &st.dialog {
        Dialog::TabClose { side, tab, n } => (*side, *tab, *n),
        _ => return,
    };

    let mut cui = content_ui(ui, content);
    cui.add_space(14.0);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(
            RichText::new(tfmt(
                "此标签页仍有 {0} 个进行中的传输任务，如何处理？",
                &[&n],
            ))
            .size(13.0),
        );
    });
    cui.add_space(12.0);

    // 右对齐按钮组（C# ContentDialog 布局 Primary 左 / Secondary 中 / Close 右，
    // DefaultButton=Close → 「不关了」最右）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_stay = btn_w(ui, t("不关了"));
    let w_cancel = btn_w(ui, t("取消队列并关闭"));
    let w_bg = btn_w(ui, t("后台继续"));
    let x0 = content.right() - CONTENT_PAD.0;
    let r_stay = Rect::from_min_size(Pos2::new(x0 - w_stay, y), egui::vec2(w_stay, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_stay - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );
    let r_bg = Rect::from_min_size(
        Pos2::new(x0 - w_stay - 8.0 - w_cancel - 8.0 - w_bg, y),
        egui::vec2(w_bg, BTN_H),
    );

    let bg = std_button(ui, r_bg, t("后台继续"), p.text_primary, p);
    let cancel = std_button(ui, r_cancel, t("取消队列并关闭"), p.text_primary, p);
    let stay = std_button(ui, r_stay, t("不关了"), p.text_primary, p);
    if stay {
        st.dialog = Dialog::None;
    } else if cancel {
        // 取消队列并关闭：只取消归属该 tab 的活动任务（P3 per-tab，C#
        // LogScope 语义；Waiting 即时结算 / Transferring 块边界生效），root()
        // 等「该 tab 归属活动任务清零」后执行关闭（5s 超时兜底放行）。
        if let Some(q) = &st.transfer {
            q.cancel_owned((side as u8, tab));
        }
        st.dialog = Dialog::None;
        st.tab_close_wait = Some((side, tab, std::time::Instant::now()));
    } else if bg {
        // 后台继续（C# 同注：传输连接独立于标签页，tab 关闭后照常跑完）。
        st.dialog = Dialog::None;
        super::finish_close_tab(st, side, tab);
    }
}

// ── 主机指纹确认覆盖层（C# HostKeyConfirmDialog，P0 G4/G6）──
/// 指纹确认覆盖层。不复用 shell（其关闭路径写 st.dialog 会杀掉底下弹窗）：
/// 全屏遮罩（点击不关闭，防误拒）+ 居中卡片（标题行 + 换行正文 + 右下按钮）。
/// Enter = 默认按钮（首次 → 信任并继续；变更 → 取消，防中间人）；Esc = 取消。
fn fp_confirm_overlay(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    // 查找序：窗格会话池（侧 0 → 侧 1 逐会话）→ 测试连接槽；无请求不绘制。
    let Some(ask) = st
        .remotes
        .iter()
        .find_map(|r| r.sessions.iter().find_map(|s| s.fp_ask.as_ref()))
        .or(st.fp_ask_test.as_ref())
    else {
        return;
    };
    let first = ask.first;
    let (w, h) = if first {
        (420.0, 250.0)
    } else {
        (420.0, 330.0)
    };
    let title = if first {
        t("确认主机指纹")
    } else {
        t("主机密钥已变更")
    };
    // 正文（C# S4.1/S4.2 逐字对齐；中文态 t 原样回键，英文查表）。
    let body = if first {
        tfmt(
            "首次连接 {0}，无法验证主机身份。\n\n主机密钥指纹 (SHA256)：\n{1}\n\n信任该主机并继续连接？",
            &[&ask.host, &ask.presented],
        )
    } else {
        tfmt(
            "服务器 {0} 出示的主机密钥与此前信任的不一致。\n\n此前信任：{1}\n本次出示：{2}\n\n常见原因：服务器重装/更换机器，或主机卡片曾修改过 IP。\n若你知晓此变更，可选择更新信任；否则请拒绝，以防中间人攻击。",
            &[
                &ask.host,
                &ask.stored.as_deref().unwrap_or(""),
                &ask.presented,
            ],
        )
    };

    let full = ui.max_rect();
    let esc = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape));
    let enter = ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));

    // 全屏遮罩（Sense::hover：点击不关闭 —— C# ContentDialog 遮罩无点击语义）。
    ui.allocate_rect(full, Sense::hover());
    ui.painter().rect_filled(full, 0.0, theme::mask());

    // 卡片：投影 + 一体底 + 描边 + 命中面（吸收穿透，同 shell 口径）。
    let card = Rect::from_center_size(full.center(), egui::vec2(w, h));
    theme::draw_shadow(ui, card, theme::CONTAINER_RADIUS, theme::ShadowKind::Dialog);
    ui.painter()
        .rect_filled(card, theme::CONTAINER_RADIUS, p.dialog_title_bar);
    ui.painter().rect_stroke(
        card,
        theme::CONTAINER_RADIUS,
        Stroke::new(1.0, p.card_stroke),
        egui::StrokeKind::Inside,
    );
    ui.allocate_rect(card, Sense::click());
    let inner = card.shrink(1.0);

    // 标题行（16px brand 字族，同 shell 大标题口径；无关闭钮 —— 必须经按钮决断）。
    let title_bar = Rect::from_min_max(
        inner.min,
        Pos2::new(inner.right(), inner.top() + TITLE_BAR_H),
    );
    ui.painter().text(
        Pos2::new(inner.left() + TITLE_PAD.0, title_bar.center().y),
        Align2::LEFT_CENTER,
        title,
        FontId::new(16.0, FontFamily::Name("brand".into())),
        p.text_primary,
    );

    // 正文：左缩进 + 13px 自动换行（top_down 布局 wrap 到卡片内宽）。
    let btn_y = inner.bottom() - CONTENT_PAD.1 - BTN_H;
    let body_rect = Rect::from_min_max(
        Pos2::new(inner.left() + CONTENT_PAD.0, title_bar.bottom() + 6.0),
        Pos2::new(inner.right() - CONTENT_PAD.0, btn_y - 8.0),
    );
    let mut bui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    bui.label(RichText::new(&body).size(13.0));

    // 右下按钮组：取消左、主钮右（间距 8，同传输确认弹窗排布）。
    let primary = if first {
        t("信任并继续")
    } else {
        t("更新并信任")
    };
    let w_pri = btn_w(ui, primary);
    let w_cancel = btn_w(ui, t("取消"));
    let x0 = inner.right() - CONTENT_PAD.0;
    let r_pri = Rect::from_min_size(Pos2::new(x0 - w_pri, btn_y), egui::vec2(w_pri, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_pri - 8.0 - w_cancel, btn_y),
        egui::vec2(w_cancel, BTN_H),
    );
    let pri = std_button(ui, r_pri, primary, p.text_primary, p);
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);

    // 决断：按钮点击 / Enter（默认按钮随场景）/ Esc；None = 继续等待 worker。
    let decision = if pri {
        Some(true)
    } else if cancel || esc {
        Some(false)
    } else if enter {
        Some(first) // 首次默认信任；变更默认取消（防中间人，C# S4.2）。
    } else {
        None
    };
    if let Some(ok) = decision {
        // 回写与查找序同构：窗格会话池优先，其次测试连接槽。
        let a = st
            .remotes
            .iter_mut()
            .find_map(|r| r.sessions.iter_mut().find_map(|s| s.fp_ask.take()))
            .or_else(|| st.fp_ask_test.take());
        if let Some(a) = a {
            let _ = a.reply.send(ok);
        }
    }
}

// ============================================================================
// 主机 tab 命名测试（第三批）—— 翻译自 C# HostTabNamingTests（8 用例）
// 追加到 src/ui/dialogs.rs 末尾。
// C# 的 null 参数在 Rust 用 "" 表达（host_tab_naming 签名 &str）。
// ============================================================================

#[cfg(test)]
mod naming_tests {
    use super::host_tab_naming;

    #[test]
    fn naming_table() {
        assert_eq!(
            host_tab_naming("我的服务器", "192.168.1.10"),
            "我的服务器 (192.168.1.10)"
        );
        assert_eq!(
            host_tab_naming("  web  ", "  example.com  "),
            "web (example.com)"
        ); // 两端裁剪
        assert_eq!(host_tab_naming("", "10.0.0.1"), "10.0.0.1"); // 名称空只显地址
        assert_eq!(host_tab_naming("", "10.0.0.1"), "10.0.0.1"); // C# null → ""
        assert_eq!(host_tab_naming("10.0.0.1", "10.0.0.1"), "10.0.0.1"); // 同名不重复
        assert_eq!(host_tab_naming("vps", ""), "vps"); // 地址缺失回退名称
        assert_eq!(host_tab_naming("", ""), ""); // 双空：空标题
        assert_eq!(host_tab_naming("   ", ""), ""); // 纯空白名称：空标题
    }
}
