//! 标题栏 1:1（对照 C# Views/Header.xaml）。
//!
//! 结构：整条 33px 拖拽区（含品牌区鼠标穿透同效果——按钮后挂命中优先）→
//! 左侧品牌区（logo 26 + 应用名 16 SemiBold，左距 10、组距 10）→
//! 右端系统 caption 按钮（最小化/最大化/关闭，46×33，painter 直绘 Fluent 字形）。
//! 右侧功能按钮组（模式胶囊/主机/设置/主题/语言）在 T2-2 接入。
//!
//! 注意：不要持有 `ui.painter()` 跨 allocate_rect（Painter 借用 Ui）。

use egui::{Align2, Color32, FontId, Pos2, Rect};

use crate::i18n::t;
use crate::theme;

/// 单个 caption 按钮宽（WinUI 默认）。
const CAPTION_W: f32 = 46.0;
/// caption 字形半宽（Fluent 10px 线长）。
const GLYPH: f32 = 5.0;

/// C# FontIcon 14 档（Header.xaml：设置 E713 / 主题 E706·E708 / 语言 E8C1）的
/// Phosphor 渲染边长。Phosphor 字形留白大于 Segoe Fluent Icons，同框视觉偏小
/// 一号（用户实测），+2 补偿：14 → 16。1x 屏恰取 16 桶 1:1 采样。
const ICON14: f32 = 16.0;
/// C# FontIcon 13 档（Header.xaml：主机 E968 / 胶囊 E895·E8C8）的 Phosphor
/// 渲染边长，同上 +2 补偿：13 → 15。
const ICON13: f32 = 15.0;

pub fn draw(ui: &mut egui::Ui, rect: Rect, st: &mut super::State, p: &theme::Palette) {
    // 拖拽区：整条先挂；双击最大化/还原；按住拖动移动窗口（ViewportCommand::StartDrag）。
    let drag = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    if drag.double_clicked() {
        toggle_maximize(ui.ctx());
    }
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    // 品牌区：logo 26×26 + 应用名 16 SemiBold（独立 Name family，见 main.rs BRAND_FONT）。
    let cy = rect.center().y;
    if let Some(img) = st.icons.get_raw(ui.ctx(), "app-logo", 26.0) {
        let logo_r = Rect::from_center_size(
            Pos2::new(rect.left() + 10.0 + 13.0, cy),
            egui::vec2(26.0, 26.0),
        );
        img.paint_at(ui, logo_r);
    }
    ui.painter().text(
        Pos2::new(rect.left() + 10.0 + 26.0 + 10.0, cy),
        Align2::LEFT_CENTER,
        "iFreeSftp",
        theme::font_title(true),
        p.text_primary,
    );

    caption_buttons(ui, rect, p);
    function_buttons(ui, rect, st, p);
}

