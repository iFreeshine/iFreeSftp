//! 四容器内容（对照 C# IdeTabStrip.xaml / FileView.xaml / QueueContainer.xaml /
//! TerminalContainer.xaml，1:1 复刻背景 / padding / 字号 / 边线 / spacing）。
//! 文件面板行结构 1:1：面包屑 26 → 工具栏 → 表头(11 SemiBold) → 列表(12px) → 状态栏；
//! 列布局函数共享，保证表头与行、竖分隔线严格对齐（ColSpacing 12 / 图标格 22）。

use std::path::{Component, PathBuf};

use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke};

use super::dialogs::{Dialog, Hooks, MenuState};
use crate::i18n::{t, tfmt};
use crate::theme;

/// 图标格（C# IconCellSize）。
const ICON_CELL: f32 = 22.0;
/// 列间距（C# ColSpacing，行与表头共用）。
const COL_GAP: f32 = 12.0;
/// 目录树侧栏宽度（C# TreeView 列）。
const TREE_W: f32 = 200.0;
/// 目录树行高 / 缩进步进 / 箭头钮尺寸。
const TREE_ROW: f32 = 20.0;
const TREE_INDENT: f32 = 14.0;
/// 行内图标尺寸。
const ROW_ICON: f32 = 18.0;
/// 文件行文字。
const ROW_FONT: f32 = 12.0;
/// 文件行高（C# ItemContainerStyle Padding 4,1 + 图标格 22 = 24）。
const ROW_H: f32 = 24.0;
/// 表头文字。
const HEADER_FONT: f32 = 11.0;
/// 面包屑行高。
const CRUMB_H: f32 = 30.0;
/// 工具钮。
const TOOL_SIZE: f32 = 24.0;
/// 刷新动效时长（秒，比 C# 静态刷新更可感知）：列表淡入遮罩与工具栏刷新钮
/// 旋转共用一旗（pane.refresh_at），触发后转一整圈 / 渐显一轮；远程慢到账时
/// 由 Listing 入账再触发下一轮（仅在上一轮结束后重启，保旋转角度连续）。
pub(crate) const REFRESH_ANIM: f64 = 0.45;
/// 上移入场总距离（px）：对齐 C# WinUI ListView 平台默认 entrance 的
/// 「条目从下方滑入」，按 egui 紧凑行高（24px）等比缩放的克制值
/// （C# 平台默认偏移 ≈2.5 行高，此处 ≈0.7 行高）。
const REFRESH_SLIDE: f32 = 16.0;
/// 每行动画时长（秒）：REFRESH_SLIDE 归位用；行延迟上限 0.15s（stagger
/// 20ms/行），合计 0.45s = REFRESH_ANIM，整表在动效窗口内完成。
const REFRESH_ROW_DUR: f32 = 0.3;

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Name,
    Size,
    Type,
    Modified,
    Permissions,
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: String,
    pub kind: String,
    pub modified: String,
    pub perms: String,
    /// 字节大小（排序键；目录恒 0，展示走 size 文本）。
    pub size_bytes: u64,
    /// 修改时间 Unix 秒（比对 FilesDiffer 的精确键；展示走 modified 文本）。
    pub mtime_secs: i64,
}

impl Entry {
    /// 比对占位行（C# FileItemViewModel.CreatePlaceholder）：全空；
    /// 绘制与交互按 `name.is_empty()` 识别（真实条目名不可能为空）。
    pub(crate) fn placeholder() -> Self {
        Self {
            name: String::new(),
            is_dir: false,
            size: String::new(),
            kind: String::new(),
            modified: String::new(),
            perms: String::new(),
            size_bytes: 0,
            mtime_secs: 0,
        }
    }
}

/// 框选拖拽上下文（FV-24；C# FileView.xaml.cs:2263-2388 字段组镜像）。
pub struct RubberState {
    /// 按下起点（屏幕坐标；_rubberOrigin）。
    origin: Pos2,
    /// Ctrl 加选（_rubberAdditive）：框选前基准 ∪ 框内。
    additive: bool,
    /// 加选基准快照（_rubberBase）。
    base: Vec<usize>,
}

/// 跨窗格条目拖拽载荷（FV-25；C# DragItemsStarting args.Items）：来源侧 + 被拖
/// 条目行号。egui DragAndDrop 插件托管，指针释放 / ESC 自动清除。
pub(crate) struct DragPayload {
    pub side: usize,
    pub rows: Vec<usize>,
}

/// 单侧文件面板状态（C# FileView + FileColumnState 镜像）。
pub struct FilePane {
    pub name_w: f32,
    pub size_w: f32,
    pub type_w: f32,
    pub modified_w: f32,
    pub perms_w: f32,
    /// 回涨目标宽（C# *ColDef：默认值或最近一次拖拽后的宽，富余空间回涨上限）。
    pub name_def_w: f32,
    pub size_def_w: f32,
    pub type_def_w: f32,
    pub modified_def_w: f32,
    pub perms_def_w: f32,
    /// 拖拽中的列（C# _draggingCol：自适应压缩时其他列先被挤、本列最后被压）。
    pub dragging_col: Option<Col>,
    pub show_size: bool,
    pub show_type: bool,
    pub show_modified: bool,
    pub show_perms: bool,
    pub tree_open: bool,
    /// 树侧栏宽（FV-33，C# DirTree.Width：分隔条拖拽钳 120~500）。
    pub tree_w: f32,
    pub show_hidden: bool,
    pub sort_col: SortKey,
    pub sort_asc: bool,
    /// 选中行集合（C# SelectedItems 多选：点击 = 单选替换、框选 = 集合应用）。
    pub selected: Vec<usize>,
    /// 框选拖拽上下文（FV-24；None = 未在框选。C# _rubberOrigin/_rubberActive/
    /// _rubberAdditive/_rubberBase）。
    pub rubber: Option<RubberState>,
    pub empty: bool,
    /// 当前目录；None = 「此电脑」根（驱动器列表，C# _currentPath 空串同语义）。
    pub path: Option<PathBuf>,
    pub entries: Vec<Entry>,
    /// 远程模式：Some = 当前远程目录（"/" 起）；None = 本地模式。
    pub remote_path: Option<String>,
    /// 空态文案（连接中 / 失败时由外层改写）。
    pub empty_title: String,
    pub empty_sub: String,
    /// 目录枚举进行中（状态栏提示）。
    pub loading: bool,
    /// 状态栏消息（列目录失败等；下一次成功枚举清除）。
    pub msg: Option<String>,
    /// 目录树侧栏根节点（tree_open 时初始化；本地=驱动器 / 远程="/"）。
    pub tree: Vec<TreeNode>,
    /// 树同步已入账位置（pane_pos 快照；None=根态已清除）。边沿触发：与当前
    /// 位置不同才重走同步（C# SyncTreeIfVisible :915-921 导航后显式调用的镜像）。
    pub tree_synced_for: Option<String>,
    /// 树重建后待恢复的展开链（C# RebuildTreePreservingAsync :936-968 逐条
    /// ExpandTreePathAsync(select:false, expandTarget:true) 的恢复队列）。
    pub tree_reexpand: Vec<String>,
    /// 文件比对行高亮（与 entries 平行；None=比对未激活）。0 无 / 1 Caution / 2 Critical。
    pub compare_marks: Option<Vec<u8>>,
    /// 刷新动效触发时刻（egui 时间秒；None = 无动效）：列表淡入 + 刷新钮旋转
    /// 共用，时长 REFRESH_ANIM。触发点 = 刷新按钮/菜单、远程 Listing 入账、
    /// 上传完成静沿刷新；入账侧仅在上轮动画结束后重启（保角度连续）。
    pub refresh_at: Option<f64>,
}

/// 目录树节点（C# TreeDir + TreeViewNode 懒展开语义）。
pub struct TreeNode {
    /// 显示名（根/驱动器 = 路径本身）。
    pub name: String,
    /// 完整路径（导航用）。
    pub path: String,
    pub expanded: bool,
    /// 远程懒展开在途（tree_req 等待响应）。
    pub loading: bool,
    /// 子级已枚举（含确认真空目录）——同步步依据它避免对空目录反复重发请求。
    pub loaded: bool,
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    fn new(name: &str, path: &str) -> Self {
        Self {
            name: name.to_owned(),
            path: path.to_owned(),
            expanded: false,
            loading: false,
            loaded: false,
            children: Vec::new(),
        }
    }
}

impl Default for FilePane {
    fn default() -> Self {
        Self {
            name_w: 160.0,
            size_w: 90.0,
            type_w: 100.0,
            modified_w: 120.0,
            perms_w: 90.0,
            // 回涨目标宽初值 = 各列默认宽（C# NameColDef 等）。
            name_def_w: 160.0,
            size_def_w: 90.0,
            type_def_w: 100.0,
            modified_def_w: 120.0,
            perms_def_w: 90.0,
            dragging_col: None,
            show_size: true,
            show_type: true,
            show_modified: true,
            // 权限列默认开：仅远程模式显示（C# ShowPermissions = true，本地由 Mode 隐藏）。
            show_perms: true,
            tree_open: false,
            // C# DirTree.Width NaN → 200（TreeSplitter_DragDelta 初始化 :1098-1100）。
            tree_w: TREE_W,
            show_hidden: false,
            sort_col: SortKey::Name,
            sort_asc: true,
            selected: Vec::new(),
            rubber: None,
            empty: false,
            path: None,
            entries: Vec::new(),
            remote_path: None,
            empty_title: t("先选择一个主机").to_owned(),
            empty_sub: t("选择要浏览的本地或远端文件系统").to_owned(),
            tree: Vec::new(),
            tree_synced_for: None,
            tree_reexpand: Vec::new(),
            compare_marks: None,
            refresh_at: None,
            loading: false,
            msg: None,
        }
    }
}

/// 可见列（名称恒在；权限列仅远程模式生效——C# ShowPermissions 默认开、本地由
/// Mode 整列隐藏）。
fn visible_cols(p: &FilePane) -> Vec<Col> {
    let mut v = vec![Col::Name];
    if p.show_size {
        v.push(Col::Size);
    }
    if p.show_type {
        v.push(Col::Type);
    }
    if p.show_modified {
        v.push(Col::Modified);
    }
    if p.show_perms && p.remote_path.is_some() {
        v.push(Col::Permissions);
    }
    v
}

fn col_width(p: &FilePane, c: Col) -> f32 {
    match c {
        Col::Name => p.name_w,
        Col::Size => p.size_w,
        Col::Type => p.type_w,
        Col::Modified => p.modified_w,
        Col::Permissions => p.perms_w,
    }
}

/// 写列宽（C# UpdateColumnFit 的 set 回调）。
fn set_col_w(p: &mut FilePane, c: Col, w: f32) {
    match c {
        Col::Name => p.name_w = w,
        Col::Size => p.size_w = w,
        Col::Type => p.type_w = w,
        Col::Modified => p.modified_w = w,
        Col::Permissions => p.perms_w = w,
    }
}

/// 拖拽钳制边界（C# FileColumnState：Name 60..1200，其余 40..400）。
fn col_bounds(p: &FilePane, c: Col) -> (f32, f32, f32) {
    let w = col_width(p, c);
    match c {
        Col::Name => (w, 60.0, 1200.0),
        _ => (w, 40.0, 400.0),
    }
}

/// 回涨目标宽（C# *ColDef；拖拽松手后更新为该列当前宽）。
fn set_col_def(p: &mut FilePane, c: Col, w: f32) {
    match c {
        Col::Name => p.name_def_w = w,
        Col::Size => p.size_def_w = w,
        Col::Type => p.type_def_w = w,
        Col::Modified => p.modified_def_w = w,
        Col::Permissions => p.perms_def_w = w,
    }
}

/// 列宽动态自适应（C# UpdateColumnFit 1:1）：可见列总宽超出列表可用宽度时，
/// 按可压缩余量等比压缩（每列不低于最小列宽，列不消失）；空间富余时低于回涨
/// 目标宽的列等比回涨。压缩两轮：第 1 轮压非拖拽列（拖拽列最后被压 → 拖拽列
/// 先拿足增量、其他列被挤到最小值），第 2 轮仍超宽才压拖拽列使其卡在剩余空间
/// 上限。0.5 容差防抖，压缩/回涨互斥防震荡。`avail`：列表可用宽度。
fn update_column_fit(p: &mut FilePane, avail: f32) {
    if avail <= 0.0 {
        return; // 首帧布局前宽度未定（C# 同）
    }
    // 可见列（名称恒在）：(列, 当前宽, 最小宽, 回涨目标宽)。
    let mut cols: Vec<(Col, f32, f32, f32)> = vec![(Col::Name, p.name_w, 60.0, p.name_def_w)];
    if p.show_size {
        cols.push((Col::Size, p.size_w, 40.0, p.size_def_w));
    }
    if p.show_type {
        cols.push((Col::Type, p.type_w, 40.0, p.type_def_w));
    }
    if p.show_modified {
        cols.push((Col::Modified, p.modified_w, 40.0, p.modified_def_w));
    }
    if p.show_perms && p.remote_path.is_some() {
        cols.push((Col::Permissions, p.perms_w, 40.0, p.perms_def_w));
    }
    // 总需求 = 固定开销（左右 Padding 8 + 图标列 22 + 名称列前间距 12）
    //          + Σ(每列宽 + 列间距 12)；几何常量与 C# XAML 列模板同源。
    const FIXED_OVERHEAD: f32 = 42.0; // C# NameColFixedOverhead = RowPaddingH*2 + IconCellSize + ColSpacing
    let mut needed = FIXED_OVERHEAD;
    for &(_, w, _, _) in &cols {
        needed += w + COL_GAP;
    }
    let mut excess = needed - avail;
    let dragging = p.dragging_col;
    if excess > 0.5 {
        for round in 0..2 {
            if excess <= 0.5 {
                break;
            }
            if round == 1 && dragging.is_none() {
                break; // 无拖拽列：第 1 轮已按等比处理全部列
            }
            let mut squeezable = 0.0;
            for &(c, w, min, _) in &cols {
                let is_drag = Some(c) == dragging;
                if (round == 0) == is_drag {
                    continue; // 第 1 轮跳过拖拽列 / 第 2 轮只压拖拽列
                }
                squeezable += (w - min).max(0.0);
            }
            if squeezable <= 0.0 {
                continue;
            }
            let ratio = (excess / squeezable).min(1.0);
            for &(c, w, min, _) in &cols {
                let is_drag = Some(c) == dragging;
                if (round == 0) == is_drag {
                    continue;
                }
                let shrunk = (w - (w - min) * ratio).max(min);
                if shrunk < w - 0.05 {
                    set_col_w(p, c, shrunk);
                    excess -= w - shrunk;
                }
            }
        }
    } else if -excess > 0.5 {
        // 空间富余 → 回涨恢复：被挤压列从当前宽等比回涨到各自目标宽为止；
        // 拖拽列与已达目标宽的列不动（单调收敛）。
        let mut deficit = 0.0;
        for &(c, w, _, def) in &cols {
            if Some(c) != dragging {
                deficit += (def - w).max(0.0);
            }
        }
        if deficit > 0.0 {
            let ratio = (-excess / deficit).min(1.0);
            for &(c, w, _, def) in &cols {
                if Some(c) == dragging || def <= w {
                    continue;
                }
                let grown = (w + (def - w) * ratio).min(def);
                if grown > w + 0.05 {
                    set_col_w(p, c, grown);
                }
            }
        }
    }
}

