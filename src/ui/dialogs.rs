//! 弹窗与浮层（对照 C# RenameInputDialog / DeleteConfirmDialog / ChecksumDialog /
//! PermissionDialog / HostPickerPanel / SettingsPanel + FileView 右键菜单）。
//!
//! 统一外壳（README 3.9 弹框统一原则，令牌清单 §7.2）：
//! 遮罩 #80000000（点击关闭）→ 居中卡片（DialogTitleBarBrush 一体底 + 1px 描边 +
//! 圆角 8 + 阴影）→ 标题行（padding 左右 = inset5+15 / 上5 下2 / 关闭钮 28×28 /
//! 标题 14 或 16）→ 内容圆弧容器（距边 5 / 圆角 6 / CardBackgroundFill 底 /
//! 内边距 15）。
//! ESC / 遮罩 / 关闭钮 均关闭；标题行拖动窗口（C# TitleBar_PointerMoved 同效果）：
//! 主机/设置面板在主窗口内平移（C# 无边框窗口语义），小弹窗保持视口拖动。
//! 设置面板值每帧经 `Settings::sync_to_store` 整包回写 AppSettings（T19-1/T19-3
//! 主题/语言「未动保快照」语义，见 from_store / theme_write_back / lang_write_back）。

use egui::{
    Align, Align2, Area, Color32, FontId, Id, Layout, Margin, Order, Pos2, Rect, RichText, Sense,
    Stroke, Vec2,
};

use crate::i18n::{t, t_ctx, tfmt};
use crate::theme::{self, BTN_H, CLOSE_SIZE, CONTENT_INSET, CONTENT_PAD, GROUP_GAP, TITLE_BAR_H};

use super::State;
use super::{CursorLayer, cursor_intent};

/// C3①（F-9/F5）：本帧有 card_combo 展开态（模块级旗标——card_combo 无 State
/// 入参）。语义：shell 入口 swap 读走「上一帧终态」并清底；card_combo 末尾
/// 终态仍展开才写 true（关闭的组合不写 false——同帧其他组合可能仍展开，
/// store-only-true 防互踩；旗标底由 shell 入口每帧 swap 清，无残留窗口）。
/// Esc 展开态先收组合（F5：Esc 不再一击关掉整个弹窗），Enter 不触发主钮。
static COMBO_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// ── 外壳令牌（theme-tokens-csharp.md §7.2 + 用户定案 15px 口径）──────────────
// 布局常量统一收口 theme.rs（F-11：dialogs / import_dialog / host_panel 三份
// 合一，经顶部 use 导入 TITLE_BAR_H / CLOSE_SIZE / CONTENT_PAD / CONTENT_INSET /
// GROUP_GAP / BTN_H）。此处仅保留派生量与弹窗专属量（MENU_* / TBL_*）。
/// 标题行 padding（左,上,右,下）。左/右 = 内容 inset 5 + 内容 padding 15：
/// 标题文本与 content 文本左对齐、关闭钮右缘与 content 右界（按钮行右缘）
/// 对齐（2026-11-11 用户定：title 两侧与 content 对齐，非距卡片边 15）。
const TITLE_PAD: (f32, f32, f32, f32) = (
    CONTENT_INSET + CONTENT_PAD.0,
    5.0,
    CONTENT_INSET + CONTENT_PAD.0,
    2.0,
);
/// 右键菜单宽（令牌清单 §7.3）。
pub(super) const MENU_W: f32 = 200.0;
/// 右键菜单项高（MenuFlyoutItem：Padding `11,8,11,9` + 14px 字行高 ≈ 19，
/// MinHeight 32 被内容撑过，实测 ≈ 36）。
pub(super) const MENU_ITEM_H: f32 = 36.0;
/// 菜单分隔线槽高（MenuFlyoutSeparator：Height 1 + Margin 上下 1）。
const MENU_SEP_H: f32 = 3.0;
/// 菜单体上下留白（MenuFlyoutPresenterThemePadding `0,2,0,2`）。
pub(super) const MENU_BODY_PAD: f32 = 2.0;

// ── 冲突对比表令牌（TransferConfirm：批量队列 / 单文件覆盖警告共用）──────
/// 表头字号。
const TBL_HEAD_SIZE: f32 = 12.0;
/// 表身字号（比表头小一号，2026-11-11 用户定）。
const TBL_BODY_SIZE: f32 = 11.0;
/// 单元格内边距（四周各 2px，2026-11-11 用户定）。
const TBL_CELL_PAD: i8 = 2;
/// size 与日期同行之间的空隔（同一行不换行，中间留空隔开）。
const TBL_VAL_GAP: f32 = 12.0;

// ── 弹窗状态 ────────────────────────────────────────────────
/// 当前弹窗。`None` = 无弹窗。
pub enum Dialog {
    None,
    /// 重命名：窗格 + 原名 + 输入中名字。
    Rename {
        pane: usize,
        orig: String,
        name: String,
        /// 打开首帧旗标（C# :64-68 打开即全选原名 + 聚焦；首帧消费置 false）。
        fresh: bool,
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
    /// 大小+mtime），逐项勾选分流（F-10 方案A：勾 = 覆盖上传 / 不勾 = 跳过，
    /// 「应用」只处理勾选项；取消整批）。勾选态存 State.conflict_sel。
    TransferConfirm {
        /// 冲突清单行（目标较新行目标列标警示色）。
        rows: Vec<ConflictRow>,
        /// 范围行（T20-H 流式落位）：Some = 「已落位 M 项已开始传输，本次
        /// 确认只处理剩余 N 项」——流式已落位项不可逆，弹窗必须写明范围；
        /// None = 无流式前置（整批在手）。
        note: Option<String>,
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
        /// 卡片拖拽状态（幽灵卡跟随/落点高亮，见 host_panel::CardDrag）。
        drag: Option<super::host_panel::CardDrag>,
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
    /// 远端冲突确认（C# ShowRemoteConflictAsync）：回传前探测到远端 mtime
    /// 与下载基准差 ≥1s；「覆盖」→ 直接回传。外部编辑器与内置编辑器共用
    /// 本弹层，按 src 分派覆盖动作（文案/按钮两路一致）。
    ExtEditConflict {
        side: usize,
        /// 发起 tab id（ext_upload 定位该 tab 的会话与槽位；内置路仅占位）。
        tab: u32,
        name: String,
        remote_path: String,
        /// 冲突来源（CR-6）：「覆盖」据此分派 doc_upload / ext_upload。
        src: ExtConflictSrc,
    },
    /// 写锁等待超时兜底（项6）：外部编辑器持写句柄 ≥10s 时的一次性退出
    /// 对话框——「立即重试探测」重置等待态下轮立即再探 /「取消本次回传」
    /// 放弃上传（基准更新为当前快照，本地副本保留）/ Esc·遮罩·× = 继续等待。
    /// 绝不提供「强制上传」。
    ExtEditLockWait {
        side: usize,
        /// 发起 tab id（定位该 tab 的 ext_edits 会话）。
        tab: u32,
        name: String,
        remote_path: String,
    },
}

impl Dialog {
    /// C3②（F-9）：变体判别值——与 State.dialog_kind 比对检出「弹窗打开首帧」。
    fn kind(&self) -> u8 {
        match self {
            Dialog::None => 0,
            Dialog::Rename { .. } => 1,
            Dialog::Delete { .. } => 2,
            Dialog::Checksum { .. } => 3,
            Dialog::Permission { .. } => 4,
            Dialog::HostPicker { .. } => 5,
            Dialog::TransferConfirm { .. } => 6,
            Dialog::Settings { .. } => 7,
            Dialog::HostPanel { .. } => 8,
            Dialog::Import { .. } => 9,
            Dialog::SyncMismatch { .. } => 10,
            Dialog::TabClose { .. } => 11,
            Dialog::ExtEditChanged { .. } => 12,
            Dialog::ExtEditConflict { .. } => 13,
            Dialog::ExtEditLockWait { .. } => 14,
        }
    }
}

/// 远端冲突确认来源（CR-6）：外部编辑器走 ext_upload（tab 槽位回传），
/// 内置编辑器走 doc_upload（doc_id + 副本路径回传）。弹层文案、按钮
/// （取消/覆盖两颗）、焦点逻辑两路一致，仅「覆盖」动作分派不同。
#[derive(Clone)]
pub enum ExtConflictSrc {
    /// 内置编辑器文档：doc_id 定位 EditorDoc，local_path 为本地副本路径。
    Internal {
        doc_id: u64,
        local_path: std::path::PathBuf,
    },
    /// 外部编辑器（既有路径，按 side+tab 定位 ext_edits 槽位）。
    External,
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
    /// T20-1 列显隐菜单（表头右键；四勾选项，C# FileView.xaml:359-367
    /// ToggleMenuFlyoutItem 同观感）。
    pub cols: bool,
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

/// 设置面板值（打开时 from_store 快照 + 每帧 sync_to_store 整包回写）。
pub struct Settings {
    /// 当前大类页下标。
    pub category: usize,
    /// 主题下拉选中项（0 跟随系统 / 1 深色 / 2 浅色；打开时由 store theme 派生，
    /// store "" = 跟随系统 → 0）。切换即时 install，落盘走 theme_write_back。
    pub theme_picked: usize,
    /// 打开时 store 的 theme 快照（T19-1：面板内未动主题 = 原样回写，
    /// ""/显式值语义不被整包回写抹掉）。
    pub theme_loaded: String,
    /// 面板内动过主题下拉（仅动过才回写显式值，对齐 C# SetTheme 仅选择时写）。
    pub theme_touched: bool,
    pub lang_en: bool,
    /// 打开时 store 的 language 快照（T19-3：面板内未动语言 = 原样回写，
    /// "" 不被整包回写抹成 "zh-CN"）。
    pub lang_loaded: String,
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
    /// 流式落位阈值透传（T20-H 内定值不进面板 UI；整包回写原样带过，防抹档）。
    pub dnd_stream_batch: i32,
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
            theme_picked: 0,
            theme_loaded: String::new(),
            theme_touched: false,
            lang_en: false,
            lang_loaded: String::new(),
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
            dnd_stream_batch: 256,
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

/// 语言回写值（T19-3，C# ThemeService.SetLanguage 仅显式选择时写）：
/// 面板内未动（lang_en 与打开快照一致）→ 原样保留快照——"" 保持 ""（默认
/// 中文语义不落盘，C# Language 空值同口径）；动过 → 显式值。
fn lang_write_back(loaded: &str, lang_en: bool) -> String {
    if lang_en == (loaded == "en-US") {
        loaded.to_owned()
    } else if lang_en {
        "en-US".into()
    } else {
        "zh-CN".into()
    }
}

/// 主题回写值（T19-1，C# ThemeService.SetTheme 仅显式选择时写）：面板内动过
/// 下拉 → 按选中项写（0 跟随系统 = ""，1/2 = "Dark"/"Light"）；未动 → 原样
/// 保留快照——""（跟随系统）不被抹成显式值，显式值不被抹回 ""。
fn theme_write_back(loaded: &str, picked: usize, touched: bool) -> String {
    if touched {
        match picked {
            0 => String::new(),
            1 => "Dark".into(),
            _ => "Light".into(),
        }
    } else {
        loaded.to_owned()
    }
}

#[cfg(test)]
mod store_write_back_tests {
    use super::{lang_write_back, theme_write_back};

    /// T19-3：语言未配置（""）打开面板不动 → 仍 ""（不再被整包回写抹成
    /// "zh-CN"，对齐 C# Language 空值 = 默认中文不落盘）；显式选择 → 写对应值。
    #[test]
    fn language_empty_preserved_until_explicit() {
        assert_eq!(lang_write_back("", false), "");
        assert_eq!(lang_write_back("", true), "en-US");
        assert_eq!(lang_write_back("zh-CN", false), "zh-CN");
        assert_eq!(lang_write_back("zh-CN", true), "en-US");
        assert_eq!(lang_write_back("en-US", false), "zh-CN");
        assert_eq!(lang_write_back("en-US", true), "en-US");
    }

    /// F-6 主题第三项·未动：面板内没动下拉 → 打开快照原样——"" 保持 ""
    /// （跟随系统语义不被抹掉）；显式 Dark/Light 也不被抹回 ""。
    #[test]
    fn theme_untouched_keeps_loaded_snapshot() {
        assert_eq!(theme_write_back("", 0, false), "");
        assert_eq!(theme_write_back("Dark", 1, false), "Dark");
        assert_eq!(theme_write_back("Light", 2, false), "Light");
    }

    /// F-6 主题第三项·选「跟随系统」：写回 ""，且重启路径 resolve_dark("")
    /// 按系统偏好解析（系统亮→亮 / 系统暗→暗），即重启后跟随系统。
    #[test]
    fn theme_follow_system_writes_empty_and_follows_on_restart() {
        assert_eq!(theme_write_back("Dark", 0, true), "");
        assert_eq!(theme_write_back("Light", 0, true), "");
        let follow = theme_write_back("Dark", 0, true);
        assert!(!crate::theme::resolve_dark(&follow, true)); // 系统亮 → 亮
        assert!(crate::theme::resolve_dark(&follow, false)); // 系统暗 → 暗
    }

    /// F-6 主题第三项·显式选择：动过下拉写显式 "Dark"/"Light"（含从另一
    /// 显式值切换）。
    #[test]
    fn theme_explicit_pick_writes_dark_or_light() {
        assert_eq!(theme_write_back("", 1, true), "Dark");
        assert_eq!(theme_write_back("", 2, true), "Light");
        assert_eq!(theme_write_back("Dark", 2, true), "Light");
        assert_eq!(theme_write_back("Light", 1, true), "Dark");
    }
}

impl Settings {
    /// 启动初值：读 settings::AppSettings（settings.json）。
    pub fn from_store() -> Self {
        crate::settings::with(|s| Self {
            category: 0,
            theme_picked: match s.theme.as_str() {
                "Dark" => 1,
                "Light" => 2,
                _ => 0, // ""/未知 = 跟随系统
            },
            theme_loaded: s.theme.clone(),
            theme_touched: false,
            lang_en: s.language == "en-US",
            lang_loaded: s.language.clone(),
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
            dnd_stream_batch: s.dnd_stream_batch,
            editor_builtin: s.editor_builtin,
            editor_path: s.editor_path.clone(),
            reg_code: String::new(),
            update_check: s.update_check,
            reg_hint: None,
            update_checking: false,
            update_check_result: String::new(),
        })
    }

    /// 每帧回写 store（内部整包比对置脏，防抖落盘）。
    /// 主题/语言（T19-1/T19-3）：面板内显式改动 → 写显式值；未动 → 原样回写
    /// 打开快照——"" 的「跟随系统 / 默认中文」语义不被整包回写抹掉。
    fn sync_to_store(&self) {
        crate::settings::sync_from_dialog(crate::settings::AppSettings {
            theme: theme_write_back(&self.theme_loaded, self.theme_picked, self.theme_touched),
            language: lang_write_back(&self.lang_loaded, self.lang_en),
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
            dnd_stream_batch: self.dnd_stream_batch,
            dirty: false,
            last_change: None,
            // CR-2：退避时刻是 store 内部态，面板不回写；整包比对已忽略它。
            last_attempt: None,
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
    /// T20-4 跨窗格拖拽就绪（C# CanTransferToOpposite，root() 每帧按两侧窗格
    /// 已导航 + 远程侧会话在池折算；false = pane 层禁 chip/DropHint/放置）。
    pub dnd_ready: bool,
    /// 文件夹选择在途槽（FV-01：open_dir 发起后台 STA 线程弹框；root() 轮询
    /// 回执跳转。Some = 在途，工具栏按钮此间禁用防重复弹框）。
    pub pick: &'a mut Option<crate::local::PickJob>,
    /// G17 活动标签关闭请求（(侧, tab id)；root() 走确认门与资源清理，C#
    /// Tabs_TabCloseRequested → TabCloseRequested 事件）。
    pub tab_close: Option<(usize, u32)>,
}

/// 任一侧会话池存在待决指纹确认（含测试连接槽）——覆盖层挂载 / Esc 抑制判定共用。
pub(crate) fn fp_ask_pending(st: &State) -> bool {
    st.remotes
        .iter()
        .any(|rs| rs.sessions.iter().any(|s| s.fp_ask.is_some()))
        || st.fp_ask_test.is_some()
}

// ── 每帧入口 ────────────────────────────────────────────────
pub fn draw(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    // C2（F-9）：root 帧首 stash 的滚轮回填（弹窗内部滚动区消费；无弹窗帧
    // stash 恒零 no-op）。ScrollArea 读后清零且不看层，frame 内首个命中者
    // 独占——不回填则设置页/传输确认表/导入预览永远拿不到滚轮。
    if st.scroll_stash != egui::Vec2::ZERO {
        let d = std::mem::take(&mut st.scroll_stash);
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = d);
    }
    // C3②（F-9）：弹窗打开首帧判定——变体判别值变更且非 None → dialog_fresh
    // （body 读它聚焦首控件）；记录判别值供下一帧比对。
    let kind = st.dialog.kind();
    st.dialog_fresh = kind != st.dialog_kind && kind != 0;
    st.dialog_kind = kind;
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
        Dialog::ExtEditLockWait { .. } => ext_edit_lock_wait_dialog(ui, st, p),
    }
    // 指纹确认覆盖层（最顶层；期间底下弹窗的 Esc 已被 shell 抑制）。
    if fp_ask_pending(st) {
        fp_confirm_overlay(ui, st, p);
    }
}

/// shell 关闭路径的暂存态处置（C1/F-9：关闭回调收口 shell 层，调用方以枚举
/// 声明关闭时的清理动作——工单「关闭回调须允许调用方清暂存态」）。
#[derive(Clone, Copy, Default)]
pub(crate) enum CloseAction {
    /// 无暂存态（默认）：直接关闭。
    #[default]
    None,
    /// 冲突确认弹窗（宿主 TransferConfirm）：关闭路径（Esc / 遮罩 / ×）与
    /// 「取消」按钮同语义——按取消结算悬挂 pending（清暂存态 + 已落位行
    /// 回溯清行 + 终端对账日志），防 seq=0 批次永久驻留（F1）。
    CancelPendingStream,
}

/// shell 可选行为（C1/F-9）。默认：遮罩点击可关、无关闭回调。
#[derive(Clone, Copy)]
pub(crate) struct ShellOpts {
    /// 遮罩点击是否关闭（F19：有未保存编辑的面板可配置不关；Esc / × 仍关，
    /// 遮罩点击仍被吸收不透传）。HostPanel = false（显式保存语义，未保存
    /// 工作集不允许遮罩一次误点丢弃——用户拍板「保持无提示丢弃」指 Esc/×
    /// 关闭路径不加 dirty 确认，遮罩防误触与之一致）。
    pub mask_close: bool,
    /// 关闭路径回调（见 CloseAction）。
    pub on_close: CloseAction,
}

impl Default for ShellOpts {
    // mask_close 默认 true（bool derive 默认 false，不合遮罩语义），手写。
    fn default() -> Self {
        Self {
            mask_close: true,
            on_close: CloseAction::None,
        }
    }
}

/// 统一外壳。正文经 `body` 闭包绘制——闭包在弹窗 Foreground Area（= C3⑤
/// modal 焦点层）内执行，入参（层内 ui / 内容圆弧容器矩形 / State）；本帧已
/// 关闭（Esc / 关闭钮 / 遮罩点击若 opts.mask_close，C1/F-9）则不调用、返回
/// false。返回 true = 弹窗存活且正文已绘（正文按钮自行关窗不经此出口）。
///
/// 为什么正文必须画进本层（2026-11-12 egui 0.37 源码实证，F-9 收口）：
/// ① 绘制序——主界面 Middle 层先画、Foreground 层后画，留在 Middle 的正文
///   被整层盖住（半透明遮罩罩暗 + 卡片不透明底盖死，层内按序、层间按 Order）；
/// ② 交互——set_modal_layer 提交后 allows_interaction(Middle)=false，正文
///   从次帧起 hover/点击全灭（interaction.rs 只在允许层收集点击候选）；
/// ③ 焦点——create_widget 对不允许层控件 interested_in_focus=false 并
///   ignore_focus 清焦点（含已聚焦控件），Enter 合成点击 has_focus 门随之
///   失效。正文入层后三层同灭，modal 屏蔽只作用于遮罩下主界面。
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
    // C1/F-9：关闭行为声明（遮罩可关性 + 关闭回调）——调用方自持，shell 不再
    // 对弹窗变体特判（「统一收口到 shell 层，不要每个弹窗各写一遍」）。
    opts: ShellOpts,
    // 正文闭包（详见 fn 文档①②③）。State 由 shell 转借：shell 持有
    // &mut State，闭包期间经第三参供给——调用方闭包直接捕获会双重可变借用。
    body: impl FnOnce(&mut egui::Ui, Rect, &mut State),
) -> bool {
    let full = ui.max_rect();
    // C7/F3：卡片尺寸与视口各夹一次（MIN_SIZE 900×600 下 920×640 面板高溢 20px、
    // 关闭钮中心出屏；导入结果页动态高同受保护）——clamp 后关闭钮恒在窗内。
    let (w, h) = (w.min(full.width()), h.min(full.height()));
    // 指纹确认覆盖层打开时 Esc 归覆盖层（防一击双关：底弹窗不应同时响应）。
    // C3①（F-9/F5）：上一帧有 combo 展开态则本帧 Esc 归组合（card_combo 真消费
    // 收起）——Esc 先收组合再关弹窗，不再一击穿透；consume_key 真消费（NONE
    // 修饰符匹配忽略额外 Shift/Alt，与原 key_pressed 同口径）。
    let combo_open = COMBO_OPEN.swap(false, std::sync::atomic::Ordering::Relaxed);
    let esc = !fp_ask_pending(st)
        && !combo_open
        && ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    // C3③（F20/F-9）：Enter→主钮旗标。豁免「焦点在可点控件」——焦点控件收到
    // Space/Enter 时 egui 直接触发其点击（FAKE_PRIMARY_CLICKED），再全局响应
    // 即双触发；文本框焦点放行（egui 单行框不消费 Enter，rename 据此提交）。
    st.shell_enter = !fp_ask_pending(st)
        && !combo_open
        && (ui.ctx().memory(|m| m.focused().is_none()) || ui.ctx().text_edit_focused())
        && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));

    // C2/C9（方案 A）：弹窗主体升 Foreground 独立层——主界面控件全部在默认层，
    // 跨层 hover/点击高层胜：遮罩下的 splitter / 列宽 thumb 不再 hovered 抢
    // 写光标意图（F-8 起全部改上报、帧尾 ui::cursor_decide 唯一一次写入），
    // edge_resize 边缘带不再抢命中。
    // 滚轮走 root 帧首 stash + 本函数入口回填（ScrollArea 读后清零且不看层，
    // Sense 无 scroll 位可用）。
    Area::new(Id::unique("dialog-shell"))
        .order(Order::Foreground)
        .fixed_pos(full.min)
        .constrain_to(full)
        .show(ui.ctx(), |ui| {
            // C3⑤（F-9）：modal 焦点层限定到弹窗层——下一帧起遮罩下控件不再注册
            // 焦点兴趣，Tab 循环只在弹窗层内（指针交互不受影响，遮罩继续负责）。
            ui.ctx().memory_mut(|m| m.set_modal_layer(ui.layer_id()));
            // C3②（F-9）：打开首帧清残留文本输入焦点（Tab/Enter 不串到底下主界面）。
            if st.dialog_fresh {
                ui.ctx().memory_mut(|m| m.stop_text_input());
            }
            // 遮罩：先挂（层内 z 最底），点击即关。
            let mask = ui.allocate_rect(full, Sense::click() - Sense::FOCUSABLE);
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
                theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.card_stroke),
                egui::StrokeKind::Inside,
            );
            // 卡片命中面：吸收内部点击，避免穿透到遮罩造成误关（- FOCUSABLE：C3⑤
            // Tab 不落在无视觉反馈的命中面上，首 Tab 直达首个真控件）。
            ui.allocate_rect(card, Sense::click() - Sense::FOCUSABLE);

            // 子元素排在描边内缩 1px 的 inner 区（C# Card BorderThickness=1）。
            let inner = card.shrink(1.0);
            // 标题行：窄标题栏通栏贴边。面板模式累计指针位移整体平移（阴影/标题/
            // 内容全由 card 派生，自动跟随）；视口模式拖动 OS 主窗口。
            let title_bar = Rect::from_min_max(
                inner.min,
                Pos2::new(inner.right(), inner.top() + TITLE_BAR_H),
            );
            let drag = ui.allocate_rect(title_bar, Sense::click_and_drag() - Sense::FOCUSABLE);
            if let Some(off) = win_drag.as_deref_mut() {
                let delta = drag.drag_delta();
                if delta != Vec2::ZERO {
                    *off += delta;
                    // C8：偏移钳制在「卡完全留在视口内」的半径（w/h 已夹视口，半径恒
                    // ≥0）——面板拖不出屏幕，标题栏不会消失导致拖不回来。
                    let max = (full.size() - egui::vec2(w, h)) * 0.5;
                    off.x = off.x.clamp(-max.x, max.x);
                    off.y = off.y.clamp(-max.y, max.y);
                }
            } else if drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            let icon_size = if large { 11.0 } else { 10.0 };
            ui.painter().text(
                Pos2::new(inner.left() + TITLE_PAD.0, title_bar.center().y),
                Align2::LEFT_CENTER,
                title,
                // FontWeight=SemiBold → brand 字族（theme.rs 注册的 SemiBold 品牌面）。
                theme::font_title(large),
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
            // M-12：弹窗关闭钮自绘无控件语义，补 widget_info（C# 自绘 TitleBar 关闭钮同位）。
            close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, t("关闭")));
            if close.hovered() {
                let bg = if close.is_pointer_button_down_on() {
                    p.row_pressed
                } else {
                    p.row_hover
                };
                ui.painter().rect_filled(close_r, 4.0, bg);
            }
            // C3④（F18/F-9）：键盘焦点框（Tab 可达关闭钮，2px FocusVisual）。
            if close.has_focus() {
                ui.painter().rect_stroke(
                    close_r,
                    4.0,
                    theme::stroke(ui.ctx().pixels_per_point(), 2.0, p.focus_stroke),
                    egui::StrokeKind::Inside,
                );
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
                Pos2::new(inner.left() + CONTENT_INSET, title_bar.bottom() + row_gap),
                Pos2::new(
                    inner.right() - CONTENT_INSET,
                    inner.bottom() - CONTENT_INSET,
                ),
            );
            if content_card {
                ui.painter()
                    .rect_filled(content, theme::SHELL_RADIUS, p.card_bg);
            }

            if esc || close.clicked() || (opts.mask_close && mask.clicked()) {
                // C1/F-9：三路关闭共用调用方声明的暂存态处置（原 TransferConfirm
                // 特判迁出为 CloseAction::CancelPendingStream——F1：与「取消」按钮
                // 同语义，清暂存态 + 已落位行回溯清行 + 终端对账日志，防 seq=0
                // 批次永久驻留）。mask_close=false 时遮罩点击仍被吸收（不透传）
                // 只是不关（F19：有未保存编辑的面板防一次误点丢弃）。
                match opts.on_close {
                    CloseAction::CancelPendingStream => super::cancel_pending_stream(st),
                    CloseAction::None => {}
                }
                st.dialog = Dialog::None;
                return false;
            }
            // 正文画进本 Area 层（modal 层，fn 文档①②③）。正文内部按钮关窗
            // （st.dialog = None）不经本出口，draw 派发的回填逻辑照旧成立。
            body(&mut content_ui(ui, content), content, st);
            true
        })
        .inner
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
    std_button_fs(ui, rect, label, fg, p, theme::font_body())
}