/// 右侧功能按钮组（C# TitleButtonsPanel，间距 4；胶囊右额外 8）：
/// 模式胶囊(同步/比对) → 主机(蓝) → 设置 → 主题 → 语言；最右是 caption 三钮。
fn function_buttons(ui: &mut egui::Ui, rect: Rect, st: &mut super::State, p: &theme::Palette) {
    const CAPTION_TOTAL: f32 = 46.0 * 3.0;
    const GAP: f32 = 4.0;
    const W36: f32 = 36.0;

    let cy = rect.center().y;
    // 内容区：条上下各收 5 逻辑px（C# Header.xaml.cs UpdateTitleBarMetrics：
    // 按钮高 = caption 条高 − 10，上下各留 5，元素条内垂直居中不顶边）。
    const TITLE_PAD: f32 = 5.0;
    let content = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + TITLE_PAD),
        Pos2::new(rect.right(), rect.bottom() - TITLE_PAD),
    );
    let row = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top()),
        Pos2::new(rect.right() - CAPTION_TOTAL, rect.bottom()),
    );
    let mut x = row.right();

    // 语言 / 主题 / 设置：36×33 透明图标钮。
    x -= W36;
    // tooltip（C# LangToggleBtn 动态文案）：当前中文 → English，反之 → 中文。
    let lang_tip = if crate::i18n::is_en() {
        t("中文")
    } else {
        t("English")
    };
    let lang_btn = super::accent_tip(
        icon_button(
            ui,
            Rect::from_min_size(
                Pos2::new(x, content.top()),
                egui::vec2(W36, content.height()),
            ),
            st,
            "translate",
            ICON14,
            p.text_primary,
            lang_tip,
            p,
        ),
        lang_tip,
    );
    if lang_btn.clicked() {
        // 中英全局翻转（C# LangToggleBtn_Click：SetCulture(zh-CN ↔ en-US)）。
        crate::settings::toggle_language(ui.ctx());
        // 弹窗镜像同步：否则设置弹窗打开时 sync_to_store 用旧值把语言翻回去。
        st.settings.lang_en = !st.settings.lang_en;
    }
    x -= GAP + W36;
    let theme_icon = match ui.ctx().theme() {
        egui::Theme::Light => "moon",
        egui::Theme::Dark => "sun",
    };
    let theme_r = Rect::from_min_size(
        Pos2::new(x, content.top()),
        egui::vec2(W36, content.height()),
    );
    let theme_btn = super::accent_tip(
        icon_button(
            ui,
            theme_r,
            st,
            theme_icon,
            ICON14,
            p.text_primary,
            t("切换主题"),
            p,
        ),
        t("切换主题"),
    );
    if theme_btn.clicked() {
        let next = match ui.ctx().theme() {
            egui::Theme::Light => egui::Theme::Dark,
            egui::Theme::Dark => egui::Theme::Light,
        };
        crate::theme::install(ui.ctx(), next);
        // 显式值落盘 settings.json theme 键（T19-1，C# ThemeService.Toggle :59
        // 即时落盘同语义；空=跟随系统仅存在于从未配置状态）。
        crate::settings::set_theme(next == egui::Theme::Dark);
    }
    x -= GAP + W36;
    let settings_r = Rect::from_min_size(
        Pos2::new(x, content.top()),
        egui::vec2(W36, content.height()),
    );
    let settings_btn = super::accent_tip(
        icon_button(
            ui,
            settings_r,
            st,
            "gear",
            ICON14,
            p.text_primary,
            t("设置"),
            p,
        ),
        t("设置"),
    );
    if settings_btn.clicked() {
        // 每次打开重读存档快照（C# SettingsPanel 构造即读当前值同语义）：
        // theme/language 的 loaded 基线必须新鲜，否则面板未动的字段会把期间
        // 经标题栏钮写入的显式值按旧快照覆盖回去（T19-1/T19-3）。
        st.settings = super::dialogs::Settings::from_store();
        st.dialog = super::dialogs::Dialog::Settings {
            win_drag: egui::Vec2::ZERO,
        };
    }

    // 主机钮：蓝底白字 + 图标（C# HostBtn）。
    let host_font = theme::font(theme::TextRole::Compact);
    let text_w = ui
        .painter()
        .layout_no_wrap(t("主机").to_owned(), host_font.clone(), Color32::WHITE)
        .size()
        .x;
    let host_w = 10.0 + ICON13 + 6.0 + text_w + 10.0;
    x -= GAP + host_w;
    let host_r = Rect::from_min_size(
        Pos2::new(x, content.top()),
        egui::vec2(host_w, content.height()),
    );
    if host_button(ui, host_r, st, host_font).clicked() {
        super::host_panel::open(st);
    }

    // 更新徽标（C# UpdateBadgeBtn，位置在主机钮之后、模式胶囊之前）：更新检查
    // 开启 + 远端报有新版才显示；透明底 accent 图标+文字，点击经 UrlGuard 白名单
    // 打开下载页（C# UrlGuard.TryLaunchAsync(UpdateUrl)）。
    let reg = crate::registration::service::global().state();
    if st.settings.update_check && reg.update_available && !reg.latest_version.is_empty() {
        let font = theme::font_small();
        let label = crate::i18n::tfmt("新版本 v{0}", &[&reg.latest_version]);
        let tw = ui
            .painter()
            .layout_no_wrap(label.clone(), font.clone(), theme::ACCENT)
            .size()
            .x;
        // Padding 8,0 + FontIcon 12（Phosphor +2 补偿渲染 14）+ Spacing 5 + 文字。
        const ICON12: f32 = 14.0;
        let badge_w = 8.0 + ICON12 + 5.0 + tw + 8.0;
        x -= GAP + badge_w;
        let badge_r = Rect::from_min_size(
            Pos2::new(x, content.top()),
            egui::vec2(badge_w, content.height()),
        );
        if update_badge(ui, badge_r, st, &label, font).clicked() {
            crate::registration::guard::launch_url(&reg.update_url);
        }
    }

    // 模式胶囊：Subtle 底 + 1px 分隔线描边 + 圆角 16，内两枚 30×27 圆角 12 开关。
    const CAPSULE_PAD: f32 = 2.0;
    const TG_W: f32 = 30.0;
    const TG_H: f32 = 27.0;
    const TG_GAP: f32 = 2.0;
    let capsule_w = CAPSULE_PAD * 2.0 + TG_W * 2.0 + TG_GAP;
    x -= GAP + 8.0 + capsule_w;
    let capsule = Rect::from_min_size(
        Pos2::new(x, content.top()),
        egui::vec2(capsule_w, content.height()),
    );
    let pr = ui.painter();
    // 胶囊底 = SubtleFillColorSecondary（Header.xaml Capsule Border 未覆写默认）。
    pr.rect_filled(capsule, 16.0, p.row_hover);
    pr.rect_stroke(
        capsule,
        16.0,
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
        egui::StrokeKind::Inside,
    );

    let tg_rect = |i: usize| {
        Rect::from_min_size(
            Pos2::new(
                capsule.left() + CAPSULE_PAD + i as f32 * (TG_W + TG_GAP),
                cy - TG_H / 2.0,
            ),
            egui::vec2(TG_W, TG_H),
        )
    };
    // 规则②（对称口径）：联动面未就绪（两侧非一本地一远程配对 / 远程侧无活动
    // 视图）→ 两钮禁用（C# 同款；C# 对侧恒本地，对称窗格后按 tab 类型判定）。
    let has_remote = super::sync::pair_ready(st);
    // P4 活动对口径：挂接对切走（挂起，base 保留）→ 灯灭（切回自动复亮，
    // 也可点击以当时活动对重新挂接）；sync_browse 状态位本身不销毁。
    let synced = st.sync_browse && super::sync::pair_is_active(st);
    let sync = capsule_toggle(
        ui,
        tg_rect(0),
        st,
        p,
        "arrows-clockwise",
        synced,
        has_remote,
        t("同步浏览"),
    );
    // tooltip（C# HeaderSyncBrowse）：禁用钮不参与命中测试 → 不挂。
    let sync = if has_remote {
        super::accent_tip(sync, t("同步浏览"))
    } else {
        sync
    };
    if sync.clicked() {
        if synced {
            // 关闭联动：一并熄灭比对（C# ToggleSyncBrowse(false) 同语义）。
            st.sync_browse = false;
            if st.compare {
                st.compare = false;
                super::sync::compare_restore_both(st);
            }
        } else {
            // 规则③（手动口径）：点击建立 → 以两侧当前打开目录为基准根直接
            // 建立联动（不依赖主机预设同步目录）；本地侧未打开具体目录等
            // 失败情形弹窗提示。
            match super::sync::attach_manual(st) {
                Ok(()) => st.sync_browse = true,
                Err(key) => super::sync::report_mismatch(st, key),
            }
        }
    }
    // 比对开关（依赖同步浏览活动对；开启时按挂接对两侧列表立即重排，C# ApplyBoth）。
    let cmp_ready = has_remote && synced;
    let cmp = capsule_toggle(
        ui,
        tg_rect(1),
        st,
        p,
        "files",
        st.compare,
        cmp_ready,
        t("文件比对"),
    );
    // tooltip（C# HeaderCompare）：禁用钮不参与命中测试 → 不挂。
    let cmp = if cmp_ready {
        super::accent_tip(cmp, t("文件比对"))
    } else {
        cmp
    };
    if cmp.clicked() {
        if st.compare {
            st.compare = false;
            super::sync::compare_restore_both(st);
        } else {
            st.compare = true;
            super::sync::compare_apply_both(st);
        }
    }
}

