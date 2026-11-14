//! 主题令牌：唯一来源 `docs/theme-tokens-csharp.md`（T1a 从 C# 版提取）。
//! 口径：WinUI 内部 hack 机制不迁移，最终视觉值保留（DECISIONS.md D-013）。
//! 新界面代码只允许消费本模块字段，禁止裸色值。

use egui::{Color32, Context, FontFamily, FontId, Style, TextStyle, Theme};

/// 品牌蓝系（亮暗同值，C# 全应用统一）。
pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0x78, 0xD4);
/// hover / pressed（C# ButtonBackgroundPointerOver / Pressed）。
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x10, 0x6E, 0xBE);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x00, 0x5A, 0x9E);
/// 成功绿（Header 连接 toggle checked 口径）。
pub const SUCCESS: Color32 = Color32::from_rgb(0x10, 0x7C, 0x10);
/// 警示黄（SettingsPanel 到期提醒前景）。
pub const WARNING: Color32 = Color32::from_rgb(0xD4, 0xA0, 0x17);
/// Ubuntu 橙（HostPickerPanel 主机 logo 圆角方块底，亮暗同值）。
pub const UBUNTU: Color32 = Color32::from_rgb(0xE9, 0x54, 0x20);
/// 右键菜单危险红（C# MenuItem isDangerous 硬编码，明暗同值）。
pub const MENU_DANGER: Color32 = Color32::from_rgb(0xE8, 0x11, 0x23);
/// caption 关闭钮 hover 红（C# CaptionButtonStyle 关闭钮悬停 #C42B1C，明暗同值——
/// 与 Palette.critical 亮值同色但暗色不翻转，故独立一枚 B 类有意差异令牌）。
pub const CAPTION_CLOSE_HOVER: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);
/// 图标钮 tooltip 前景（C# AccentToolTipStyle Foreground=White，亮暗同值：
/// App.xaml 未分主题字典；背景 = ACCENT 同值不另立令牌）。
pub const TOOLTIP_FG: Color32 = Color32::WHITE;
/// 布局测量占位色（`layout_no_wrap` 只取宽高不实际绘制——非视觉令牌，
/// 收口具名以免被当裸色值误报/误改）。
pub const MEASURE_ONLY: Color32 = Color32::WHITE;
/// 复选框选中勾色（C# CheckBox CheckGlyph = TextOnAccentFillColorPrimary 亮值白；
/// 基线 checkbox 亮暗同值白——B 类有意差异：品牌蓝亮暗同值，蓝底白勾对比恒定，
/// 与 toggle 的 p.on_accent_fg 暗色翻转不同源，以 import_dialog 基线为准值不动）。
pub const CHECK_FG: Color32 = Color32::WHITE;
/// 强调钮前景（C# AccentButtonStyle 前景 TextOnAccentFillColorPrimary = 白，
/// 亮暗同值——品牌蓝亮暗同值 B 类；与 toggle 的 p.on_accent_fg 暗色翻转不同源，
/// 以 import_dialog 基线为准值不动）。
pub const ON_ACCENT_FG: Color32 = Color32::WHITE;
/// 强调底上的禁用字（C# AccentButtonStyle 禁用态 TextOnAccentFillColorDisabled，
/// 半透明白，亮暗同值；本项目品牌蓝亮暗同值故字色不随主题翻转）。
pub const TEXT_ON_ACCENT_DISABLED: Color32 =
    Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 0x5D);
/// 文件列表选中：品牌蓝 50% 透明，三态同值。
pub fn selection() -> Color32 {
    Color32::from_rgba_unmultiplied(0x00, 0x78, 0xD4, 0x80)
}
/// 弹窗遮罩。
pub fn mask() -> Color32 {
    Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x80)
}

/// 到期徽章描边（WARNING 金 40% 透明——badge 描边场景派生，值不动）。
pub fn warning_border() -> Color32 {
    Color32::from_rgba_unmultiplied(0xD4, 0xA0, 0x17, 0x66)
}

/// 文本 50% 淡化（C# TextBlock Opacity=0.5：Delete/Checksum/Permission 弹窗条目名
/// EntryName 叠 TextFillColorPrimary 的合成效果，alpha 减半）。
pub fn dim50(c: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a() / 2)
}

/// 描边线宽 = 1 物理像素（C# WinUI 边框任意 DPI 下都是实覆盖；egui 逻辑 1px
/// 在 >100% 缩放下物理 1.5px 会被抗锯齿摊薄，暗主题淡白线会淡到整条不可见）。
pub fn border_width(ppp: f32) -> f32 {
    1.0 / ppp
}

/// 逻辑宽描边 → 物理像素（全库 1.0 / 1.5 / 1.6 描边统一走本函数：>100% DPI 下
/// 逻辑宽被抗锯齿摊薄，100% 缩放下取值不变）。w = C# 设备像素口径线宽。
pub fn stroke(ppp: f32, w: f32, color: Color32) -> egui::Stroke {
    egui::Stroke::new(w / ppp, color)
}

/// 控件描边令牌（按钮类 control_stroke / 输入框类 control_stroke_default
/// 统一经此取描边）：CONTROL_BORDER=false 时 0 宽——egui 跳过 0 宽描边绘制，
/// 开关重开即恢复 1 物理像素 C# 描边（原色值逻辑保留，各态不变）。
pub fn control_border(ppp: f32, color: Color32) -> egui::Stroke {
    egui::Stroke::new(
        if CONTROL_BORDER {
            border_width(ppp)
        } else {
            0.0
        },
        color,
    )
}

/// 描边 rect 像素对齐（C# WinUI 渲染像素对齐；egui 布局坐标常含半像素——
/// 1px 描边落在半像素上会被抗锯齿摊成两条半透明细线，视觉近似无边框）。
/// ppp = ctx.pixels_per_point()。
pub fn snap_rect(rect: egui::Rect, ppp: f32) -> egui::Rect {
    // 对齐到物理像素网格（ppp 缩放下逻辑整数未必落在物理整数上）。
    egui::Rect::from_min_max(
        egui::Pos2::new(
            (rect.min.x * ppp).round() / ppp,
            (rect.min.y * ppp).round() / ppp,
        ),
        egui::Pos2::new(
            (rect.max.x * ppp).round() / ppp,
            (rect.max.y * ppp).round() / ppp,
        ),
    )
}