/// 标准按钮（可指定字体；C# 控件级 FontSize 覆写场景：设置卡 13 =
/// `theme::font(TextRole::Compact)` / 主机面板 12 = `theme::font_small()`）。
pub(crate) fn std_button_fs(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &str,
    fg: Color32,
    p: &theme::Palette,
    font: FontId,
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
    font: FontId,
    enabled: bool,
) -> bool {
    // 描边像素对齐（theme::snap_rect）：半像素坐标会让 1px 边线被抗锯齿摊薄。
    let rect = theme::snap_rect(rect, ui.ctx().pixels_per_point());
    let resp = ui.allocate_rect(rect, Sense::click());
    // F-8：自绘按钮 hover 手型（禁用态 / 仅焦点态不上报）。
    if enabled && resp.hovered() {
        cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::PointingHand);
    }
    // C4/C5（F-9/M-12 同款）：自绘无控件语义，补 widget_info（std_button /
    // std_button_fs / danger_button / button_row 均委托至此，一处覆盖）。
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
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
    } else if resp.has_focus() {
        // C3④（F18/F-9）：键盘焦点底（ControlFillColorInputActive 同源——
        // 亮实白 / 暗 #1E1E1E；hover/按下态优先级更高，只在静默时显出）。
        (p.control_bg_focused, p.control_stroke, fg)
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
    // C3④（F18/F-9）：键盘焦点框（2px FocusVisual；按钮系 has_focus 统一走此）。
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect,
            4.0,
            theme::stroke(ui.ctx().pixels_per_point(), 2.0, p.focus_stroke),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, label, font, text_c);
    enabled && resp.clicked()
}

/// 危险按钮：同标准按钮，字色 SystemFillColorCritical（删除钮）。
fn danger_button(ui: &mut egui::Ui, rect: Rect, label: &str, p: &theme::Palette) -> bool {
    std_button(ui, rect, label, p.critical, p)
}

/// 按钮宽（文字 + 左右 11）——与绘制同源（F-11：btn_w / std_button / accent_button
/// 共用同一 font 量宽，禁写死宽度；测量色 MEASURE_ONLY 不实际绘制）。
pub(crate) fn btn_w(ui: &egui::Ui, label: &str, font: &FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), font.clone(), theme::MEASURE_ONLY)
        .size()
        .x
        + 22.0
}

/// 弹窗文本断行模式（F-11 排版收口：painter.text 不换行 / RichText 内建 /
/// import_dialog::wrap_text 三套断行收口到 dlg_text）。
pub(crate) enum TextMode {
    /// 自动换行（说明 / 长文案；max_w = 换行宽）。
    Wrap,
    /// 单行按像素宽截断加省略号（表单元格等，import_dialog::trim_text 语义）。
    Truncate,
    /// 单行不换行不截断（标题 / 按钮文字 / 码值）。
    Single,
}

/// 弹窗文本统一绘制入口：pos = 左上角，返回 galley 高度供多行文案顺排。
/// 字体一律具名（theme::font_body() / font_small() / font(TextRole::…) / font_title…）。
pub(crate) fn dlg_text(
    pt: &egui::Painter,
    pos: Pos2,
    text: &str,
    font: FontId,
    mode: TextMode,
    max_w: f32,
    color: Color32,
) -> f32 {
    let galley = match mode {
        TextMode::Wrap => pt.layout(text.to_owned(), font, color, max_w),
        TextMode::Truncate => pt.layout_no_wrap(trim_by_width(pt, text, &font, max_w), font, color),
        TextMode::Single => pt.layout_no_wrap(text.to_owned(), font, color),
    };
    let h = galley.size().y;
    pt.galley(pos, galley, color);
    h
}

/// 按像素宽截断加省略号（import_dialog::trim_text 收口；测量色 MEASURE_ONLY）。
fn trim_by_width(pt: &egui::Painter, s: &str, font: &FontId, max_w: f32) -> String {
    if pt
        .layout_no_wrap(s.to_owned(), font.clone(), theme::MEASURE_ONLY)
        .size()
        .x
        <= max_w
    {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        let t: String = chars[..n].iter().collect::<String>() + "…";
        if pt
            .layout_no_wrap(t.clone(), font.clone(), theme::MEASURE_ONLY)
            .size()
            .x
            <= max_w
        {
            return t;
        }
        n -= 1;
    }
    "…".to_owned()
}

/// 条目名等长文本按 22 显示字符截断加省略号（F-11 工单定案：删除 / 校验码 /
/// 权限弹窗文件名统一口径——字符数上限而非像素宽，中英文观感一致）。
pub(crate) fn elide22(s: &str) -> String {
    let mut out: String = s.chars().take(21).collect();
    if s.chars().count() > 21 {
        out.push('…');
    }
    out
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
        let w = btn_w(ui, label, &theme::font_body());
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
    // C4/C5（F-9/M-12 同款）：自绘无控件语义，补 widget_info（Switch 角色携带
    // 选中态；行标签由调用处行文本承载，不在此重复）。
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, enabled, on, ""));
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
        theme::stroke(
            ui.ctx().pixels_per_point(),
            1.0,
            mix(stroke_c, Color32::TRANSPARENT, t),
        ),
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
    font: FontId,
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
    pt.circle_stroke(
        icon.center(),
        9.5,
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, ring_c),
    );
    if checked {
        pt.circle_filled(icon.center(), 4.0, accent);
    }
    pt.text(
        Pos2::new(rect.left() + icon_d + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        p.text_primary,
    );
    resp.clicked()
}

/// 复选框（视觉方块 18×18，Rect 由调用处传入；返回 true = 本次点击翻转）。
/// 态口径（WinUI CheckBox 模板默认资源，C# 未覆写）：选中底/勾边 = Accent；
/// 未选中底 = ControlFillColorDefault、边 = ControlStrokeColorSecondary；
/// 禁用 = ControlFillColorDisabled + ControlStrokeColorDisabled（不可点）。
/// F-11 两套 checkbox 合一：以 import_dialog 版为基线（圆角 4 / ✓ 12 号字；
/// 旧 dialogs 版圆角 3 / 线勾 1.6 弃用）。
pub(crate) fn checkbox(
    ui: &mut egui::Ui,
    r: Rect,
    on: bool,
    enabled: bool,
    p: &theme::Palette,
) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    let painter = ui.painter().clone();
    let ppp = ui.ctx().pixels_per_point();
    if !enabled {
        painter.rect_filled(r, theme::CONTROL_RADIUS, p.control_disabled);
        painter.rect_stroke(
            r,
            theme::CONTROL_RADIUS,
            theme::stroke(ppp, 1.0, p.control_stroke_disabled),
            egui::StrokeKind::Inside,
        );
        return false;
    }
    if resp.hovered() {
        painter.rect_filled(r.expand(2.0), 3.0, p.control_hover);
    }
    let border = if on { theme::ACCENT } else { p.control_stroke };
    let bg = if on { theme::ACCENT } else { p.control_bg };
    painter.rect_filled(r, theme::CONTROL_RADIUS, bg);
    painter.rect_stroke(
        r,
        theme::CONTROL_RADIUS,
        theme::stroke(ppp, 1.0, border),
        egui::StrokeKind::Inside,
    );
    if on {
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            "✓",
            theme::font_small(),
            theme::CHECK_FG,
        );
    }
    resp.clicked()
}

/// accent 钮三态底色（C# AccentButtonStyle：Accent / hover ACCENT_HOVER /
/// pressed ACCENT_PRESSED；禁用 accent_disabled）。F-11 提公共层：
/// dialogs::accent_button 与 host_panel::accent_icon_button 共用（F-9 C5）。
pub(crate) fn accent_bg(down: bool, hovered: bool, enabled: bool, p: &theme::Palette) -> Color32 {
    if !enabled {
        p.accent_disabled
    } else if down {
        theme::ACCENT_PRESSED
    } else if hovered {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    }
}

/// 主色文字钮（C# AccentButtonStyle：同意页开始扫描 / 导入 / 完成）。
/// 底 AccentFillColorDefault = 品牌蓝三态（与 header/host_panel 同口径）；禁用 =
/// AccentFillColorDisabled 底 + TextOnAccentFillColorDisabled 字（点击不响应）。
/// 高 32、宽 = 文字 + 22（内容左右 padding 11，调用处经 btn_w 同源量宽）。
/// F-11 从 import_dialog 基线原样上移为公共层，字号参数化（调用处传具名 font）。
pub(crate) fn accent_button(
    ui: &mut egui::Ui,
    r: Rect,
    label: &str,
    enabled: bool,
    p: &theme::Palette,
    font: FontId,
) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    #[cfg(test)]
    eprintln!(
        "[btn] enabled={} visible={} clip={} r={} focused_now={:?} allows={}",
        ui.is_enabled(),
        ui.is_visible(),
        ui.clip_rect(),
        r,
        ui.ctx().memory(|m| m.focused()),
        ui.ctx().memory(|m| m.allows_interaction(ui.layer_id()))
    );
    // C4/C5（F-9/M-12 同款）：自绘无控件语义，补 widget_info。
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let (bg, text_c) = if !enabled {
        (p.accent_disabled, theme::TEXT_ON_ACCENT_DISABLED)
    } else if resp.is_pointer_button_down_on() {
        (theme::ACCENT_PRESSED, theme::ON_ACCENT_FG)
    } else if resp.hovered() {
        (theme::ACCENT_HOVER, theme::ON_ACCENT_FG)
    } else {
        (theme::ACCENT, theme::ON_ACCENT_FG)
    };
    ui.painter().rect_filled(r, 4.0, bg);
    // C3④（F18/F-9）：键盘焦点框（accent 底恒品牌蓝不随焦点变，只加 2px 焦点框）。
    if resp.has_focus() {
        ui.painter().rect_stroke(
            r,
            4.0,
            theme::stroke(ui.ctx().pixels_per_point(), 2.0, p.focus_stroke),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter()
        .text(r.center(), Align2::CENTER_CENTER, label, font, text_c);
    enabled && resp.clicked()
}

// ── 重命名（320）────────────────────────────────────────────
fn rename_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (pane, orig, fresh) = match &st.dialog {
        Dialog::Rename {
            pane, orig, fresh, ..
        } => (*pane, orig.clone(), *fresh),
        _ => return,
    };
    let name = match &st.dialog {
        Dialog::Rename { name, .. } => name.clone(),
        _ => return,
    };
    // 实时校验：空 / 与原名相同（C# :99 IsValidNewName）；本地另查 Windows
    // 非法字符（远程豁免，C# isRemote 分流）；列表内重名禁点（C# :101-109，
    // 本地宽松 / 远程 Ordinal——复用同目录守卫 same_dest_dir 的分流口径）。
    let remote = menu_side(st, pane).is_some();
    let illegal = !remote && name.chars().any(|c| "\\/:*?\"<>|".contains(c));
    let view = pane_view(st, pane);
    let dup = view.entries.iter().enumerate().any(|(i, e)| {
        view.selected.first() != Some(&i)
            && !e.name.is_empty()
            && crate::transfer::same_dest_dir(&name, &e.name, remote)
    });
    let ok = !name.is_empty() && name != orig && !illegal && !dup;

    shell(
        ui,
        st,
        p,
        320.0,
        144.0,
        t("重命名"),
        false,
        0.0,
        true,
        None,
        ShellOpts::default(),
        |ui, content, st| rename_body(ui, st, p, content, pane, fresh, name, ok),
    );
}

