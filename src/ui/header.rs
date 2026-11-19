use egui::{Align2, Color32, FontId, Pos2, Rect};

use crate::i18n::t;
use crate::theme;

const CAPTION_W: f32 = 46.0;

const GLYPH: f32 = 5.0;

const ICON14: f32 = 16.0;

const ICON13: f32 = 15.0;

pub fn draw(ui: &mut egui::Ui, rect: Rect, st: &mut super::State, p: &theme::Palette) {

    let drag = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    if drag.double_clicked() {
        toggle_maximize(ui.ctx());
    }
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

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

fn function_buttons(ui: &mut egui::Ui, rect: Rect, st: &mut super::State, p: &theme::Palette) {
    const CAPTION_TOTAL: f32 = 46.0 * 3.0;
    const GAP: f32 = 4.0;
    const W36: f32 = 36.0;

    let cy = rect.center().y;

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

    x -= W36;

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

        crate::settings::toggle_language(ui.ctx());

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

        st.settings = super::dialogs::Settings::from_store();
        st.dialog = super::dialogs::Dialog::Settings {
            win_drag: egui::Vec2::ZERO,
        };
    }

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

    #[cfg(feature = "official")]
    {
        let reg = crate::registration::service::global().state();
        if st.settings.update_check && reg.update_available && !reg.latest_version.is_empty() {
            let font = theme::font_small();
            let label = crate::i18n::tfmt("新版本 v{0}", &[&reg.latest_version]);
            let tw = ui
                .painter()
                .layout_no_wrap(label.clone(), font.clone(), theme::ACCENT)
                .size()
                .x;

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
    }

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

    let has_remote = super::sync::pair_ready(st);

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

    let sync = if has_remote {
        super::accent_tip(sync, t("同步浏览"))
    } else {
        super::accent_tip_sub(sync, t("同步浏览"), Some(t("需两侧为本地+远程目录配对")))
    };
    if sync.clicked() {
        if synced {

            st.sync_browse = false;
            if st.compare {
                st.compare = false;
                super::sync::compare_restore_both(st);
            }
        } else {

            match super::sync::attach_manual(st) {
                Ok(()) => st.sync_browse = true,
                Err(key) => super::sync::report_mismatch(st, key),
            }
        }
    }

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

    let cmp = if cmp_ready {
        super::accent_tip(cmp, t("文件比对"))
    } else if has_remote {
        super::accent_tip_sub(cmp, t("文件比对"), Some(t("需先开启同步浏览")))
    } else {
        super::accent_tip_sub(cmp, t("文件比对"), Some(t("需两侧为本地+远程目录配对")))
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

fn icon_button(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    icon: &str,
    icon_size: f32,
    fg: Color32,

    label: &'static str,
    p: &theme::Palette,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());

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

fn host_button(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    font: FontId,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());

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

fn update_badge(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    label: &str,
    font: FontId,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());

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

fn capsule_toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut super::State,
    p: &theme::Palette,
    icon: &str,
    checked: bool,
    enabled: bool,

    label: &'static str,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());

    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, enabled, checked, label));
    let (bg, fg) = if checked {
        (Some(theme::SUCCESS), Color32::WHITE)
    } else if !enabled {

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
