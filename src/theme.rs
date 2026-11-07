//! 主题令牌：唯一来源 `docs/theme-tokens-csharp.md`（T1a 从 C# 版提取）。
//! 口径：WinUI 内部 hack 机制不迁移，最终视觉值保留（DECISIONS.md D-013）。
//! 新界面代码只允许消费本模块字段，禁止裸色值。

use egui::{Color32, Context, FontId, Style, TextStyle, Theme};

/// 品牌蓝系（亮暗同值，C# 全应用统一）。
pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0x78, 0xD4);
/// hover / pressed（C# ButtonBackgroundPointerOver / Pressed）。
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x10, 0x6E, 0xBE);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x00, 0x5A, 0x9E);
/// 成功绿（Header 连接 toggle checked 口径）。
pub const SUCCESS: Color32 = Color32::from_rgb(0x10, 0x7C, 0x10);
/// 警示黄（SettingsPanel 到期提醒前景）。
#[expect(dead_code)] // T3+ 设置页到期提示条接入后消费
pub const WARNING: Color32 = Color32::from_rgb(0xD4, 0xA0, 0x17);
/// Ubuntu 橙（HostPickerPanel 主机 logo 圆角方块底，亮暗同值）。
pub const UBUNTU: Color32 = Color32::from_rgb(0xE9, 0x54, 0x20);
/// 右键菜单危险红（C# MenuItem isDangerous 硬编码，明暗同值）。
pub const MENU_DANGER: Color32 = Color32::from_rgb(0xE8, 0x11, 0x23);
/// 图标钮 tooltip 前景（C# AccentToolTipStyle Foreground=White，亮暗同值：
/// App.xaml 未分主题字典；背景 = ACCENT 同值不另立令牌）。
pub const TOOLTIP_FG: Color32 = Color32::WHITE;
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
/// 控件描边开关（2026-10-31 二次定案：恢复边框且完全参照 C#——标准按钮单色
/// ControlStrokeColorSecondary；TextBox/ComboBox 走 elevation_border 顶淡底深
/// 渐变近似（C# ControlElevationBorderBrush）。C# 观感：亮主题边线明显、
/// 暗主题两端色近同故不明显。checkbox 方框 / toggle 轨道 / 卡片选中边等
/// 语义描边不受此开关影响。false = 去边框（首版定案，已作废）。
pub const CONTROL_BORDER: bool = true;
/// 下排（队列/终端）初始高度。
pub const BOTTOM_ROW_MIN: f32 = 120.0;

// ── 滚动条令牌（2026-11-06 定案：原 C# 浮动式 4/12 观感偏宽且压住滚动容器
//    右缘描边；整体减半 + 外移 + 50% 透明。WinUI 无对应资源键可引，
//    为项目观感定案档位，裸值只收口此处）─────────────────────
/// 交互态滚动条宽（原 12 减半）。
pub const SCROLL_BAR_WIDTH: f32 = 6.0;
/// 隐藏态浮条宽（原 4 减半）。
pub const SCROLL_BAR_FLOATING_WIDTH: f32 = 2.0;
/// 滚动条距滚动容器右缘的外移量（经负 bar_outer_margin 实现，不压容器描边；
/// 父 painter 裁剪贴边处会吃掉外移量，仅变薄不减位）。
pub const SCROLL_BAR_OUTER_OFFSET: f32 = 2.0;
/// 滚动条手柄不透明度（悬停/拖拽统一 50%；隐藏态保持 egui 默认 0，不悬停不出现）。
pub const SCROLL_BAR_OPACITY: f32 = 0.5;

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
/// 滚动条浮动式（隐藏态 2px / 交互态 6px、外移 2px、手柄 50%，
/// 2026-11-06 减半+外移定案，令牌见本文件顶部滚动条令牌区）、
/// 菜单浮层 OverlayCornerRadius 8、TextEdit 底 ControlFillColorDefault。
fn apply_native_controls(style: &mut Style, p: &Palette) {
    let s = &mut style.spacing;
    s.scroll.bar_width = SCROLL_BAR_WIDTH;
    s.scroll.floating_width = SCROLL_BAR_FLOATING_WIDTH;
    // 负 margin = 整条外移到容器右缘外（egui：max_cross = outer_rect.max - margin）。
    s.scroll.bar_outer_margin = -SCROLL_BAR_OUTER_OFFSET;
    s.scroll.active_handle_opacity = SCROLL_BAR_OPACITY;
    s.scroll.interact_handle_opacity = SCROLL_BAR_OPACITY;
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
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text_secondary);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.separator);
    v.widgets.inactive.corner_radius = CONTROL_RADIUS.into();

    v.widgets.hovered.bg_fill = p.hover_bg;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.separator);
    v.widgets.hovered.corner_radius = CONTROL_RADIUS.into();

    v.widgets.active.bg_fill = p.pressed_bg;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
    v.widgets.active.corner_radius = CONTROL_RADIUS.into();

    v.widgets.open.bg_fill = p.hover_bg;
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

/// 间距基线：ButtonPadding `11,5,11,6`（App.xaml 未覆写它，标准钮用 WinUI 默认）；
/// C# 的 TextControlThemePadding `10,7,10,4` 是垂直居中 hack，机制不迁移（D-013）。
fn apply_spacing(style: &mut Style) {
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 4.0);
    s.button_padding = egui::vec2(11.0, 5.0);
}