/// 重命名正文（shell 回调，见 shell 文档①②③）。pane = 目标窗格，fresh =
/// 打开首帧旗标（全选原名 + 聚焦），name = 输入框工作副本，ok = 实时校验结果。
#[allow(clippy::too_many_arguments)]
fn rename_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    pane: usize,
    fresh: bool,
    mut name: String,
    ok: bool,
) {
    let mut cui = content_ui(ui, content);
    let w = content.width() - CONTENT_PAD.0 * 2.0;

    let te = egui::TextEdit::singleline(&mut name)
        .font(egui::TextStyle::Body)
        .desired_width(w)
        .min_size(egui::vec2(w, BTN_H));
    let te_resp = cui.put(
        Rect::from_min_size(
            Pos2::new(
                content.left() + CONTENT_PAD.0,
                content.top() + CONTENT_PAD.1,
            ),
            egui::vec2(w, BTN_H),
        ),
        te,
    );
    // 右键菜单（复制/剪切/粘贴/全选）：自动注册侧道。
    super::ctx_menu::register_auto(te_resp.id, te_resp.interact_rect);
    // 打开首帧全选原名（C# RenameInputDialog :64-68 SelectAll + Focus）：
    // fresh 旗随首帧消费，选区经 TextEditState 游标写入。
    if fresh {
        te_resp.request_focus();
        let mut ts =
            egui::widgets::text_edit::TextEditState::load(ui.ctx(), te_resp.id).unwrap_or_default();
        ts.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(name.chars().count()),
        )));
        ts.store(ui.ctx(), te_resp.id);
        if let Dialog::Rename { fresh: f, .. } = &mut st.dialog {
            *f = false;
        }
    }

    let btn_y = content.top() + CONTENT_PAD.1 + BTN_H + GROUP_GAP;
    // 确认钮：IsValidNewName 才启用（C# RenameInputDialog.xaml.cs:62 初始
    // IsEnabled=false、:99 校验通过启用）；禁用态点击不响应。
    let confirm_w = btn_w(ui, t("重命名"), &theme::font_body());
    let cancel_w = btn_w(ui, t("取消"), &theme::font_body());
    let bx = content.right() - CONTENT_PAD.0;
    // C6（F-9）：主钮补 accent 色（AccentButtonStyle，禁用态 = accent_disabled 底
    // + TEXT_ON_ACCENT_DISABLED 字，IsEnabled 语义不变）。
    let confirm = accent_button(
        ui,
        Rect::from_min_size(
            Pos2::new(bx - confirm_w, btn_y),
            egui::vec2(confirm_w, BTN_H),
        ),
        t("重命名"),
        ok,
        p,
        theme::font_body(),
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
    // C3③（F20/F-9）：Enter 提交走 shell 旗标（焦点在输入框——egui 单行框
    // 不消费 Enter，旗标已放行；弹窗不再各写一遍键盘监听）。
    let enter = ok && st.shell_enter;
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
    // P0-1 重命名落盘（C# FileView :2522-2530 本地 File/Directory.Move、
    // :2560-2562 远程 SFTP RenameAsync）：此前仅改列表条目、刷新即打回原名的
    // 假动作（ADR-031 第一红线）。
    let orig = match &st.dialog {
        Dialog::Rename { orig, .. } => orig.clone(),
        _ => String::new(),
    };
    match menu_side(st, pane) {
        Some(side) => {
            // 远程：进 rename_reqs 落盘管线（同远程新建：inflight 定位 + 保活，
            // Ev::Created 统一重列；成败细节走终端日志）。列表不改名——真实
            // 结果以服务端为准（重列后按实显名）。
            let tab_id = match super::active_remote_tab(st, side) {
                Some(t) => t.id,
                None => return,
            };
            if let Some(rp) = pane_view(st, pane).remote_path.clone() {
                let old = super::join_remote_path(&rp, &orig);
                let new = super::join_remote_path(&rp, name);
                st.rename_reqs.push((side, tab_id, old, new));
            }
        }
        None => {
            // 本地：同步改名（C# File.Move/Directory.Move 同步同义），失败入
            // 状态栏、列表不动；成功后重列（含树重建）。
            let job = pane_view(st, pane).path.clone().and_then(|dir| {
                pane_view(st, pane)
                    .selected
                    .first()
                    .and_then(|&i| pane_view(st, pane).entries.get(i))
                    .map(|e| (dir.join(&e.name), dir.join(name), e.name.clone()))
            });
            if let Some((from, to, old)) = job
                && let Err(err) = std::fs::rename(&from, &to)
            {
                let es = err.to_string();
                pane_view_mut(st, pane).msg = Some(tfmt("重命名失败：{0}（{1}）", &[&old, &es]));
            }
            let entries = pane_view_mut(st, pane);
            crate::local::refresh(entries);
            // 重命名影响树中目录名 → 重建路径树（C# :1613-1617 →
            // RefreshListKeepingPath 首行 RefreshTreeIfVisible 同走）。
            crate::ui::panes::tree_mark_dirty(entries);
        }
    }
}

// ── 删除确认（320）──────────────────────────────────────────
fn delete_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Dialog::Delete { pane, indices } = &st.dialog else {
        return;
    };
    let pane = *pane;
    let entries = pane_view(st, pane);
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

    // 高 190：说明两行（英文态约 500px 文案在 290px 卡内宽必换行）+ 底锚
    // 按钮（底距 15）+ 14 组间距（F-11 F2：说明改 galley 换行后 +18 容纳）。
    shell(
        ui,
        st,
        p,
        320.0,
        190.0,
        t("删除"),
        false,
        0.0,
        true,
        None,
        ShellOpts::default(),
        |ui, content, st| delete_body(ui, st, p, content, pane, shown),
    );
}

/// 删除确认正文（shell 回调，见 shell 文档①②③）。
fn delete_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    pane: usize,
    shown: String,
) {
    let cui = content_ui(ui, content);
    let x = content.left() + CONTENT_PAD.0;

    // 条目行：危险红删除图标 + 名称 50% 淡化（F-11 F2：painter.text 单行
    // 不截断 → galley 单行 + 22 字符 elide，超长文件名不再画穿遮罩）。
    if let Some(img) = st.icons.get(ui.ctx(), "trash", p.critical, 16.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(
                Pos2::new(x + 8.0, content.top() + CONTENT_PAD.1 + 10.0),
                egui::vec2(16.0, 16.0),
            ),
        );
    }
    dlg_text(
        cui.painter(),
        Pos2::new(x + 26.0, content.top() + CONTENT_PAD.1),
        &elide22(&shown),
        theme::font_body(),
        TextMode::Single,
        0.0,
        // Opacity=0.5 叠 TextFillColorPrimary（C# EntryName）。
        theme::dim50(p.text_primary),
    );

    let body_y = content.top() + CONTENT_PAD.1 + 20.0 + GROUP_GAP;
    // 文案随删除语义分流（#10，C# DeleteConfirmDialog.xaml.cs:44-65
    // toRecycleBin 参数）：本地 Shell API 进回收站可还原 → 回收站文案；
    // 远程 SFTP 删除不可恢复 → 永久删除文案。判定与 apply_delete 的
    // 分支同口径（menu_side）。（F-11 F2：painter.text 不换行 → galley
    // 换行，宽 = 卡内宽，英文态两行不再画到卡片外。）
    let recycle = menu_side(st, pane).is_none();
    dlg_text(
        cui.painter(),
        Pos2::new(x, body_y),
        if recycle {
            t("所选内容将移入回收站，可在回收站还原。")
        } else {
            t("所选内容将被永久删除，此操作无法恢复。")
        },
        theme::font_body(),
        TextMode::Wrap,
        content.width() - CONTENT_PAD.0,
        p.text_primary,
    );

    // 删除（危险红字）+ 取消：底锚按钮（底距 15 = CONTENT_PAD.1，与其他
    // 警告弹窗同口径），与正文之间保持 GROUP_GAP。
    let btn_y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let del_w = btn_w(ui, t("删除"), &theme::font_body());
    let cancel_w = btn_w(ui, t("取消"), &theme::font_body());
    // C3②（F-9）：打开首帧初焦点 → 主钮。请求必须在创建之后：egui 新 Area
    // 首帧 = sizing pass（AreaState 未持久化 → ui invisible/disabled），创建时
    // create_widget 走 interested_in_focus=false → ignore_focus 会清掉同帧先置
    // 的焦点；创建后请求则无后续 ignore_focus 干扰，end_pass 按本帧新落焦保留。
    let del_id = ui.next_auto_id();
    let del = danger_button(
        ui,
        Rect::from_min_size(
            Pos2::new(content.right() - CONTENT_PAD.0 - del_w, btn_y),
            egui::vec2(del_w, BTN_H),
        ),
        t("删除"),
        p,
    );
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(del_id));
    }
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
    if del || st.shell_enter {
        apply_delete(st);
        // 弹窗绘制于帧末：CR-1 起本地删除只登记请求 + 置 loading（本帧已画完），
        // 主动约一帧让「正在删除…」即时上屏；远程侧 busy 链（del_pending）本就保活。
        ui.ctx().request_repaint();
    }
    if del || st.shell_enter || cancel {
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
        // CR-1：本地删除 + 删后重列整段交工作线程（帧内零文件系统 I/O）。
        // 逐项成败日志由 `ui::poll_local_jobs` 回执入账，文案与旧版逐字一致
        // （local::delete_log_lines）；回收站语义仍由 local::delete_to_bin 保证。
        let view = pane_view(st, *pane);
        let Some(dir) = view.path.clone() else {
            return; // 根视图（驱动器列表）不可删
        };
        let targets: Vec<String> = indices
            .iter()
            .filter_map(|i| view.entries.get(*i).map(|e| e.name.clone()))
            .collect();
        let keep = crate::local::selected_names(view);
        let view = pane_view_mut(st, *pane);
        view.pending_local = Some(crate::local::LocalPaneReq::Delete { dir, targets, keep });
        view.loading = true;
        view.loading_label = Some(t("正在删除…"));
        return;
    };
    // 远程：CR-7 一个 DeleteBatch（一个 tag），文件项在 worker 内分块并发、
    // 目录项仍串行；逐项成败走 Ev::Log，完成计数回零后刷新。
    // 顺序借用：先收齐待删条目并定路径，再确认会话、发号记 inflight
    // （Ev::Deleted 按其定位重列 tab），最后借该 tab 挂靠会话发批。
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
    // CR-7：一个 tag 装 N 项，del_pending 必须按「批」+1（发批）/ -1（回执）；
    // 若仍按项 += N，一条 Ev::Deleted 只减 1 → 永远不回零、卡在「正在删除…」。
    // paths 空 / 无会话时与旧版 sent=0 同语义：零发号、零 loading 变更。
    if paths.is_empty() || super::session_of_tab(st, side, tab_id).is_none() {
        return;
    }
    let Some((tag, batch)) = begin_delete_batch(st, side, tab_id, paths) else {
        return;
    };
    if let Some(ss) = super::session_of_tab(st, side, tab_id) {
        ss.ssh.delete_batch(tag, batch);
    }
}

/// CR-7 单批删除发号：一个批一个 tag、inflight 一条、del_pending 按批 +1
/// （回执侧 Ev::Deleted 减去同一条 → 回零才重列）。空批返回 None，不发号不改态。
/// 从 apply_delete 抽出以便在无远程主机环境下用纯状态夹具钉住计数语义。
fn begin_delete_batch(
    st: &mut State,
    side: usize,
    tab_id: u32,
    paths: Vec<(String, bool)>,
) -> Option<(u64, Vec<(String, bool)>)> {
    if paths.is_empty() {
        return None;
    }
    let tag = super::next_seq(st);
    st.inflight.insert(tag, (side, tab_id));
    if let Some(tab) = super::active_remote_tab_mut(st, side) {
        tab.pane.loading = true;
        tab.del_pending += 1;
    }
    Some((tag, paths))
}

// ── 校验码（420）────────────────────────────────────────────
const ALGOS: [&str; 3] = ["SHA-256", "SHA-512", "SM3"];

/// 本地文件哈希（ring / SM3 自带实现，256KB 分块流式；C# LocalComputeAsync 同口径）。
fn hash_local(path: &str, algo: usize) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    use std::io::Read;
    if algo == 2 {
        // M-11：SM3 自带流式实现（src/sm3.rs，GB/T 32905-2016）。
        let mut h = crate::sm3::Sm3::new();
        loop {
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        return Ok(h.finish_hex());
    }
    use ring::digest as dg;
    let alg = if algo == 0 { &dg::SHA256 } else { &dg::SHA512 };
    let mut ctx = dg::Context::new(alg);
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

/// 启动校验计算：本地开线程直算（ring / SM3 自带实现，M-11）；远程走该侧
/// exec 通道（sha256sum / sha512sum / SM3 经 openssl dgst，G9 S6.5）。
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
    let (name, path, side, algo, copied) = match &st.dialog {
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
    shell(
        ui,
        st,
        p,
        420.0,
        242.0,
        t("校验码"),
        false,
        0.0,
        true,
        None,
        ShellOpts::default(),
        |ui, content, st| checksum_body(ui, st, p, content, name, path, side, algo, copied),
    );
}

/// 校验码正文（shell 回调，见 shell 文档①②③）。
#[allow(clippy::too_many_arguments)]
fn checksum_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    name: String,
    path: String,
    side: Option<usize>,
    mut algo: usize,
    mut copied: Option<f64>,
) {
    let mut cui = content_ui(ui, content);
    let x = content.left() + CONTENT_PAD.0;
    let w = content.width() - CONTENT_PAD.0 * 2.0;
    let mut y = content.top() + CONTENT_PAD.1;

    // 条目行：文件图标 + 名称 50% 淡化（F-11 F2：galley 单行 + 22 字符
    // elide，超长文件名不再画穿遮罩）。
    if let Some(img) = st.icons.get(ui.ctx(), "file", p.text_secondary, 16.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(Pos2::new(x + 8.0, y + 10.0), egui::vec2(16.0, 16.0)),
        );
    }
    dlg_text(
        cui.painter(),
        Pos2::new(x + 26.0, y),
        &elide22(&name),
        theme::font_body(),
        TextMode::Single,
        0.0,
        // Opacity=0.5 叠 TextFillColorPrimary（C# EntryName）。
        theme::dim50(p.text_primary),
    );
    y += 20.0 + GROUP_GAP;

    // 算法行：标签 + 下拉（选中即计算）。
    cui.painter().text(
        Pos2::new(x, y + 15.0),
        Align2::LEFT_CENTER,
        t("算法"),
        theme::font_body(),
        p.text_primary,
    );
    // 标签 Auto 宽 + ColumnSpacing 10（C# 算法行 Grid ColumnSpacing=10）。
    let algo_w = cui
        .painter()
        .layout_no_wrap(t("算法").to_owned(), theme::font_body(), Color32::WHITE)
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
    let prev_algo = algo;
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
    // C3②（F-9）：打开首帧初焦点 → 算法组合框（id 为 salt 固定，取响应直接聚焦；
    // 同帧生效，焦点框下帧起由 ComboBox 内部渲染）。
    if st.dialog_fresh {
        resp.response.request_focus();
    }
    // 选中即计算。不读 resp.response.changed()——egui 0.37 的 ComboBox::show_ui
    // 只在 show_index 显式 mark_changed，show_ui 弹层内 selectable_value 的
    // 变更不回传按钮响应，changed() 恒 false（切算法不重算，走查 D2 实证）；
    // 改比本地 algo 前后值（selectable_value 点击帧已写入）。
    if algo != prev_algo {
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
    let copy_w = btn_w(ui, copy_label, &theme::font_body());
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
    let text = if computing {
        t("计算中…").to_owned()
    } else {
        value.clone()
    };
    // 码值内容：垂直滚动（C# TextBox TextWrapping=Wrap 溢出滚动同语义）——
    // SHA-512/SM3 长码折行超过视口高（50-16=34 ≈ 2 行）自动出滚动条，
    // 不超高不出条；max_rect 内收 8 = 原左上 8px 内边距，宽随条显隐自适应。
    let mut val_ui =
        cui.new_child(egui::UiBuilder::new().max_rect(value_r.shrink2(egui::vec2(8.0, 8.0))));
    egui::ScrollArea::vertical()
        .id_salt("chk-value")
        .auto_shrink(false)
        // egui 0.37 ScrollArea 默认 min_scrolled_size=splat(64)：34px 视口被
        // 强扩到 64 → 内容（56）< 64 恒不出条，且裁剪框向下溢出 30px、文字
        // 画进底部按钮区（2026-11-12 走查实证）。归零后视口严格等于 max_rect。
        .min_scrolled_height(0.0)
        .show(&mut val_ui, |s| {
            let galley = s.painter().layout(
                text,
                theme::font_mono_small(),
                p.text_primary,
                s.max_rect().width(),
            );
            let (rect, _resp) = s.allocate_exact_size(galley.size(), Sense::hover());
            s.painter().galley(rect.left_top(), galley, p.text_primary);
        });
    // 复制钮（高 50）：算完才启用（C# ChecksumDialog.xaml.cs:91/103）。
    let copy_ok = !computing && matches!(&st.chk_local, Some(Ok(_)));
    if std_button_en(
        ui,
        Rect::from_min_size(Pos2::new(x + w - copy_w, y), egui::vec2(copy_w, 50.0)),
        copy_label,
        p.text_primary,
        p,
        theme::font_body(),
        copy_ok,
    ) {
        ui.ctx().copy_text(value.to_owned());
        copied = Some(ui.ctx().time());
    }
    y += 50.0 + GROUP_GAP;

    // 底部操作：关闭。
    // C3③（F20/F-9）：Enter 走 shell 旗标触发唯一钮「关闭」。
    if button_row(ui, content.right() - CONTENT_PAD.0, y, &[t("关闭")], p) || st.shell_enter {
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
    let (pane, name, is_dir, bits) = match &st.dialog {
        Dialog::Permission {
            pane,
            name,
            is_dir,
            bits,
            ..
        } => (*pane, name.clone(), *is_dir, *bits),
        _ => return,
    };
    shell(
        ui,
        st,
        p,
        384.0,
        303.0,
        t("权限"),
        false,
        0.0,
        true,
        None,
        ShellOpts::default(),
        |ui, content, st| permission_body(ui, st, p, content, pane, name, is_dir, bits),
    );
}

/// 权限九宫格正文（shell 回调，见 shell 文档①②③）。
#[allow(clippy::too_many_arguments)]
fn permission_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    pane: usize,
    name: String,
    is_dir: bool,
    mut bits: [bool; 9],
) {
    let cui = content_ui(ui, content);
    let x = content.left() + CONTENT_PAD.0;
    let mut y = content.top() + CONTENT_PAD.1;

    // 条目行：类型图标 + 名称 50% 淡化（F-11 F2：galley 单行 + 22 字符
    // elide，超长文件名不再画穿遮罩）。
    let icon = if is_dir { "folder" } else { "file" };
    if let Some(img) = st.icons.get(ui.ctx(), icon, p.text_secondary, 16.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(Pos2::new(x + 8.0, y + 10.0), egui::vec2(16.0, 16.0)),
        );
    }
    dlg_text(
        cui.painter(),
        Pos2::new(x + 26.0, y),
        &elide22(&name),
        theme::font_body(),
        TextMode::Single,
        0.0,
        // Opacity=0.5 叠 TextFillColorPrimary（C# EntryName）。
        theme::dim50(p.text_primary),
    );
    y += 20.0 + GROUP_GAP;

    // 九宫格：行=属主/属组/其他，列=读/写/执行（C# Grid 列 */72/72/72、RowSpacing=4：
    // 表头 14pt 行高 19，行块 32 高 + 4 间距 → 行距 36）。
    let rows = [t("所有者"), t("群组"), t("其他")];
    let cols = [t("读"), t("写"), t("执行")];
    let grid_w = content.width() - CONTENT_PAD.0 * 2.0;
    // 三个 72 列合计 216，右缘与网格右缘对齐（* 列填充余宽）；列中心 = 左缘
    // +36（此前误写 -180 = 第一列中心，循环 +36 重复计偏移 → 末列溢出右缘）。
    let cb_cx0 = x + grid_w - 216.0;
    for (ci, label) in cols.iter().enumerate() {
        let cx = cb_cx0 + 36.0 + ci as f32 * 72.0;
        cui.painter().text(
            Pos2::new(cx, y + 9.5),
            Align2::CENTER_CENTER,
            *label,
            theme::font_body(),
            p.text_secondary,
        );
    }
    for (ri, row) in rows.iter().enumerate() {
        let row_y = y + 19.0 + 4.0 + 16.0 + ri as f32 * 36.0;
        cui.painter().text(
            Pos2::new(x, row_y),
            Align2::LEFT_CENTER,
            *row,
            theme::font_body(),
            p.text_primary,
        );
        for (ci, _) in cols.iter().enumerate() {
            let cx = cb_cx0 + 36.0 + ci as f32 * 72.0;
            let idx = ri * 3 + ci;
            if checkbox(
                ui,
                Rect::from_center_size(Pos2::new(cx, row_y), egui::vec2(18.0, 18.0)),
                bits[idx],
                true,
                p,
            ) {
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
        theme::font_small(),
        p.text_secondary,
    );
    y += 16.0 + GROUP_GAP;

    // 操作按钮：应用 / 取消。
    let apply_y = y;
    let apply_w = btn_w(ui, t("应用"), &theme::font_body());
    let cancel_w = btn_w(ui, t("取消"), &theme::font_body());
    // C3②（F-9）：打开首帧初焦点 → 主钮「应用」。请求必须在创建之后
    //（同 delete_body 注：首帧 sizing pass 会在创建时清先置焦点）。
    let apply_id = ui.next_auto_id();
    // C6（F-9）：主钮「应用」补 accent 色。
    let apply = accent_button(
        ui,
        Rect::from_min_size(
            Pos2::new(content.right() - CONTENT_PAD.0 - apply_w, apply_y),
            egui::vec2(apply_w, BTN_H),
        ),
        t("应用"),
        true,
        p,
        theme::font_body(),
    );
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(apply_id));
    }
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
    if apply || st.shell_enter {
        apply_perm(st, pane, &value);
    }
    if apply || st.shell_enter || cancel {
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
    shell(
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
        ShellOpts::default(),
        |ui, content, st| host_picker_body(ui, st, p, content, hosts, list_h, target, new_tab),
    );
}