/// 列几何：x 起点按 [图标22 gap12 列.. gap12 尾部*] 走。
/// 返回 (列, 内容Rect)。Size/Type/Modified/Permissions 内部左右各让 6（C# Padding 6,0）。
fn col_rects(p: &FilePane, x0: f32) -> Vec<(Col, Rect)> {
    let mut x = x0;
    let icon = Rect::from_min_size(Pos2::new(x, 0.0), egui::vec2(ICON_CELL, 0.0));
    x += ICON_CELL + COL_GAP;
    let mut out = Vec::with_capacity(6);
    let _ = icon; // 图标格固定占位，不参与竖线
    for c in visible_cols(p) {
        let w = col_width(p, c);
        out.push((
            c,
            Rect::from_min_size(Pos2::new(x, 0.0), egui::vec2(w, 0.0)),
        ));
        x += w + COL_GAP;
    }
    out
}

/// tab 条高（C# IdeTabStrip：标签 MinHeight 36 + Margin 上下各 1 = 行高 38）。
pub const TAB_BAR_H: f32 = 38.0;
/// tab 项高（C# TabViewItem MinHeight=36）。
const TAB_ITEM_H: f32 = 36.0;
/// tab 图标（C# IconFontSize=9 的 Segoe Fluent em 尺寸；字形墨迹约占 em 的
/// 80~90%，Phosphor size 是墨迹边长，等观感补偿 ≈ 9 / 0.83 ≈ 11）。
const TAB_ICON_FONT: f32 = 11.0;
/// tab 标题字号（C# TitleFontSize=12）。
const TAB_TITLE_FONT: f32 = 12.0;
/// 关闭图标（C# CloseFontSize=9，同上 em→墨迹补偿 → 11）。
const TAB_CLOSE_FONT: f32 = 11.0;
/// 关闭钮 18×18（Margin 6,0,4,0 → 列定宽 28）。
const TAB_CLOSE_SIZE: f32 = 18.0;
/// 关闭列定宽（模板 ColumnDefinition Width=28）。
const TAB_CLOSE_COL: f32 = 28.0;
/// 新建 (+) 钮 28×28（C# NewTabBtn）。
const TAB_NEW_SIZE: f32 = 28.0;
/// 亮相动画时长秒（G17-ANIM）：近似 C# TabItems.Add 触发的 WinUI 系统默认
/// AddDeleteThemeTransition（TabViewListView 继承 ListViewBase 的默认容器过渡），
/// 系统实现参数未公开，取 WinUI 主题动画常规 250ms。
const TAB_APPEAR_SECS: f32 = 0.25;
/// 亮相动画起始下探 px（C# 视觉：新 tab 自条下方冒头上移至位 + 渐显）。
const TAB_APPEAR_OFFSET: f32 = 10.0;

/// tab 种类（双侧多标签 P1）：本地文件系统 tab / 远程主机 tab。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    Local,
    Remote,
}

/// 单个 tab 项（双窗格改造，对齐 C# 每 tab 独立 FileView 的感知层）：
/// 稳定 id（State 派发，绘制层不解释）+ 标题 + 种类。左侧 = 「本地」tab 与
/// 远程主机 tab 并存（切换显示、会话保活）；右侧 = 远程主机 tab。种类决定
/// 活动 tab 映射到哪个面板（本地 pane / 该侧远程 pane）。
#[derive(Clone)]
pub struct TabItem {
    /// 稳定 id：State 侧以 id 寻址 tab（P1 由 mod.rs 合成 tab 条视图时填入）。
    pub id: u32,
    pub title: String,
    pub kind: TabKind,
}

/// IDE tab 条状态（C# IdeTabStrip 复刻）。
pub struct Tabs {
    pub items: Vec<TabItem>,
    pub active: usize,
    /// 亮相动画（G17-ANIM）：最近新增 tab 的 (下标, 起始时刻)。C# TabItems.Add
    /// 即播系统 AddDeleteThemeTransition；egui 无主题动画，帧级手绘近似
    /// （tab_strip 绘制时推进，结束清除）。
    pub born: Option<(usize, std::time::Instant)>,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            items: vec![TabItem {
                id: 0,
                title: t("本地").to_owned(),
                kind: TabKind::Local,
            }],
            active: 0,
            born: None,
        }
    }
}

/// tab 条：1:1 复刻 C# IdeTabStrip。条高 38 / 项高 36（Margin 0,1,1,1）/ 常态底
/// SubtleFillColorTertiary / 选中底 TabActiveBrush / 图标 9px 恒 AccentBrush /
/// 标题 12px 随态取 TabViewItemHeaderForeground*（常态·悬停 Secondary / 按下
/// Tertiary / 选中覆写主色）/ 关闭钮 18×18 仅 hover·按下·选中可见 / 指示条高 2
/// 下探 1px 盖基线 / 新建钮 28×28 固定最右。`remote`：窗格模式（决定图标与新建页签标题）。
/// 返回标签内的关闭请求（C# Tabs_TabCloseRequested）：本层不移除标签，交
/// 调用方处置（活动 tab 走确认门与资源清理，非活动 tab 直接移除标题）。
pub fn tab_strip(
    ui: &mut egui::Ui,
    rect: Rect,
    tabs: &mut Tabs,
    st_icons: &mut crate::icons::Cache,
    p: &theme::Palette,
    remote: bool,
    side: usize,
    hooks: &mut Hooks,
) -> Option<usize> {
    ui.painter().rect_filled(rect, 0.0, p.folder_area);
    // 基线（TabStripBaseline：C# Border Margin top 37 Height 1，占 y ∈ [37,38]；
    // Fill=TabViewBorderBrush，IdeTabStrip 亮主题资源字典显式覆盖 #E3E3E3 /
    // 暗无覆盖走系统默认 CardStrokeColorDefault 黑10%）。
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 37.5),
            Pos2::new(rect.right(), rect.top() + 37.5),
        ],
        Stroke::new(1.0, p.ide_tab_border),
    );

    // 项几何：Margin 0,1,1,1 → y0 = top+1、高 36、项距 1。
    let y0 = rect.top() + 1.0;
    let mut x = rect.left();
    let title_font = FontId::proportional(TAB_TITLE_FONT);
    let mut close_req: Option<usize> = None;
    let mode_icon = if remote { "desktop-tower" } else { "monitor" };

    // 亮相动画推进（G17-ANIM，C# TabItems.Add → AddDeleteThemeTransition）：
    // 进度 t<1 应用位移+渐显并请求下一帧；t≥1 清除停播。easing 取 cubic
    // ease-out（WinUI 主题动画 fast-out-slow-in 近似）。
    let mut appear: Option<(usize, f32)> = None;
    if let Some((idx, t0)) = tabs.born {
        let t = (t0.elapsed().as_secs_f32() / TAB_APPEAR_SECS).min(1.0);
        if t >= 1.0 {
            tabs.born = None;
        } else {
            appear = Some((idx, 1.0 - (1.0 - t).powi(3)));
            ui.ctx().request_repaint();
        }
    }

    for i in 0..tabs.items.len() {
        let title = tabs.items[i].title.clone();
        let active = i == tabs.active;
        let text_w = galley_w(ui, &title, TAB_TITLE_FONT);
        // 图标 Margin 左 8 + 图标 9 + 标题 Margin 左 8 + 文字 + 关闭列 28。
        let item_w = 8.0 + TAB_ICON_FONT + 8.0 + text_w + TAB_CLOSE_COL;
        let mut item = Rect::from_min_size(Pos2::new(x, y0), egui::vec2(item_w, TAB_ITEM_H));

        // 亮相动画（G17-ANIM）：整体 visual 下探+渐显（命中几何随动，C#
        // AddDeleteThemeTransition 同作用于整个 item visual）。alpha 统一乘子
        // 走克隆 painter（rect/文本全生效）；图标纹理烘焙色不可按帧变参缓存，
        // 经 Image::tint 白色乘 alpha 单独补乘。
        let mut painter = ui.painter().clone();
        let mut alpha = 1.0;
        if let Some((idx, e)) = appear {
            if idx == i {
                item = item.translate(egui::vec2(0.0, (1.0 - e) * TAB_APPEAR_OFFSET));
                alpha = e;
            }
        }
        painter.set_opacity(alpha);

        let resp = ui.allocate_rect(item, egui::Sense::click());
        // 三态底（模板 VSM）：选中 TabActiveBrush / hover·按下
        // SubtleFillColorSecondary / 常态 SubtleFillColorTertiary（方角贴基线，
        // CornerRadius 0）。
        let bg = if active {
            p.tab_selected_bg
        } else if resp.is_pointer_button_down_on() || resp.hovered() {
            p.tab_hover
        } else {
            p.tab_rest
        };
        painter.rect_filled(item, 0.0, bg);
        if resp.clicked() {
            tabs.active = i;
        }
        // 指示条（选中）：Height 2 + Margin bottom -1 → 项底上 1px + 下探 1px 盖基线，
        // ColumnSpan=2 全项宽。
        if active {
            let ind = Rect::from_min_max(
                Pos2::new(item.left(), item.bottom() - 1.0),
                Pos2::new(item.right(), item.bottom() + 1.0),
            );
            painter.rect_filled(ind, 0.0, theme::ACCENT);
        }

        let cy = item.center().y - 1.0; // .cs 垂直偏移 -1
        // 模式图标 9px 恒 AccentBrush（本地 monitor / 远程 desktop-tower）。
        if let Some(img) = st_icons.get(ui.ctx(), mode_icon, theme::ACCENT, TAB_ICON_FONT) {
            img.tint(egui::Color32::WHITE.gamma_multiply(alpha))
                .paint_at(
                    ui,
                    Rect::from_center_size(
                        Pos2::new(item.left() + 8.0 + TAB_ICON_FONT / 2.0, cy),
                        egui::vec2(TAB_ICON_FONT, TAB_ICON_FONT),
                    ),
                );
        }
        // 标题 12px（HeaderPresenter 前景随态：常态·悬停 = TabViewItemHeaderForeground
        // 系统值 Secondary / 按下 = …ForegroundPressed Tertiary / 选中 =
        // …ForegroundSelected，IdeTabStrip 覆写 #222222·#FFFFFF）。
        let title_fg = if active {
            p.tab_active_fg
        } else if resp.is_pointer_button_down_on() {
            p.text_tertiary
        } else {
            p.text_secondary
        };
        painter.text(
            Pos2::new(item.left() + 8.0 + TAB_ICON_FONT + 8.0, cy),
            Align2::LEFT_CENTER,
            title,
            title_font.clone(),
            title_fg,
        );

        // 关闭钮 18×18 CornerRadius 3（Margin 6,0,4,0），仅 hover / 按下 / 选中可见。
        if active || resp.hovered() || resp.is_pointer_button_down_on() {
            let close_r = Rect::from_center_size(
                Pos2::new(item.right() - 4.0 - TAB_CLOSE_SIZE / 2.0, item.center().y),
                egui::vec2(TAB_CLOSE_SIZE, TAB_CLOSE_SIZE),
            );
            let close_resp = ui.allocate_rect(close_r, egui::Sense::click());
            if close_resp.hovered() {
                // hover 底来自默认 Button 模板 ButtonBackgroundPointerOver
                // （= ControlFillColorSecondary）。
                painter.rect_filled(close_r, 3.0, p.control_hover);
            }
            if close_resp.clicked() {
                // G17（C# Tabs_TabCloseRequested）：本层不移除标签，记录请求交
                // 调用方——活动 tab 走确认门与资源清理（C# TabCloseRequested 事件）；
                // 无 len>1 门槛（C# 最后一个 tab 关闭回空态）。
                close_req = Some(i);
            }
            if let Some(img) = st_icons.get(ui.ctx(), "x", p.text_primary, TAB_CLOSE_FONT) {
                img.tint(egui::Color32::WHITE.gamma_multiply(alpha))
                    .paint_at(
                        ui,
                        Rect::from_center_size(
                            close_r.center(),
                            egui::vec2(TAB_CLOSE_FONT, TAB_CLOSE_FONT),
                        ),
                    );
            }
        }

        x += item_w + 1.0; // Margin 右 1
    }

    // 新建 (+) 钮 28×28 固定最右（Margin 0,5,4,0；hover ButtonBackgroundPointerOver）。
    let new_r = Rect::from_min_size(
        Pos2::new(rect.right() - 4.0 - TAB_NEW_SIZE, rect.top() + 5.0),
        egui::vec2(TAB_NEW_SIZE, TAB_NEW_SIZE),
    );
    let resp = ui.allocate_rect(new_r, egui::Sense::click());
    if resp.hovered() {
        // NewTabBtn 资源字典覆写 ButtonBackgroundPointerOver（亮 #E3E3E3 / 暗 #2B3138）。
        ui.painter().rect_filled(new_r, 3.0, p.tab_new_hover);
    }
    if resp.clicked() {
        // C# NewTabBtn_Click → NewTabRequested → FolderContainer(:51)
        // OpenPickerRequested(PaneName) → Header(:136) HostPickerOverlay.OpenFor：
        // + 号打开「打开目标」选择器（本地文件系统 / 主机列表），选定后才落新 tab
        // （HostPicked 强制 TabIntent="new" 新标签页 / LocalPicked 非空态 AddNewTab），
        // 不直接加 tab；new_tab=true 供选择器落位判定（空态按钮入口传 false 就地）。
        *hooks.dialog = Dialog::HostPicker {
            target: side,
            new_tab: true,
        };
    }
    // "+" 图标（C# NewTabBtn FontIcon FontSize=14，em→墨迹补偿 ≈ 14 / 0.93 ≈ 15）。
    if let Some(img) = st_icons.get(ui.ctx(), "plus", p.text_primary, 15.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(new_r.center(), egui::vec2(15.0, 15.0)),
        );
    }
    close_req
}