// ── 阴影（全工程唯一入口）────────────────────────────────────
/// 阴影档位。C# 侧均为系统 Fluent / ThemeShadow 软投影（无资源键可引），
/// 参数为观感近似档位；按约定裸色值只收口在本文件。
pub(crate) enum ShadowKind {
    /// 浮层菜单 / 弹出卡片（MenuFlyout 系统阴影观感）。
    Flyout,
    /// 小型浮条 / 空态确认卡（更淡）。
    Soft,
    /// 弹窗外壳（ThemeShadow Z=32：下移更大、羽化更宽）。
    Dialog,
}

/// 全工程唯一的阴影绘制入口（PRINCIPLES.md §1：禁止 `rect_filled(rect.expand(n)
/// / translate(偏移), 深色)` 实心环模拟阴影——实心环会渲染成一圈「粗透明
/// 边框」伪影；新阴影一律走本函数）。
pub(crate) fn draw_shadow(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    radius: impl Into<egui::CornerRadius>,
    kind: ShadowKind,
) {
    let (offset, blur, alpha) = match kind {
        ShadowKind::Flyout => ([0, 2], 16, 46),
        ShadowKind::Soft => ([0, 2], 16, 40),
        ShadowKind::Dialog => ([0, 6], 28, 60),
    };
    ui.painter().add(
        egui::Shadow {
            offset,
            blur,
            spread: 0,
            color: Color32::from_black_alpha(alpha),
        }
        .as_shape(rect, radius),
    );
}

/// TextBox/ComboBox 描边令牌（C# ControlElevationBorderBrush 垂直渐变简化：
/// 顶/左/右 = 淡端色，底边 1px 条带 = 深端色——亮主题 #0F→#29 底边明显、
/// 暗主题 #12→#18 近同不明显，与 C# 观感一致）。CONTROL_BORDER=false 不画。
pub fn elevation_border(
    pt: &egui::Painter,
    rect: egui::Rect,
    radius: f32,
    top: Color32,
    bottom: Color32,
) {
    if !CONTROL_BORDER {
        return;
    }
    // 调用点未 snap 的兜底：逻辑整数对齐（半像素描边会被抗锯齿摊薄）。
    let rect = rect.round();
    // 线宽 = 1 物理像素（暗主题淡白线被抗锯齿摊薄会整条不可见）。
    let w = border_width(pt.ctx().pixels_per_point());
    pt.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(w, top),
        egui::StrokeKind::Inside,
    );
    pt.rect_filled(
        egui::Rect::from_min_max(
            egui::Pos2::new(rect.left() + w, rect.bottom() - w),
            egui::Pos2::new(rect.right() - w, rect.bottom()),
        ),
        0.0,
        bottom,
    );
}

/// 主容器间隔（竖/横分隔条宽度一致）。
pub const GUTTER: f32 = 6.0;
/// 内容容器圆角（四个容器、弹窗卡、终端卡统一）。
pub const CONTAINER_RADIUS: f32 = 8.0;
/// 控件圆角（WinUI 默认控制圆角）。
pub const CONTROL_RADIUS: f32 = 4.0;
// ── 弹窗外壳几何（§7.2 弹框统一原则；dialogs / import_dialog / host_panel
//    三份布局常量合一后只此一份）────────────────────────────
/// 标题行总高（上 5 + 关闭钮 28 + 下 2）。
pub const TITLE_BAR_H: f32 = 35.0;
/// 关闭钮边长。
pub const CLOSE_SIZE: f32 = 28.0;
/// 内容区 padding（横,纵）——统一 15 口径（title/content/button 全套同边距）。
pub const CONTENT_PAD: (f32, f32) = (15.0, 15.0);
/// 内容区距卡片边 inset（shell 内容圆弧容器统一距边 5，对齐 C# Margin 5）。
pub const CONTENT_INSET: f32 = 5.0;
/// 弹窗内组间距。
pub const GROUP_GAP: f32 = 14.0;
/// 标准钮 / 输入框高（WinUI 控件默认 32）。
pub const BTN_H: f32 = 32.0;
/// 弹窗内子卡圆角（shell 内容圆弧容器 / 设置双卡 / host_panel 子区 /
/// 空态引导卡 / 导入结果图标槽——OverlayCornerRadius 6 具名化）。
pub const SHELL_RADIUS: f32 = 6.0;
/// Header 悬浮胶囊圆角（窗口控制钮 hover/按下的 12 圆角底）。
pub const PILL_RADIUS: f32 = 12.0;
/// 控件描边开关（2026-10-31 二次定案：恢复边框且完全参照 C#——标准按钮单色
/// ControlStrokeColorSecondary；TextBox/ComboBox 走 elevation_border 顶淡底深
/// 渐变近似（C# ControlElevationBorderBrush）。C# 观感：亮主题边线明显、
/// 暗主题两端色近同故不明显。checkbox 方框 / toggle 轨道 / 卡片选中边等
/// 语义描边不受此开关影响。false = 去边框（首版定案，已作废）。
pub const CONTROL_BORDER: bool = true;
/// 下排（队列/终端）初始高度。
pub const BOTTOM_ROW_MIN: f32 = 120.0;

// ── 滚动条令牌（2026-11-06 定案：原 C# 浮动式 4/12 观感偏宽且压住滚动容器
//    右缘描边；整体减半 + 外移 + 50% 透明。2026-11-08 改常显 + 定距：不悬停
//    也 30% 浮在容器右缘外、与边缘留 2px 间隙。WinUI 无对应资源键可引，
//    为项目观感定案档位，裸值只收口此处）─────────────────────
/// 交互态滚动条宽（原 12 减半）。
pub const SCROLL_BAR_WIDTH: f32 = 6.0;
/// 隐藏态浮条宽（原 4 减半）。
pub const SCROLL_BAR_FLOATING_WIDTH: f32 = 2.0;
/// 滚动条距滚动容器右缘的间隙（条整体在容器外侧）。bar_outer_margin 从
/// egui 的「条右缘 = 内容区右缘 − margin」反推：内容区已内缩 1px 描边 +
/// 间隙 2 + 隐藏态条宽 2 = 5，取负让隐藏态条左缘落在容器右缘外 2px；
/// 悬停变宽（2→6）时右缘固定、向左扩展越过容器边，为 egui 浮条固有方向。
pub const SCROLL_BAR_OUTER_OFFSET: f32 = 5.0;
/// 滚动条手柄不透明度（常显 30%：隐藏态与悬停/拖拽统一，2026-11-08 二次调淡）。
pub const SCROLL_BAR_OPACITY: f32 = 0.3;
/// 队列栏滚动条单独覆盖（公用令牌基调上的例外）：队列滚动列表右缘与容器内缘
/// 重合，全局外浮会把条画到容器圆角边线外 → 该处条贴缘内收 1px；宽度/透明度
/// 仍走 SCROLL_BAR_WIDTH / SCROLL_BAR_OPACITY。
pub const SCROLL_BAR_INNER_QUEUE: f32 = 1.0;