/// 打开内容选择器正文（shell 回调，见 shell 文档①②③）。hosts / list_h =
/// 头部预检的主机区快照与滚动区高。
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
fn host_picker_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    hosts: Vec<crate::hosts::HostProfile>,
    list_h: f32,
    target: usize,
    new_tab: bool,
) {
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
            theme::font_small(),
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
            theme::font_body(),
            p.text_primary,
        );
        cui.painter().text(
            Pos2::new(local_row.right() - 10.0, local_row.center().y),
            Align2::RIGHT_CENTER,
            t("浏览本地文件"),
            theme::font_small(),
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
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.row_separator),
    );
    y += 6.0;

    // 主机区头（C# 区头 Margin 16,12,16,4 → 占 32）。
    cui.painter().text(
        Pos2::new(content.left() + pad_x, y + 12.0 + 8.0),
        Align2::LEFT_CENTER,
        t_ctx("主机", "Hosts"),
        theme::font_small(),
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
                    theme::font_small(),
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
                    theme::font_body(),
                    p.text_primary,
                );
                s.painter().text(
                    Pos2::new(row.right() - 10.0, row.center().y),
                    Align2::RIGHT_CENTER,
                    h.detail(),
                    theme::font_small(),
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
            lock_msg_on: false,
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
        let s = theme::stroke(ui.ctx().pixels_per_point(), 1.5, p.text_secondary);
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
    let resp = c.add_enabled(enabled, te);
    // 右键菜单（复制/剪切/粘贴/全选）：拿不到 CtxMenu 实例，走自动注册侧道
    // （ctx_menu::frame_start 排空并入注册表，命中测试用上一帧 rect）。
    super::ctx_menu::register_auto(resp.id, resp.interact_rect);
}

// ── 设置（920×640，侧栏 150 + 分区卡）───────────────────────
const CATS: [&str; 6] = ["外观", "连接", "文件编辑", "传输", "注册", "关于"];

fn settings_dialog(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    win_drag: &mut Vec2,
) -> bool {
    // 唯一 bool 透传点：shell true = 存活且正文已绘（回写在 body 尾）、false = 已关。
    // win_drag 仅 shell 标题栏拖动消费（面板偏移在卡片几何内闭环），body 不需要。
    shell(
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
        ShellOpts::default(),
        |ui, content, st| settings_body(ui, st, p, content),
    )
}