/// 文件夹窗格：tab 条 + FileView 五行结构。`side`：0=左（本地）1=右（远程）。
pub fn folder(
    ui: &mut egui::Ui,
    rect: Rect,
    pane: &mut FilePane,
    tabs: &mut Tabs,
    st_icons: &mut crate::icons::Cache,
    sys_icons: &mut crate::icons_sys::Cache,
    hooks: &mut Hooks,
    side: usize,
) {
    let p = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(rect, theme::CONTAINER_RADIUS, p.folder_area);

    // 裁剪到容器圆角内。
    let inner_full = rect.shrink2(egui::vec2(1.0, 1.0));

    // C# SetHeaderVisible(false)：空态隐藏 tab 条头部，空态面板铺满整个容器。
    if pane.empty {
        empty_state(
            ui,
            inner_full,
            st_icons,
            &p,
            hooks,
            side,
            (&pane.empty_title, &pane.empty_sub),
        );
        return;
    }

    // tab 条永远在最上（C# FolderContainer = IdeTabStrip + FileView）。
    let tab_bar = Rect::from_min_max(
        inner_full.left_top(),
        Pos2::new(inner_full.right(), inner_full.top() + TAB_BAR_H),
    );
    if let Some(i) = tab_strip(
        ui,
        tab_bar,
        tabs,
        st_icons,
        &p,
        pane.remote_path.is_some(),
        side,
        hooks,
    ) {
        // G17（C# IdeTabStrip 移除标签后 TabCloseRequested 交使用方清理）：
        // 关闭请求按 (侧, tab id) 上报交 root() 处置——活动 tab 走确认门与
        // 资源清理，非活动 tab 在临时视图上直接移除标题（mod.rs 按 id 差集
        // 映射回真实状态）。
        if i == tabs.active {
            hooks.tab_close = Some((side, tabs.items[i].id));
        } else {
            tabs.items.remove(i);
            if i < tabs.active {
                tabs.active -= 1;
            }
        }
    }
    let inner = Rect::from_min_max(
        Pos2::new(inner_full.left(), tab_bar.bottom()),
        inner_full.right_bottom(),
    );

    let y0 = inner.top();
    let crumb = Rect::from_min_max(inner.left_top(), Pos2::new(inner.right(), y0 + CRUMB_H));
    breadcrumb(ui, crumb, pane, st_icons, &p, hooks, side);

    let toolbar_h = TOOL_SIZE + 2.0; // C# 工具栏 Padding 4,1：钮 24 + 上下各 1 = 26
    let toolbar = Rect::from_min_max(
        Pos2::new(inner.left(), crumb.bottom()),
        Pos2::new(inner.right(), crumb.bottom() + toolbar_h),
    );
    toolbar_row(ui, toolbar, pane, st_icons, &p, hooks, side);

    let header_h = HEADER_FONT + 16.0 + 1.0; // C# HeaderGrid Padding 4,6 + 11px 行高 ≈15 + 底线 1 = 28

    let status_h = ROW_FONT + 12.0 + 1.0; // padding 8,4 双侧 + 行高 ≈16 + 顶边线 = 25
    let status = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - status_h),
        inner.right_bottom(),
    );

    // 目录树侧栏（C# TreeView 列）：表头顶→状态栏顶，列表/表头右移。
    if pane.tree_open && pane.tree.is_empty() {
        init_tree_roots(pane, hooks);
    }
    let tree_w = if pane.tree_open { pane.tree_w } else { 0.0 };
    let list_left = inner.left() + tree_w;
    let actions = if pane.tree_open {
        draw_tree(
            ui,
            Rect::from_min_max(
                Pos2::new(inner.left(), toolbar.bottom()),
                Pos2::new(list_left, status.top()),
            ),
            pane,
            st_icons,
            &p,
        )
    } else {
        Vec::new()
    };
    for (path, toggle) in actions {
        tree_apply(pane, hooks, side, &path, toggle);
    }
    // 树同步步进（C# SyncTreeIfVisible + RebuildTreePreservingAsync 镜像）：
    // 位置变化 → 下行确立高亮基准；重建后 → 恢复展开链。
    tree_sync_step(pane, hooks, side);

    // FV-33（C# TreeSeparator 1px SeparatorLineBrush + TreeSplitter 13px 透明命中
    // 区，xaml:496-527 / TreeSplitter_DragDelta :1097-1105）：分隔线画在树右缘，
    // 拖手跨线可拖，树宽钳 120~500（SplitterThumb hover 光标自带 → 恒示
    // SizeWestEast 等效光标）。
    if pane.tree_open {
        let line_x = list_left + 0.5;
        ui.painter().line_segment(
            [
                Pos2::new(line_x, toolbar.bottom()),
                Pos2::new(line_x, status.top()),
            ],
            Stroke::new(1.0, p.separator),
        );
        let thumb = Rect::from_min_size(
            Pos2::new(list_left - 6.0, toolbar.bottom()),
            egui::vec2(13.0, status.top() - toolbar.bottom()),
        );
        let resp = ui.allocate_rect(thumb, egui::Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if resp.dragged() {
            pane.tree_w = (pane.tree_w + resp.drag_delta().x).clamp(120.0, 500.0);
        }
    }

    let header = Rect::from_min_max(
        Pos2::new(list_left, toolbar.bottom()),
        Pos2::new(inner.right(), toolbar.bottom() + header_h),
    );
    let list = Rect::from_min_max(
        Pos2::new(list_left, header.bottom()),
        Pos2::new(inner.right(), status.top()),
    );

    header_row(ui, header, pane, st_icons, &p);
    file_rows(ui, list, pane, st_icons, sys_icons, &p, hooks, side);

    // 列分隔竖线：跨表头+列表贯穿，画在相邻列之间（C# 覆盖层：N 列 N-1 条线，
    // 位于列间距 12 正中 = 左列右缘 +6；名称列左无线、最右列右缘无线）。
    // 同位置 16px 宽透明命中区（C# ColResizeThumbStyle）置于表头行，拖拽调整
    // 左列宽度；注册晚于表头 → 命中优先，其余表头区域仍走排序点击。
    let content_x = list_left + 4.0; // 行 padding 左 4
    let rects = col_rects(pane, content_x);
    for (idx, (c, r)) in rects.iter().enumerate() {
        if idx + 1 == rects.len() {
            break; // 最右列右缘无线（C# 无尾部线）
        }
        let x = r.right() + 6.0;
        if x < inner.right() - 8.0 {
            ui.painter().line_segment(
                [Pos2::new(x, header.top()), Pos2::new(x, list.bottom())],
                Stroke::new(1.0, p.separator),
            );
            let thumb = Rect::from_min_size(
                Pos2::new(x - 8.0, header.top()),
                egui::vec2(16.0, header.height()),
            );
            let thumb_resp = ui.allocate_rect(thumb, Sense::click_and_drag());
            if thumb_resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if thumb_resp.dragged() {
                // 增量累加到当前列宽并按 min/max 钳制（C# ColumnThumb_DragDelta）；
                // 超宽挤压由随后的 update_column_fit 处理（其他列先被挤、本列最后压）。
                pane.dragging_col = Some(*c);
                let (_, min, max) = col_bounds(pane, *c);
                let nw = col_width(pane, *c) + thumb_resp.drag_delta().x;
                set_col_w(pane, *c, nw.clamp(min, max));
            }
            if thumb_resp.drag_stopped() && pane.dragging_col == Some(*c) {
                // 松手：拖拽结果成为该列新的回涨目标宽（C# ColumnThumb_DragCompleted）。
                let w = col_width(pane, *c);
                set_col_def(pane, *c, w);
                pane.dragging_col = None;
            }
        }
    }

    // 列宽动态自适应（C# UpdateColumnFit：拖拽列宽 / 窗格缩放 / 滚动条出现时收敛；
    // 每帧跑幂等，压缩到最小列宽为止、富余回涨到目标宽，列不消失）。
    update_column_fit(pane, list.width());

    status_bar(ui, status, pane, &p);
}