/// 每主题一组区域色。字段名对齐令牌清单第 1、4 节的 C# 键名。
#[derive(Clone, Copy)]
pub struct Palette {
    /// HeaderAreaBrush：主区域打底。
    pub header_area: Color32,
    /// FolderAreaBrush：内容容器底。
    pub folder_area: Color32,
    /// DialogTitleBarBrush：弹框标题栏。
    pub dialog_title_bar: Color32,
    /// SeparatorLineBrush：竖分隔线。
    pub separator: Color32,
    /// RowSeparatorLineBrush：行横线。
    pub row_separator: Color32,
    /// 正文一级（Fluent 默认 89.64% 阶）。
    pub text_primary: Color32,
    /// 次级文字（60.63% 阶）。
    pub text_secondary: Color32,
    /// 三级文字（44.65% 阶）。
    pub text_tertiary: Color32,
    /// 禁用文字（TextFillColorDisabled：亮 36% 黑 / 暗 36% 白）。
    pub text_disabled: Color32,
    /// 默认控件 hover 底（设置面板按钮态口径）。
    pub hover_bg: Color32,
    /// 默认控件按下底。
    pub pressed_bg: Color32,
    /// TerminalBackgroundBrush：终端卡底。
    pub terminal_bg: Color32,
    /// TerminalTextBrush：终端正文。
    pub terminal_text: Color32,
    /// TerminalTimeBrush：终端时间戳。
    pub terminal_time: Color32,
    /// TabView 系统默认底线（TabViewBorderBrush = CardStrokeColorDefault：
    /// 亮 #0F000000 黑6% / 暗 #19000000 黑10%，明暗均为黑 alpha）。主机面板页签
    /// 模板 stubs 消费；IDE tab 条亮主题被 IdeTabStrip 资源字典覆盖，见 ide_tab_border。
    pub tab_border: Color32,
    /// IDE tab 条自绘基线（TabStripBaseline，Fill=TabViewBorderBrush：亮主题
    /// IdeTabStrip 资源字典显式覆盖 #E3E3E3 / 暗无覆盖走系统默认黑10%）。
    pub ide_tab_border: Color32,
    /// IDE 页签 hover/按下（SubtleFillColorSecondary：亮 #09000000 黑3.5% /
    /// 暗 #0FFFFFFF 白6%；C# IdeTabStrip 模板 VSM 直引，非控件级覆写）。
    pub tab_hover: Color32,
    /// IDE 页签常态（SubtleFillColorTertiary：亮 #06000000 黑2.3% / 暗 #0AFFFFFF 白3.9%）。
    pub tab_rest: Color32, // 亮：from_black_alpha(6) / 暗：from_white_alpha(0x0A)，见构造处
    /// tab 选中字（TabViewItemHeaderForegroundSelected）。
    pub tab_active_fg: Color32,
    /// 未选中文件行 hover（SubtleFillColorSecondary，ListViewItemBackgroundPointerOver 同值）。
    pub row_hover: Color32,
    /// 未选中文件行按下（SubtleFillColorTertiary，ListViewItemBackgroundPressed 同值）。
    pub row_pressed: Color32,
    /// 弹窗内容圆弧容器 / 菜单底（CardBackgroundFillColorDefault，半透明白叠层）。
    pub card_bg: Color32,
    /// 弹窗卡描边（CardStrokeColorDefault）。
    pub card_stroke: Color32,
    /// 标准钮 / 输入框底（ControlFillColorDefault）。
    pub control_bg: Color32,
    /// 标准钮 hover（ControlFillColorSecondary）。
    pub control_hover: Color32,
    /// 标准钮按下（ControlFillColorTertiary）。
    pub control_pressed: Color32,
    /// 标准钮描边（ControlStrokeColorSecondary）。
    pub control_stroke: Color32,
    /// 禁用控件底（ControlFillColorDisabled，C# 未覆写取 Fluent 默认：亮 50% #F9F9F9 / 暗 50% #2D2D2D）。
    pub control_disabled: Color32,
    /// 禁用控件描边（ControlStrokeColorDisabled，Fluent 默认：亮 11.57% 黑 / 暗 7.06% 白）。
    pub control_stroke_disabled: Color32,
    /// 输入类控件淡边框（ControlStrokeColorDefault：亮 6% 黑 / 暗 7% 白）。
    /// WinUI TextBox/ComboBox 静默态描边（ControlElevationBorderBrush 底端色），
    /// 比钮类 control_stroke（Secondary）更淡一档。
    pub control_stroke_default: Color32,
    /// 聚焦输入框底（ControlFillColorInputActive：亮 #FFFFFF / 暗 70% #1E1E1E）。
    pub control_bg_focused: Color32,
    /// 键盘焦点框 = WinUI FocusVisual PrimaryBrush 的半透明弱化版（65% 黑 /
    /// 75% 白，2px），保留 C3④ 键盘焦点框但降低默认态刺眼程度；与 ACCENT 主钮
    /// 底色混合后对比度 ≥3:1（CR-9，原值亮纯黑 / 暗纯白）。F-9 C3④：按钮系
    /// has_focus 描边焦点框。次级内描省略——有意差异。
    pub focus_stroke: Color32,
    /// 强调弱轨（ControlStrongFillColorDefault：亮 45% 黑 / 暗 55% 白）。
    /// Slider 未走过轨道填充（SliderTrackFill）与 ToggleSwitch off 描边（ToggleSwitchOffStroke）
    /// 在 WinUI 同源，共用一枚。
    pub control_strong_fill: Color32,
    /// 强调弱轨禁用（ControlStrongStrokeColorDisabled：亮 22% 黑 / 暗 16% 白）。
    /// Slider 禁用轨道（SliderTrackFillDisabled）与开关 off 禁用描边共用。
    pub control_strong_fill_disabled: Color32,
    /// 强调底上的文字（TextOnAccentFillColorPrimary：亮 #FFFFFF / 暗 #000000）。
    /// 开关 on 态拇指（KnobFillOn）。
    pub on_accent_fg: Color32,
    /// 开关 off 轨道底（ControlAltFillColorSecondary：亮 2.3% 黑 / 暗 10% 黑）。
    pub toggle_off_fill: Color32,
    /// 开关 off 悬停轨道底（ControlAltFillColorTertiary：亮 5.9% 黑 / 暗 4.3% 白）。
    pub toggle_off_fill_hover: Color32,
    /// 开关 off 按下轨道底（ControlAltFillColorQuarternary：亮 9.4% 黑 / 暗 7.1% 白）。
    pub toggle_off_fill_pressed: Color32,
    /// 下拉浮层底（AcrylicInAppFillColorDefault：egui 无亚克力材质，
    /// 取近实色近似：亮 #F9F9F9 / 暗 #2C2C2C）。
    pub combo_popup_bg: Color32,
    /// 下拉浮层描边（SurfaceStrokeColorFlyout：亮 6% 黑 / 暗 20% 黑）。
    pub combo_popup_stroke: Color32,
    /// 危险红（SystemFillColorCritical）。
    pub critical: Color32,
    /// 比对 Caution 行底（SystemFillColorCautionBackground）。
    pub caution_bg: Color32,
    /// 比对 Critical 行底（SystemFillColorCriticalBackground）。
    pub critical_bg: Color32,
    /// IDE tab 选中底（IdeTabStrip TabActiveBrush：亮白 / 暗 #253140）。
    pub tab_selected_bg: Color32,
    /// IDE 新建(+)钮 hover（IdeTabStrip NewTabBtn 资源字典覆写
    /// ButtonBackgroundPointerOver：亮 #E3E3E3 / 暗 #2B3138）。
    pub tab_new_hover: Color32,
    /// 设置面板导航选中底（SettingsPanel.xaml.cs SetCategoryState：亮 #DADADA / 暗 #0D1420）。
    pub nav_selected_bg: Color32,
    /// 设置面板导航未选中底（SetCategoryState：亮无块底=透明 / 暗 #243040）。
    pub nav_rest_bg: Color32,
    /// 超链接前景（HyperlinkButtonForeground = SystemAccentColor：亮 #005FB8 / 暗 #4CC2FF）。
    pub accent: Color32,
    /// 列分割线（DividerStrokeColorDefault：亮 6% 黑 / 暗 8% 白）。
    pub divider_stroke: Color32,
    /// 主按钮禁用底（AccentFillColorDisabled：亮 22% 黑 / 暗 16% 白）。
    pub accent_disabled: Color32,
    /// 系统成功文字（SystemFillColorSuccess：亮 #0F5B00 / 暗 #6CCB5F）。
    pub success_fg: Color32,
    /// InfoBar 成功底（SystemFillColorSuccessBackground：亮 #E6F2E6 / 暗 #102A1A）。
    pub success_bg: Color32,
    /// 右键菜单底（PaneMenuPresenterStyle：亮 #FFFFFF / 暗 #26303D，不透明）。
    pub menu_bg: Color32,
    /// 空态卡片底（FileView 空目录卡，覆盖 CardBackgroundFillColorSecondary）。
    pub empty_card_bg: Color32,
    /// caption 钮 hover（Header.xaml.cs：亮 #14000000 / 暗 #1FFFFFFF）。
    pub caption_hover: Color32,
    /// caption 钮按下（Header.xaml.cs：亮 13 黑 / 暗 15 白）。
    pub caption_pressed: Color32,
}