/// 透明图标钮：hover/按下用 WinUI 默认控件态（Header.xaml 未覆写 → ControlFill 系）。
fn icon_button(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    icon: &str,
    icon_size: f32,
    fg: Color32,
    // 无障碍名（M-12：与 tooltip 同源；C# XAML Button 的 AutomationProperties.Name 同位）。
    label: &'static str,
    p: &theme::Palette,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    // M-12：自绘图标钮无控件语义，补 widget_info（屏幕阅读器 / AccessKit 检视可见）。
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if resp.hovered() {
        let bg = if resp.is_pointer_button_down_on() {
            p.control_pressed
        } else {
            p.control_hover
        };
        ui.painter().rect_filled(rect, 4.0, bg);
    }
    if let Some(img) = st.icons.get(ui.ctx(), icon, fg, icon_size) {
        img.paint_at(
            ui,
            Rect::from_center_size(rect.center(), egui::vec2(icon_size, icon_size)),
        );
    }
    resp
}

/// 主机钮：品牌蓝底，白图标 + 白字 13（hover/按下加深，白字不变）。
fn host_button(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    font: FontId,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    // M-12：自绘主机钮无控件语义，补 widget_info（C# HostBtn 内容文字同位）。
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, t("主机")));
    let bg = if resp.is_pointer_button_down_on() {
        theme::ACCENT_PRESSED
    } else if resp.hovered() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    ui.painter().rect_filled(rect, 4.0, bg);

    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 10.0, rect.top()),
        Pos2::new(rect.right() - 10.0, rect.bottom()),
    );
    if let Some(img) = st
        .icons
        .get(ui.ctx(), "desktop-tower", Color32::WHITE, ICON13)
    {
        img.paint_at(
            ui,
            Rect::from_min_size(
                Pos2::new(inner.left(), rect.center().y - ICON13 / 2.0),
                egui::vec2(ICON13, ICON13),
            ),
        );
    }
    ui.painter().text(
        Pos2::new(inner.left() + ICON13 + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        t("主机"),
        font,
        Color32::WHITE,
    );
    resp
}