// ── 面包屑行 ──────────────────────────────────────────────
fn breadcrumb(
    ui: &mut egui::Ui,
    rect: Rect,
    pane: &mut FilePane,
    st_icons: &mut crate::icons::Cache,
    p: &theme::Palette,
    hooks: &mut Hooks,
    side: usize,
) {
    ui.painter().rect_filled(rect, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.bottom() - 0.5),
            Pos2::new(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, p.row_separator),
    );

    // 点击空白切输入框（先分配在下层；house/段命中面在后分配可优先吃点击）。
    let blank = ui.allocate_rect(rect, Sense::click());

    // 编辑态：全行输入框，回车 / 失焦退出（C# BreadcrumbTextBox 同交互）。
    if matches!(&*hooks.crumb_edit, Some((s, _)) if *s == side) {
        let pad = 6.0;
        let edit_r = Rect::from_min_max(
            Pos2::new(rect.left() + pad, rect.top() + 3.0),
            Pos2::new(rect.right() - pad, rect.bottom() - 3.0),
        );
        if let Some((_, text)) = hooks.crumb_edit.as_mut() {
            let mut text = std::mem::take(text);
            // 显式 id：show 前后都按它读写 TextEditState。
            let te_id = egui::Id::NULL.with(("crumb_edit_te", side));
            let open_id = te_id.with("open");
            // 打开沿即夺焦：TextEdit 无焦点时 egui 每帧把选区折叠成单光标
            // （builder.rs owns_ime_events=has_focus），框选活不过一帧。
            let first_frame = !ui
                .ctx()
                .data(|d| d.get_temp::<bool>(open_id))
                .unwrap_or(false);
            let te = egui::TextEdit::singleline(&mut text)
                .id_salt(te_id)
                .font(egui::FontId::proportional(12.0))
                .desired_width(edit_r.width())
                .min_size(egui::vec2(edit_r.width(), edit_r.height()));
            let resp = ui.put(edit_r, te);
            if first_frame {
                ui.ctx().data_mut(|d| d.insert_temp(open_id, true));
                resp.request_focus();
            }
            // 右键菜单走全局钩子（src/ui/ctx_menu.rs）：show 后注册真实 id 与
            // rect，帧首钩子拦截 Secondary press 快照选区，浮层菜单动作经事件
            // 注入执行（examples/ctx_menu_hook.rs 黑箱验证定案，取代早期包内
            // 「右键冻结选区 + 键盘 Ctrl+C」方案）。
            hooks.ctx_menu.register(resp.id, edit_r);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            // 菜单项松开帧与动作注入帧抑制提交：egui 0.37 默认 Clicks 收焦
            // （收焦在 click 完成帧，见 ctx_menu submit_guard 注释），lost
            // 沿落在菜单项松开帧，若照常提交则菜单一点编辑态即退出。
            let suppress = hooks.ctx_menu.suppress_submit();
            if !suppress && (resp.lost_focus() || enter) {
                // 回车导航到输入路径；本地无效路径 navigate 失败保持原目录，
                // 远程无效路径由列目录失败消息反馈。
                let t = text.trim().to_owned();
                if !t.is_empty() {
                    if pane.remote_path.is_some() {
                        hooks.list_req.push((side, t));
                    } else if crate::local::navigate(pane, std::path::Path::new(&t)) {
                        *hooks.sync_nav = Some((side, t));
                    }
                }
                *hooks.crumb_edit = None;
                // 清编辑态打开旗标，下次打开重新走打开沿夺焦。
                ui.ctx().data_mut(|d| d.insert_temp(open_id, false));
            } else if let Some((_, t)) = hooks.crumb_edit.as_mut() {
                *t = text;
            }
        }
        return;
    }

    let pad_x = 6.0;
    let mut x = rect.left() + pad_x;
    let cy = rect.center().y;

    // 逐级路径段（驱动器 → 目录名 / 远程根 → 目录名）。
    /// 段点击的导航目标（本地路径 / 远程目录）。
    enum Nav {
        Local(PathBuf),
        Remote(String),
    }
    let mut segs: Vec<(String, Nav)> = Vec::new();
    if let Some(rp) = &pane.remote_path {
        // 远程：根段 "/" + 逐级目录（SFTP 绝对路径）。
        segs.push(("/".to_owned(), Nav::Remote("/".to_owned())));
        let mut acc = String::new();
        for part in rp.split('/').filter(|s| !s.is_empty()) {
            acc = format!("{acc}/{part}");
            segs.push((part.to_owned(), Nav::Remote(acc.clone())));
        }
    } else if let Some(path) = &pane.path {
        let mut acc = PathBuf::new();
        for comp in path.components() {
            match comp {
                Component::Prefix(pr) => {
                    let s = pr.as_os_str().to_string_lossy().into_owned();
                    segs.push((s.clone(), Nav::Local(PathBuf::from(&s))));
                    acc = PathBuf::from(s);
                }
                Component::RootDir => {
                    acc = PathBuf::from(format!("{}\\", acc.to_string_lossy()));
                    if let Some(last) = segs.last_mut() {
                        last.1 = Nav::Local(acc.clone());
                    }
                }
                Component::CurDir => {}
                other => {
                    acc.push(other.as_os_str());
                    segs.push((
                        other.as_os_str().to_string_lossy().into_owned(),
                        Nav::Local(acc.clone()),
                    ));
                }
            }
        }
    }

    // 根段：house 图标替代文字（C# CreateCrumbButton：根段 Path 为空用 Home 图标），
    // 点击回「此电脑」根视图（NavigateLocalRoot）。命中区 24×20 行内垂直居中 / 圆角 4。
    let vpad = (CRUMB_H - 20.0) * 0.5;
    let house_r = Rect::from_min_size(Pos2::new(x, rect.top() + vpad), egui::vec2(24.0, 20.0));
    let house_resp = ui.allocate_rect(house_r, Sense::click());
    if house_resp.hovered() {
        ui.painter().rect_filled(house_r, 4.0, p.row_hover);
    }
    // 根视图（无路径段）时根段即当前段 → 主色，否则为历史段 → 次级。
    let house_fg = if segs.is_empty() {
        p.text_primary
    } else {
        p.text_secondary
    };
    if let Some(img) = st_icons.get(ui.ctx(), "house", house_fg, 12.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(Pos2::new(x + 12.0, cy), egui::vec2(12.0, 12.0)),
        );
    }
    x += 24.0 + 2.0;

    for (i, (text, nav)) in segs.iter().enumerate() {
        let last = i == segs.len() - 1;
        // 当前段 SemiBold 品牌字 + 主色，其余常规 + 次级（C# 重建面包屑）。
        let font = if last {
            FontId::new(12.0, egui::FontFamily::Name("brand".into()))
        } else {
            FontId::proportional(12.0)
        };
        let color = if last {
            p.text_primary
        } else {
            p.text_secondary
        };
        // 段文字 MaxWidth 160 截断（C# TextBlock MaxWidth + TextTrimming CharacterEllipsis）。
        let shown = ellipsize(ui, text, &font, 160.0);
        let galley = ui
            .painter()
            .layout_no_wrap(shown.clone(), font.clone(), color);
        // 段命中面（Border Padding 6,0 → 命中宽 = 6+文字+6；点击导航到该段截止的完整路径）。
        let hit_w = galley.size().x + 12.0;
        let seg_r = Rect::from_min_max(
            Pos2::new(x, rect.top() + vpad),
            Pos2::new(x + hit_w, rect.top() + vpad + 20.0),
        );
        let resp = ui.allocate_rect(seg_r, Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(seg_r, 4.0, p.row_hover);
        }
        ui.painter().galley(
            Pos2::new(x + 6.0, cy - galley.size().y / 2.0),
            galley,
            color,
        );
        x += hit_w + 2.0;
        if !last {
            // C# CrumbSeparator Glyph E76C（ChevronRight）= > 形折线，非实心三角：
            // 两段 1px 线，高 10（FontSize=10）居中，前色 TextFillColorTertiary。
            let cx = x + 5.0;
            let crumb_stroke = Stroke::new(1.0, p.text_tertiary);
            ui.painter().line_segment(
                [Pos2::new(cx - 2.5, cy - 5.0), Pos2::new(cx + 2.5, cy)],
                crumb_stroke,
            );
            ui.painter().line_segment(
                [Pos2::new(cx + 2.5, cy), Pos2::new(cx - 2.5, cy + 5.0)],
                crumb_stroke,
            );
            x += 10.0 + 2.0;
        }
        if resp.clicked() {
            match nav {
                Nav::Local(pth) => {
                    if crate::local::navigate(pane, pth) {
                        *hooks.sync_nav = Some((side, pth.to_string_lossy().into_owned()));
                    }
                }
                Nav::Remote(target) => hooks.list_req.push((side, target.clone())),
            }
        }
    }

    if house_resp.clicked() {
        if pane.remote_path.is_some() {
            hooks.list_req.push((side, "/".to_owned()));
        } else {
            crate::local::navigate_root(pane);
            // 根视图（此电脑）：越出基准根 → 同步联动视为失配（C# RelToBase null 同语义）。
            *hooks.sync_nav = Some((side, String::new()));
        }
    }
    if blank.clicked() {
        // 编辑初值 = 当前路径：本地取 path；远程 path 为 None，取 remote_path
        //（否则右侧面包屑点击后编辑框成空）。
        let text = if let Some(rp) = &pane.remote_path {
            rp.clone()
        } else {
            pane.path
                .as_ref()
                .map(|pth| pth.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        *hooks.crumb_edit = Some((side, text));
    }
}

// ── 工具栏行 ──────────────────────────────────────────────
fn toolbar_row(
    ui: &mut egui::Ui,
    rect: Rect,
    pane: &mut FilePane,
    st_icons: &mut crate::icons::Cache,
    p: &theme::Palette,
    hooks: &mut Hooks,
    side: usize,
) {
    ui.painter().rect_filled(rect, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.bottom() - 0.5),
            Pos2::new(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, p.row_separator),
    );

    let tools = [
        ("folder-open", "open_dir", "打开目录"),
        ("arrow-up", "up", "上一级"),
        ("arrows-clockwise", "refresh", "刷新"),
        ("list", "tree", "切换树形目录"),
        ("folder-plus", "new_folder", "新建文件夹"),
        ("plus", "new_file", "新建文件"),
        ("eye", "hidden", "显示/隐藏隐藏文件"),
    ];
    // 禁用策略（C# XAML IsEnabled / UpdateNewButtons）：打开目录仅本地启用
    // （!isRemote），选择框在途时禁用（单实例防重复弹框）；新建夹/新建文件 =
    // 本地有路径 或 远程已浏览到目录（C# 远程同样可新建，CreateRemoteEntryAsync）；
    // 其余（上一级/刷新/树/隐藏）恒可用。
    let is_remote = pane.remote_path.is_some();
    let can_new = pane.path.is_some() || pane.remote_path.is_some();
    let mut x = rect.left() + 4.0;
    let cy = rect.center().y;
    for (icon, id, label) in tools {
        let enabled = match id {
            // Linux 降级（CROSS_PLATFORM §3）：无系统文件夹选择框实现（rfd 需
            // GTK3 C 库链接，零依赖口径不引入），「打开目录」灰显——遵守禁用钮
            // 无 tooltip 的 WinUI 语义，降级声明见文档。
            "open_dir" => cfg!(windows) && !is_remote && hooks.pick.is_none(),
            "new_folder" | "new_file" => can_new,
            _ => true,
        };
        let r = Rect::from_min_size(
            Pos2::new(x, cy - TOOL_SIZE / 2.0),
            egui::vec2(TOOL_SIZE, TOOL_SIZE),
        );
        // 禁用钮不参与命中测试（WinUI 禁用语义）：无 hover 底、不响应点击、无 tooltip。
        let resp = ui.allocate_rect(
            r,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let on = match id {
            "tree" => pane.tree_open,
            "hidden" => pane.show_hidden,
            _ => false,
        };
        // 切换钮选中底：WinUI ToggleButton 默认 Checked = ControlFillColorTertiary、
        // CheckedPointerOver = ControlFillColorSecondary（Header.xaml 仅覆写连接开关，不涉此处）。
        if enabled {
            if on && resp.hovered() {
                ui.painter().rect_filled(r, 4.0, p.control_hover);
            } else if on {
                ui.painter().rect_filled(r, 4.0, p.control_pressed);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, p.hover_bg);
            }
        }
        // 图标：启用恒主色（C# FontIcon 无按态前景覆写；FontSize=9 的 em→墨迹
        // 补偿 ≈ 11）；禁用前景 TextFillColorDisabled。
        let fg = if enabled {
            p.text_primary
        } else {
            p.text_disabled
        };
        if let Some(img) = st_icons.get(ui.ctx(), icon, fg, 11.0) {
            // 刷新钮旋转（比 C# 更好的刷新表现）：动效期内转一整圈，给出
            // 「刷新进行中」的动作反馈；快服务器到账不重启（角度连续），
            // 慢到账由 Listing 入账再触发下一轮。
            let img = match (id, pane.refresh_at) {
                ("refresh", Some(t0)) => {
                    let el = ui.ctx().input(|i| i.time) - t0;
                    if el < REFRESH_ANIM {
                        img.rotate(
                            (el / REFRESH_ANIM) as f32 * std::f32::consts::TAU,
                            egui::vec2(0.5, 0.5),
                        )
                    } else {
                        img
                    }
                }
                _ => img,
            };
            img.paint_at(
                ui,
                Rect::from_center_size(r.center(), egui::vec2(11.0, 11.0)),
            );
        }
        // tooltip：仅启用钮挂（WinUI 禁用控件不参与命中测试，tooltip 永不显示）。
        // accent_tip 内部即含 hover 判定，启用态直接挂。
        let resp = if enabled {
            super::accent_tip(resp, t(label))
        } else {
            resp
        };
        if resp.clicked() {
            match id {
                "tree" => {
                    pane.tree_open = !pane.tree_open;
                    // 打开即重建全新树并重新同步高亮（C# TreeToggleBtn_Click
                    // :901-912 show → RebuildTree + SyncTreeIfVisible「打开即
                    // 高亮当前目录」）；根由 folder() 的 init_tree_roots 惰性重建。
                    if pane.tree_open {
                        pane.tree.clear();
                        pane.tree_synced_for = None;
                        pane.tree_reexpand.clear();
                    }
                }
                "hidden" => {
                    pane.show_hidden = !pane.show_hidden;
                    // 隐藏项可见性影响树内容 → 整树重建保展开（C# :1613-1617
                    // → RefreshListKeepingPath :1621-1639 首行 RefreshTreeIfVisible）。
                    tree_mark_dirty(pane);
                    pane.refresh_at = Some(ui.ctx().input(|i| i.time));
                    if let Some(rp) = pane.remote_path.clone() {
                        hooks.list_req.push((side, rp));
                    } else {
                        crate::local::refresh(pane);
                    }
                }
                "up" => {
                    // 上一级（C# UpBtn_Click :803-832）：远程取父目录（根 "/" 不动）；
                    // 本地进父目录，盘符根（GetParent = null）→ NavigateLocalRoot
                    // 回「此电脑」驱动器列表。
                    if let Some(rp) = pane.remote_path.clone() {
                        let parent = match rp.rfind('/') {
                            Some(0) | None => "/".to_owned(),
                            Some(i) => rp[..i].to_owned(),
                        };
                        hooks.list_req.push((side, parent));
                    } else if let Some(cur) = pane.path.clone() {
                        match cur.parent() {
                            Some(parent) => {
                                if crate::local::navigate(pane, parent) {
                                    *hooks.sync_nav =
                                        Some((side, parent.to_string_lossy().into_owned()));
                                }
                            }
                            None => crate::local::navigate_root(pane),
                        }
                    }
                }
                "refresh" => {
                    // 触发刷新动效（列表淡入 + 钮旋转）；远程实际重列随 Listing
                    // 入账（慢到账时再触发下一轮），本地立即生效。
                    pane.refresh_at = Some(ui.ctx().input(|i| i.time));
                    if let Some(rp) = pane.remote_path.clone() {
                        hooks.list_req.push((side, rp));
                    } else {
                        crate::local::refresh(pane);
                    }
                }
                // FV-01（C# OpenDirBtn_Click :835-847）：弹系统文件夹选择框
                // （IFileOpenDialog，起始目录 = 当前目录），选中即跳转 + 同步联动。
                // Rust 版异步：IFileDialog::Show 的模态消息泵不得跑在 egui 主线程
                // （重入 WndProc → wgpu surface 重入 acquire → 崩溃），改专用 STA
                // 线程弹框，回执由 root() 轮询消费。取消 / 失败无动作（回执 None，
                // C# 返回 null 同）；跳转失败保持原状（navigate 内部语义）。空态下
                // 工具栏整体隐藏（pane.empty 提前 return），C# 的 DismissEmptyState
                // 在此为防御冗余。
                "open_dir" => {
                    if hooks.pick.is_none() {
                        // owner 须在 UI 线程点击瞬间取（GetActiveWindow 按线程
                        // 生效，选择线程取不到主窗）。
                        *hooks.pick = Some(crate::local::spawn_pick_folder(
                            pane.path.clone(),
                            crate::local::owner_window(),
                            side,
                        ));
                    }
                }
                // 新建：本地同步创建重列；远程发新建请求（Ev::Created 回执后
                // 重列 + 重建树，对齐 C# CreateRemoteEntryAsync →
                // RefreshListKeepingPath）。
                "new_folder" => {
                    if let Some(rp) = pane.remote_path.clone() {
                        let name = dedupe_remote(pane, t("新建文件夹"));
                        hooks
                            .new_req
                            .push((side, super::join_remote_path(&rp, &name), true));
                    } else {
                        crate::local::create_dir(pane, t("新建文件夹"));
                    }
                }
                "new_file" => {
                    if let Some(rp) = pane.remote_path.clone() {
                        let name = dedupe_remote(pane, t("新建文本文档.txt"));
                        hooks
                            .new_req
                            .push((side, super::join_remote_path(&rp, &name), false));
                    } else {
                        crate::local::create_file(pane, t("新建文本文档.txt"));
                    }
                }
                _ => {}
            }
        }
        x += TOOL_SIZE + 2.0;
    }
}