pub const LIGHT: Palette = Palette {
    header_area: Color32::from_rgb(0xE9, 0xE9, 0xE9),
    folder_area: Color32::from_rgb(0xF9, 0xF9, 0xF9),
    dialog_title_bar: Color32::from_rgb(0xE9, 0xE9, 0xE9),
    separator: Color32::from_rgb(0xED, 0xED, 0xED),
    row_separator: Color32::from_rgb(0xE3, 0xE3, 0xE3),
    text_primary: Color32::from_black_alpha(228),
    text_secondary: Color32::from_black_alpha(158),
    text_tertiary: Color32::from_black_alpha(114),
    text_disabled: Color32::from_black_alpha(0x5C),
    hover_bg: Color32::from_rgb(0xE9, 0xE9, 0xE9),
    pressed_bg: Color32::from_rgb(0xE0, 0xE0, 0xE0),
    terminal_bg: Color32::from_rgb(0xF9, 0xFC, 0xF9),
    terminal_text: Color32::from_rgb(0x3A, 0x4C, 0x3A),
    terminal_time: Color32::from_rgb(0x7A, 0x82, 0x7A),
    tab_border: Color32::from_black_alpha(0x0F),
    ide_tab_border: Color32::from_rgb(0xE3, 0xE3, 0xE3),
    tab_hover: Color32::from_black_alpha(0x09),
    tab_rest: Color32::from_black_alpha(6),
    tab_active_fg: Color32::from_rgb(0x22, 0x22, 0x22),
    row_hover: Color32::from_black_alpha(9),
    row_pressed: Color32::from_black_alpha(6),
    card_bg: Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 0xB3),
    card_stroke: Color32::from_black_alpha(15),
    control_bg: Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 0xB3),
    control_hover: Color32::from_rgba_unmultiplied_const(0xF9, 0xF9, 0xF9, 0x80),
    control_pressed: Color32::from_rgba_unmultiplied_const(0xF9, 0xF9, 0xF9, 0x4D),
    control_stroke: Color32::from_black_alpha(0x29),
    control_disabled: Color32::from_rgba_unmultiplied_const(0xF9, 0xF9, 0xF9, 0x80),
    control_stroke_disabled: Color32::from_black_alpha(0x1D),
    control_stroke_default: Color32::from_black_alpha(0x0F),
    control_bg_focused: Color32::WHITE,
    focus_stroke: Color32::from_black_alpha(0xA6), // CR-9：原 Color32::BLACK
    control_strong_fill: Color32::from_black_alpha(0x72),
    control_strong_fill_disabled: Color32::from_black_alpha(0x37),
    on_accent_fg: Color32::WHITE,
    toggle_off_fill: Color32::from_black_alpha(6),
    toggle_off_fill_hover: Color32::from_black_alpha(0x0F),
    toggle_off_fill_pressed: Color32::from_black_alpha(0x18),
    combo_popup_bg: Color32::from_rgb(0xF9, 0xF9, 0xF9),
    combo_popup_stroke: Color32::from_black_alpha(0x0F),
    critical: Color32::from_rgb(0xC4, 0x2B, 0x1C),
    caution_bg: Color32::from_rgb(0xFF, 0xF4, 0xCE),
    critical_bg: Color32::from_rgb(0xFD, 0xE7, 0xE9),
    tab_selected_bg: Color32::WHITE,
    tab_new_hover: Color32::from_rgb(0xE3, 0xE3, 0xE3),
    nav_selected_bg: Color32::from_rgb(0xDA, 0xDA, 0xDA),
    nav_rest_bg: Color32::TRANSPARENT,
    accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
    divider_stroke: Color32::from_black_alpha(0x0F),
    accent_disabled: Color32::from_black_alpha(0x37),
    success_fg: Color32::from_rgb(0x0F, 0x5B, 0x00),
    success_bg: Color32::from_rgb(0xE6, 0xF2, 0xE6),
    menu_bg: Color32::WHITE,
    empty_card_bg: Color32::from_rgb(0xE0, 0xE0, 0xE0),
    caption_hover: Color32::from_black_alpha(0x14),
    caption_pressed: Color32::from_black_alpha(0x0D),
};