/// 设置面板正文（shell 回调，见 shell 文档①②③）。win_drag 由 shell 消费。
fn settings_body(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette, content: Rect) {
    // C# SettingsPanel 主体 Grid Margin 5 已含在 shell 的 content 几何（与 HostPanel
    // 同构、无统一容器），body 不再二次缩边，左右子卡直接挂。
    let body = content;

    // 左卡片：大类导航（C# 左列宽 150；钮 Padding 14,9 / FontSize 13 / 圆角 4 → 高≈35）。
    // SetCategoryState（SettingsPanel.xaml.cs）：选中 = nav_selected_bg 底 + tab_active_fg 字，
    // 不响应 hover；未选中 = nav_rest_bg 块底（亮主题透明）+ text_primary，按下走 pressed_bg。
    let nav = Rect::from_min_max(body.min, Pos2::new(body.left() + 150.0, body.bottom()));
    ui.painter().rect_filled(nav, 6.0, p.card_bg);
    let mut y = nav.top() + 8.0;
    for (i, cat) in CATS.iter().enumerate() {
        let item = Rect::from_min_size(
            Pos2::new(nav.left() + 8.0, y),
            egui::vec2(nav.width() - 16.0, 35.0),
        );
        let resp = ui.allocate_rect(item, Sense::click());
        // C3②（F-9）：打开首帧初焦点 → 第一项大类。
        if st.dialog_fresh && i == 0 {
            resp.request_focus();
        }
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
        // C3④（F18/F-9）：键盘焦点框（nav 项命中面即焦点面）。
        if resp.has_focus() {
            ui.painter().rect_stroke(
                item,
                4.0,
                theme::stroke(ui.ctx().pixels_per_point(), 2.0, p.focus_stroke),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().text(
            Pos2::new(item.left() + 14.0, item.center().y),
            Align2::LEFT_CENTER,
            t(*cat),
            theme::font(theme::TextRole::Compact),
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
    ui.painter().rect_filled(page, 6.0, p.card_bg);
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
}

/// 页标题（C# 页首 TextBlock：FontSize 20 / SemiBold / Margin 0,0,0,12；
/// brand 字族即注册的 Inter-SemiBold，行高按实测排版取值）。
fn page_title(ui: &mut egui::Ui, title: &str, p: &theme::Palette) {
    let font = theme::font_page_title();
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
        theme::font(theme::TextRole::Compact),
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
        theme::font(theme::TextRole::Compact),
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

/// settings_card 激活卡变体：`hint` = Some(是否成功) 时描述前景着色（成功
/// p.success_fg / 失败 p.critical，F-11 收编裸色；C# RegCodeHint 同语义），
/// None = 常规描述色（引导文案）；控件字号 13 = Compact（RegCodeBox /
/// RegActivateBtn FontSize 13）。
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
    let pc = theme::palette(ui.ctx());
    let desc_c = match hint {
        Some(true) => pc.success_fg,
        Some(false) => pc.critical,
        None => pc.text_secondary,
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
        theme::font(theme::TextRole::Compact),
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
        Some(ui.painter().layout(
            desc.to_owned(),
            theme::font(theme::TextRole::Caption),
            desc_c,
            text_w,
        ))
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
    ui.painter().rect_filled(rect, 4.0, p.card_bg);
    ui.painter().rect_stroke(
        rect,
        4.0,
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.card_stroke),
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
    // C4/C5（F-9/M-12 同款）：自绘无控件语义，补 widget_info（Slider 角色
    // 携带当前值；行标签由调用处行文本承载）。
    resp.widget_info(|| egui::WidgetInfo::slider(enabled, f64::from(*val), ""));
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
        theme::font(theme::TextRole::Compact),
        p.text_primary,
    );
}

/// 下拉框（WinUI ComboBox 自绘）：容器 32 高圆角 4 + 1px ControlStrokeColorDefault
/// 淡边，底色三态（rest ControlFillColorDefault / hover Secondary / 按下·展开
/// Tertiary，展开时描边换 text_primary 近似 FocusStrokeColorOuter）；展开浮层
/// AcrylicInAppFillColorDefault 底 + SurfaceStrokeColorFlyout 描边（圆角 8），
/// 条目高 40（ComboBoxItem 默认），悬停 / 选中 Subtle 系底色，文字左缩进 12
/// （ComboBox Padding 12,4）。`fs` 为文字字号。`enabled=false` 禁用态（CR-5）：
/// 仅 hover 命中不可点击、不展开浮层不写值（视觉禁用 = 实际禁用），配色
/// ControlFillColorDisabled 底 + ControlStrokeColorDisabled 描边 +
/// TextFillColorDisabled 文字。
#[expect(clippy::too_many_arguments)]
pub(crate) fn card_combo(
    ui: &mut egui::Ui,
    r: Rect,
    id_str: &str,
    val: &mut usize,
    options: &[&str],
    w: f32,
    fs: f32,
    enabled: bool,
) {
    let ctx = ui.ctx().clone();
    let p = theme::palette(ui.ctx());
    let id = Id::unique(id_str);
    let open_id = id.with("open");
    // 禁用态无视持久展开态（浮层不展开、旗标不上报；再启用从收起态重开）。
    let open = enabled && ctx.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    let rect = Rect::from_center_size(r.center(), egui::vec2(w, 32.0));
    // 描边像素对齐（theme::snap_rect）：半像素坐标会让 1px 边线被抗锯齿摊薄。
    let rect = theme::snap_rect(rect, ctx.pixels_per_point());
    let resp = ui.allocate_rect(
        rect,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    // C4/C5（F-9/M-12 同款）：自绘无控件语义，补 widget_info（ComboBox 角色，
    // 标签 = 当前选中项文本；enabled 随禁用态如实上报）。
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::ComboBox,
            enabled,
            options.get(*val).copied().unwrap_or(""),
        )
    });
    let (bg, stroke_c) = if !enabled {
        (p.control_disabled, p.control_stroke_disabled)
    } else if open || resp.is_pointer_button_down_on() {
        (p.control_pressed, p.text_primary)
    } else if resp.hovered() {
        (p.control_hover, p.control_stroke_default)
    } else {
        (p.control_bg, p.control_stroke_default)
    };
    let pt = ui.painter_at(rect);
    pt.rect_filled(rect, theme::CONTROL_RADIUS, bg);
    // 描边：展开/按下单色深线（禁用态 ControlStrokeColorDisabled 同走单色），
    // 静默/悬停走 elevation 渐变近似（顶淡底重）。
    if open || resp.is_pointer_button_down_on() || !enabled {
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
    // C3④（F18/F-9）：键盘焦点框（触发行即焦点面，画在描边之后不被盖）。
    if resp.has_focus() {
        pt.rect_stroke(
            rect,
            theme::CONTROL_RADIUS,
            theme::stroke(ctx.pixels_per_point(), 2.0, p.focus_stroke),
            egui::StrokeKind::Inside,
        );
    }
    let txt_c = if enabled {
        p.text_primary
    } else {
        p.text_disabled
    };
    pt.text(
        Pos2::new(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        options.get(*val).copied().unwrap_or(""),
        FontId::proportional(fs),
        txt_c,
    );
    // 展开箭头（右缩进 17，两段 1.5px 线画 ˅；禁用随文字同灰）。
    let cx = rect.right() - 17.0;
    let cy = rect.center().y;
    let ch = theme::stroke(
        ui.ctx().pixels_per_point(),
        1.5,
        if enabled {
            p.text_secondary
        } else {
            p.text_disabled
        },
    );
    pt.line_segment([Pos2::new(cx - 4.0, cy - 2.0), Pos2::new(cx, cy + 2.0)], ch);
    pt.line_segment([Pos2::new(cx, cy + 2.0), Pos2::new(cx + 4.0, cy - 2.0)], ch);
    if enabled && resp.clicked() {
        ctx.data_mut(|d| d.insert_temp(open_id, !open));
    }

    let mut close = false;
    if open {
        // C3①（F5/F-9）：展开态 Esc 先收组合（shell 入口见 COMBO_OPEN 旗标
        // 让路，不再一击关掉整个弹窗）。
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            close = true;
        }
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
                    theme::stroke(pt.ctx().pixels_per_point(), 1.0, p.combo_popup_stroke),
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
    // C3①（F-9）：本帧终态仍展开 → 上报旗标（shell 入口每帧 swap 清底；
    // store-only-true 防同帧多组合互踩，Esc/Enter 豁免据此放行）。
    if open && !close {
        COMBO_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn appearance_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("外观"), p);
    let w = ui.max_rect().width();

    // 主题：切换即时生效。下拉三项 = 跟随系统 / 深色 / 浅色（store "" = 跟随，
    // F-6 与描述「界面配色跟随系统，或手动指定深浅」对齐）；选「跟随系统」
    // 当场按系统偏好应用，落盘 ""（重启经 startup_theme_dark/resolve_dark 再解析）。
    let picked_now = st.settings.theme_picked;
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
            let mut i = picked_now;
            card_combo(
                c,
                r,
                "set-theme",
                &mut i,
                &[t("跟随系统"), t("深色"), t("浅色")],
                150.0,
                theme::role_size(theme::TextRole::Compact),
                true,
            );
            i
        },
    );
    if picked != picked_now {
        // 动过下拉才允许整包回写（T19-1：未动时快照原样，"" 不被抹掉）。
        st.settings.theme_touched = true;
        st.settings.theme_picked = picked;
        let theme = match picked {
            0 => {
                if crate::theme::system_prefers_light() {
                    egui::Theme::Light
                } else {
                    egui::Theme::Dark
                }
            }
            1 => egui::Theme::Dark,
            _ => egui::Theme::Light,
        };
        crate::theme::install(ui.ctx(), theme);
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
                theme::role_size(theme::TextRole::Compact),
                true,
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
                theme::role_size(theme::TextRole::Compact),
                true,
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
                if radio(
                    c,
                    rr,
                    sel == i,
                    label,
                    theme::font(theme::TextRole::Compact),
                    &pc,
                ) {
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
        t("外部编辑器可执行文件（如 notepad++、VS Code）；留空使用内置编辑器。"),
        300.0,
        !builtin,
        8.0,
        24.0,
        |c, r| {
            let pc = theme::palette(c.ctx());
            // 控件字号 13（TextBox / 浏览钮 FontSize 13）= 具名 Compact：
            // TextEdit 与量宽同源，不再局部覆盖全局 Body=14（text_styles.insert 归零）。
            let bw = c
                .painter()
                .layout_no_wrap(
                    t("浏览…").to_owned(),
                    theme::font(theme::TextRole::Compact),
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
            let te = egui::TextEdit::singleline(&mut s.editor_path)
                .font(theme::font(theme::TextRole::Compact));
            let te_resp = c.put(box_r, te);
            // 右键菜单（复制/剪切/粘贴/全选）：自动注册侧道。
            super::ctx_menu::register_auto(te_resp.id, te_resp.interact_rect);
            let btn_r = Rect::from_min_size(
                Pos2::new(box_r.right() + 6.0, r.center().y - 16.0),
                egui::vec2(bw, 32.0),
            );
            // C# EditorBrowseBtn_Click：FileOpenPicker 仅 .exe 过滤，选中即填入
            // 路径框（TextChanged 即时落盘；此处 sync_to_store 每帧回写等价）。
            if std_button_fs(
                c,
                btn_r,
                t("浏览…"),
                pc.text_primary,
                &pc,
                theme::font(theme::TextRole::Compact),
            ) && let Some(path) = pick_exe_file()
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
    // （「保留修改时间」卡 C# 已摘除，Rust 按实现保留：transfer.rs 结算点
    // SETSTAT/filetime + tar 头天然保留）
    // 母卡 gap 1（卡组 StackPanel Spacing 1）+ 子卡 indent 24（Margin 24,0,0,0），组尾 gap 8。
    settings_card(
        ui,
        w,
        t("并发传输数"),
        t(
            "传输一律使用独立连接：1 = 单连接串行，大于 1 时多连接并发，多文件传输更快；部分服务器可能限制连接数。",
        ),
        186.0,
        true,
        8.0,
        0.0,
        |c, r| card_slider(c, r, &mut s.concurrent, 1.0, 3.0, 1.0, "{0} 个", true),
    );
    let del_desc = if s.del_server_cmd {
        t(
            "优先用服务端命令 rm -rf 删除远程目录（海量文件秒级）；命令通道不可用或未能删净时回退 SFTP 递归。",
        )
    } else {
        t("通过 SFTP 协议逐条递归删除，兼容所有服务器；大目录耗时较长。")
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
            "中断后从断点继续：目标小于源时追加，否则全量重传；同名旧文件先弹窗确认，不会误追加。默认开启。",
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
            "完成后经 exec 通道比对本地与远端文件杂凑，不一致的任务标记失败；命令通道不可用时跳过校验，不影响传输结果。默认关闭。",
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
        t("自动：按可用性顺位选用（SHA256 → SHA512 → SM3）；固定算法不可用时校验跳过"),
        110.0,
        verify_on,
        8.0,
        24.0,
        |c, r| {
            let mut i = verify_algo;
            // CR-5：enabled 随「传输后校验」开关（verify=false 禁用下拉，防改值落盘）。
            card_combo(
                c,
                r,
                "set-verify",
                &mut i,
                &[t("自动"), "SHA-256", "SHA-512", "SM3"],
                110.0,
                theme::role_size(theme::TextRole::Compact),
                verify_on,
            );
            i
        },
    );
    s.verify_algo = picked;

    // 保留修改时间（C# 位于校验组与 tar 组之间；上传远端 SETSTAT mtime /
    // 下载本地 filetime 设 mtime，见 transfer.rs 结算点。F-6 起说明为 Rust
    // 精简版，有意不逐字对齐 C# PreserveMtimeHint，分叉记录见 MIGRATION §3.3）。
    settings_card(
        ui,
        w,
        t("传输后保留修改时间"),
        t(
            "传输成功时把源文件的修改时间同步到目标侧（tar 路径经 tar 头天然保留）；仅同步文件，目录不做；失败只记日志。默认开启。",
        ),
        46.0,
        true,
        8.0,
        0.0,
        |c, r| card_toggle(c, r, true, &mut s.preserve_mtime),
    );

    // tar 打包加速母子组。
    settings_card(
        ui,
        w,
        t("tar 打包加速"),
        t(
            "目录文件数达到阈值时用单流打包替代逐文件往返（海量小文件数量级提速）；远端 tar+gzip 不可用或本地扫描失败时回退逐文件传输，开始传输后失败只能整体重试。",
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
        // 同续传阈值布局；值文字为文件数（无单位，最宽 "10000"）。
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
                "{0}",
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
    let ctrl_font = theme::font(theme::TextRole::Compact);
    let act_bw = ui
        .painter()
        .layout_no_wrap(t("激活").to_owned(), ctrl_font.clone(), Color32::WHITE)
        .size()
        .x
        + 24.0; // Padding 12,5 左右
    let buy_cn_w = if s.purchase_cn.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(
                t("购买注册码 (爱发电)").to_owned(),
                ctrl_font.clone(),
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
                ctrl_font.clone(),
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
            // 控件字号 13（RegCodeBox / RegActivateBtn FontSize 13）= 具名
            // Compact：TextEdit 显式 font(Compact)，全局 Body=14 不被覆盖。
            // 右列垂直居中（C# StackPanel VerticalAlignment=Center，两行共 72）。
            let row_top = r.center().y - 36.0;
            let box_r = Rect::from_min_size(
                Pos2::new(r.right() - 250.0 - 8.0 - act_bw, row_top),
                egui::vec2(250.0, 32.0),
            );
            let te = egui::TextEdit::singleline(&mut st.settings.reg_code)
                .font(theme::font(theme::TextRole::Compact))
                .desired_width(250.0);
            let te_resp = c.put(box_r, te);
            // 右键菜单（复制/剪切/粘贴/全选）：自动注册侧道。
            super::ctx_menu::register_auto(te_resp.id, te_resp.interact_rect);
            let btn_r = Rect::from_min_size(
                Pos2::new(box_r.right() + 8.0, row_top),
                egui::vec2(act_bw, 32.0),
            );
            // C# RegActivateBtn_Click：err = Activate(...)；hint = err ?? 激活成功，
            // 成功绿 / 失败红；输入框不清空（C# RegCodeBox.Text 无清空）。
            if std_button_fs(
                c,
                btn_r,
                t("激活"),
                p.text_primary,
                p,
                theme::font(theme::TextRole::Compact),
            ) {
                st.settings.reg_hint = Some(
                    match crate::registration::service::global().activate(&st.settings.reg_code) {
                        crate::registration::service::ActivateResult::Success(_) => {
                            (t("激活成功，感谢支持！").to_owned(), true)
                        }
                        crate::registration::service::ActivateResult::OfficialOnly => (
                            t("激活组件未随本仓库源码分发，请从 ifrees.top 下载发布版。")
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
                                //错误提示（不再 unreachable! 崩溃；源码编译版/异常
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
                        theme::font(theme::TextRole::Compact),
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
                        theme::font(theme::TextRole::Compact),
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
        .layout_no_wrap(text.to_owned(), theme::font_small(), theme::MEASURE_ONLY)
        .size()
        .x;
    12.0 + 13.0 + 6.0 + tw + 12.0
}

/// 勋章胶囊（C# RegBadgePill：Padding 12,5 / 圆角 12 / bg #26D4A017 边 #66D4A017，
/// 内金色 #D4A017 图标 + 文字 12；C# E734 徽章星形 icons-src 无对应资产 → 文本 ★
/// 近似）。右贴齐控件区右缘，垂直居中 32 高。
fn badge_pill(ui: &mut egui::Ui, r: Rect, text: &str) {
    let gold = theme::WARNING;
    let pill_w = badge_pill_w(ui, text);
    let pill = Rect::from_min_size(
        Pos2::new(r.right() - pill_w, r.center().y - 16.0),
        egui::vec2(pill_w, 32.0),
    );
    ui.painter().rect_filled(
        pill,
        theme::PILL_RADIUS,
        Color32::from_rgba_unmultiplied(0xD4, 0xA0, 0x17, 0x26),
    );
    ui.painter().rect_stroke(
        pill,
        theme::PILL_RADIUS,
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, theme::warning_border()),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        Pos2::new(pill.left() + 12.0 + 6.5, pill.center().y),
        Align2::CENTER_CENTER,
        "★",
        theme::font(theme::TextRole::Compact),
        gold,
    );
    ui.painter().text(
        Pos2::new(pill.left() + 12.0 + 13.0 + 6.0, pill.center().y),
        Align2::LEFT_CENTER,
        text,
        theme::font_small(),
        gold,
    );
}

/// 超链接按钮镜像（C# HyperlinkButton：FontSize 13 / accent 前景 / hover 下划线，新标签页打开）。
fn card_link(ui: &mut egui::Ui, r: Rect, label: &str, url: &str, p: &theme::Palette) {
    let font = theme::font(theme::TextRole::Compact);
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
        cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::PointingHand);
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
            theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.accent),
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
        theme::font_brand_compact(),
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
            // 检查更新钮（FontSize 13 = Compact；宽度经 btn_w 与 std_button_en
            // 绘制同源量出，英文 "Check for updates" 自动撑宽不再溢出；检查中
            // 禁用）。右缘恒 -58：与右邻开关（宽 46 / 中心 -23）保持 12 间距。
            let upd_w = btn_w(c, t("检查更新"), &theme::font(theme::TextRole::Compact));
            let btn_r = Rect::from_min_size(
                Pos2::new(r.right() - 58.0 - upd_w, r.center().y - 16.0),
                egui::vec2(upd_w, 32.0),
            );
            if std_button_en(
                c,
                btn_r,
                t("检查更新"),
                p.text_primary,
                p,
                theme::font(theme::TextRole::Compact),
                !checking,
            ) {
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
            "本应用以 GNU AGPL-3.0 许可证开源发布。ifrees.top 发布版额外集成了独立分发的增值组件，这些组件不包含在本仓库源码。",
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
        t("如果它对你有帮助，欢迎注册成为支持者——这是对开发的支持，不与任何功能绑定。"),
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
/// 批量队列菜单六项显隐规则（C# QueueContainer.xaml.cs:148-155 逐条翻译；
/// 照 transfer/tar.rs gate_open 纯函数范式，渲染只消费结果）。
/// 返回位序：[暂停, 继续传输, 停止, 清除已完成, 清除所有, 重置并重新请求所有]。
/// 计数为各状态实数（非 any 布尔）；清除所有 = 队列非空即显（C# total 口径）；
/// total==0 六位全 false → 调用侧 items 为空，菜单层置 None 不弹。
fn batch_menu_rules(
    paused: bool,
    waiting: i64,
    transferring: i64,
    done: i64,
    failed: i64,
    cancelled: i64,
) -> [bool; 6] {
    let finished = done + failed + cancelled > 0;
    [
        !paused && transferring > 0, // 暂停：有传输中且未暂停
        paused,                      // 继续传输：暂停中
        waiting + transferring > 0,  // 停止：有等待或传输中
        finished,                    // 清除已完成：有已完结
        // 清除所有：队列非空即显（五态计数和 = 队列规模）。
        waiting + transferring + done + failed + cancelled > 0,
        // 重置并重新请求所有：静止（无等待/传输中）且有已完结。
        waiting + transferring == 0 && finished,
    ]
}

fn draw_menu(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(m) = st.menu.as_ref() else {
        return;
    };
    // 菜单打开期间以 Dialog 层压住遮罩下控件的 hover 意图（原做法是抢一次
    // set_cursor_icon，会被帧尾 edge_resize 反转；现由帧尾统一裁决）。
    cursor_intent(ui.ctx(), CursorLayer::Dialog, egui::CursorIcon::Default);
    let pos = m.pos;
    let pane = m.pane;
    let row = m.row;
    let queue = m.queue;
    let term_menu = m.terminal;
    let cols_menu = m.cols;
    let ppp = ui.ctx().pixels_per_point();

    // 菜单模型：行菜单（本地/远程两套）/ 空白区菜单 / 队列菜单（C# BuildItemMenu / BuildBlankMenu）/
    // 终端菜单（C# TerminalMenuFlyout）/ 列显隐菜单（T20-1 表头右键）。
    enum Item {
        /// 文本, 可用, 危险红。
        Label(&'static str, bool, bool),
        /// 勾选型（T20-1，C# ToggleMenuFlyoutItem）：键, 可用, 勾选中。
        Toggle(&'static str, bool, bool),
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
                // C# Root_RightTapped：六项按各自条件独立显隐（C#
                // QueueContainer.xaml.cs:148-155 逐条翻译，规则集中在
                // batch_menu_rules）。计数从 any(...) 布尔改各状态实数——原三分支
                // 互斥会丢「只有等待 → 停止+清除所有」边角、且等待未静止时误出
                // 「重置并重新请求所有」。total==0 → 六位全 false → items 空 →
                // 菜单层置 None 不弹。
                let (paused, waiting, transferring, done, failed, cancelled) = match &st.transfer {
                    Some(q) => {
                        let (w, x, d, f, c) = q.with_items(|items| {
                            let (mut w, mut x, mut d, mut f, mut c) =
                                (0i64, 0i64, 0i64, 0i64, 0i64);
                            for i in items {
                                match i.status {
                                    crate::transfer::Status::Waiting => w += 1,
                                    // 校验中计入传输中（C# TransferQueueService.cs:695
                                    // Transferring or Verifying）：否则校验期间菜单缺
                                    // 暂停/停止入口。扫描中同口径（T20-2：扫描期
                                    // 亦需暂停/停止入口可用）。
                                    crate::transfer::Status::Transferring
                                    | crate::transfer::Status::Verifying
                                    | crate::transfer::Status::Scanning => x += 1,
                                    crate::transfer::Status::Done => d += 1,
                                    crate::transfer::Status::Failed => f += 1,
                                    crate::transfer::Status::Cancelled => c += 1,
                                }
                            }
                            (w, x, d, f, c)
                        });
                        (q.paused(), w, x, d, f, c)
                    }
                    None => (false, 0, 0, 0, 0, 0),
                };
                let rules =
                    batch_menu_rules(paused, waiting, transferring, done, failed, cancelled);
                // 位序与 batch_menu_rules 返回值一致；六项文案均在 i18n 表内。
                const LABELS: [&str; 6] = [
                    "暂停",
                    "继续传输",
                    "停止",
                    "清除已完成",
                    "清除所有",
                    "重置并重新请求所有",
                ];
                let mut v = vec![];
                for (label, on) in LABELS.into_iter().zip(rules) {
                    if on {
                        v.push(Item::Label(label, true, false));
                    }
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
                                    | crate::transfer::Status::Scanning
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
                    // 实际重建见命令区遍历）。S-8：在途显示「正在重连…」置灰
                    // （C# RowReconnectItem 等待态同位），Ev 回账归零后恢复。
                    if st.reconnecting > 0 {
                        v.push(Item::Label("正在重连…", false, false));
                    } else {
                        let can_reconnect = remote
                            && st
                                .remotes
                                .iter()
                                .any(|rs| rs.sessions.iter().any(|s| !s.ssh.is_connected()));
                        v.push(Item::Label("重连", can_reconnect, false));
                    }
                }
                v.push(Item::Label("移除", true, false));
                v
            }
        }
    } else if cols_menu {
        // 列显隐菜单（T20-1，C# FileView.xaml:359-367 四 ToggleMenuFlyoutItem）：
        // 勾选型四项；权限项仅远程模式可用（本地由 Mode 整列隐藏，C# :367
        // IsEnabled="False" 同口径）。键复用表头 i18n（零新键，判据⑥）。
        let vis = super::panes::col_vis();
        let remote = menu_side(st, pane).is_some();
        vec![
            Item::Toggle("大小", true, vis.size),
            Item::Toggle("类型", true, vis.ty),
            Item::Toggle("修改时间", true, vis.modified),
            Item::Toggle("权限", remote, vis.perms),
        ]
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
                // 空白区菜单（C# BuildBlankMenu 四项：新建文件夹 / 新建文件 / ─ /
                // 刷新，启用 = 远程该侧已连 / 本地当前路径非空）。Rust 有意差异
                // （T20-G④ 取证补能力）：右键内容跟随远程状态——远程侧尾部追加
                // 「断开」（会话在池时显示）。C# 无显式断开入口，会话只在关 tab /
                // 重连时隐式出池；工单④需要在 tab 保持打开的前提下断开会话验证
                // DragOver 判定门。
                let p = pane_view(st, pane);
                let side = menu_side(st, pane);
                // 新建可用 = 本地有路径 / 远程已浏览到目录（与工具栏 can_new 同口径）。
                let can_new = match side {
                    Some(s) => super::pane_view(st, s).remote_path.is_some(),
                    None => p.path.is_some(),
                };
                let mut items = vec![
                    Item::Label("新建文件夹", can_new, false),
                    Item::Label("新建文件", can_new, false),
                    Item::Sep,
                    Item::Label("刷新", true, false),
                ];
                // 远程侧且该 tab 会话在池 → 追加「断开」。
                if let Some(s) = side
                    && super::active_remote_session(st, s).is_some()
                {
                    items.push(Item::Sep);
                    items.push(Item::Label("断开", true, false));
                }
                items
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

    // 菜单体感应面：兜住体上一切点击（含项间空隙与内边距），防穿透到下层控件；
    // 体上点击不关菜单（C# MenuFlyout 体上不 dismiss）。「体外按下即关」已上移到
    // root() 帧首（关在窗格绘制之前，抬起照常落到下层控件，右键同势关旧开新）。
    // 这里不再挂全窗 catcher——全窗 catcher 在覆盖层最后注册，吞掉一切点击且只认
    // 左键（clicked()），右键被静默吞掉：菜单开着时必须先关才能再右键，即
    // 「右键很涩」的根因。
    ui.allocate_rect(rect, Sense::click());
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        st.menu = None;
        return;
    }
    // 本帧菜单体 rect 回写（root() 下一帧帧首「体外按下即关」判定用）。
    st.menu_rect = Some(rect);

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
                    theme::font_body(),
                    fg,
                );
                if *enabled && resp.clicked() {
                    action = Some(label);
                }
                iy += MENU_ITEM_H;
            }
            Item::Toggle(label, enabled, checked) => {
                let r = Rect::from_min_size(
                    Pos2::new(rect.left() + 4.0, iy),
                    egui::vec2(MENU_W - 8.0, MENU_ITEM_H),
                );
                let resp = ui.allocate_rect(r, Sense::click());
                if *enabled && resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(r, 4.0, p.row_pressed);
                } else if *enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }
                let fg = if *enabled {
                    p.text_primary
                } else {
                    p.text_disabled
                };
                // WinUI ToggleMenuFlyoutItem：勾选字形列在文字左侧（选中画 ✓ 强调
                // 色，未选中留空占位对齐），文字列随字形列整体右移。
                if *checked {
                    ui.painter().text(
                        Pos2::new(r.left() + 11.0 + 4.0, r.center().y - 0.5),
                        Align2::LEFT_CENTER,
                        "✓",
                        theme::font_body(),
                        p.accent,
                    );
                }
                ui.painter().text(
                    Pos2::new(r.left() + 11.0 + 18.0, r.center().y - 0.5),
                    Align2::LEFT_CENTER,
                    t(*label),
                    theme::font_body(),
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
        // 列显隐菜单（T20-1）：翻转全局 ColVisibility（WinUI ToggleMenuFlyoutItem
        // 点击即关 flyout 同势）；落盘随每帧变更检测走 window.ini 0.3s 节流。
        if cols_menu {
            super::panes::toggle_col(label);
            return;
        }
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
                            fresh: true,
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
            "断开" => {
                // T20-G④ 断开入口（Rust 有意差异，空白区菜单）：活动 tab 会话
                // refs 减一、归零即 disconnect 出池（与关 tab 释放 :2106-2124
                // 同语义），tab 保持打开但回「未连接」态；pane 复位远程视图。
                if let Some(side) = menu_side(st, pane)
                    && let Some(tab_id) = super::active_tab_id(st, pane)
                    && let Some(ti) = st.remotes[side].tabs.iter().position(|x| x.id == tab_id)
                {
                    // RemoteTab.sid = 0 表示未连接（Default 初值）。
                    let osid = st.remotes[side].tabs[ti].sid;
                    if osid != 0
                        && let Some(si) =
                            st.remotes[side].sessions.iter().position(|s| s.sid == osid)
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
                    st.remotes[side].tabs[ti].sid = 0;
                    let pp = pane_view_mut(st, side);
                    pp.remote_path = None;
                    pp.entries.clear();
                    pp.selected.clear();
                    pp.sel_anchor = None;
                    pp.tree.clear();
                    pp.tree_synced_for = None;
                    pp.msg = Some(t("连接已断开").to_owned());
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
                    // 队列生命周期日志（C# QueueContainer.xaml.cs:169 LogQueuePaused）。
                    st.terminal
                        .push((crate::local::now_hms(), t("传输已暂停").to_owned()));
                }
            }
            "继续传输" => {
                if let Some(q) = &st.transfer {
                    q.resume();
                    // C# QueueContainer.xaml.cs:176 LogQueueResumed。
                    st.terminal
                        .push((crate::local::now_hms(), t("传输继续").to_owned()));
                }
            }
            "停止" => {
                if let Some(q) = &st.transfer {
                    q.stop_all();
                    // C# QueueContainer.xaml.cs:182 LogQueueStopped。
                    st.terminal
                        .push((crate::local::now_hms(), t("传输已停止").to_owned()));
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
                // C# TransferQueueService.RequeueAllAsync :890-911：全部行回等待
                // （含 Done）且显式解除暂停；:920-922 LogQueueRerun 记条数（批次
                // 结束汇总由 drain_batch_reports 随重跑再出一条，同 :928-929）。
                if let Some(q) = &st.transfer {
                    let n = q.with_items(|items| items.len());
                    q.reset_all();
                    st.terminal.push((
                        crate::local::now_hms(),
                        tfmt("重置并重新请求所有：{0} 项", &[&n.to_string()]),
                    ));
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
                // S-8：记账在途（菜单态「正在重连…」置灰 + 专用成功/失败日志，
                // mod.rs Ev::Connected / Ev::Failed(连接) 逐个回账归零）。
                st.reconnecting = downs.len() as u32;
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
/// 冲突对比表展示行：(名称, 源(大小·mtime), 目标(大小·mtime), 目标较新)。
type ConfirmRows = Vec<(String, (String, String), (String, String), bool)>;

fn transfer_confirm_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    // 行数据先拷为展示串（头部预检脱离 st.dialog 借用）；同时取勾选默认值。
    let (rows, note, defaults): (ConfirmRows, Option<String>, Vec<bool>) = match &st.dialog {
        Dialog::TransferConfirm { rows, note } => {
            let defaults = super::conflict_defaults(rows);
            let rows = rows
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
                .collect();
            (rows, note.clone(), defaults)
        }
        _ => return,
    };
    // F-10 方案A：勾选态与 rows 行序严格对齐（长度失配 = 残留/重建，重置默认）。
    if st.conflict_sel.as_ref().map(|s| s.len()) != Some(defaults.len()) {
        st.conflict_sel = Some(defaults);
    }
    shell(
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
        // C1/F-9：关闭路径与「取消」同语义（F1），宿主自声明暂存态处置。
        ShellOpts {
            on_close: CloseAction::CancelPendingStream,
            ..Default::default()
        },
        |ui, content, st| transfer_confirm_body(ui, st, p, content, rows, note),
    );
}

/// 传输覆盖确认正文（shell 回调，见 shell 文档①②③）。
#[allow(clippy::too_many_arguments)]
fn transfer_confirm_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    rows: ConfirmRows,
    note: Option<String>,
) {
    // 内容容器统一内边距 15（全局令牌 CONTENT_PAD：文本行 / 对比表 / 按钮 /
    // 提示语同边距）。

    let mut cui = content_ui(ui, content);
    cui.add_space(CONTENT_PAD.1);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(RichText::new(tfmt("目标已存在 {0} 个同名项目：", &[&rows.len()])).size(14.0));
    });
    // 范围行（T20-H 流式落位）：已落位项不可逆，写明本次确认范围防误解。
    // T20-E-R：英文文案代入 M/N/C 后约 118 字符，单行 Label 在 640 卡片
    // 的 horizontal 内会裁掉尾部「(N conflicts)」——显式折行，实数全可见。
    if let Some(note) = &note {
        cui.horizontal(|row| {
            row.add_space(CONTENT_PAD.0);
            row.add(
                egui::Label::new(
                    RichText::new(note.as_str())
                        .size(12.0)
                        .color(theme::WARNING),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
    }
    cui.add_space(6.0);

    // 冲突清单对比表（勾选 / 名称 / 源 / 目标，大小+mtime 两行；超出可视高
    // 滚动）。底部预留：按钮行高 + 上方提示语一行。
    // F-10 方案A：勾选态借作用域借用（块尾归还，按钮段再动 st）。
    {
        let sel: &mut [bool] = st
            .conflict_sel
            .as_mut()
            .expect("TransferConfirm 创建帧已按 rows 对齐勾选态");
        let list_h =
            (content.bottom() - CONTENT_PAD.1 - BTN_H - 8.0 - 18.0 - cui.cursor().top()).max(40.0);
        egui::ScrollArea::vertical()
            .id_salt("tx-conflict-list")
            .max_height(list_h)
            .show(&mut cui, |s| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: CONTENT_PAD.0 as i8,
                        right: CONTENT_PAD.0 as i8,
                        top: 0,
                        bottom: 0,
                    })
                    .show(s, |s| {
                        // 表格横向几何：gx0..gx1 = 内边距 15 后的内容宽（贴满右缘）。
                        let gx0 = s.cursor().left();
                        let avail = s.available_width();
                        let gx1 = gx0 + avail;
                        let cell_w = 2.0 * TBL_CELL_PAD as f32;
                        // 名称列最小适应：按表身字体（TBL_BODY_SIZE）实测最长文件名
                        // + 格内边距（列宽判定按 widget_rect = 文字实宽，名称列即随
                        // 内容收缩）。
                        let name_w = rows
                            .iter()
                            .map(|(name, ..)| {
                                s.painter()
                                    .layout_no_wrap(
                                        name.clone(),
                                        egui::FontId::proportional(TBL_BODY_SIZE),
                                        Color32::WHITE,
                                    )
                                    .size()
                                    .x
                            })
                            .fold(0.0_f32, f32::max)
                            + cell_w;
                        // 首列 checkbox（F-10 方案A）：本体 ~18 + 格内边距。
                        let chk_w = 18.0 + cell_w;
                        // 源/目标列均分剩余宽（列间距 16×3；下限 120 防极端长名挤没）。
                        let rest_w = ((avail - chk_w - name_w - 48.0) / 2.0).max(120.0);
                        // 很浅的表格横线：画在行间垂直间距（5px）的中线上。
                        let line_color = p.text_secondary.gamma_multiply(0.2);
                        let hline = |g: &mut egui::Ui| {
                            let y = g.cursor().top() - 2.5;
                            g.painter().rect_filled(
                                Rect::from_min_size(Pos2::new(gx0, y), egui::vec2(gx1 - gx0, 1.0)),
                                0.0,
                                line_color,
                            );
                        };
                        // 单元格：内边距 TBL_CELL_PAD 的 Frame 包裹，禁折行（Extend）；
                        // 多段文本（size + 日期）同行绘制，段间空隔 TBL_VAL_GAP。
                        // min_w = 列宽下限（vertical scope 的 min_rect 计入 widget_rect）。
                        let cell =
                            |v: &mut egui::Ui, texts: &[egui::RichText], min_w: Option<f32>| {
                                egui::Frame::new()
                                    .inner_margin(egui::Margin::same(TBL_CELL_PAD))
                                    .show(v, |v| {
                                        if let Some(w) = min_w {
                                            v.set_min_width(w - cell_w);
                                        }
                                        if texts.len() == 1 {
                                            v.add(
                                                egui::Label::new(texts[0].clone())
                                                    .wrap_mode(egui::TextWrapMode::Extend),
                                            );
                                        } else {
                                            v.horizontal(|h| {
                                                for (i, tx) in texts.iter().enumerate() {
                                                    if i > 0 {
                                                        h.add_space(TBL_VAL_GAP);
                                                    }
                                                    h.add(
                                                        egui::Label::new(tx.clone())
                                                            .wrap_mode(egui::TextWrapMode::Extend),
                                                    );
                                                }
                                            });
                                        }
                                    });
                            };
                        // 勾选格：同 cell 内边距的 Frame 包 checkbox（行对齐）。
                        let chk_cell = |v: &mut egui::Ui, checked: &mut bool| {
                            egui::Frame::new()
                                .inner_margin(egui::Margin::same(TBL_CELL_PAD))
                                .show(v, |v| {
                                    v.add(egui::Checkbox::without_text(checked));
                                });
                        };
                        egui::Grid::new("tx-conflict-grid")
                            .num_columns(4)
                            .spacing([16.0, 5.0])
                            .show(s, |g| {
                                // 表头首列：全选 checkbox（F-10 方案A；tooltip 复用
                                // 既有键「应用到本批所有冲突项」）。
                                egui::Frame::new()
                                    .inner_margin(egui::Margin::same(TBL_CELL_PAD))
                                    .show(g, |v| {
                                        let mut on = sel.iter().all(|b| *b);
                                        let r = v.add(egui::Checkbox::without_text(&mut on));
                                        let flipped = r.changed();
                                        r.on_hover_text(t("应用到本批所有冲突项"));
                                        if flipped {
                                            select_all(sel, on);
                                        }
                                    });
                                cell(
                                    g,
                                    &[RichText::new(t("名称"))
                                        .size(TBL_HEAD_SIZE)
                                        .color(p.text_secondary)],
                                    None,
                                );
                                cell(
                                    g,
                                    &[RichText::new(t("源"))
                                        .size(TBL_HEAD_SIZE)
                                        .color(p.text_secondary)],
                                    Some(rest_w),
                                );
                                cell(
                                    g,
                                    &[RichText::new(t("目标"))
                                        .size(TBL_HEAD_SIZE)
                                        .color(p.text_secondary)],
                                    Some(rest_w),
                                );
                                g.end_row();
                                hline(g);
                                for (ri, (name, src, dst, newer)) in rows.iter().enumerate() {
                                    // 行首勾选（F-10 方案A：直接绑定 State.conflict_sel）。
                                    chk_cell(g, &mut sel[ri]);
                                    let d_col = if *newer {
                                        theme::WARNING
                                    } else {
                                        p.text_secondary
                                    };
                                    cell(
                                        g,
                                        &[RichText::new(name.as_str()).size(TBL_BODY_SIZE)],
                                        None,
                                    );
                                    // 源列：size 与日期同行（空隔开，不换行）。
                                    let src_pair = [
                                        RichText::new(src.0.as_str())
                                            .size(TBL_BODY_SIZE)
                                            .color(p.text_secondary),
                                        RichText::new(src.1.as_str())
                                            .size(TBL_BODY_SIZE)
                                            .color(p.text_secondary),
                                    ];
                                    cell(g, &src_pair, Some(rest_w));
                                    // 目标列：size / 日期按有无同行拼接，双空 = 「—」。
                                    if dst.0.is_empty() && dst.1.is_empty() {
                                        cell(
                                            g,
                                            &[RichText::new("—")
                                                .size(TBL_BODY_SIZE)
                                                .color(p.text_secondary)],
                                            Some(rest_w),
                                        );
                                    } else {
                                        let mut pair: Vec<egui::RichText> = Vec::new();
                                        if !dst.0.is_empty() {
                                            pair.push(
                                                RichText::new(dst.0.as_str())
                                                    .size(TBL_BODY_SIZE)
                                                    .color(d_col),
                                            );
                                        }
                                        if !dst.1.is_empty() {
                                            pair.push(
                                                RichText::new(dst.1.as_str())
                                                    .size(TBL_BODY_SIZE)
                                                    .color(d_col),
                                            );
                                        }
                                        cell(g, &pair, Some(rest_w));
                                    }
                                    g.end_row();
                                    hline(g);
                                }
                            });
                    });
            });
    }

    // 右对齐按钮组：取消 / 应用（F-10 方案A：批级三选一收敛为逐项勾选 +
    // 应用，勾选即决策；C6 规则A：主钮「应用」恒最右）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_apply = btn_w(ui, t("应用"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_apply = Rect::from_min_size(Pos2::new(x0 - w_apply, y), egui::vec2(w_apply, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_apply - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    // 勾选语义提示（F-10 方案A）：勾 = 覆盖上传，不勾 = 跳过。位置：按钮行
    // 上方，左缘与「取消」左缘同线。
    ui.painter().text(
        Pos2::new(r_cancel.left(), y - 14.0),
        Align2::LEFT_CENTER,
        t("勾选的冲突项覆盖上传，未勾选的跳过"),
        theme::font(theme::TextRole::Caption),
        p.text_secondary,
    );

    // C3②（F-9）：打开首帧初焦点 → 主钮「应用」。请求必须在创建之后
    //（同 delete_body 注：首帧 sizing pass 会在创建时清先置焦点）。
    let apply_id = ui.next_auto_id();
    // C6（F-9）：主钮「应用」补 accent 色。
    let apply = accent_button(ui, r_apply, t("应用"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(apply_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {
        // 取消整批（丢弃 pending；T20-H 流式已落位行按批次号回溯清行；
        // 勾选态随 cancel_pending_stream 一并清空）。
        super::cancel_pending_stream(st);
        st.dialog = Dialog::None;
    } else if apply || st.shell_enter {
        // 应用：按逐项勾选分流（勾 → force 入队，不勾 → skipped 计数；
        // 勾选态随 enqueue_pending take 清空）。
        super::enqueue_pending(st);
        st.dialog = Dialog::None;
    }
}

/// 全选/清空翻转（表头 checkbox → 整列，F-10 方案A；纯逻辑便于钉子）。
fn select_all(sel: &mut [bool], on: bool) {
    sel.iter_mut().for_each(|b| *b = on);
}

// ── 同步失配提示（C# ReportMismatch ContentDialog）────────────
fn sync_mismatch_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let msg = match &st.dialog {
        Dialog::SyncMismatch { msg } => msg.clone(),
        _ => return,
    };
    shell(
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
        ShellOpts::default(),
        |ui, content, st| sync_mismatch_body(ui, st, p, content, msg),
    );
}

/// 同步失配提示正文（shell 回调，见 shell 文档①②③）。
fn sync_mismatch_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    msg: String,
) {
    let mut cui = content_ui(ui, content);
    cui.add_space(CONTENT_PAD.1);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(RichText::new(&msg).size(14.0));
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

    // 右对齐按钮组：关闭 / 重新建立（C6 规则A：主钮「重新建立」恒最右，
    // 有意差异于 C# ContentDialog Primary 左 / Close 右排布）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_retry = btn_w(ui, t("重新建立"), &theme::font_body());
    let w_close = btn_w(ui, t("关闭"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_retry = Rect::from_min_size(Pos2::new(x0 - w_retry, y), egui::vec2(w_retry, BTN_H));
    let r_close = Rect::from_min_size(
        Pos2::new(x0 - w_retry - 8.0 - w_close, y),
        egui::vec2(w_close, BTN_H),
    );

    // C3②（F-9）：打开首帧初焦点 → 主钮「重新建立」。请求必须在创建之后：
    // egui 新 Area 首帧 = sizing pass（AreaState 未持久化 → ui invisible/disabled），
    // 创建时 create_widget 走 interested_in_focus=false → ignore_focus 会清掉
    // 同帧先置的焦点（含已聚焦控件，memory/mod.rs:946）；创建后请求则无后续
    // ignore_focus 干扰，end_pass 按「本帧新落焦」保留。
    let retry_id = ui.next_auto_id();
    let retry = accent_button(ui, r_retry, t("重新建立"), true, p, theme::font_body());
    let close = std_button(ui, r_close, t("关闭"), p.text_primary, p);
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(retry_id));
    }
    if retry || st.shell_enter {
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
    let (side, tab, name, remote_path) = match &st.dialog {
        Dialog::ExtEditChanged {
            side,
            tab,
            name,
            remote_path,
        } => (*side, *tab, name.clone(), remote_path.clone()),
        _ => return,
    };
    shell(
        ui,
        st,
        p,
        380.0,
        150.0,
        t("编辑"),
        false,
        0.0,
        false,
        None,
        ShellOpts::default(),
        |ui, content, st| ext_edit_changed_body(ui, st, p, content, side, tab, name, remote_path),
    );
}

/// 缓存变化询问正文（shell 回调，见 shell 文档①②③）。
#[allow(clippy::too_many_arguments)]
fn ext_edit_changed_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    side: usize,
    tab: u32,
    name: String,
    remote_path: String,
) {
    let mut cui = content_ui(ui, content);
    cui.add_space(CONTENT_PAD.1);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(
            RichText::new(tfmt(
                "\"{0}\" 的文件缓存已经改变，是否保存编辑结果？",
                &[&name],
            ))
            .size(14.0),
        );
    });

    // 右对齐按钮组：取消 / 保存编辑结果（C6 规则A：主钮恒最右，有意差异于
    // C# ContentDialog Primary 左 / Close 右排布）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_save = btn_w(ui, t("保存编辑结果"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_save = Rect::from_min_size(Pos2::new(x0 - w_save, y), egui::vec2(w_save, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_save - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    // C3②（F-9）：打开首帧初焦点 → 主钮「保存编辑结果」。请求必须在创建
    // 之后（同 delete_body 注：首帧 sizing pass 会在创建时清先置焦点）。
    let save_id = ui.next_auto_id();
    // C6（F-9）：主钮「保存编辑结果」补 accent 色。
    let save = accent_button(ui, r_save, t("保存编辑结果"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(save_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {
        // 暂不保存：会话保留（基准已在触发时更新），下次保存快照再变会再问。
        st.dialog = Dialog::None;
    } else if save || st.shell_enter {
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
    let (side, tab, remote_path, src) = match &st.dialog {
        Dialog::ExtEditConflict {
            side,
            tab,
            remote_path,
            src,
            ..
        } => (*side, *tab, remote_path.clone(), src.clone()),
        _ => return,
    };
    shell(
        ui,
        st,
        p,
        380.0,
        150.0,
        t("编辑"),
        false,
        0.0,
        false,
        None,
        ShellOpts::default(),
        |ui, content, st| ext_edit_conflict_body(ui, st, p, content, side, tab, remote_path, &src),
    );
}

/// 远端冲突确认正文（shell 回调，见 shell 文档①②③）。
#[allow(clippy::too_many_arguments)]
fn ext_edit_conflict_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    side: usize,
    tab: u32,
    remote_path: String,
    src: &ExtConflictSrc,
) {
    let mut cui = content_ui(ui, content);
    cui.add_space(CONTENT_PAD.1);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(RichText::new(t("远端文件已被修改，仍要覆盖吗？")).size(14.0));
    });

    // 右对齐按钮组：取消 / 覆盖（C6 规则A：主钮恒最右，有意差异于
    // C# ContentDialog Primary 左 / Close 右排布）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_over = btn_w(ui, t("覆盖"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_over = Rect::from_min_size(Pos2::new(x0 - w_over, y), egui::vec2(w_over, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_over - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    // C3②（F-9）：打开首帧初焦点 → 主钮「覆盖」。请求必须在创建之后
    //（同 delete_body 注：首帧 sizing pass 会在创建时清先置焦点）。
    let over_id = ui.next_auto_id();
    // C6（F-9）：主钮「覆盖」补 accent 色。
    let overwrite = accent_button(ui, r_over, t("覆盖"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(over_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {
        // 不覆盖：会话保留（基准已在触发时更新），下次保存快照再变会再问。
        st.dialog = Dialog::None;
    } else if overwrite || st.shell_enter {
        // 覆盖：按来源分派回传链路（CR-6⑤）——内置编辑器走 doc_upload
        //（请求号 = doc_id），外部编辑器走 ext_upload（请求号 = stat_seq）。
        // 按钮数量、文案、焦点逻辑两路一致。
        match src {
            ExtConflictSrc::Internal { doc_id, local_path } => {
                super::doc_upload(st, ui.ctx(), *doc_id, local_path, &remote_path);
            }
            ExtConflictSrc::External => super::ext_upload(st, side, tab, &remote_path),
        }
        st.dialog = Dialog::None;
    }
}

/// 写锁等待超时兜底（项6）：外部编辑器持写句柄 ≥10s 时的退出对话框。
/// 「立即重试探测」= 重置等待态、下轮 poll 立即再探；「取消本次回传」=
/// 放弃上传（基准更新为当前快照，本地副本保留）；Esc / 遮罩 / × = 继续
/// 等待。安全底线：绝不提供「强制上传」。
fn ext_edit_lock_wait_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (side, tab, name, remote_path) = match &st.dialog {
        Dialog::ExtEditLockWait {
            side,
            tab,
            name,
            remote_path,
        } => (*side, *tab, name.clone(), remote_path.clone()),
        _ => return,
    };
    shell(
        ui,
        st,
        p,
        420.0,
        170.0,
        t("编辑"),
        false,
        0.0,
        false,
        None,
        ShellOpts::default(),
        |ui, content, st| ext_edit_lock_wait_body(ui, st, p, content, side, tab, name, remote_path),
    );
}

/// 写锁超时兜底正文（shell 回调，见 shell 文档①②③）。
#[allow(clippy::too_many_arguments)]
fn ext_edit_lock_wait_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    side: usize,
    tab: u32,
    name: String,
    remote_path: String,
) {
    let mut cui = content_ui(ui, content);
    cui.add_space(CONTENT_PAD.1);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(
            RichText::new(tfmt(
                "文件 {0} 的写锁仍未释放，回传已暂缓。本地副本完好，不会上传半写文件。",
                &[&name],
            ))
            .size(14.0),
        );
    });

    // 右对齐按钮组：取消本次回传 / 立即重试探测（主钮恒最右）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_retry = btn_w(ui, t("立即重试探测"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消本次回传"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_retry = Rect::from_min_size(Pos2::new(x0 - w_retry, y), egui::vec2(w_retry, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_retry - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    // C3②（F-9）：打开首帧初焦点 → 主钮「立即重试探测」。
    let retry_id = ui.next_auto_id();
    let retry = accent_button(ui, r_retry, t("立即重试探测"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(retry_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消本次回传"), p.text_primary, p);
    // 弹窗自身关闭（Esc / 遮罩 / ×）→ 继续等待：本弹层无暂存态，shell 关闭
    // 出口直接 Dialog::None，等待态原样保留（下轮 poll 续计时）。
    if cancel {
        // 取消本次回传：放弃上传——基准更新为当前快照（下次保存快照再变
        // 会再问）、清锁等待态、保留本地副本。
        if let Some(t) = super::find_remote_tab_mut(st, side, tab)
            && let Some(e) = t
                .ext_edits
                .iter_mut()
                .find(|e| e.remote_path == remote_path)
        {
            e.baseline = super::ext_snapshot(&e.local);
            e.quiet_since = None;
            e.quiet_snap = None;
            e.lock_wait_since = None;
            e.lock_warned = false;
            e.lock_dialog_done = false; // 下次等锁超时可再弹（CR-10）
            t.lock_msg_on = false;
        }
        st.terminal.push((
            crate::local::now_hms(),
            tfmt("已取消回传：{0}（本地副本保留）", &[&name]),
        ));
        st.dialog = Dialog::None;
    } else if retry || st.shell_enter {
        // 立即重试探测：重置等待态，下轮 poll 立即再探（写锁多半已释放）。
        if let Some(t) = super::find_remote_tab_mut(st, side, tab)
            && let Some(e) = t
                .ext_edits
                .iter_mut()
                .find(|e| e.remote_path == remote_path)
        {
            e.quiet_since = None;
            e.quiet_snap = None;
            e.lock_wait_since = None;
            e.lock_warned = false;
            e.lock_dialog_done = false; // 下次等锁超时可再弹（CR-10）
            t.lock_msg_on = false;
        }
        st.dialog = Dialog::None;
    }
}

// ── 关闭标签页确认（G17，C# FolderContainer.OnTabCloseConfirming ContentDialog）──
/// 三键（Primary 后台继续 / Secondary 取消队列并关闭 / Close 不关了，
/// DefaultButton=Close）：确认判据为该 tab 归属的活动任务数（P3 per-tab，
/// C# LogScope 语义）；「取消队列并关闭」也只取消归属该 tab 的活动任务。
fn tab_close_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (side, tab, n) = match &st.dialog {
        Dialog::TabClose { side, tab, n } => (*side, *tab, *n),
        _ => return,
    };
    shell(
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
        ShellOpts::default(),
        |ui, content, st| tab_close_body(ui, st, p, content, side, tab, n),
    );
}

/// 关闭标签页确认正文（shell 回调，见 shell 文档①②③）。
fn tab_close_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    side: usize,
    tab: u32,
    n: usize,
) {
    let mut cui = content_ui(ui, content);
    cui.add_space(CONTENT_PAD.1);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(
            RichText::new(tfmt(
                "此标签页仍有 {0} 个进行中的传输任务，如何处理？",
                &[&n],
            ))
            .size(14.0),
        );
    });
    cui.add_space(12.0);

    // 右对齐按钮组（C# ContentDialog 布局 Primary 左 / Secondary 中 / Close 右，
    // DefaultButton=Close → 「不关了」最右）。
    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_stay = btn_w(ui, t("不关了"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消队列并关闭"), &theme::font_body());
    let w_bg = btn_w(ui, t("后台继续"), &theme::font_body());
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
    // C3②（F-9）：打开首帧初焦点 → 默认钮「不关了」（C# DefaultButton=Close
    // 最右）。请求必须在创建之后（同 delete_body 注：首帧 sizing pass 会在
    // 创建时清先置焦点）。
    let stay_id = ui.next_auto_id();
    // C6（F-9）：默认钮「不关了」补 accent 色（C# DefaultButton=Close 主按钮观感）。
    let stay = accent_button(ui, r_stay, t("不关了"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(stay_id));
    }
    if stay || st.shell_enter {
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
    // C3②（F-9）：覆盖层打开首帧 = 此前无 fp_ask 在途、本帧有（fp overlay 不经
    // Dialog 变体，dialog_kind 检测不适用；temp 旗标记「上帧在途态」）。
    let fp_open = st.fp_ask_test.is_some()
        || st
            .remotes
            .iter()
            .any(|r| r.sessions.iter().any(|s| s.fp_ask.is_some()));
    let fp_seen_id = Id::unique("fp-ask-seen");
    let fresh = fp_open
        && !ui
            .ctx()
            .data(|d| d.get_temp::<bool>(fp_seen_id).unwrap_or(false));
    ui.ctx().data_mut(|d| d.insert_temp(fp_seen_id, fp_open));

    // C10/F4（方案 A）：覆盖层升 Foreground 独立层，创建晚于 dialog-shell
    //（同层后者在上 → 稳压底下弹窗及其遮罩）。遮罩由 Sense::hover 改 click：
    // hover 放行点击穿透（点覆盖层遮罩会同时关掉底下弹窗）——改 click 吸收
    // 点击但**不关闭**（C# ContentDialog 遮罩无点击语义）。
    let (pri, cancel) = Area::new(Id::unique("fp-overlay"))
        .order(Order::Foreground)
        .fixed_pos(full.min)
        .constrain_to(full)
        .show(ui.ctx(), |ui| {
            // C3⑤（F-9）：覆盖层即模态层（语义同 dialog-shell），遮罩命中面
            // 去焦点化——Tab 陷阱不漏到底下弹窗。
            ui.ctx().memory_mut(|m| m.set_modal_layer(ui.layer_id()));
            ui.allocate_rect(full, Sense::click() - Sense::FOCUSABLE);
            ui.painter().rect_filled(full, 0.0, theme::mask());

            // 卡片：投影 + 一体底 + 描边 + 命中面（吸收穿透，同 shell 口径）。
            let card = Rect::from_center_size(full.center(), egui::vec2(w, h));
            theme::draw_shadow(ui, card, theme::CONTAINER_RADIUS, theme::ShadowKind::Dialog);
            ui.painter()
                .rect_filled(card, theme::CONTAINER_RADIUS, p.dialog_title_bar);
            ui.painter().rect_stroke(
                card,
                theme::CONTAINER_RADIUS,
                theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.card_stroke),
                egui::StrokeKind::Inside,
            );
            ui.allocate_rect(card, Sense::click() - Sense::FOCUSABLE);
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
                theme::font_title(true),
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
            bui.label(RichText::new(&body).size(14.0));

            // 右下按钮组：取消左、主钮右（间距 8，同传输确认弹窗排布）。
            let primary = if first {
                t("信任并继续")
            } else {
                t("更新并信任")
            };
            let w_pri = btn_w(ui, primary, &theme::font_body());
            let w_cancel = btn_w(ui, t("取消"), &theme::font_body());
            let x0 = inner.right() - CONTENT_PAD.0;
            let r_pri = Rect::from_min_size(Pos2::new(x0 - w_pri, btn_y), egui::vec2(w_pri, BTN_H));
            let r_cancel = Rect::from_min_size(
                Pos2::new(x0 - w_pri - 8.0 - w_cancel, btn_y),
                egui::vec2(w_cancel, BTN_H),
            );
            // C3②（F-9）：打开首帧初焦点随默认钮——首次连接默认信任 → 主钮「信任并
            // 继续」；指纹变更默认取消（防中间人，C# S4.2）→ 「取消」钮（焦点在可点
            // 控件时 Enter 由控件自身触发，与 decision 的 Enter 默认分支同向，主钮
            // 聚焦会经 FAKE_PRIMARY_CLICKED 越过防中间人默认）。请求必须在创建之后
            //（同 delete_body 注：首帧 sizing pass 会在创建时清先置焦点）。
            let pri_id = ui.next_auto_id();
            // C6（F-9）：主钮补 accent 色。
            let pri = accent_button(ui, r_pri, primary, true, p, theme::font_body());
            let cancel_id = ui.next_auto_id();
            let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
            if fresh {
                let target = if first { pri_id } else { cancel_id };
                ui.ctx().memory_mut(|m| m.request_focus(target));
            }
            (pri, cancel)
        })
        .inner;

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

// ============================================================================
// 校验码弹窗本地计算测试（M-11）：hash_local algo=2 走 src/sm3.rs，
// 钉 GB/T 32905-2016 附录 A 向量（此前 SM3 本地校验码弹窗明确报不支持）。
// ============================================================================

#[cfg(test)]
mod hash_tests {
    use super::hash_local;

    #[test]
    fn hash_local_sm3_official_vector() {
        let mut p = std::env::temp_dir();
        p.push("ifree-sm3-dialog-case.tmp");
        std::fs::write(&p, b"abc").unwrap();
        let h = hash_local(p.to_str().unwrap(), 2).expect("SM3 本地校验码必须可算");
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            h,
            "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
        );
        // 非 UTF-8 文件名不存在 → Err（弹窗侧显式报错语义不变）。
        assert!(hash_local(r"Z:\definitely\not\exists", 2).is_err());
    }
}

// ============================================================================
// 批量队列菜单六项显隐规则测试（T13 第 1 轮）—— 六态钉住（C# .cs:148-155 口径）
// 位序：[暂停, 继续传输, 停止, 清除已完成, 清除所有, 重置并重新请求所有]。
// ============================================================================

#[cfg(test)]
mod batch_menu_tests {
    use super::batch_menu_rules;

    #[test]
    fn batch_menu_rules_empty_queue_all_hidden() {
        // 全空：total==0 → 六位全 false（调用侧 items 空，菜单层置 None 不弹）。
        assert_eq!(batch_menu_rules(false, 0, 0, 0, 0, 0), [false; 6]);
    }

    #[test]
    fn batch_menu_rules_waiting_only_shows_stop_and_clear_all() {
        // 边角钉住①：只有等待（无传输无完结）→ 菜单非空：停止 + 清除所有；
        // 无暂停（无传输中）、无重置。
        assert_eq!(
            batch_menu_rules(false, 3, 0, 0, 0, 0),
            [false, false, true, false, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_waiting_with_finished_no_reset() {
        // 边角钉住②：等待 + 完结（failed/cancelled）→ 有等待未静止，
        // 不得出「重置并重新请求所有」。
        assert_eq!(
            batch_menu_rules(false, 2, 0, 0, 1, 1),
            [false, false, true, true, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_transferring_shows_pause_and_stop() {
        // 传输中：暂停 + 停止；无完结 → 无清除类。
        assert_eq!(
            batch_menu_rules(false, 1, 2, 0, 0, 0),
            [true, false, true, false, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_paused_shows_resume() {
        // 暂停中：出「继续传输」、不出「暂停」；停止仍显（等待/传输未清）。
        assert_eq!(
            batch_menu_rules(true, 1, 2, 0, 0, 0),
            [false, true, true, false, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_all_finished_shows_reset() {
        // 全部完结静止：清除已完成 + 清除所有 + 重置并重新请求所有；
        // 无暂停/继续/停止。
        assert_eq!(
            batch_menu_rules(false, 0, 0, 1, 1, 1),
            [false, false, false, true, true, true]
        );
    }
}

// ============================================================================
// 按钮宽度同源测试（F-11 拍板）：btn_w 预算 = 文本量宽 + 2×WinUI ButtonPadding
// 11,5（theme::apply_spacing）。settings 检查更新钮写死 74 宽英文态溢出不再
// 回归：padding 或预算任一侧脱钩即炸。
// ============================================================================

#[cfg(test)]
mod btn_w_tests {
    use super::btn_w;
    use crate::theme::{self, MEASURE_ONLY};

    #[test]
    fn btn_w_budget_matches_button_padding() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let pad = ui.style().spacing.button_padding;
                assert_eq!(
                    pad,
                    egui::vec2(11.0, 5.0),
                    "theme::apply_spacing 的 WinUI ButtonPadding 11,5"
                );
                // 英文态曾因写死 74 宽溢出的标签（settings 检查更新钮）。
                let label = "Check for updates";
                let w = btn_w(ui, label, &theme::font_body());
                let measured = ui
                    .painter()
                    .layout_no_wrap(label.to_owned(), theme::font_body(), MEASURE_ONLY)
                    .size()
                    .x;
                assert_eq!(
                    w - measured,
                    2.0 * pad.x,
                    "btn_w 预算必须 = 量宽 + 2×button_padding.x（同源，禁写死）"
                );
            });
        });
        // 无渲染器消费字体纹理增量（epaint Drop debug_assert 会恐慌）——显式清空。
        out.textures_delta.clear();
    }
}

#[cfg(test)]
mod select_all_tests {
    use super::select_all;

    /// 钉子（F-10 方案A）：表头全选/清空翻转——纯逻辑，整列覆写。
    #[test]
    fn select_all_flips_every_row() {
        let mut sel = vec![true, false, true];
        select_all(&mut sel, false);
        assert_eq!(sel, vec![false, false, false], "清空整列");
        select_all(&mut sel, true);
        assert_eq!(sel, vec![true, true, true], "全选整列");
        select_all(&mut sel, true);
        assert_eq!(sel, vec![true, true, true], "重复全选幂等");
    }
}

// ============================================================================
// 统一外壳 headless 测试（f9-6/F-9 判据「钉子」）：五枚钉子钉住 shell 共性——
// ①C3②/C3③ F20 打开首帧初焦点落主钮 + Enter 经 FAKE_PRIMARY_CLICKED 触发
//   （焦点控件豁免全局 shell_enter，防双触发）；
// ②C3⑤ modal 焦点层限定：Tab 只在弹窗层内循环不逃逸（帧 1/2 对照组证明
//   焦点机制本身可用，排除「焦点整体失灵」假阳）；
// ③C1/F19 遮罩点击关闭性由调用方 ShellOpts.mask_close 声明；
// ④C3①/F5 combo 展开态 Esc 让路：shell 入口旗标短路不消费、card_combo 真
//   消费收起（弹窗存活，temp 与旗标双双归位）；
// ⑤C7/F3 卡片尺寸与视口各夹一次，内容容器不溢出、关闭钮恒在窗内。
// ============================================================================

#[cfg(test)]
mod dialog_shell_tests {
    use std::sync::Mutex;

    use super::*;

    /// COMBO_OPEN 是进程级静态（所有测试共享一份）——串行化防并发互踩
    /// （其余测试每帧 shell 入口 swap 会清走 combo 测试预置的旗标）。
    /// 每个测试都自建 egui::Context，互不串扰，只有旗标需要这把锁。
    static TEST_SER: Mutex<()> = Mutex::new(());

    /// 800×600 视口空输入（RawInput::default 的 screen_rect=None 首帧按
    /// 0×0 处理，几何断言必须显式给视口）。
    fn plain_input() -> egui::RawInput {
        let mut i = egui::RawInput::default();
        i.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        i
    }

    /// 单键按下事件帧。
    fn key_input(key: egui::Key) -> egui::RawInput {
        let mut i = plain_input();
        i.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        i
    }

    /// 点击两帧：press（带 PointerMoved 定位）+ release。egui 自家容器测试
    /// 同款最小帧序（containers/window.rs 点击样板），clicked 在 release 帧
    /// 成立。
    fn click_frames(pos: egui::Pos2) -> [egui::RawInput; 2] {
        let mut press = plain_input();
        press.events.push(egui::Event::PointerMoved(pos));
        press.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let mut release = plain_input();
        release.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        [press, release]
    }

    /// 真实帧序（主容器 + 弹层一帧）：先放一枚可聚焦按钮当「主界面锚点」
    /// （modal 逃逸断言的靶子），再走 draw 全量弹层入口。
    fn frame(
        ctx: &egui::Context,
        input: egui::RawInput,
        st: &mut State,
        main_btn: &mut Option<egui::Id>,
        p: &theme::Palette,
    ) {
        let mut out = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                *main_btn = Some(ui.add(egui::Button::new("main-ui-widget")).id);
                draw(ui, st, p);
            });
        });
        // 无渲染器消费字体纹理增量（epaint Drop debug_assert 会恐慌）——显式清空。
        out.textures_delta.clear();
    }

    /// 直调 shell 一帧（绕过 draw：sync_mismatch 调用点固定 ShellOpts，
    /// 遮罩可关性用例需要自选）。参数镜像 sync_mismatch_dialog 的
    /// 400×172 / 「同步浏览」调用点。返回 true = 存活（正文空实现）。
    fn shell_frame(
        ctx: &egui::Context,
        input: egui::RawInput,
        st: &mut State,
        opts: ShellOpts,
        p: &theme::Palette,
    ) -> bool {
        let mut r = false;
        let mut out = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                r = shell(
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
                    opts,
                    |_, _, _| {},
                );
            });
        });
        out.textures_delta.clear();
        r
    }

    // ── 码值 ScrollArea 回归钉子（egui 0.37 min_scrolled_size=splat(64) 坑）──
    // 模式复现：视口被强扩到 64 → 内容(56) < 64 恒不出条。修复 = 显式
    // min_scrolled_height(0)。视口必须 ≤ 34（=50px 框内收 8×2）且内容超高。
    #[test]
    fn chk_value_scrollarea_viewport_fits_and_overflows() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        let mut res = None;
        for _ in 0..3 {
            let mut out = ctx.run_ui(plain_input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let value_r = egui::Rect::from_min_size(
                        egui::Pos2::new(50.0, 100.0),
                        egui::vec2(320.0, 50.0),
                    );
                    let mut val_ui = ui.new_child(
                        egui::UiBuilder::new().max_rect(value_r.shrink2(egui::vec2(8.0, 8.0))),
                    );
                    let text = "0123456789abcdef".repeat(8);
                    let o = egui::ScrollArea::vertical()
                        .id_salt("chk-value")
                        .auto_shrink(false)
                        .min_scrolled_height(0.0)
                        .show(&mut val_ui, |s| {
                            let galley = s.painter().layout(
                                text.clone(),
                                theme::font_mono_small(),
                                egui::Color32::WHITE,
                                s.max_rect().width(),
                            );
                            let (rect, _resp) =
                                s.allocate_exact_size(galley.size(), egui::Sense::hover());
                            s.painter()
                                .galley(rect.left_top(), galley, egui::Color32::WHITE);
                        });
                    res = Some((o.content_size, o.inner_rect.size()));
                });
            });
            out.textures_delta.clear();
        }
        let (content, viewport) = res.unwrap();
        assert!(
            viewport.y <= 34.0 + f32::EPSILON,
            "egui 0.37 min_scrolled_size 默认 64 会强扩视口：viewport={viewport:?}"
        );
        assert!(
            content.y > viewport.y,
            "SHA-512 折行内容必须高于视口才出滚动条 content={content:?} viewport={viewport:?}"
        );
    }

    // ── 码值 galley 裁剪钉子（真实 checksum 弹窗路径）────────────────────
    // 裁剪矩形必须收在 50px 码值框内：此前 min_scrolled_size 64 强扩裁剪框
    // 向下溢出 30px，文字画进底部按钮区（用户走查「关闭钮底边被隐藏」观感）。
    #[test]
    fn chk_sha512_galley_clipped_inside_value_box() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        let mut st = State::default();
        st.dialog = Dialog::Checksum {
            name: "big.bin".into(),
            path: "C:/tmp/big.bin".into(),
            side: None,
            algo: 1,
            copied: None,
        };
        st.chk_local = Some(Ok("0123456789abcdef".repeat(8)));
        let p = theme::palette(&ctx);
        let mut main_btn = None;
        let mut shapes = vec![];
        for _ in 0..3 {
            let mut out = ctx.run_ui(plain_input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    main_btn = Some(ui.add(egui::Button::new("main-ui-widget")).id);
                    draw(ui, &mut st, &p);
                });
            });
            shapes = out.shapes;
            out.textures_delta.clear();
        }
        let mut found = 0;
        for cs in &shapes {
            if let egui::Shape::Text(g) = &cs.shape
                && g.galley.text().contains("0123456789abcdef")
            {
                found += 1;
                assert!(
                    cs.clip_rect.height() <= 34.0 + f32::EPSILON,
                    "码值 galley 裁剪框必须 = 视口 34（min_scrolled_size=64 坑）：clip={:?}",
                    cs.clip_rect
                );
            }
        }
        assert!(found > 0, "必须找到 hash 文本 galley");
    }

    /// 夹具：独立 Context（dark 主题）+ 默认 State 挂 SyncMismatch 弹窗
    /// （重试路径 evaluate 纯读，sync_base=None → Err 终端日志，headless 安全）。
    fn sync_fixture() -> (egui::Context, State) {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        let mut st = State::default();
        st.dialog = Dialog::SyncMismatch {
            msg: "两侧目录不一致".into(),
        };
        (ctx, st)
    }

    /// 钉子 1（C3②/C3③ F20/F-9）：打开首帧初焦点落弹窗主钮；次帧焦点控件
    /// 收到 Enter → egui 合成主钮点击（shell_enter 豁免）→ 弹窗关闭且无
    /// 双触发。
    #[test]
    fn enter_activates_fresh_primary_button() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        let p = theme::palette(&ctx);
        let mut main_btn = None;

        // 帧 1：fresh → request_focus 主钮「重新建立」。
        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        assert!(
            ctx.memory(|m| m.focused()).is_some(),
            "C3② 打开首帧初焦点必须落位（主钮「重新建立」）"
        );

        // 帧 2：焦点控件 + Enter → FAKE_PRIMARY_CLICKED 直接触发主钮
        // （shell_enter 豁免：focused 非空且非文本框，全局旗标不响应）。
        frame(
            &ctx,
            key_input(egui::Key::Enter),
            &mut st,
            &mut main_btn,
            &p,
        );
        assert!(
            matches!(st.dialog, Dialog::None),
            "Enter = 主钮触发，弹窗关闭"
        );
        assert!(
            !st.shell_enter,
            "焦点控件自触发即豁免全局 shell_enter（防 FAKE_PRIMARY_CLICKED 双触发）"
        );
    }

    /// 钉子 2（C3⑤ F-9）：modal 焦点层限定——弹窗打开后 Tab 只在弹窗层内
    /// 循环，永不逃逸到遮罩下的主界面控件。
    #[test]
    fn tab_focus_cannot_escape_modal_dialog() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        // 对照期先不开弹窗。
        st.dialog = Dialog::None;
        let p = theme::palette(&ctx);
        let mut main_btn = None;

        // 帧 1：无弹窗、无输入 → 无焦点。
        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        let main = main_btn.expect("主界面锚点按钮 id");
        assert!(
            ctx.memory(|m| m.focused()).is_none(),
            "无弹窗无输入 = 无焦点"
        );

        // 帧 2（对照组）：Tab → 主界面按钮获焦（egui：无焦点 + Tab → 首个
        /// 注册焦点兴趣的控件，memory/mod.rs interested_in_focus）。
        frame(&ctx, key_input(egui::Key::Tab), &mut st, &mut main_btn, &p);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(main),
            "对照组：无弹窗时 Tab 应聚焦主界面按钮（机制可用）"
        );

        // 帧 3：打开弹窗 → 初焦点落弹窗内主钮（非主界面按钮）。
        st.dialog = Dialog::SyncMismatch {
            msg: "两侧目录不一致".into(),
        };
        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        let f3 = ctx.memory(|m| m.focused());
        assert!(
            f3.is_some() && f3 != Some(main),
            "弹窗打开首帧焦点必须落弹窗内（C3②），实测 {f3:?}"
        );

        // 帧 4/5：modal 层生效后 Tab 连击 → 焦点只在弹窗层内循环
        // （遮罩下主界面控件不再注册焦点兴趣）。
        frame(&ctx, key_input(egui::Key::Tab), &mut st, &mut main_btn, &p);
        assert_ne!(
            ctx.memory(|m| m.focused()),
            Some(main),
            "C3⑤ Tab 不逃逸到遮罩下主界面（帧 4）"
        );
        frame(&ctx, key_input(egui::Key::Tab), &mut st, &mut main_btn, &p);
        assert_ne!(
            ctx.memory(|m| m.focused()),
            Some(main),
            "C3⑤ Tab 不逃逸到遮罩下主界面（帧 5）"
        );
    }

    /// 钉子 3（C1/F19 F-9）：遮罩点击关闭性由调用方 ShellOpts.mask_close 声明
    /// ——false = 点击被吸收不关闭（防误点丢未保存态）；true = 关闭并置
    /// Dialog::None。卡片 400×172 居中于 800×600 → (10,10) 在遮罩上、卡片外。
    /// warm-up 两帧：potential_click_id 依赖 press 帧 hit-test 命中上一帧
    /// rects，而 egui 新 Area 首帧必为 sizing pass（AreaState 未持久化 →
    /// widget visible=false，interaction 候选跳过不可见控件）——第 1 帧落
    /// AreaState，第 2 帧才有可命中的正常 rects。
    #[test]
    fn mask_click_respects_mask_close_opt() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let mask_pos = egui::pos2(10.0, 10.0);

        // 场景 A：mask_close=false → 遮罩点击被吸收，弹窗存活。
        let (ctx_a, mut st_a) = sync_fixture();
        let p = theme::palette(&ctx_a);
        let [press, release] = click_frames(mask_pos);
        let opts = ShellOpts {
            mask_close: false,
            ..Default::default()
        };
        assert!(
            shell_frame(&ctx_a, plain_input(), &mut st_a, opts, &p),
            "warm-up sizing 帧存活"
        );
        assert!(
            shell_frame(&ctx_a, plain_input(), &mut st_a, opts, &p),
            "warm-up 正常帧存活"
        );
        assert!(
            shell_frame(&ctx_a, press, &mut st_a, opts, &p),
            "按下帧不关闭"
        );
        assert!(
            shell_frame(&ctx_a, release, &mut st_a, opts, &p),
            "F19：mask_close=false 遮罩点击被吸收不关闭"
        );
        assert!(
            matches!(st_a.dialog, Dialog::SyncMismatch { .. }),
            "遮罩点击后弹窗存活"
        );

        // 场景 B：mask_close=true（默认）→ 遮罩点击关闭。
        let (ctx_b, mut st_b) = sync_fixture();
        let p = theme::palette(&ctx_b);
        let [press, release] = click_frames(mask_pos);
        let opts = ShellOpts::default();
        assert!(
            shell_frame(&ctx_b, plain_input(), &mut st_b, opts, &p),
            "warm-up sizing 帧存活"
        );
        assert!(
            shell_frame(&ctx_b, plain_input(), &mut st_b, opts, &p),
            "warm-up 正常帧存活"
        );
        let _ = shell_frame(&ctx_b, press, &mut st_b, opts, &p);
        assert!(
            !shell_frame(&ctx_b, release, &mut st_b, opts, &p),
            "mask_close=true 遮罩点击关闭"
        );
        assert!(
            matches!(st_b.dialog, Dialog::None),
            "关闭路径置 Dialog::None"
        );
    }

    /// 钉子 4（C3①/F5 F-9）：combo 展开态 Esc 让路——shell 入口旗标短路
    /// 不消费 Esc（弹窗存活），card_combo 真消费收起组合；temp 与旗标双双
    /// 归位（收起终态不上报旗标，store-only-true）。
    #[test]
    fn combo_open_esc_consumed_dialog_survives() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        let p = theme::palette(&ctx);
        COMBO_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
        // 预置展开态（模拟上一帧终态组合仍展开）：temp open=true +
        // 进程级旗标=true（shell 入口 swap 读走的就是它）。
        let open_id = Id::unique("test-combo").with("open");
        ctx.data_mut(|d| d.insert_temp(open_id, true));
        COMBO_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);

        let mut val = 0usize;
        let mut out = ctx.run_ui(key_input(egui::Key::Escape), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // shell 入口：旗标 swap=true → esc 短路让路 → 返回 true（存活）；
                // 正文（card_combo）经回调画进弹窗层。
                let alive = shell(
                    ui,
                    &mut st,
                    &p,
                    400.0,
                    172.0,
                    t("同步浏览"),
                    false,
                    0.0,
                    false,
                    None,
                    ShellOpts::default(),
                    |ui, content, _st| {
                        let mut cui = content_ui(ui, content);
                        // card_combo 内部：open=temp true → consume_key(Escape) 真消费
                        // → insert_temp(false) 收起；终态收起不上报旗标。
                        card_combo(
                            &mut cui,
                            content,
                            "test-combo",
                            &mut val,
                            &["a", "b"],
                            150.0,
                            13.0,
                            true,
                        );
                    },
                );
                assert!(alive, "F5：combo 展开态 Esc 必须让路给组合，弹窗不得关闭");
            });
        });
        out.textures_delta.clear();

        assert!(
            matches!(st.dialog, Dialog::SyncMismatch { .. }),
            "Esc 让路：弹窗存活（未被一击关掉）"
        );
        assert_eq!(
            ctx.data(|d| d.get_temp::<bool>(open_id)),
            Some(false),
            "card_combo 真消费 Esc：组合收起（temp 归位）"
        );
        assert_eq!(
            COMBO_OPEN.load(std::sync::atomic::Ordering::Relaxed),
            false,
            "收起终态不上报旗标（store-only-true）"
        );
    }

    /// 钉子 5（C7/F3 F-9）：卡片尺寸与视口各夹一次——920×640 面板在 400×300
    /// 视口 clamp 为满视口；内容容器按夹后卡片派生（宽 388 = 400−2×内缩1−
    /// 2×inset5）、不溢出视口，关闭钮中心恒在窗内（MIN_SIZE 900×600 曾出屏）。
    /// CentralPanel 用 Frame::NONE（默认 margin 8 会缩水 full，几何断言失真）。
    #[test]
    fn shell_clamps_panel_and_close_button_stays_inside_window() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        let p = theme::palette(&ctx);
        let mut st = State::default();
        let vp = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(vp);

        // 镜像 host_panel 调用点：920×640 / large / row_gap 4 / 无容器底。
        let mut content = None;
        let mut out = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let alive = shell(
                        ui,
                        &mut st,
                        &p,
                        920.0,
                        640.0,
                        t("主机设置"),
                        true,
                        4.0,
                        false,
                        None,
                        ShellOpts::default(),
                        |_ui, c, _st| content = Some(c),
                    );
                    assert!(alive, "面板不因 clamp 关闭");
                });
        });
        out.textures_delta.clear();
        let content = content.expect("存活帧正文必绘");

        // clamp 生效：内容容器宽 = 卡片宽 400 − 2×内缩 1 − 2×inset 5 = 388。
        assert!(
            (content.width() - 388.0).abs() < 0.01,
            "C7：920 宽夹到 400 视口，内容容器宽应 388，实测 {}",
            content.width()
        );
        assert!(
            vp.contains_rect(content),
            "内容容器不得溢出视口：内容 {:?} / 视口 {:?}",
            content.min,
            content.max
        );

        // 关闭钮中心复算（与 shell 内几何同源）：inner_right = content.right +
        // CONTENT_INSET；cx = inner_right − TITLE_PAD.2 − CLOSE_SIZE/2；
        // cy = 标题行中线 = content.top − row_gap − TITLE_BAR_H/2。
        let inner_right = content.right() + CONTENT_INSET;
        let close_c = egui::pos2(
            inner_right - TITLE_PAD.2 - CLOSE_SIZE / 2.0,
            content.top() - 4.0 - TITLE_BAR_H / 2.0,
        );
        let close_r = egui::Rect::from_center_size(close_c, egui::vec2(CLOSE_SIZE, CLOSE_SIZE));
        assert!(
            vp.contains_rect(close_r),
            "C7：关闭钮恒在窗内，中心 {:?}",
            close_c
        );
    }

    /// 钉子 6（CR-5）：card_combo 禁用态 = 实际禁用——warm-up 两帧后合成点击
    /// 控件中心：val 不变、无展开态（temp 无 true）、COMBO_OPEN 旗标不置位。
    /// warm-up 两帧：CentralPanel 首帧 sizing pass 无可命中 rects（同钉子 3）。
    #[test]
    fn card_combo_disabled_click_is_inert() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        COMBO_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
        let r = egui::Rect::from_min_size(egui::Pos2::new(100.0, 100.0), egui::vec2(150.0, 32.0));
        let open_id = Id::unique("cr5-disabled").with("open");
        let mut val = 0usize;
        let run = |input: egui::RawInput, val: &mut usize| {
            let mut out = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    card_combo(ui, r, "cr5-disabled", val, &["a", "b"], 150.0, 13.0, false);
                });
            });
            out.textures_delta.clear();
        };
        run(plain_input(), &mut val);
        run(plain_input(), &mut val);
        let [press, release] = click_frames(r.center());
        run(press, &mut val);
        run(release, &mut val);
        assert_eq!(val, 0, "CR-5：禁用下拉合成点击不得改值");
        assert_eq!(
            ctx.data(|d| d.get_temp::<bool>(open_id)),
            None,
            "CR-5：禁用下拉不得展开（无展开态 temp）"
        );
        assert_eq!(
            COMBO_OPEN.load(std::sync::atomic::Ordering::Relaxed),
            false,
            "CR-5：禁用下拉不置位 COMBO_OPEN（Esc 让路无对象）"
        );
    }

    /// 钉子 7（CR-5 对照）：enabled=true 同点击序列正常工作——点开浮层、
    /// 条目点击写值、选后收起（证明钉子 6 的「无动静」是 enabled 门控而非
    /// 测试脚手架失灵）。浮层首帧 sizing pass 后空 pass 一帧再点条目。
    #[test]
    fn card_combo_enabled_click_selects() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        COMBO_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
        let r = egui::Rect::from_min_size(egui::Pos2::new(100.0, 100.0), egui::vec2(150.0, 32.0));
        let open_id = Id::unique("cr5-enabled").with("open");
        let mut val = 0usize;
        let run = |input: egui::RawInput, val: &mut usize| {
            let mut out = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    card_combo(ui, r, "cr5-enabled", val, &["a", "b"], 150.0, 13.0, true);
                });
            });
            out.textures_delta.clear();
        };
        // warm-up 两帧 + 点开浮层（release 帧置 open，值不变）。
        run(plain_input(), &mut val);
        run(plain_input(), &mut val);
        let [press, release] = click_frames(r.center());
        run(press, &mut val);
        run(release, &mut val);
        // 浮层前两帧不可命中：首帧 sizing pass 无 rects，且按下帧命中测试取
        // 上一帧 rects → 再空 pass 一帧落正常 rects，第三帧才能起按。
        run(plain_input(), &mut val);
        run(plain_input(), &mut val);
        // 条目 1 中心（条目高 40：浮层顶 = 控件底 + 4，首条目顶 1px 内缩）。
        let item = egui::pos2(r.center().x, r.bottom() + 4.0 + 1.0 + 40.0 + 20.0);
        let [press2, release2] = click_frames(item);
        run(press2, &mut val);
        run(release2, &mut val);
        assert_eq!(val, 1, "CR-5 对照：enabled 下拉条目点击写值");
        assert_eq!(
            ctx.data(|d| d.get_temp::<bool>(open_id)),
            Some(false),
            "CR-5 对照：选值后组合收起"
        );
        // 进程级旗标复位（TEST_SER 串行锁内，防泄漏进后续用例）。
        COMBO_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    // ── CR-6⑤ 冲突弹窗来源分派钉子 ─────────────────────────────

    /// 会话池夹具（离线 SSH 句柄：任何 write_file 请求必回 FileWritten，
    /// 用 poll() 断言「发/没发」）。
    fn cr6_sess(host: &str) -> crate::ui::SideSession {
        crate::ui::SideSession {
            sid: 1,
            key: crate::ui::SessionKey {
                host: host.into(),
                port: 22,
                user: "u".into(),
            },
            ssh: crate::ssh::Ssh::spawn(),
            host: None,
            profile: None,
            state: crate::ui::RemoteState::Off,
            root: None,
            fp_ask: None,
            refs: 1,
        }
    }

    /// 等 worker 回执（无连接 → write_file 必回 FileWritten::Err）。
    fn cr6_has_file_written(ssh: &crate::ssh::Ssh) -> bool {
        for _ in 0..40 {
            if ssh
                .poll()
                .iter()
                .any(|e| matches!(e, crate::ssh::Ev::FileWritten { .. }))
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// 钉子（CR-6 冲突「取消」）：取消路径（Esc 关闭与取消按钮共用关闭出口，
    /// CloseAction::None = 直接关）不写远端——零 FileWritten 回执、doc 在途态
    /// 原样保留（副本在、下次保存可重试）、STAT 槽不挂。挂上匹配会话证明
    /// 「没派发」而非「链路不可达」。
    #[test]
    fn conflict_internal_cancel_keeps_remote_untouched() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        let p = theme::palette(&ctx);
        let copy = std::env::temp_dir().join("ifreesftp-cr6-cancel-copy.txt");
        std::fs::write(&copy, b"payload").unwrap();
        st.editors
            .open_remote("h1", &copy, "/r.txt".into(), Some(1000), true);
        st.remotes[0].sessions.push(cr6_sess("h1"));
        {
            let mut d = st.editors.docs[0].lock().unwrap();
            d.pending_upload = true;
            d.dirty = true; // save 入队后的在途态
        }
        st.dialog = Dialog::ExtEditConflict {
            side: 0,
            tab: 0,
            name: "r.txt".into(),
            remote_path: "/r.txt".into(),
            src: ExtConflictSrc::Internal {
                doc_id: 1,
                local_path: copy.clone(),
            },
        };
        let mut main_btn = None;
        // 帧 1：fresh → 焦点落主钮「覆盖」（打开首帧语义照常）。
        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        // 帧 2：Esc → 关闭路径（与取消按钮同出口）。
        frame(
            &ctx,
            key_input(egui::Key::Escape),
            &mut st,
            &mut main_btn,
            &p,
        );
        assert!(matches!(st.dialog, Dialog::None), "取消关闭弹窗");
        assert!(st.doc_stat.is_none(), "取消不挂任何在途槽");
        assert!(copy.exists(), "副本保留");
        let d = st.editors.docs[0].lock().unwrap();
        assert!(d.dirty && d.pending_upload, "取消不动在途态（可重试）");
        drop(d);
        assert!(
            !cr6_has_file_written(&st.remotes[0].sessions[0].ssh),
            "取消后 write_file 调用数为 0（远端内容不变）"
        );
    }

    /// 钉子（CR-6⑤ 外部回归）：src=External 时「覆盖」仍走 ext_upload——
    /// 请求号 = stat_seq 发号并占用挂靠 tab 的 ext_write 槽（内置路才会用
    /// doc_id 当请求号），写请求确实发出（worker 回 FileWritten）。
    #[test]
    fn conflict_external_overwrite_routes_ext_upload() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        let p = theme::palette(&ctx);
        st.remotes[0].sessions.push(cr6_sess("h1"));
        // 挂靠 tab（sid=1）+ 外部编辑会话（副本真实落盘）。
        let copy = std::env::temp_dir().join("ifreesftp-cr6-ext-copy.txt");
        std::fs::write(&copy, b"payload").unwrap();
        st.remotes[0]
            .tabs
            .push(super::super::placeholder_remote_tab(9));
        st.remotes[0].tabs[0].sid = 1;
        st.remotes[0].tabs[0].ext_edits.push(crate::ui::ExtEdit {
            host_tag: "[h1]".into(),
            remote_path: "/r.txt".into(),
            name: "r.txt".into(),
            local: copy.clone(),
            base_mtime: Some(1000),
            baseline: (7, 1000),
            quiet_since: None,
            quiet_snap: None,
            lock_wait_since: None,
            lock_warned: false,
            lock_dialog_done: false,
        });
        st.dialog = Dialog::ExtEditConflict {
            side: 0,
            tab: 9,
            name: "r.txt".into(),
            remote_path: "/r.txt".into(),
            src: ExtConflictSrc::External,
        };
        let mut main_btn = None;
        // 帧 1：fresh → 焦点落主钮「覆盖」。
        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        // 帧 2：Enter → egui 合成主钮点击（FAKE_PRIMARY_CLICKED）→ 分派。
        frame(
            &ctx,
            key_input(egui::Key::Enter),
            &mut st,
            &mut main_btn,
            &p,
        );
        assert!(matches!(st.dialog, Dialog::None), "覆盖后弹窗关闭");
        assert_eq!(st.stat_seq, 1, "请求号 = stat_seq 发号（ext_upload 路）");
        assert_eq!(
            st.remotes[0].tabs[0].ext_write,
            Some(1),
            "ext_write 槽占用（Ev::FileWritten 外部分支按它认领）"
        );
        assert!(
            cr6_has_file_written(&st.remotes[0].sessions[0].ssh),
            "写请求确实发出（外部回归：仍走 ext_upload）"
        );
    }
}