/// 远程重名规避（序号插扩展名前，同本地 dedupe 规则）：以当前面板条目为准
/// 同步查重（C# ExistsAsync 逐条 STAT 往返的同步镜像，免去网络往返）。
pub(crate) fn dedupe_remote(pane: &FilePane, base: &str) -> String {
    let names: std::collections::HashSet<&str> =
        pane.entries.iter().map(|e| e.name.as_str()).collect();
    if !names.contains(base) {
        return base.to_owned();
    }
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base, ""),
    };
    let mut n = 2;
    loop {
        let cand = format!("{stem} ({n}){ext}");
        if !names.contains(cand.as_str()) {
            return cand;
        }
        n += 1;
    }
}

// ── 表头行 ────────────────────────────────────────────────
fn header_row(
    ui: &mut egui::Ui,
    rect: Rect,
    pane: &mut FilePane,
    st_icons: &mut crate::icons::Cache,
    p: &theme::Palette,
) {
    ui.painter().rect_filled(rect, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.bottom() - 0.5),
            Pos2::new(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, p.row_separator),
    );

    // 表头字体（C# ColumnHeader SemiBold 11 → brand=Inter-SemiBold）。
    let font = FontId::new(HEADER_FONT, egui::FontFamily::Name("brand".into()));
    let content_x = rect.left() + 4.0;
    let cy = rect.center().y;
    for (c, r) in col_rects(pane, content_x) {
        let (label, right_align) = match c {
            Col::Name => (t("名称"), false),
            Col::Size => (t("大小"), true),
            Col::Type => (t("类型"), false),
            Col::Modified => (t("修改时间"), false),
            Col::Permissions => (t("权限"), false),
        };
        let cell = if right_align {
            Rect::from_min_max(
                Pos2::new(r.left() + 6.0, rect.top()),
                Pos2::new(r.right() - 6.0, rect.bottom()),
            )
        } else {
            r
        };
        let anchor = if right_align {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        let tx = if right_align {
            cell.right()
        } else {
            cell.left()
        };

        // 名称列：文字 + 排序箭头（4 间距）；其余列文字单排。
        let arrow_w = if c == active_sort_col(pane) {
            10.0 + 4.0
        } else {
            0.0
        };
        let text_x = if right_align { tx - arrow_w } else { tx };
        ui.painter().text(
            Pos2::new(text_x, cy),
            anchor,
            label,
            font.clone(),
            p.text_primary,
        );

        if c == active_sort_col(pane) {
            let icon = if pane.sort_asc {
                "caret-up"
            } else {
                "caret-down"
            };
            let ax = if right_align {
                tx
            } else {
                tx + galley_w(ui, label, HEADER_FONT) + 4.0
            };
            if let Some(img) = st_icons.get(ui.ctx(), icon, theme::ACCENT, 10.0) {
                img.paint_at(
                    ui,
                    Rect::from_center_size(Pos2::new(ax + 5.0, cy), egui::vec2(10.0, 10.0)),
                );
            }
        }

        // 点击表头排序（列宽拖拽走分隔线 Thumb，见 folder）。
        let resp = ui.allocate_rect(r, egui::Sense::click());
        if resp.clicked() {
            if active_sort_col(pane) == c {
                pane.sort_asc = !pane.sort_asc;
            } else {
                pane.sort_col = sort_key(c);
                pane.sort_asc = true;
            }
            crate::local::apply_sort(pane);
        }
    }
    let _ = st_icons;
}

fn galley_w(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(size), Color32::WHITE)
        .size()
        .x
}

#[derive(Clone, Copy, PartialEq)]
pub enum SortKey {
    Name,
    Size,
    Type,
    Modified,
}

fn sort_key(c: Col) -> SortKey {
    match c {
        Col::Name | Col::Permissions => SortKey::Name,
        Col::Size => SortKey::Size,
        Col::Type => SortKey::Type,
        Col::Modified => SortKey::Modified,
    }
}

fn active_sort_col(p: &FilePane) -> Col {
    match p.sort_col {
        SortKey::Name => Col::Name,
        SortKey::Size => Col::Size,
        SortKey::Type => Col::Type,
        SortKey::Modified => Col::Modified,
    }
}

// ── 文件行 ────────────────────────────────────────────────
fn file_rows(
    ui: &mut egui::Ui,
    rect: Rect,
    pane: &mut FilePane,
    st_icons: &mut crate::icons::Cache,
    sys_icons: &mut crate::icons_sys::Cache,
    p: &theme::Palette,
    hooks: &mut Hooks,
    side: usize,
) {
    let row_font = FontId::proportional(ROW_FONT);
    let content_x = rect.left() + 4.0;
    let row_h = ROW_H; // C# 模板 Padding 4,1 + 图标格 22 = 24

    // FV-25（C# DragOver :3393-3447 + DragItemsStarting DragUI）：跨窗格条目拖拽。
    // 源侧 = 光标旁 caption chip「传输到 {leaf}」（DragDropCaption，leaf = 当前目录
    // 末段，根显示 "/"）；对侧悬停 = DropHint 高亮（填充 0.08 + 描边 2px 圆角 2，
    // C# DropHintFill/DropHintBorder），释放即传输被拖条目（C# Drop →
    // TransferToOppositeAsync）。外部应用拖入 / 本侧自拖不显示任何提示（C#
    // DragOver 判定 Source.OppositeView == this 同口径）。
    let released = ui.input(|i| i.pointer.any_released());
    if let Some(pl) = egui::DragAndDrop::payload::<DragPayload>(ui.ctx()) {
        if pl.side == side {
            if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                let tip = tfmt("传输到 {0}", &[&dir_leaf(pane)]);
                let font = FontId::proportional(ROW_FONT);
                let galley = ui
                    .painter()
                    .layout_no_wrap(tip, font.clone(), p.on_accent_fg);
                let chip = Rect::from_min_size(
                    pos + egui::vec2(14.0, 12.0),
                    galley.size() + egui::vec2(16.0, 8.0),
                );
                ui.painter().rect_filled(chip, 4.0, p.accent);
                ui.painter()
                    .galley(chip.min + egui::vec2(8.0, 4.0), galley, p.on_accent_fg);
            }
        } else if ui.rect_contains_pointer(rect) {
            if released {
                // Drop：取走载荷走批量传输入队（覆盖确认/异步 STAT 复用现有链路）。
                egui::DragAndDrop::clear_payload(ui.ctx());
                *hooks.transfer = Some((side, pl.rows.clone()));
            } else {
                ui.painter()
                    .rect_filled(rect, 2.0, p.accent.gamma_multiply(0.08));
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    Stroke::new(2.0, p.accent),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }

    // 比对标记长度失配 = 任一侧列表已重载，请求重排。
    if pane
        .compare_marks
        .as_ref()
        .is_some_and(|m| m.len() != pane.entries.len())
    {
        *hooks.compare_req = true;
    }

    // 滚动列表（C# ScrollViewer）：内容占位 = 行数×行高，不足视口铺满（空白区右键同 C#）。
    let n = pane.entries.len();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt(("file-rows", side))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let (content_r, _) = ui.allocate_exact_size(
                egui::vec2(rect.width(), (n as f32 * row_h).max(rect.height())),
                Sense::hover(),
            );
            let top = content_r.top();
            let clip = ui.clip_rect();
            let marks = pane
                .compare_marks
                .as_ref()
                .filter(|m| m.len() == pane.entries.len());
            let mut open_idx: Option<usize> = None;
            for (i, e) in pane.entries.iter().enumerate() {
                let mut row = Rect::from_min_max(
                    Pos2::new(rect.left(), top + i as f32 * row_h),
                    Pos2::new(rect.right(), top + (i + 1) as f32 * row_h),
                );
                // 视口外整行跳过绘制（占位仍在，滚动正确）。
                if row.bottom() < clip.top() {
                    continue;
                }
                if row.top() > clip.bottom() {
                    break;
                }
                // 上移入场（对齐 C# WinUI ListView 平台默认 entrance：条目从
                // 下方滑入 + 逐行 stagger，叠加整表渐显）——dy 从 REFRESH_SLIDE
                // 缓收至 0（ease-out cubic），仅动效窗口内生效、逐帧重算无残留；
                // 交互区（allocate_rect）随之偏移，动画结束后归位。
                if let Some(t0) = pane.refresh_at {
                    let el = (ui.ctx().input(|i| i.time) - t0) as f32;
                    if el < REFRESH_ANIM as f32 {
                        let delay = (i as f32 * 0.02).min(0.15);
                        let pi = ((el - delay) / REFRESH_ROW_DUR).clamp(0.0, 1.0);
                        let ease = 1.0 - (1.0 - pi).powi(3);
                        let dy = REFRESH_SLIDE * (1.0 - ease);
                        row.min.y += dy;
                        row.max.y += dy;
                    }
                }
                // 比对占位行：本侧缺此文件（空行与对侧行对齐，C# IsPlaceholder）。
                // 占位行也用 click sense（右键呼空白区菜单，C# 占位行按空白处理）；
                // 左键选中 / 双击进入以 !ph 守卫屏蔽。
                let ph = e.name.is_empty();
                // click_and_drag：左键拖行 = 跨窗格拖拽传输源（FV-25）；短按不动
                // 仍为 click，点击/双击/右键语义不变。
                let resp = ui.allocate_rect(row, Sense::click_and_drag());
                // 选中 / hover 底先画（C# ListView 选中底在容器层）。
                let selected = !ph && pane.selected.contains(&i);
                if selected {
                    ui.painter().rect_filled(row, 0.0, theme::selection());
                } else if !ph {
                    // 行交互色（ListViewItemBackground）：按下 Tertiary 优先于 hover Secondary。
                    if resp.is_pointer_button_down_on() {
                        ui.painter().rect_filled(row, 0.0, p.row_pressed);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(row, 0.0, p.row_hover);
                    }
                }
                // 比对行底色后画：C# 比对 Border 在数据模板内，绘制晚于 ListView 选中底
                // → 比对色盖选中蓝（SystemFillColorCaution/CriticalBackground）。
                match marks.and_then(|m| m.get(i).copied()).unwrap_or(0) {
                    1 => {
                        ui.painter().rect_filled(row, 0.0, p.caution_bg);
                    }
                    2 => {
                        ui.painter().rect_filled(row, 0.0, p.critical_bg);
                    }
                    _ => {}
                }
                if !ph && resp.clicked() {
                    pane.selected = vec![i];
                }
                // 双击目录进入（C# EnterDirectory → EnterLocalDirectory）。
                if !ph && resp.double_clicked() {
                    open_idx = Some(i);
                }
                // FV-25（C# DragItemsStarting :3366 / CanDragItems）：左键拖行发起
                // 跨窗格拖拽；未选中行拖拽先单选该行（WinUI 拖拽跟随 SelectedItems）。
                if !ph && resp.drag_started_by(egui::PointerButton::Primary) {
                    if !pane.selected.contains(&i) {
                        pane.selected = vec![i];
                    }
                    egui::DragAndDrop::set_payload(
                        ui.ctx(),
                        DragPayload {
                            side,
                            rows: pane.selected.clone(),
                        },
                    );
                }
                // 右键呼出菜单（C# BuildItemMenu）：条目行 → 条目菜单；比对占位行
                // 按空白处理（C# RightTapped 占位行走 BuildBlankMenu）。
                // 右键未在选中集 → 切单选；命中已选集保持多选（C# :2405-2407）。
                if resp.secondary_clicked() {
                    if !ph && !pane.selected.contains(&i) {
                        pane.selected = vec![i];
                    }
                    let pos = resp.interact_pointer_pos().unwrap_or(row.center());
                    *hooks.menu = Some(MenuState {
                        pos,
                        pane: side,
                        row: if ph { None } else { Some(i) },
                        queue: None,
                        terminal: false,
                    });
                }

                if ph {
                    continue;
                }

                let cy = row.center().y;

                // 图标格 22 两层叠放（C# FileView.xaml 模板）：系统真实图标 18×18
                // 优先（Image，SystemIconVisible），取不到回退通用字形（FontIcon
                // E188/E7C3 → Phosphor folder/file，均默认主色）。
                let cell = Rect::from_center_size(
                    Pos2::new(content_x + ICON_CELL / 2.0, cy),
                    egui::vec2(ICON_CELL, ICON_CELL),
                );
                // 系统图标键：本地根视图盘符条目 full=e.name（"C:\" 命中盘符缓存）；
                // 普通本地条目 full=父目录拼名；远程条目按文件名扩展名解析。
                let is_local = pane.remote_path.is_none();
                let full = if is_local {
                    match &pane.path {
                        Some(dir) => dir.join(&e.name).to_string_lossy().into_owned(),
                        None => e.name.clone(),
                    }
                } else {
                    String::new()
                };
                let glyph = if e.is_dir { "folder" } else { "file" };
                let sys_img = sys_icons.get(ui.ctx(), e.is_dir, &e.name, &full, is_local, ROW_ICON);
                let img =
                    sys_img.or_else(|| st_icons.get(ui.ctx(), glyph, p.text_primary, ROW_ICON));
                if let Some(img) = img {
                    img.paint_at(
                        ui,
                        Rect::from_center_size(cell.center(), egui::vec2(ROW_ICON, ROW_ICON)),
                    );
                }

                for (c, r) in col_rects(pane, content_x) {
                    let (text, align_right, color) = match c {
                        Col::Name => (e.name.clone(), false, p.text_primary),
                        Col::Size => (e.size.clone(), true, p.text_secondary),
                        Col::Type => (e.kind.clone(), false, p.text_secondary),
                        Col::Modified => (e.modified.clone(), false, p.text_secondary),
                        Col::Permissions => (e.perms.clone(), false, p.text_secondary),
                    };
                    let cell_r = if align_right {
                        Rect::from_min_max(
                            Pos2::new(r.left() + 6.0, row.top()),
                            Pos2::new(r.right() - 6.0, row.bottom()),
                        )
                    } else {
                        Rect::from_min_max(
                            Pos2::new(r.left(), row.top()),
                            Pos2::new(r.right(), row.bottom()),
                        )
                    };
                    let anchor = if align_right {
                        Align2::RIGHT_CENTER
                    } else {
                        Align2::LEFT_CENTER
                    };
                    let tx = if align_right {
                        cell_r.right()
                    } else {
                        cell_r.left()
                    };
                    // 名称/类型列按列宽单行截断（C# TextTrimming=CharacterEllipsis）；
                    // 大小/时间列短格式不截断（C# 同）。条目无 tooltip（C# 模板未挂）。
                    let text = match c {
                        Col::Name | Col::Type => ellipsize(ui, &text, &row_font, cell_r.width()),
                        _ => text,
                    };
                    ui.painter()
                        .text(Pos2::new(tx, cy), anchor, text, row_font.clone(), color);
                }
            }

            // 双击目录：进入（本地直读；远程发列目录请求）。
            if let Some(i) = open_idx
                && let Some(e) = pane.entries.get(i)
            {
                if let Some(rp) = &pane.remote_path {
                    let target = if rp == "/" {
                        format!("/{}", e.name)
                    } else {
                        format!("{rp}/{}", e.name)
                    };
                    hooks.list_req.push((side, target));
                } else if pane.path.is_none() {
                    // 本地根视图（此电脑）：条目即盘符 "C:\"，直接按盘符导航
                    // （原实现 path=None 时双击盘符无任何动作 = 点不开）。
                    let target = std::path::PathBuf::from(e.name.as_str());
                    if crate::local::navigate(pane, &target) {
                        *hooks.sync_nav = Some((side, target.to_string_lossy().into_owned()));
                    }
                } else if let Some(base) = pane.path.clone() {
                    let target = base.join(&e.name);
                    if crate::local::navigate(pane, &target) {
                        *hooks.sync_nav = Some((side, target.to_string_lossy().into_owned()));
                    }
                }
            }

            // 行下方空白区右键：新建/刷新菜单（C# BuildBlankMenu）；左键拖动 =
            // 框选橡皮筋（FV-24，C# OnRubberBandPointerPressed :2271，条目上按下
            // 交常规选择/拖拽故不触发）。
            let blank = Rect::from_min_max(
                Pos2::new(rect.left(), top + n as f32 * row_h),
                Pos2::new(rect.right(), content_r.bottom()),
            );
            if blank.height() > 0.0 {
                let resp = ui.allocate_rect(blank, Sense::click_and_drag());
                if resp.secondary_clicked() {
                    let pos = resp.interact_pointer_pos().unwrap_or(blank.center());
                    *hooks.menu = Some(MenuState {
                        pos,
                        pane: side,
                        row: None,
                        queue: None,
                        terminal: false,
                    });
                }
                rubber_band(ui, pane, &resp, top, rect, row_h, clip);
            }
        });

    // 刷新淡入（比 C# 更好的刷新表现）：触发/入账后 REFRESH_ANIM 秒内以容器
    // 底色遮罩缓收渐显（ease-out），列表「动」起来可感知刷新已生效；动画期间
    // 逐帧请求重绘（egui 无事件不重绘），结束帧清旗后自然停止。
    if let Some(t0) = pane.refresh_at {
        let now = ui.ctx().input(|i| i.time);
        let el = now - t0;
        if el >= REFRESH_ANIM {
            pane.refresh_at = None;
        } else {
            let k = (1.0 - el / REFRESH_ANIM) as f32; // 1 → 0
            let a = (k * k * 200.0) as u8; // ease-out：起始 ≈78% 遮罩
            let bg = p.folder_area;
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), a),
            );
            ui.ctx().request_repaint();
        }
    }
}