/// 暗色调色板。白阶 alpha 不能用 const 构造，故为函数。
pub fn dark() -> Palette {
    Palette {
        header_area: Color32::from_rgb(0x26, 0x30, 0x3D),
        folder_area: Color32::from_rgb(0x1B, 0x24, 0x30),
        dialog_title_bar: Color32::from_rgb(0x1B, 0x24, 0x30),
        separator: Color32::from_rgb(0x20, 0x2B, 0x38),
        row_separator: Color32::from_rgb(0x31, 0x3F, 0x4F),
        text_primary: Color32::from_white_alpha(255),
        text_secondary: Color32::from_white_alpha(197),
        text_tertiary: Color32::from_white_alpha(135),
        text_disabled: Color32::from_white_alpha(0x5D),
        hover_bg: Color32::from_rgb(0x2C, 0x3B, 0x52),
        pressed_bg: Color32::from_rgb(0x22, 0x31, 0x4A),
        terminal_bg: Color32::from_rgb(0x1B, 0x2A, 0x30),
        terminal_text: Color32::from_rgb(0xC6, 0xD2, 0xC6),
        terminal_time: Color32::from_rgb(0x8A, 0x98, 0x8A),
        tab_border: Color32::from_black_alpha(0x19),
        ide_tab_border: Color32::from_black_alpha(0x19),
        tab_hover: Color32::from_white_alpha(0x0F),
        tab_rest: Color32::from_white_alpha(0x0A),
        tab_active_fg: Color32::WHITE,
        row_hover: Color32::from_white_alpha(0x0F),
        row_pressed: Color32::from_white_alpha(0x0A),
        card_bg: Color32::from_white_alpha(0x0D),
        card_stroke: Color32::from_black_alpha(0x19),
        control_bg: Color32::from_white_alpha(0x0F),
        control_hover: Color32::from_white_alpha(0x15),
        control_pressed: Color32::from_white_alpha(0x08),
        control_stroke: Color32::from_white_alpha(0x18),
        control_disabled: Color32::from_rgba_unmultiplied_const(0x2D, 0x2D, 0x2D, 0x80),
        control_stroke_disabled: Color32::from_white_alpha(0x12),
        control_stroke_default: Color32::from_white_alpha(0x12),
        control_bg_focused: Color32::from_rgba_unmultiplied_const(0x1E, 0x1E, 0x1E, 0xB3),
        focus_stroke: Color32::from_white_alpha(0xC0), // CR-9：原 Color32::WHITE
        control_strong_fill: Color32::from_white_alpha(0x8B),
        control_strong_fill_disabled: Color32::from_white_alpha(0x28),
        on_accent_fg: Color32::BLACK,
        toggle_off_fill: Color32::from_black_alpha(0x19),
        toggle_off_fill_hover: Color32::from_white_alpha(0x0B),
        toggle_off_fill_pressed: Color32::from_white_alpha(0x12),
        combo_popup_bg: Color32::from_rgb(0x2C, 0x2C, 0x2C),
        combo_popup_stroke: Color32::from_black_alpha(0x33),
        critical: Color32::from_rgb(0xFF, 0x99, 0xA4),
        caution_bg: Color32::from_rgb(0x43, 0x35, 0x19),
        critical_bg: Color32::from_rgb(0x44, 0x27, 0x26),
        tab_selected_bg: Color32::from_rgb(0x25, 0x31, 0x40),
        tab_new_hover: Color32::from_rgb(0x2B, 0x31, 0x38),
        nav_selected_bg: Color32::from_rgb(0x0D, 0x14, 0x20),
        nav_rest_bg: Color32::from_rgb(0x24, 0x30, 0x40),
        accent: Color32::from_rgb(0x4C, 0xC2, 0xFF),
        divider_stroke: Color32::from_white_alpha(0x15),
        accent_disabled: Color32::from_white_alpha(0x28),
        success_fg: Color32::from_rgb(0x6C, 0xCB, 0x5F),
        success_bg: Color32::from_rgb(0x10, 0x2A, 0x1A),
        menu_bg: Color32::from_rgb(0x26, 0x30, 0x3D),
        empty_card_bg: Color32::from_rgb(0x2C, 0x30, 0x38),
        caption_hover: Color32::from_white_alpha(0x1F),
        caption_pressed: Color32::from_white_alpha(0x0F),
    }
}

/// 当前主题的调色板。切换主题后取值随之变化。
pub fn palette(ctx: &Context) -> Palette {
    match ctx.theme() {
        Theme::Light => LIGHT,
        Theme::Dark => dark(),
    }
}

/// 把令牌写入指定主题。幂等，可对明暗各调用一次完成初始化；
/// 未来切换主题时对目标主题重调一次再 `set_theme` 即可。
pub fn install(ctx: &Context, theme: Theme) {
    ctx.style_mut_of(theme, |style| {
        let p = match theme {
            Theme::Light => LIGHT,
            Theme::Dark => dark(),
        };
        apply_palette(style, p);
        apply_type_scale(style);
        apply_spacing(style);
        apply_native_controls(style, &p);
    });
    ctx.set_theme(theme);
}