/// 更新徽标（C# Header.xaml UpdateBadgeBtn）：透明底 + accent（#0078D4）E896
/// 图标 + 文字 12；hover #106EBE / 按下 #005A9E（= accent hover/pressed 系），无描边。
fn update_badge(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    label: &str,
    font: FontId,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    // M-12：自绘更新徽标无控件语义，补 widget_info（C# UpdateBadgeBtn 同位）。
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let fg = if resp.is_pointer_button_down_on() {
        theme::ACCENT_PRESSED
    } else if resp.hovered() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    if let Some(img) = st.icons.get(ui.ctx(), "download-simple", fg, 14.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(
                Pos2::new(rect.left() + 8.0 + 7.0, rect.center().y),
                egui::vec2(14.0, 14.0),
            ),
        );
    }
    ui.painter().text(
        Pos2::new(rect.left() + 8.0 + 14.0 + 5.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    resp
}

/// 胶囊内开关：未选中透明底 + 一级色图标（Header.xaml ToggleButton 未覆写前景），
/// hover/按下 ControlFill 系；选中成功绿白图标（C# checked 覆写）。
fn capsule_toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    p: &theme::Palette,
    icon: &str,
    checked: bool,
    enabled: bool,
    // 无障碍名（M-12：与 tooltip 同源，C# HeaderSyncBrowse/HeaderCompare 同位）。
    label: &'static str,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    // M-12：胶囊开关自绘无控件语义，补 widget_info（C# ToggleButton → AccessKit
    // Switch 双态语义等价）+ 选中态；禁用态如实上报 enabled=false。
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, enabled, checked, label));
    let (bg, fg) = if checked {
        (Some(theme::SUCCESS), Color32::WHITE)
    } else if !enabled {
        // 禁用：TextFillColorDisabled（无底）。
        (None, p.text_disabled)
    } else if resp.is_pointer_button_down_on() {
        (Some(p.control_pressed), p.text_primary)
    } else if resp.hovered() {
        (Some(p.control_hover), p.text_primary)
    } else {
        (None, p.text_primary)
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, theme::PILL_RADIUS, bg);
    }
    if let Some(img) = st.icons.get(ui.ctx(), icon, fg, ICON13) {
        img.paint_at(
            ui,
            Rect::from_center_size(rect.center(), egui::vec2(ICON13, ICON13)),
        );
    }
    resp
}