// ── 框选橡皮筋（FV-24；C# FileView.xaml.cs:2271-2388 1:1）─────────────────

/// 空白处按住左键拖出选择框：框内条目多选（与资源管理器一致）；Ctrl = 框选前
/// 基准 ∪ 框内；按下即应用（无修饰点击空白 = 清除选择，:2300）；反方向拖按
/// min/abs 归一矩形；命中 = 矩形与行矩形任意非空交集；比对占位行排除、滚出
/// 视口行不参与（C# 虚拟化同口径）；松开结束（C# Released/Canceled 同处理）。
fn rubber_band(
    ui: &mut egui::Ui,
    pane: &mut FilePane,
    resp: &egui::Response,
    top: f32,
    list: Rect,
    row_h: f32,
    clip: Rect,
) {
    let primary = egui::PointerButton::Primary;
    let stopped = resp.drag_stopped_by(primary);
    if stopped {
        pane.rubber = None;
        return;
    }
    let started = resp.drag_started_by(primary);
    if started {
        // Ctrl 加选先快照当前选择（C# _rubberBase.AddRange(SelectedItems)）。
        let additive = ui.input(|i| i.modifiers.ctrl);
        pane.rubber = Some(RubberState {
            origin: resp.interact_pointer_pos().unwrap_or(list.left_top()),
            additive,
            base: if additive {
                pane.selected.clone()
            } else {
                Vec::new()
            },
        });
    }
    // 拖动帧（含按下帧）应用选择并绘制矩形。
    if !(resp.dragged_by(primary) || started) {
        return;
    }
    let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    let Some(rub) = pane.rubber.as_ref() else {
        return;
    };
    let band = Rect::from_two_pos(rub.origin, pos);
    let (additive, base) = (rub.additive, rub.base.clone());
    // 命中应用（C# ApplyRubberSelection :2340）。
    let mut hit = Vec::new();
    for (i, e) in pane.entries.iter().enumerate() {
        if e.name.is_empty() {
            continue; // 比对占位行（IsPlaceholder）
        }
        let row = Rect::from_min_max(
            Pos2::new(list.left(), top + i as f32 * row_h),
            Pos2::new(list.right(), top + (i + 1) as f32 * row_h),
        );
        if row.bottom() < clip.top() || row.top() > clip.bottom() {
            continue; // 虚拟化未实例化
        }
        if band.intersects(row) {
            hit.push(i);
        }
    }
    pane.selected = if additive {
        let mut v = base;
        v.extend(hit);
        v.sort_unstable();
        v.dedup();
        v
    } else {
        hit
    };
    // 橡皮筋矩形（C# RubberBand：AccentFill 填充 + 1px 同色描边、圆角 2、
    // Opacity 0.35，xaml:567-576）。
    let p = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(band, 2.0, p.accent.gamma_multiply(0.35));
    ui.painter().rect_stroke(
        band,
        2.0,
        Stroke::new(1.0, p.accent),
        egui::StrokeKind::Inside,
    );
}

/// 当前目录末段显示名（C# DragDropCaption leaf：_currentPath 去尾分隔后末段，
/// 根显示 "/"；本地「此电脑」根同 "/"）。
fn dir_leaf(pane: &FilePane) -> String {
    if let Some(rp) = &pane.remote_path {
        return if rp == "/" {
            "/".to_owned()
        } else {
            rp.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("/")
                .to_owned()
        };
    }
    match &pane.path {
        Some(p) => p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string_lossy().into_owned()),
        None => "/".to_owned(),
    }
}

// ── 状态栏 ────────────────────────────────────────────────
fn status_bar(ui: &mut egui::Ui, rect: Rect, pane: &FilePane, p: &theme::Palette) {
    ui.painter().rect_filled(rect, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 0.5),
            Pos2::new(rect.right(), rect.top() + 0.5),
        ],
        Stroke::new(1.0, p.row_separator),
    );
    let dirs = pane.entries.iter().filter(|e| e.is_dir).count();
    let files = pane.entries.len() - dirs;
    // 选中数（C# UpdateStatus :4515：SelectedItems 计数、排除比对占位行）。
    let sel_n = pane
        .selected
        .iter()
        .filter(|&&i| pane.entries.get(i).is_some_and(|e| !e.name.is_empty()))
        .count();
    let sel = if sel_n > 0 {
        format!("  ·  {}", tfmt("已选 {0} 项", &[&sel_n]))
    } else {
        String::new()
    };
    let load = if pane.loading {
        format!("  ·  {}", t("加载中…"))
    } else {
        String::new()
    };
    ui.painter().text(
        Pos2::new(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!(
            "{}, {}{sel}{load}",
            tfmt("{0} 个目录", &[&dirs]),
            tfmt("{0} 个文件", &[&files])
        ),
        FontId::proportional(ROW_FONT),
        p.text_secondary,
    );
    if let Some(m) = &pane.msg {
        ui.painter().text(
            Pos2::new(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            m,
            FontId::proportional(ROW_FONT),
            p.text_secondary,
        );
    }
}

// ── 空态引导（C# EmptyStatePanel）────────────────────────
fn empty_state(
    ui: &mut egui::Ui,
    rect: Rect,
    st_icons: &mut crate::icons::Cache,
    p: &theme::Palette,
    hooks: &mut Hooks,
    side: usize,
    texts: (&str, &str),
) {
    let (title, sub) = texts;
    ui.painter().rect_filled(rect, 0.0, p.folder_area);

    // 垂直位置按容器高度 40%（C# EmptyContent Margin）。
    let top = rect.top() + rect.height() * 0.40;
    let cx = rect.center().x;

    // 图标块 56×56 r11（C# 空态卡底覆盖 CardBackgroundFillColorSecondary）。
    let block = Rect::from_center_size(Pos2::new(cx, top + 28.0), egui::vec2(56.0, 56.0));
    ui.painter().rect_filled(block, 11.0, p.empty_card_bg);
    if let Some(img) = st_icons.get(ui.ctx(), "compass", theme::ACCENT, 23.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(block.center(), egui::vec2(23.0, 23.0)),
        );
    }

    // 标题 / 副标题 / 按钮位置按 C# StackPanel Spacing（中心 84 / 110，按钮顶 140）。
    let title_font = FontId::new(18.0, egui::FontFamily::Name("brand".into()));
    ui.painter().text(
        Pos2::new(cx, top + 84.0),
        Align2::CENTER_CENTER,
        title,
        title_font,
        p.text_primary,
    );
    ui.painter().text(
        Pos2::new(cx, top + 110.0),
        Align2::CENTER_CENTER,
        sub,
        FontId::proportional(ROW_FONT),
        p.text_secondary,
    );

    // 蓝色主按钮（C# EmptyStateBtn：Padding 24,8，图标 14 + 文字）。
    let y = top + 140.0;
    let label = t("选择主机");
    let btn_font = FontId::proportional(14.0);
    let text_w = galley_w(ui, label, 14.0);
    let btn_w = 24.0 + 14.0 + 8.0 + text_w + 24.0;
    let btn_h = 8.0 * 2.0 + 20.0;
    let btn = Rect::from_min_size(Pos2::new(cx - btn_w / 2.0, y), egui::vec2(btn_w, btn_h));
    let resp = ui.allocate_rect(btn, egui::Sense::click());
    let bg = if resp.is_pointer_button_down_on() {
        theme::ACCENT_PRESSED
    } else if resp.hovered() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    ui.painter().rect_filled(btn, 4.0, bg);
    if let Some(img) = st_icons.get(ui.ctx(), "plus", Color32::WHITE, 14.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(
                Pos2::new(btn.left() + 24.0 + 7.0, btn.center().y),
                egui::vec2(14.0, 14.0),
            ),
        );
    }
    ui.painter().text(
        Pos2::new(btn.left() + 24.0 + 14.0 + 8.0, btn.center().y),
        Align2::LEFT_CENTER,
        label,
        btn_font,
        Color32::WHITE,
    );
    if resp.clicked() {
        *hooks.dialog = Dialog::HostPicker {
            target: side,
            new_tab: false,
        };
    }
}

// ── 队列 / 终端（C# QueueContainer / TerminalContainer）──

use crate::transfer::{Status as TStatus, VerifyKind};

/// 队列第四列状态语义（C# TransferItem.StatusText）：
/// 等待→「等待中」（全局暂停「已暂停」）；传输中→模式前缀+实时速度（无回报仅模式文案）；
/// 校验中→「校验中」；完成/失败→校验结论前缀+时刻；已取消→「已取消」。
fn queue_status_text(i: &crate::transfer::Item, paused: bool) -> String {
    match i.status {
        TStatus::Waiting => {
            if paused {
                t("已暂停").to_owned()
            } else {
                t("等待中").to_owned()
            }
        }
        TStatus::Transferring => {
            if i.speed > 0 {
                if i.resumed {
                    format!("{} {}/s", t("续传中"), crate::local::format_size(i.speed))
                } else {
                    format!("{}/s", crate::local::format_size(i.speed))
                }
            } else if i.phase == 1 {
                // tar 阶段旗标：服务端打包/解包无进度回报，无实时速度时显示阶段
                // 文案（流式上传期间有速度回报，速度列照常）。
                t("服务端打包中").to_owned()
            } else if i.phase == 2 {
                t("解包中").to_owned()
            } else if i.resumed {
                t("续传中").to_owned()
            } else {
                t("传输中").to_owned()
            }
        }
        TStatus::Verifying => t("校验中").to_owned(),
        // C# VerifyTail：结论文案 + " " + 时刻（G12 校验结论前缀）。
        TStatus::Done => match i.verify_result {
            VerifyKind::Match => format!("{} {}", t("已校验"), i.time),
            VerifyKind::Skipped => format!("{} {}", t("校验跳过"), i.time),
            _ => i.time.clone(),
        },
        TStatus::Failed => match i.verify_result {
            VerifyKind::Mismatch => format!("{} {}", t("校验不匹配"), i.time),
            _ => i.time.clone(),
        },
        TStatus::Cancelled => t("已取消").to_owned(),
    }
}

/// 单行文本按像素宽截断（… 结尾）。
fn ellipsize(ui: &egui::Ui, text: &str, font: &FontId, max_w: f32) -> String {
    if text.is_empty() || max_w <= 12.0 {
        return text.to_owned();
    }
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE);
    if galley.size().x <= max_w {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(2).collect();
    for c in text.chars().skip(2) {
        let trial = format!("{cut}{c}…");
        let g = ui
            .painter()
            .layout_no_wrap(trial.clone(), font.clone(), Color32::WHITE);
        if g.size().x > max_w {
            break;
        }
        cut = format!("{cut}{c}");
    }
    format!("{cut}…")
}