/// 原生控件换皮（WinUI Fluent 默认口径，令牌清单 §10.2）：
/// 滚动条浮动式常显（隐藏态 2px / 交互态 6px、浮在容器右缘外留 2px 间隙、
/// 手柄全态 30%，2026-11-06 减半+外移、2026-11-08 常显+定距+调淡，
/// 令牌见本文件顶部滚动条令牌区）、
/// 菜单浮层 OverlayCornerRadius 8、TextEdit 底 ControlFillColorDefault。
fn apply_native_controls(style: &mut Style, p: &Palette) {
    let s = &mut style.spacing;
    s.scroll.bar_width = SCROLL_BAR_WIDTH;
    s.scroll.floating_width = SCROLL_BAR_FLOATING_WIDTH;
    // 负 margin = 条右缘外移（egui：max_cross = outer_rect.max - margin）；
    // 5 = 内容区内缩 1 + 容器边缘间隙 2 + 隐藏态条宽 2 → 隐藏态条整体浮在
    // 容器右缘外、留 2px 间隙。
    s.scroll.bar_outer_margin = -SCROLL_BAR_OUTER_OFFSET;
    s.scroll.active_handle_opacity = SCROLL_BAR_OPACITY;
    s.scroll.interact_handle_opacity = SCROLL_BAR_OPACITY;
    // 常显：不悬停也 30%（条固定浮在容器右缘外 2px，悬停 2→6px 变宽向左扩）。
    s.scroll.dormant_handle_opacity = SCROLL_BAR_OPACITY;
    style.visuals.menu_corner_radius = CONTAINER_RADIUS.into();
    style.visuals.text_edit_bg_color = Some(p.control_bg);
}

fn apply_palette(style: &mut Style, p: Palette) {
    let v = &mut style.visuals;
    v.panel_fill = p.folder_area;
    v.window_fill = p.folder_area;
    v.extreme_bg_color = p.header_area;
    v.faint_bg_color = p.separator;
    v.window_stroke = egui::Stroke::new(1.0, p.separator);
    v.window_corner_radius = CONTAINER_RADIUS.into();

    v.widgets.noninteractive.bg_fill = p.header_area;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.separator);

    v.widgets.inactive.bg_fill = p.folder_area;
    // F-12：ComboBox 等弱底控件读 weak_bg_fill（此前只写 bg_fill，下拉校验码
    // ComboBox 走了 egui 默认色）——与 bg_fill 同源三态 + open 态补齐。
    v.widgets.inactive.weak_bg_fill = p.control_bg;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text_secondary);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.separator);
    v.widgets.inactive.corner_radius = CONTROL_RADIUS.into();

    v.widgets.hovered.bg_fill = p.hover_bg;
    v.widgets.hovered.weak_bg_fill = p.control_hover;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.separator);
    v.widgets.hovered.corner_radius = CONTROL_RADIUS.into();

    v.widgets.active.bg_fill = p.pressed_bg;
    v.widgets.active.weak_bg_fill = p.control_pressed;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
    v.widgets.active.corner_radius = CONTROL_RADIUS.into();

    v.widgets.open.bg_fill = p.hover_bg;
    v.widgets.open.weak_bg_fill = p.control_hover;
    v.widgets.open.corner_radius = CONTROL_RADIUS.into();

    v.selection.bg_fill = selection();
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
}

/// 字号档：标题 16 / 正文与按钮 14 / 次级 12 / 等宽 13（对齐令牌清单第 8 节）。
fn apply_type_scale(style: &mut Style) {
    let ts = &mut style.text_styles;
    ts.insert(TextStyle::Heading, FontId::proportional(16.0));
    ts.insert(TextStyle::Body, FontId::proportional(14.0));
    ts.insert(TextStyle::Button, FontId::proportional(14.0));
    ts.insert(TextStyle::Small, FontId::proportional(12.0));
    ts.insert(TextStyle::Monospace, FontId::monospace(13.0));
}

// ── 字号角色（§8 九档具名化；字面 FontId 禁令见 theme_literals_all_use_tokens）──
/// 具名字号角色：Rust 侧消费的 §8 档位收口，字面 `FontId::proportional(数字)`
/// 全库禁用（theme.rs 本体除外；editor_window.rs 独立窗口域豁免——其编辑器
/// 字号 11/12/14 属独立域，F-11 维持范围外）。
#[derive(Clone, Copy)]
pub enum TextRole {
    /// 表头小注（11：冲突表表身、设置卡描述、导入警告、主机卡副行）。
    Caption,
    /// 紧凑正文（13，比例族——C# NavButton/HostPanel 卡片文字/主机钮文字；
    /// 仅设置面板 / 主机面板 / Header 主机钮可用，弹窗正文一律 Body 14）。
    Compact,
    /// 空态主文案（18 比例族档位；当前消费者走 brand 族 font_empty_title）。
    #[expect(dead_code)] // §8 档位兜底，上量时移除
    EmptyTitle,
    /// 页标题（20 比例族档位；结果图标走 font_result_glyph，页首未上量）。
    #[expect(dead_code)] // §8 档位兜底，上量时移除
    PageTitle,
    /// 空态图标（23，C# EmptyState 图标）。
    #[expect(dead_code)] // §8 档位兜底，上量时移除
    EmptyIcon,
}

/// 角色字号（§8 档位）：fs 参数位值源头具名（禁裸写 13.0 等，F-11 拍板①）。
pub fn role_size(role: TextRole) -> f32 {
    match role {
        TextRole::Caption => 11.0,
        TextRole::Compact => 13.0,
        TextRole::EmptyTitle => 18.0,
        TextRole::PageTitle => 20.0,
        TextRole::EmptyIcon => 23.0,
    }
}

/// 角色字体（比例族）：`font(TextRole::Compact)` 等——13 只准经本函数出现，
/// 禁字面量。
pub fn font(role: TextRole) -> FontId {
    FontId::proportional(role_size(role))
}

/// 次级字号（12 f32 数值口径：HostPanel 表单 fs 参数等——FontId 走 font_small()）。
pub const FONT_SMALL: f32 = 12.0;

/// 次级字体（12，§8：表头/表身/码值/描述/页签标题——TextStyle::Small 同值）。
pub fn font_small() -> FontId {
    FontId::proportional(FONT_SMALL)
}

/// 正文字体（14，§8：弹窗正文/控件/按钮——TextStyle::Body 同值恒 14）。
pub fn font_body() -> FontId {
    FontId::proportional(14.0)
}

/// 码值等宽字体（12，校验码数值；TextStyle::Monospace 13 是文本框口径）。
pub fn font_mono_small() -> FontId {
    FontId::monospace(12.0)
}

/// brand 半粗族标题（14 / 16：弹窗标题行——16 大标题 / 14 副标题，§8）。
pub fn font_title(large: bool) -> FontId {
    FontId::new(
        if large { 16.0 } else { 14.0 },
        FontFamily::Name("brand".into()),
    )
}

/// 页首标题（20，brand 半粗族：C# 页首 TextBlock FontSize 20）。
pub fn font_page_title() -> FontId {
    FontId::new(20.0, FontFamily::Name("brand".into()))
}

/// 窗格列头（11 brand 半粗，C# ListView HeaderText SemiBold 11）。
pub fn font_header() -> FontId {
    FontId::new(11.0, FontFamily::Name("brand".into()))
}