// ============================================================================
// CR-7 单批删除发号钉子：一个 tag 装 N 项时，del_pending 必须按「批」计数，
// 否则一条 Ev::Deleted 只减 1 → 「正在删除…」永不消失。
// ============================================================================
#[cfg(test)]
mod cr7_delete_batch_tests {
    use super::*;

    fn state_with_tab() -> State {
        let mut st = State::default();
        let tab = &mut st.remotes[1].tabs[0];
        tab.pane.remote_path = Some("/r".to_owned());
        tab.pane.loading = false;
        tab.del_pending = 0;
        st
    }

    #[test]
    fn delete_batch_uses_one_tag_and_one_pending() {
        let mut st = state_with_tab();
        let tab_id = st.remotes[1].tabs[0].id;
        let paths: Vec<(String, bool)> =
            (0..200).map(|i| (format!("/r/f{i}.bin"), false)).collect();
        let (tag, batch) = begin_delete_batch(&mut st, 1, tab_id, paths).expect("非空批必须发号");
        assert_eq!(batch.len(), 200, "整批一次发出，不再逐项发 N 条 Cmd");
        assert_eq!(st.inflight.len(), 1, "200 项整批只有一个在途 tag");
        assert_eq!(st.inflight.get(&tag), Some(&(1usize, tab_id)));
        assert_eq!(
            st.remotes[1].tabs[0].del_pending, 1,
            "del_pending 按批 +1，不是按项 +200"
        );
        assert!(st.remotes[1].tabs[0].pane.loading, "busy 链保活到回执");
    }

    #[test]
    fn empty_delete_batch_does_not_leave_pending() {
        let mut st = state_with_tab();
        let tab_id = st.remotes[1].tabs[0].id;
        assert!(begin_delete_batch(&mut st, 1, tab_id, Vec::new()).is_none());
        assert!(st.inflight.is_empty(), "空批不留孤立 tag");
        assert_eq!(st.remotes[1].tabs[0].del_pending, 0);
        assert!(!st.remotes[1].tabs[0].pane.loading);
    }
}