/// 传输队列（C# QueueContainer）：表头 5 列（源/目标/大小/速度或时间/原因，失败页签
/// 才有原因列）+ 按页签过滤的行 + foot 三页签（队列中=等待+传输中+已取消 / 失败 / 完成）。
pub fn queue(ui: &mut egui::Ui, rect: Rect, st: &mut super::State) {
    let p = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(rect, theme::CONTAINER_RADIUS, p.folder_area);
    let inner = rect.shrink2(egui::vec2(1.0, 1.0));
    let font = FontId::proportional(12.0);

    // 队列快照（每帧整表重绘；单 worker 无高频事件，直读即一致）。
    let (items, paused) = match &st.transfer {
        Some(q) => (q.with_items(|all| all.to_vec()), q.paused()),
        None => (Vec::new(), false),
    };
    let in_active = |s: TStatus| {
        matches!(
            s,
            TStatus::Waiting | TStatus::Transferring | TStatus::Verifying | TStatus::Cancelled
        )
    };
    let n_active = items.iter().filter(|i| in_active(i.status)).count();
    let n_failed = items.iter().filter(|i| i.status == TStatus::Failed).count();
    let n_done = items.iter().filter(|i| i.status == TStatus::Done).count();

    // 表头（Padding 12,5 + 底线）：源 2* / 目标 2* / 大小 90 / 速度或时间 90 / 原因（仅失败页签）。
    let header_h = 5.0 + 16.0 + 5.0 + 1.0; // C# 表头 Padding 12,5 + 12px 行高 ≈16 + 底线 1 = 27
    let header = Rect::from_min_max(
        inner.left_top(),
        Pos2::new(inner.right(), inner.top() + header_h),
    );
    ui.painter().rect_filled(header, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(inner.left(), header.bottom() - 0.5),
            Pos2::new(inner.right(), header.bottom() - 0.5),
        ],
        Stroke::new(1.0, p.separator),
    );
    let hy = header.center().y;
    let reason_w = if st.queue_tab == 1 { 90.0 } else { 0.0 };
    let fixed = 90.0 + 90.0 + reason_w;
    let half = ((inner.width() - 24.0 - fixed - 48.0) / 2.0).max(40.0);
    let x0 = inner.left() + 12.0;
    let xs = [
        x0,
        x0 + half + 12.0,
        x0 + half * 2.0 + 24.0,
        x0 + half * 2.0 + 24.0 + 90.0 + 12.0,
        x0 + half * 2.0 + 24.0 + 180.0 + 24.0,
    ];
    let heads = [t("源"), t("目标"), t("大小"), t("速度"), t("原因")];
    for (k, label) in heads.iter().enumerate() {
        if k == 4 && st.queue_tab != 1 {
            continue;
        }
        ui.painter().text(
            Pos2::new(xs[k], hy),
            Align2::LEFT_CENTER,
            *label,
            font.clone(),
            p.text_secondary,
        );
        // 列分割线（C# 仅表头内，Width 1 Margin right -6.5 → 中心=列右+6；第 4 条仅失败页签）。
        if k < 3 || st.queue_tab == 1 {
            let dx = xs[k] + if k < 2 { half } else { 90.0 } + 6.0;
            ui.painter().line_segment(
                [Pos2::new(dx, header.top()), Pos2::new(dx, header.bottom())],
                Stroke::new(1.0, p.divider_stroke),
            );
        }
    }

    // 行：按页签过滤（队列中=等待+传输中+已取消 / 失败 / 完成），行高 26（Padding 12,5 + 行高 ≈16）。
    let row_h = 26.0;
    let mut y = header.bottom();
    let list_bottom = inner.bottom() - 29.0; // foot 页签栏（顶线 1 + Padding 6,2 + 页签 24 = 29）
    let mut hit_row: Option<(Pos2, u64)> = None;
    for i in &items {
        let show = match st.queue_tab {
            0 => in_active(i.status),
            1 => i.status == TStatus::Failed,
            _ => i.status == TStatus::Done,
        };
        if !show {
            continue;
        }
        if y + row_h > list_bottom {
            break; // 视口裁剪（无滚动，超出丢弃；C# ListView 自滚动随后续任务）
        }
        let row = Rect::from_min_max(
            Pos2::new(inner.left(), y),
            Pos2::new(inner.right(), y + row_h),
        );
        let resp = ui.allocate_rect(row, Sense::click());
        if resp.secondary_clicked() {
            hit_row = Some((resp.interact_pointer_pos().unwrap_or(row.center()), i.id));
        }
        let cy = row.center().y;
        let w_src = half;
        for (k, text) in [
            ellipsize(ui, &i.source, &font, w_src),
            ellipsize(ui, &i.dest, &font, half),
            if i.size == 0 {
                "—".to_owned()
            } else {
                crate::local::format_size(i.size)
            },
            queue_status_text(i, paused),
            ellipsize(ui, &i.reason, &font, 90.0),
        ]
        .into_iter()
        .enumerate()
        {
            if k == 4 && st.queue_tab != 1 {
                continue;
            }
            // 行文字默认前景 = 主色（C# DataTemplate 未覆写 Foreground）。
            let color = p.text_primary;
            ui.painter().text(
                Pos2::new(xs[k], cy),
                Align2::LEFT_CENTER,
                text,
                font.clone(),
                color,
            );
        }
        y += row_h;
    }

    // 空白区/表头右键：队列菜单（C# Root_RightTapped；队列空不弹）。
    if !items.is_empty() {
        let blank = Rect::from_min_max(
            Pos2::new(inner.left(), y),
            Pos2::new(inner.right(), list_bottom),
        );
        for zone in [header, blank] {
            if zone.height() <= 0.0 {
                continue;
            }
            let resp = ui.allocate_rect(zone, Sense::click());
            if resp.secondary_clicked() {
                let pos = resp.interact_pointer_pos().unwrap_or(zone.center());
                st.menu = Some(super::dialogs::MenuState {
                    pos,
                    pane: 0,
                    row: None,
                    queue: Some(super::dialogs::QueueMenu::Batch),
                    terminal: false,
                });
            }
        }
    }
    if let Some((pos, id)) = hit_row {
        st.menu = Some(super::dialogs::MenuState {
            pos,
            pane: 0,
            row: None,
            queue: Some(super::dialogs::QueueMenu::Item(id)),
            terminal: false,
        });
    }

    // foot 页签栏（Padding 6,2 + 顶线）：队列中 n / 传输失败 n / 传输完成 n。
    let foot = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - 29.0),
        Pos2::new(inner.right(), inner.bottom()),
    );
    ui.painter().rect_filled(foot, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(foot.left(), foot.top() + 0.5),
            Pos2::new(foot.right(), foot.top() + 0.5),
        ],
        Stroke::new(1.0, p.separator),
    );
    let tabs = [
        (t("队列中"), n_active),
        (t("传输失败"), n_failed),
        (t("传输完成"), n_done),
    ];
    let mut tx = foot.left() + 6.0;
    for (k, (label, count)) in tabs.iter().enumerate() {
        let sel = st.queue_tab == k;
        let text_w = galley_w(ui, label, 12.0);
        let count_s = format!(" {count}");
        let count_w = galley_w(ui, &count_s, 12.0);
        // 页签（C# QTabStyle：Border Padding 10,4 CornerRadius 4 高 24，顶线+Padding 后顶=+3）。
        let w = text_w + 4.0 + count_w + 20.0;
        let r = Rect::from_min_size(Pos2::new(tx, foot.top() + 3.0), egui::vec2(w, 24.0));
        let resp = ui.allocate_rect(r, Sense::click());
        if resp.hovered() {
            // hover = SubtleFillColorSecondary（C# RadioButton 模板 PointerOver 底）。
            ui.painter().rect_filled(r, 4.0, p.row_hover);
        }
        // 选中 label = 强调色 + SemiBold（C# Tab_Checked 切换 VisualState）。
        let fg = if sel { p.accent } else { p.text_secondary };
        let cg = if sel { p.accent } else { p.text_tertiary };
        let sel_font = FontId::new(12.0, egui::FontFamily::Name("brand".into()));
        ui.painter().text(
            Pos2::new(r.left() + 10.0, r.center().y),
            Align2::LEFT_CENTER,
            *label,
            if sel { sel_font } else { font.clone() },
            fg,
        );
        ui.painter().text(
            Pos2::new(r.left() + 10.0 + text_w + 4.0, r.center().y),
            Align2::LEFT_CENTER,
            count_s,
            font.clone(),
            cg,
        );
        if resp.clicked() {
            st.queue_tab = k;
        }
        tx += w + 4.0;
    }
}

/// 终端（C# TerminalContainer）：TerminalBackgroundBrush 卡底 + [hh:mm:ss] 时间戳日志，
/// 末尾对齐（显示最后 N 条）；正文 TextWrapping=Wrap 按宽换行，续行对齐正文列；
/// 每条日志 Grid Margin bottom 7、列距 8；右键菜单 清除全部/复制全部。
pub fn terminal(ui: &mut egui::Ui, rect: Rect, st: &mut super::State) {
    let p = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(rect, theme::CONTAINER_RADIUS, p.terminal_bg);
    let inner = rect.shrink2(egui::vec2(12.0, 8.0)); // C# Padding 12,8
    let mono = FontId::monospace(11.0); // C# Cascadia Mono 11
    let lh = 15.0;
    let slot_pad = 7.0; // 每条日志 Margin bottom 7

    // 末尾对齐：从尾部逐条排版累计槽高，直到铺满视口（C# ListView 滚动到底同表现）。
    // 每条槽高 ≥ lh+7，按 lh 估条数必覆盖视口。
    let est = ((inner.height() / lh).ceil() as usize).max(1);
    let from = st.terminal.len().saturating_sub(est);
    let mut laid: Vec<(
        f32,
        std::sync::Arc<egui::Galley>,
        f32,
        std::sync::Arc<egui::Galley>,
    )> = Vec::new();
    let mut y = inner.bottom();
    for (time, text) in st.terminal[from..].iter().rev() {
        let time_g =
            ui.painter()
                .layout_no_wrap(format!("[{time}]"), mono.clone(), p.terminal_time);
        // 时间列 Auto 宽实测，正文列 = 时间列右 + ColumnSpacing 8。
        let text_x = inner.left() + time_g.size().x + 8.0;
        let wrap_w = (inner.right() - text_x).max(40.0);
        let body_g = ui
            .painter()
            .layout(text.clone(), mono.clone(), p.terminal_text, wrap_w);
        let slot = body_g.size().y.max(lh) + slot_pad;
        if y - slot < inner.top() {
            break;
        }
        y -= slot;
        laid.push((y, time_g, text_x, body_g));
    }
    // laid 逆序（最早条最后 push），反转后自上而下绘制。
    for (top, time_g, text_x, body_g) in laid.into_iter().rev() {
        ui.painter()
            .galley(Pos2::new(inner.left(), top), time_g, p.terminal_time);
        ui.painter()
            .galley(Pos2::new(text_x, top), body_g, p.terminal_text);
    }

    // 连接 loading 圈（C# TerminalContainer.xaml BusyRing）：18×18 ProgressRing
    // 悬浮右上角（Margin 0,9,12,0 → 右缩 12、顶缩 9，右/顶对齐）。C# 连接前置位
    // SetBusy(true)、finally 复位，覆盖含指纹确认在内的整个连接过程；出现/消失
    // 即时无淡入淡出，「动」即圈自转。Rust 驱动条件 RemoteState::Connecting 与
    // C# busy 生命周期同构（dialogs 连接发起置位、Ev::Connected/Failed 复位）；
    // egui Spinner 自带匀速旋转且 paint_at 内部每帧 request_repaint 保活。
    // 任一侧会话处于连接中即显示（两侧各自连接过程独立驱动本侧圈）。
    if st.remotes.iter().any(|r| {
        r.sessions
            .iter()
            .any(|s| matches!(s.state, super::RemoteState::Connecting))
    }) {
        let ring = Rect::from_min_size(
            Pos2::new(rect.right() - 12.0 - 18.0, rect.top() + 9.0),
            egui::vec2(18.0, 18.0),
        );
        egui::Spinner::new()
            .size(18.0)
            .color(p.accent)
            .paint_at(ui, ring);
    }

    // 右键菜单（C# TerminalMenuFlyout：清除全部 / 复制全部）。
    let resp = ui.allocate_rect(inner, Sense::click());
    if resp.secondary_clicked() {
        let pos = resp.interact_pointer_pos().unwrap_or(inner.center());
        st.menu = Some(super::dialogs::MenuState {
            pos,
            pane: 0,
            row: None,
            queue: None,
            terminal: true,
        });
    }
}

// ── 目录树侧栏（C# 树形目录侧栏：懒展开 / 单击导航 / ShowHidden 过滤）──

/// 树根初始化：本地 = 驱动器列表；远程视图（任一侧远程 tab）= "/"（子级懒展开）。
fn init_tree_roots(pane: &mut FilePane, hooks: &mut Hooks) {
    if pane.remote_path.is_some() {
        pane.tree = vec![TreeNode::new("/", "/")];
    } else {
        pane.tree = crate::local::list_drives()
            .iter()
            .map(|e| TreeNode::new(&e.name, &e.name))
            .collect();
    }
    let _ = hooks;
}

/// 树节点递归查找（按完整路径）。
fn find_node_mut<'a>(nodes: &'a mut Vec<TreeNode>, path: &str) -> Option<&'a mut TreeNode> {
    for n in nodes {
        if n.path == path {
            return Some(n);
        }
        if let Some(hit) = find_node_mut(&mut n.children, path) {
            return Some(hit);
        }
    }
    None
}

/// 树动作（路径, 是否折叠切换；false = 导航）。
type TreeAction = (String, bool);