/// brand 半粗小号（12：面包屑 / 注册链接 / 队列页签选中，§8 12 档半粗变体）。
pub fn font_brand_small() -> FontId {
    FontId::new(12.0, FontFamily::Name("brand".into()))
}

/// brand 半粗紧凑（13：设置面板「关于」卡名称 SemiBold 13，§8 13 档半粗变体）。
pub fn font_brand_compact() -> FontId {
    FontId::new(13.0, FontFamily::Name("brand".into()))
}

/// brand 半粗空态标题（18：C# EmptyState 主文案，§8 档 18）。
pub fn font_empty_title() -> FontId {
    FontId::new(18.0, FontFamily::Name("brand".into()))
}

/// Cascadia Mono 11（终端码值 / 状态表码列，C# Cascadia Mono 11 口径）。
pub fn font_mono_caption() -> FontId {
    FontId::monospace(11.0)
}

/// 结果页大图标字形（20 regular：导入向导结果页 ✓/ⓘ 近似 C# FontIcon
/// FontSize 20，ImportDialog ResultView xaml:285-297）。
pub fn font_result_glyph() -> FontId {
    FontId::proportional(20.0)
}

/// 间距基线：ButtonPadding `11,5,11,6`（App.xaml 未覆写它，标准钮用 WinUI 默认）；
/// C# 的 TextControlThemePadding `10,7,10,4` 是垂直居中 hack，机制不迁移（D-013）。
fn apply_spacing(style: &mut Style) {
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 4.0);
    s.button_padding = egui::vec2(11.0, 5.0);
}

// ── 主题键解析（T19-1：settings.json theme 键，与 C# ThemeService 互认）────

/// theme 键 → 是否暗色（C# ThemeService.Initialize :39-44 同语义）：
/// "Dark" 显式暗、"Light" 显式亮，其余（含空 = 未配置）跟随系统。
/// `system_light` 注入由调用方探测（单测免注册表依赖）。
pub(crate) fn resolve_dark(theme: &str, system_light: bool) -> bool {
    match theme {
        "Dark" => true,
        "Light" => false,
        _ => !system_light,
    }
}

/// Windows 系统应用主题（C# ThemeService.IsSystemThemeLight :96-109 同源）：
/// 注册表 HKCU\...\Themes\Personalize 的 AppsUseLightTheme，1 = 亮。
/// 口径对齐 C#：值缺失取其 GetValue 默认 1（亮）；读取异常回退 false（暗）。
#[cfg(windows)]
pub(crate) fn system_prefers_light() -> bool {
    unsafe extern "system" {
        fn RegGetValueW(
            hkey: isize,
            lpsubkey: *const u16,
            lpvalue: *const u16,
            dwflags: u32,
            pdwtype: *mut u32,
            pvdata: *mut std::ffi::c_void,
            pcbdata: *mut u32,
        ) -> i32;
    }
    const HKEY_CURRENT_USER: isize = 0x8000_0001;
    const RRF_RT_REG_DWORD: u32 = 0x0010;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    let key = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<u16>>();
    let value = "AppsUseLightTheme"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<u16>>();
    let mut data: u32 = 0;
    let mut size = 4u32;
    // SAFETY：键名/值名为本函数栈上 NUL 结尾 UTF-16，缓冲区为栈上 DWORD，
    // pcbdata 上限即其尺寸；仅读取用户注册表。
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    match code {
        ERROR_SUCCESS => data == 1, // 非 1 一律暗（C# Convert.ToInt32(value)==1 同口径）
        ERROR_FILE_NOT_FOUND => true, // 值缺失 = C# GetValue(name, 1) 默认亮
        _ => false,                 // 读取异常 = C# catch → false（暗）
    }
}

/// 非 Windows 无注册表语义：按 C# 异常路径口径回退暗。
#[cfg(not(windows))]
pub(crate) fn system_prefers_light() -> bool {
    false
}

#[cfg(test)]
mod theme_key_tests {
    use super::resolve_dark;

    /// T19-1：theme 键解析（C# ThemeService.Initialize :39-44）——显式值直认，
    /// 空/未知 = 跟随系统。
    #[test]
    fn resolve_dark_matches_csharp() {
        assert!(resolve_dark("Dark", false));
        assert!(resolve_dark("Dark", true));
        assert!(!resolve_dark("Light", false));
        assert!(!resolve_dark("Light", true));
        // 空（未配置）与未知值 = 跟随系统。
        assert!(resolve_dark("", false));
        assert!(!resolve_dark("", true));
        assert!(resolve_dark("whatever", false));
        assert!(!resolve_dark("whatever", true));
    }
}

// ── F-11 门禁钉子：全仓字面量扫描（仿 i18n.rs::t_literals_all_have_match_arms）──
// 无门禁则下一个新弹窗会原样退化（裸 FontId::proportional(13.0) / 局部
// text_styles.insert 覆盖全局 Body=14）。字面量只准收口在本文件（theme.rs）。
#[cfg(test)]
mod font_token_guard {
    /// 字面禁词构造子（首参数字字面即违规；变量 / 具名常量实参放行：
    /// dialogs.rs 的 fs、panes.rs 的 size、TBL_BODY_SIZE——值源头已具名）。
    const CTORS: [&str; 3] = [
        "FontId::proportional(",
        "FontId::monospace(",
        "FontId::new(",
    ];

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read src dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                collect_rs_files(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }

    /// 剥 `//` 行注释。`//` 前有奇数个 `"` 视为字符串里的 URL（https://）不剥，
    /// 兼顾「不漏报代码、不误报注释里的历史措辞」（如「text_styles.insert 归零」）。
    fn strip_line_comment(line: &str) -> &str {
        let bytes = line.as_bytes();
        let mut quotes = 0usize;
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'"' {
                quotes += 1;
            }
            if bytes[i] == b'/' && bytes[i + 1] == b'/' && quotes % 2 == 0 {
                return &line[..i];
            }
            i += 1;
        }
        line
    }

    /// 首个非空白字符是否数字 / `.5` 形态（= 裸数字字面实参）。
    fn starts_with_number(s: &str) -> bool {
        s.trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == '.')
    }

    #[test]
    fn theme_literals_all_use_tokens() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let src = std::path::Path::new(manifest).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        // 哨兵：目录挪动 / collect 失灵时宁可炸也不静默放行。
        assert!(
            files.len() > 10,
            "守卫哨兵：只扫到 {} 个 .rs，扫描器已失灵",
            files.len()
        );

        let mut hits: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for f in &files {
            // 豁免：theme.rs = 令牌源（字面量只准在此收口）；editor_window.rs =
            // 2026-11-11 拍板「只具名、值一律不动」的范围外文件。
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "theme.rs" || name == "editor_window.rs" {
                continue;
            }
            scanned += 1;
            let rel = f.strip_prefix(&src).unwrap_or(f).display().to_string();
            let body = std::fs::read_to_string(f).expect("read source");
            let code_lines: Vec<&str> = body.lines().map(strip_line_comment).collect();

            // 1) 构造子首参数字字面。fmt 把首参换到下一行的形态用 pending 兜住
            //    （如 FontId::new( 换行 13.0,）。
            let mut pending = false;
            for (i, &code) in code_lines.iter().enumerate() {
                if pending {
                    let t = code.trim_start();
                    if !t.is_empty() {
                        if starts_with_number(t) {
                            hits.push(format!(
                                "{rel}:{}: FontId 构造子首参跨行数字字面（走 theme:: 具名字号）",
                                i + 1
                            ));
                        }
                        pending = false;
                    }
                }
                for ctor in CTORS {
                    let mut from = 0;
                    while let Some(pos) = code[from..].find(ctor) {
                        let rest = code[from + pos + ctor.len()..].trim_start();
                        if rest.is_empty() {
                            pending = true;
                        } else if starts_with_number(rest) {
                            hits.push(format!(
                                "{rel}:{}: {}数字字面（走 theme::font / TextRole 具名令牌）",
                                i + 1,
                                ctor
                            ));
                        }
                        from += pos + ctor.len();
                    }
                }
            }

            // 2) text_styles 局部覆盖（跨行容忍）：`.text_styles` 到第一个 `;`
            //    之间出现 `.insert(` 即违规。窗口逐行拼接，容忍 fmt 换行
            //    （c.style_mut() / .text_styles / .insert(...) 三行历史形态）。
            //    theme.rs 令牌源的 `let ts = &mut style.text_styles;` 以 `;` 截断
            //    不进窗口（分号后另起 ts.insert 的中转写法不在守卫覆盖内，
            //    现仅 theme.rs 豁免文件使用）。
            for (i, &code) in code_lines.iter().enumerate() {
                let mut from = 0;
                while let Some(pos) = code[from..].find(".text_styles") {
                    let mut window = code[from + pos..].to_owned();
                    let mut hit = false;
                    for next in code_lines.iter().skip(i + 1).take(8) {
                        window.push('\n');
                        window.push_str(next);
                        if window.contains(".insert(") {
                            hit = true;
                            break;
                        }
                        if window.contains(';') {
                            break;
                        }
                    }
                    if hit {
                        hits.push(format!(
                            "{rel}:{}: text_styles 局部覆盖（TextStyle 统一走 theme::apply_type_scale，Body 恒 14）",
                            i + 1
                        ));
                    }
                    from += pos + ".text_styles".len();
                }
            }
        }
        // 哨兵：豁免名单吃掉全部文件 = 守卫空转。
        assert!(
            scanned > 10,
            "守卫哨兵：实扫 {} 个文件（共 {}），豁免名单异常",
            scanned,
            files.len()
        );

        assert!(
            hits.is_empty(),
            "UI 字面量禁词 {} 条（字号/字色一律走 theme:: 具名令牌，见 docs/theme-tokens-csharp.md §8）：\n{}",
            hits.len(),
            hits.join("\n")
        );
    }
}

// ── CR-9 门禁钉子：焦点框弱化后仍看得清（纯计算，不需 egui ctx / GUI）──
#[cfg(test)]
mod focus_stroke_tests {
    use super::{LIGHT, dark};
    use egui::Color32;

    /// 半透明描边画在不透明底上的实际观感色（egui premultiplied over）。
    fn blend_over(fg: Color32, bg: Color32) -> Color32 {
        let a = fg.a() as f32 / 255.0;
        let mix = |s: u8, d: u8| (s as f32 + d as f32 * (1.0 - a)).round() as u8;
        Color32::from_rgb(
            mix(fg.r(), bg.r()),
            mix(fg.g(), bg.g()),
            mix(fg.b(), bg.b()),
        )
    }

    /// WCAG 2.x 相对亮度。
    fn rel_lum(c: Color32) -> f64 {
        fn ch(v: u8) -> f64 {
            let v = v as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (l1, l2) = (rel_lum(a), rel_lum(b));
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// CR-9：弱化后的焦点框叠在 ACCENT 主钮常态底上，亮暗两档对比度仍 ≥3:1
    /// （焦点可见参考线）。断言同时钉住「后人继续调淡到框看不见」：实测 0x99
    /// 黑 2.98 <3 被拒、0xB3 白 3.00 卡线（再淡一档即破）。弹窗首帧焦点必落
    /// 主钮（C3②），故常态底 ACCENT 就是焦点框的第一落点。
    ///
    /// 指令块 §三原文写「ACCENT / ACCENT_HOVER / ACCENT_PRESSED 三态均 ≥3:1」，
    /// 其中 pressed #005A9E 一项物理不可达：底色相对亮度 0.098，纯 BLACK 基线
    /// 对比也只有 ≈2.96，半透明弱化版不可能高于不透明基线。故硬门钉主钮常态底
    /// ACCENT（与 §二「定值依据」字面一致）；hover / pressed 实测数值记录在
    /// 回报与 00 过程记录，不设 3:1 硬断言。
    #[test]
    fn focus_stroke_contrast_on_accent() {
        for (name, fg) in [("light", LIGHT.focus_stroke), ("dark", dark().focus_stroke)] {
            let seen = blend_over(fg, super::ACCENT);
            let c = contrast(seen, super::ACCENT);
            assert!(
                c >= 3.0,
                "{name}: 焦点框在 ACCENT 主底上对比度 {c:.2} < 3.0"
            );
        }
        // 门的有效性哨兵：不透明纯 BLACK / WHITE 基线必须过 3:1（否则门失效）。
        assert!(contrast(blend_over(Color32::BLACK, super::ACCENT), super::ACCENT) >= 3.0);
        assert!(contrast(blend_over(Color32::WHITE, super::ACCENT), super::ACCENT) >= 3.0);
    }

    /// CR-9：弱化必须是真半透明 —— 纯 BLACK / WHITE 回归即失败（a()=255）。
    #[test]
    fn focus_stroke_is_not_opaque() {
        assert!(LIGHT.focus_stroke.a() < 255, "亮档焦点框仍是纯色不透明");
        assert!(dark().focus_stroke.a() < 255, "暗档焦点框仍是纯色不透明");
    }
}