fn toggle_maximize(ctx: &egui::Context) {
    let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
}

/// caption 三钮：最小化 / 最大化(还原) / 关闭。
fn caption_buttons(ui: &mut egui::Ui, rect: Rect, p: &theme::Palette) {
    let maximized = ui.ctx().input(|i| i.viewport().maximized).unwrap_or(false);
    let kinds = [Kind::Minimize, Kind::Maximize, Kind::Close];
    for (i, kind) in kinds.iter().enumerate() {
        let x0 = rect.right() - CAPTION_W * (kinds.len() - i) as f32;
        let brect = Rect::from_min_size(
            Pos2::new(x0, rect.top()),
            egui::vec2(CAPTION_W, rect.height()),
        );
        let resp = ui.allocate_rect(brect, egui::Sense::click());

        // caption 态叠层（Header.xaml.cs UpdateCaptionColors：hover 亮 20 黑/暗 31 白，
        // pressed 亮 13 黑/暗 15 白）。
        let overlay = if resp.is_pointer_button_down_on() {
            p.caption_pressed
        } else if resp.hovered() {
            p.caption_hover
        } else {
            Color32::TRANSPARENT
        };
        let is_close = matches!(kind, Kind::Close);
        let fg = if resp.hovered() && is_close {
            ui.painter()
                .rect_filled(brect, 0.0, theme::CAPTION_CLOSE_HOVER);
            Color32::WHITE
        } else {
            if resp.hovered() {
                ui.painter().rect_filled(brect, 0.0, overlay);
            }
            p.text_primary
        };
        if resp.clicked() {
            ui.ctx().send_viewport_cmd(match kind {
                Kind::Minimize => egui::ViewportCommand::Minimized(true),
                Kind::Maximize => egui::ViewportCommand::Maximized(!maximized),
                Kind::Close => egui::ViewportCommand::Close,
            });
        }

        let c = brect.center();
        let stroke = theme::stroke(ui.ctx().pixels_per_point(), 1.0, fg);
        let pr = ui.painter();
        match kind {
            Kind::Minimize => {
                pr.line_segment(
                    [Pos2::new(c.x - GLYPH, c.y), Pos2::new(c.x + GLYPH, c.y)],
                    stroke,
                );
            }
            Kind::Maximize if maximized => {
                // 还原态：两个错位方框。
                let off = 2.0;
                let r0 = Rect::from_center_size(
                    Pos2::new(c.x - off / 2.0, c.y + off / 2.0),
                    egui::vec2(8.0, 8.0),
                );
                pr.rect_stroke(r0, 0.0, stroke, egui::StrokeKind::Inside);
                let r1 = Rect::from_center_size(
                    Pos2::new(c.x + off / 2.0, c.y - off / 2.0),
                    egui::vec2(8.0, 8.0),
                );
                pr.rect_stroke(r1, 0.0, stroke, egui::StrokeKind::Inside);
            }
            Kind::Maximize => {
                let r = Rect::from_center_size(c, egui::vec2(10.0, 10.0));
                pr.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
            }
            Kind::Close => {
                pr.line_segment(
                    [
                        Pos2::new(c.x - GLYPH, c.y - GLYPH),
                        Pos2::new(c.x + GLYPH, c.y + GLYPH),
                    ],
                    stroke,
                );
                pr.line_segment(
                    [
                        Pos2::new(c.x - GLYPH, c.y + GLYPH),
                        Pos2::new(c.x + GLYPH, c.y - GLYPH),
                    ],
                    stroke,
                );
            }
        }
    }
}

enum Kind {
    Minimize,
    Maximize,
    Close,
}