/// 树回调：caret 点击折叠/展开（本地同步枚举 / 远程置 loading + tree_req），
/// 行点击导航。
fn tree_apply(pane: &mut FilePane, hooks: &mut Hooks, side: usize, path: &str, toggle: bool) {
    // 远程视图（pane.remote_path Some，侧 0 远程 tab 同口径）走远程分支。
    if !toggle {
        if pane.remote_path.is_some() {
            hooks.list_req.push((side, path.to_owned()));
        } else if crate::local::navigate(pane, std::path::Path::new(path)) {
            *hooks.sync_nav = Some((side, path.to_owned()));
        }
        return;
    }
    let Some(n) = find_node_mut(&mut pane.tree, path) else {
        return;
    };
    n.expanded = !n.expanded;
    if n.expanded && n.children.is_empty() && !n.loading {
        if pane.remote_path.is_some() {
            n.loading = true;
            *hooks.tree_req = Some((side, path.to_owned()));
        } else {
            n.children = tree_load_local(path, pane.show_hidden);
            n.loaded = true;
            if n.children.is_empty() {
                n.expanded = false; // 空目录 / 不可读：弹回合上（C# 同表现）
            }
        }
    }
}

/// 本地子目录懒枚举（只取目录；ShowHidden 位 0x2 过滤；名称排序）。
fn tree_load_local(path: &str, show_hidden: bool) -> Vec<TreeNode> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if !show_hidden
                && let Ok(meta) = e.metadata()
                && crate::local::is_hidden(&e.file_name().to_string_lossy(), &meta)
            {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let full = e.path().to_string_lossy().into_owned();
            out.push(TreeNode::new(&name, &full));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 远程树懒展开回填（Ev::Listing 命中 tree_req 时调用）；只留目录，POSIX 拼路径；
/// ShowHidden 关时过滤点文件（C# DirTree_Expanding :2206-2208）。
pub(crate) fn apply_tree_listing(pane: &mut FilePane, path: &str, entries: &[Entry]) {
    let mut dirs: Vec<TreeNode> = entries
        .iter()
        .filter(|e| e.is_dir && (pane.show_hidden || !e.name.starts_with('.')))
        .map(|e| {
            // 收敛到 transfer::join_remote 唯一实现（原为同代码复制，2026-11-06）
            let full = crate::transfer::join_remote(&path, &e.name);
            TreeNode::new(&e.name, &full)
        })
        .collect();
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(n) = find_node_mut(&mut pane.tree, path) {
        n.children = dirs;
        n.loading = false;
        n.loaded = true; // 子级已枚举（含真空目录），同步步不再重发请求
        n.expanded = true;
    }
}

/// 树请求失败复位（Ev::Failed：loading 节点弹回合上，可再次展开）。
pub(crate) fn reset_tree_loading(pane: &mut FilePane) {
    fn walk(nodes: &mut [TreeNode]) {
        for n in nodes {
            if n.loading {
                n.loading = false;
                n.expanded = false;
            }
            walk(&mut n.children);
        }
    }
    walk(&mut pane.tree);
}

// ── 树同步（C# SyncTreeIfVisible / ExpandTreePathAsync / RebuildTreePreserving 镜像）──

/// 树同步单层结果。
#[derive(PartialEq)]
enum Walk {
    /// 该层已就绪（children 已枚举），可继续下行。
    Ok,
    /// 断链（节点不存在 / 目录确认空）：停在最近可达层（C# ExpandTreePathAsync
    /// :1035 break）。
    Dead,
    /// 远程枚举在途 / 请求已发出：下一帧续走（C# WaitForTreeChildrenAsync 等待）。
    Wait,
}

/// 当前位置对应树路径（远程 = remote_path；本地 = 路径串；None = 「此电脑」
/// 根态，对应 C# _currentPath 空串）。
fn pane_pos(pane: &FilePane) -> Option<String> {
    pane.remote_path
        .clone()
        .or_else(|| pane.path.as_ref().map(|p| p.to_string_lossy().into_owned()))
}

/// 收集已展开节点路径（含根；C# RebuildTreePreservingAsync Walk :940-953）。
fn collect_expanded(nodes: &[TreeNode], out: &mut Vec<String>) {
    for n in nodes {
        if n.expanded {
            out.push(n.path.clone());
            collect_expanded(&n.children, out);
        }
    }
}

/// 树结构失效（ShowHidden 切换 / 新建 / 删除 / 重命名后调用）：整树重建、保留
/// 已展开链（C# RefreshTreeIfVisible :927-934 → RebuildTreePreservingAsync
/// :936-968 收集 + 逐条 ExpandTreePathAsync(select:false, expandTarget:true)）。
/// 根由 folder() 的 init_tree_roots 惰性重建；高亮基准清除后随同步步重定位。
pub(crate) fn tree_mark_dirty(pane: &mut FilePane) {
    if !pane.tree_open || pane.tree.is_empty() {
        return;
    }
    let mut expanded = Vec::new();
    collect_expanded(&pane.tree, &mut expanded);
    pane.tree.clear();
    pane.tree_reexpand = expanded;
    pane.tree_synced_for = None;
}

/// 同步失败收尾（Ev::Failed）：位置入账为已同步、放弃未完成的展开恢复——防
/// 失败后每帧重发（C# WaitForTreeChildrenAsync :1060-1075 3 秒超时放弃同效）。
pub(crate) fn tree_sync_give_up(pane: &mut FilePane) {
    pane.tree_synced_for = pane_pos(pane);
    pane.tree_reexpand.clear();
}

/// 确保单层就绪（children 已枚举并展开）。
fn ensure_level(pane: &mut FilePane, hooks: &mut Hooks, side: usize, path: &str) -> Walk {
    let Some(n) = find_node_mut(&mut pane.tree, path) else {
        return Walk::Dead;
    };
    if n.loaded || !n.children.is_empty() {
        n.expanded = true;
        return Walk::Ok;
    }
    if pane.remote_path.is_some() {
        if n.loading || hooks.tree_req.is_some() {
            return Walk::Wait;
        }
        n.loading = true;
        n.expanded = true;
        *hooks.tree_req = Some((side, path.to_owned()));
        Walk::Wait
    } else {
        // 本地同步枚举；空目录 = 无法再下行（断链）。
        n.children = tree_load_local(path, pane.show_hidden);
        n.loaded = true;
        if n.children.is_empty() {
            n.expanded = false;
            Walk::Dead
        } else {
            n.expanded = true;
            Walk::Ok
        }
    }
}

/// 自根向 target 逐段下行（C# ExpandTreePathAsync :974-1051）：路径上各层依次
/// 枚举并展开；expand_target = false 时目标层仅定位不展开（:1043-1050，只充当
/// 高亮基准）。返回 Wait = 懒加载在途，下一帧续走。
fn tree_walk(
    pane: &mut FilePane,
    hooks: &mut Hooks,
    side: usize,
    target: &str,
    expand_target: bool,
) -> Walk {
    let remote = pane.remote_path.is_some();
    let sep = if remote { '/' } else { '\\' };
    // 根定位：远程恒 "/"；本地取盘符前缀（C# rootKey = path[..3]，如 "C:\"）。
    let mut cur = if remote {
        "/".to_owned()
    } else {
        match target.find(sep) {
            Some(i) => target[..=i].to_owned(),
            None => target.to_owned(),
        }
    };
    match ensure_level(pane, hooks, side, &cur) {
        Walk::Wait => return Walk::Wait,
        Walk::Dead => return Walk::Ok,
        Walk::Ok => {}
    }
    let mut remain = &target[cur.len()..];
    while !remain.is_empty() {
        let (seg, last) = match remain.split_once(sep) {
            Some((s, _)) => (s, false),
            None => (remain, true),
        };
        // 拼全路径（远程：根 "/" 直连；本地：盘符根直连，其余接 "\"）。
        cur = if remote {
            if cur == "/" {
                format!("/{seg}")
            } else {
                format!("{cur}/{seg}")
            }
        } else if cur.ends_with('\\') {
            format!("{cur}{seg}")
        } else {
            format!("{cur}\\{seg}")
        };
        if last && !expand_target {
            return Walk::Ok; // 目标层不展开，仅定位（C# expandTarget=false）
        }
        match ensure_level(pane, hooks, side, &cur) {
            Walk::Wait => return Walk::Wait,
            Walk::Dead => return Walk::Ok,
            Walk::Ok => {}
        }
        if last {
            return Walk::Ok;
        }
        remain = &remain[seg.len() + sep.len_utf8()..];
    }
    Walk::Ok
}

/// 每帧树同步步进（folder() 树动作应用后调用）：
/// 1. 位置变化 → 下行至当前位置确立高亮基准；「此电脑」根态清除基准
///    （C# SyncTreeIfVisible :915-921；根态 SelectedNode=null :980-984）；
/// 2. 重建后逐条恢复展开链（RebuildTreePreservingAsync 镜像）。
pub(crate) fn tree_sync_step(pane: &mut FilePane, hooks: &mut Hooks, side: usize) {
    if !pane.tree_open || pane.tree.is_empty() {
        return;
    }
    let cur = pane_pos(pane);
    if pane.tree_synced_for != cur {
        match cur {
            Some(path) => {
                if tree_walk(pane, hooks, side, &path, false) == Walk::Ok {
                    pane.tree_synced_for = Some(path);
                }
                // Wait：基准不变，下一帧续走（避免中间态入账）。
            }
            None => pane.tree_synced_for = None,
        }
    }
    while !pane.tree_reexpand.is_empty() {
        let p = pane.tree_reexpand[0].clone();
        if matches!(tree_walk(pane, hooks, side, &p, true), Walk::Wait) {
            break;
        }
        pane.tree_reexpand.remove(0);
    }
}

/// 目录树绘制：返回动作队列（先绘制后统一应用，避免绘制期间可变借用）。
fn draw_tree(
    ui: &mut egui::Ui,
    rect: Rect,
    pane: &FilePane,
    st_icons: &mut crate::icons::Cache,
    p: &theme::Palette,
) -> Vec<TreeAction> {
    ui.painter().rect_filled(rect, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(rect.right() - 0.5, rect.top()),
            Pos2::new(rect.right() - 0.5, rect.bottom()),
        ],
        Stroke::new(1.0, p.separator),
    );
    let mut actions = Vec::new();
    let cur = pane_pos(pane);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(&mut child, |ui| {
            draw_nodes(ui, &pane.tree, 0, st_icons, p, &mut actions, cur.as_deref());
        });
    actions
}

/// 递归绘制树节点行（行高 20 / 缩进 14：caret 12 + folder 14 + 名称 12）。
fn draw_nodes(
    ui: &mut egui::Ui,
    nodes: &[TreeNode],
    depth: usize,
    st_icons: &mut crate::icons::Cache,
    p: &theme::Palette,
    actions: &mut Vec<TreeAction>,
    cur: Option<&str>,
) {
    for n in nodes {
        let w = ui.available_width();
        let r = Rect::from_min_size(
            Pos2::new(ui.min_rect().left(), ui.cursor().top()),
            egui::vec2(w, TREE_ROW),
        );
        let row = ui.allocate_rect(r, Sense::click());
        // 当前目录高亮（WinUI TreeViewItem 选中态：TreeViewItemBackgroundSelected
        // = SubtleFillColorSecondary、SelectedPointerOver = SubtleFillColorTertiary
        // （generic.xaml :1824-1825）——与文件行 hover/按下同对令牌 → 复用
        // row_hover / row_pressed；前景 Selected = TextFillColorPrimary 不变）。
        let selected = cur == Some(n.path.as_str());
        if selected {
            let bg = if row.hovered() {
                p.row_pressed
            } else {
                p.row_hover
            };
            ui.painter().rect_filled(r, 0.0, bg);
        } else if row.hovered() {
            ui.painter().rect_filled(r, 0.0, p.hover_bg);
        }
        let x0 = r.left() + 6.0 + depth as f32 * TREE_INDENT;
        let font12 = egui::FontId::proportional(12.0);
        // caret（加载中画「…」 tertiary）。
        let caret_c = Pos2::new(x0 + 6.0, r.center().y);
        if n.loading {
            ui.painter().text(
                caret_c,
                Align2::CENTER_CENTER,
                "…",
                font12.clone(),
                p.text_tertiary,
            );
        } else {
            let caret_r = Rect::from_center_size(caret_c, egui::vec2(12.0, 12.0));
            let cresp = ui.allocate_rect(caret_r, Sense::click());
            let icon = if n.expanded {
                "caret-down"
            } else {
                "caret-right"
            };
            if let Some(img) = st_icons.get(ui.ctx(), icon, p.text_secondary, 9.0) {
                img.paint_at(ui, Rect::from_center_size(caret_c, egui::vec2(9.0, 9.0)));
            }
            if cresp.clicked() {
                actions.push((n.path.clone(), true));
            }
        }
        // folder 图标 + 名称。
        let icon_c = Pos2::new(x0 + 12.0 + 9.0, r.center().y);
        if let Some(img) = st_icons.get(ui.ctx(), "folder", p.text_secondary, 14.0) {
            img.paint_at(ui, Rect::from_center_size(icon_c, egui::vec2(14.0, 14.0)));
        }
        // 名称（C# TreeDirTemplate TextBlock CharacterEllipsis：随树面板宽度截断，
        // 无 tooltip）。
        let name_x = x0 + 12.0 + 18.0 + 5.0;
        let name = ellipsize(ui, &n.name, &font12, ui.clip_rect().right() - name_x);
        ui.painter().text(
            Pos2::new(name_x, r.center().y),
            Align2::LEFT_CENTER,
            name,
            font12,
            p.text_primary,
        );
        if row.clicked() {
            // 单击节点 = 导航 + 未展开则展开（C# TreeView 单击选中即懒展开；
            // 折叠只经 caret，避免点行误折叠已展开目录）。
            actions.push((n.path.clone(), false));
            if !n.expanded {
                actions.push((n.path.clone(), true));
            }
        }
        ui.advance_cursor_after_rect(r);
        if n.expanded && !n.children.is_empty() {
            draw_nodes(ui, &n.children, depth + 1, st_icons, p, actions, cur);
        }
    }
}
