use std::sync::mpsc::Receiver;

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};

use crate::hosts::{AuthMode, HostProfile};
use crate::i18n::{t, t_ctx, tfmt};
use crate::theme;

use super::State;
use super::dialogs::{self, ShellOpts, connect_profile, shell, std_button_fs};
use super::{CursorLayer, cursor_intent};

type TestReceipt = (Result<Option<String>, String>, HostProfile);

const SIZE: (f32, f32) = (920.0, 640.0);

const CARD_GAP: f32 = 5.0;

const LEFT_RATIO: f32 = 0.6;

const CARD_PAD: f32 = 8.0;

const ADD_BTN_H: f32 = 32.0;

const CELL: (f32, f32) = (258.0, 64.0);
const CELL_MARGIN: Vec2 = Vec2::new(4.0, 3.0);

const TAB_H: f32 = 40.0;

const ROW_H: f32 = 32.0;
const ROW_STEP: f32 = 46.0;

const LABEL_W: f32 = 88.0;
const COL_GAP: f32 = 12.0;

const FOOT_H: f32 = 51.0;

fn host_card_bg(dark: bool, active: bool) -> Color32 {
    match (dark, active) {
        (true, true) => Color32::from_rgb(0x22, 0x31, 0x4A),
        (true, false) => Color32::from_rgb(0x1B, 0x24, 0x30),
        (false, true) => Color32::from_rgb(0xF0, 0xF0, 0xF0),
        (false, false) => Color32::WHITE,
    }
}

fn host_card_border(dark: bool, active: bool) -> Option<Color32> {
    match (dark, active) {
        (true, true) => Some(Color32::from_rgb(0x4C, 0xC2, 0xFF)),
        (false, true) => Some(Color32::from_rgb(0x00, 0x5F, 0xB8)),
        _ => None,
    }
}

fn tab_bg(p: &theme::Palette, dark: bool, selected: bool) -> Color32 {
    match (dark, selected) {
        (true, true) => Color32::from_rgb(0x0D, 0x14, 0x20),
        (true, false) => Color32::from_rgb(0x2C, 0x3B, 0x52),
        (false, true) => p.nav_selected_bg,
        (false, false) => p.nav_rest_bg,
    }
}

fn tab_bg_pressed(p: &theme::Palette, dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(0x22, 0x31, 0x4A)
    } else {
        p.pressed_bg
    }
}

fn tab_fg(dark: bool) -> Color32 {
    if dark {
        Color32::WHITE
    } else {
        Color32::from_rgb(0x22, 0x22, 0x22)
    }
}

fn info_colors(ok: bool, p: &theme::Palette) -> (Color32, Color32) {
    if ok {
        (p.success_bg, p.success_fg)
    } else {
        (p.critical_bg, p.critical)
    }
}

pub(crate) struct CardDrag {
    src: usize,
    grab: Vec2,
    press: Pos2,
    lifted: bool,
}

#[cfg(not(windows))]
fn unsavable_password_on_this_platform(list: &[HostProfile]) -> bool {
    list.iter()
        .any(|h| h.auth_mode == AuthMode::Password && !h.password.is_empty())
}

#[cfg(windows)]
fn unsavable_password_on_this_platform(_list: &[HostProfile]) -> bool {
    false
}

pub(crate) fn open(st: &mut State) {
    let list = crate::hosts::with(|hs| hs.to_vec());
    let sel = (!list.is_empty()).then_some(0);
    let (orig_host, port_text, ka_text) = sel
        .and_then(|i| list.get(i))
        .map(|h| {
            (
                h.host.clone(),
                h.port.to_string(),
                h.keep_alive_interval_seconds.to_string(),
            )
        })
        .unwrap_or_default();
    let alert = if let Some((backup, reason)) = crate::hosts::load_notice() {
        Some((
            false,
            t("主机配置读取失败").to_owned(),
            tfmt(
                "hosts.json 读取失败（{1}），原文件已备份到 {0}",
                &[&backup, &reason],
            ),
            0.0,
            false,
        ))
    } else if list.iter().any(|h| h.credential_unavailable) {
        Some((
            false,
            t("凭据需重新录入").to_owned(),
            t("部分主机的密码无法在本平台解密，请在密码框重新输入；本次输入仅保存在内存中，不会写入存档").to_owned(),
            0.0,
            false,
        ))
    } else {
        None
    };
    st.dialog = dialogs::Dialog::HostPanel {
        list,
        sel,
        orig_host,
        tab: 0,
        port_text,
        ka_text,
        menu: None,
        alert,
        test_rx: None,
        fp_rx: None,
        testing: false,
        drag: None,
        win_drag: Vec2::ZERO,
    };
}

pub(crate) fn draw(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let dialogs::Dialog::HostPanel {
        mut list,
        mut sel,
        mut orig_host,
        mut tab,
        mut port_text,
        mut ka_text,
        mut menu,
        mut alert,
        mut test_rx,
        mut fp_rx,
        mut testing,
        mut drag,
        mut win_drag,
    } = std::mem::replace(&mut st.dialog, dialogs::Dialog::None)
    else {
        return;
    };
    let dark = ui.ctx().theme() == egui::Theme::Dark;
    let now = ui.ctx().input(|i| i.time);

    if let Some(rx) = &mut fp_rx {
        match rx.try_recv() {
            Ok(crate::ssh::Ev::AskFingerprint {
                first,
                host,
                stored,
                presented,
                reply,
            }) => {
                st.fp_ask_test = Some(dialogs::FpAsk {
                    first,
                    host,
                    stored,
                    presented,
                    reply,
                });
            }
            Ok(crate::ssh::Ev::Log { msg }) => {
                st.terminal.push((crate::local::now_hms(), msg));
                if st.terminal.len() > 400 {
                    st.terminal.drain(0..st.terminal.len() - 400);
                }
            }
            Ok(_) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => fp_rx = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    if testing
        && let Some(rx) = &mut test_rx
        && let Ok((res, hp)) = rx.try_recv()
    {
        testing = false;
        fp_rx = None;
        let (ok, title, msg) = match res {
            Ok(confirmed) => {

                if let Some(fp) = &confirmed
                    && hp.host_key_fingerprint != *fp
                {
                    let was_empty = hp.host_key_fingerprint.trim().is_empty();
                    let saved = crate::hosts::save_single_session(
                        &hp.host,
                        hp.port,
                        &hp.username,
                        fp,
                        &hp.last_remote_path,
                    );
                    if let Some(h) = list.iter_mut().find(|h| {
                        h.host == hp.host && h.port == hp.port && h.username == hp.username
                    }) {
                        h.host_key_fingerprint = fp.clone();
                    }
                    let text = match saved {
                        Ok(()) => {
                            if was_empty {
                                t("已记住主机指纹，下次连接自动校验").to_owned()
                            } else {
                                t("已更新主机指纹存档，下次连接自动校验").to_owned()
                            }
                        }

                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            t("主机尚未保存进存档，指纹仅本会话生效").to_owned()
                        }
                        Err(e) => tfmt("指纹已在本会话生效，但写入存档失败：{0}", &[&e]),
                    };
                    st.terminal.push((crate::local::now_hms(), text));
                    if st.terminal.len() > 400 {
                        st.terminal.drain(0..st.terminal.len() - 400);
                    }
                }
                (true, t("连接成功").to_owned(), String::new())
            }
            Err(e) => (false, t("连接失败").to_owned(), e),
        };
        alert = Some((ok, title, msg, now, true));
    }

    if let Some((_, _, _, t, auto)) = &alert {
        if *auto && now - *t > 2.5 {
            alert = None;
        } else if *auto {
            let left = (2.5 - (now - *t)).max(0.0);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(left + 0.05));
        }
    }
    if testing {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }

    port_text.retain(|c| c.is_ascii_digit());
    ka_text.retain(|c| c.is_ascii_digit());

    let mut connect = false;
    let mut close_requested = false;
    let alive = shell(
        ui,
        st,
        p,
        SIZE.0,
        SIZE.1,
        t("主机设置"),
        true,
        4.0,
        false,
        Some(&mut win_drag),

        ShellOpts {
            mask_close: false,
            ..Default::default()
        },
        |ui, content, st| {
            host_panel_body(
                ui,
                st,
                p,
                content,
                dark,
                now,
                &mut list,
                &mut sel,
                &mut orig_host,
                &mut tab,
                &mut port_text,
                &mut ka_text,
                &mut menu,
                &mut alert,
                &mut test_rx,
                &mut fp_rx,
                &mut testing,
                &mut drag,
                &mut connect,
                &mut close_requested,
            );
        },
    );
    if !alive {
        return;
    }
    if connect {

        let h = list[sel.unwrap_or(0)].clone();
        connect_profile(st, &h, 1, false);
        return;
    }
    if close_requested {
        return;
    }

    if matches!(st.dialog, dialogs::Dialog::None) {
        st.dialog = dialogs::Dialog::HostPanel {
            list,
            sel,
            orig_host,
            tab,
            port_text,
            ka_text,
            menu,
            alert,
            test_rx,
            fp_rx,
            testing,
            drag,
            win_drag,
        };
    }
}

#[expect(clippy::too_many_arguments)]
fn host_panel_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    dark: bool,
    now: f64,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    tab: &mut usize,
    port_text: &mut String,
    ka_text: &mut String,
    menu: &mut Option<(usize, Pos2)>,
    alert: &mut Option<(bool, String, String, f64, bool)>,
    test_rx: &mut Option<Receiver<TestReceipt>>,
    fp_rx: &mut Option<Receiver<crate::ssh::Ev>>,
    testing: &mut bool,
    drag: &mut Option<CardDrag>,
    connect: &mut bool,
    close_requested: &mut bool,
) {
    let body = content;
    let left_w = (body.width() - CARD_GAP) * LEFT_RATIO;
    let left = Rect::from_min_size(body.min, Vec2::new(left_w, body.height()));
    let right = Rect::from_min_max(Pos2::new(left.right() + CARD_GAP, body.top()), body.max);

    draw_left(
        ui, st, p, dark, left, list, sel, orig_host, port_text, ka_text, menu, drag,
    );
    draw_right(
        ui,
        p,
        dark,
        now,
        right,
        list,
        sel,
        orig_host,
        tab,
        port_text,
        ka_text,
        alert,
        test_rx,
        fp_rx,
        testing,
        close_requested,
        st.dialog_fresh,
    );
    if let Some((mi, mpos)) = *menu {
        *connect = draw_card_menu(
            ui, p, now, mpos, mi, list, sel, orig_host, port_text, ka_text, alert, menu,
        );
    }
    draw_alert(ui, st, p, right, alert);
}

fn sync_sel(
    list: &[HostProfile],
    sel: Option<usize>,
    orig_host: &mut String,
    port_text: &mut String,
    ka_text: &mut String,
) {
    if let Some(h) = sel.and_then(|i| list.get(i)) {
        *orig_host = h.host.clone();
        *port_text = h.port.to_string();
        *ka_text = h.keep_alive_interval_seconds.to_string();
    }
}

#[expect(clippy::too_many_arguments)]
fn draw_left(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    dark: bool,
    area: Rect,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    port_text: &mut String,
    ka_text: &mut String,
    menu: &mut Option<(usize, Pos2)>,
    drag: &mut Option<CardDrag>,
) {
    let painter = ui.painter().clone();

    painter.rect_filled(area, 6.0, theme::palette(ui.ctx()).card_bg);

    let btn_gap = 8.0;
    let lbl_add = t("新增主机");
    let lbl_imp = t("导入主机");
    let text_w = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), theme::font_small(), Color32::WHITE)
            .size()
            .x
    };

    let w_add = 12.0 + 16.0 + 6.0 + text_w(lbl_add) + 12.0;
    let w_imp = 12.0 + 16.0 + 6.0 + text_w(lbl_imp) + 12.0;
    let bx = area.center().x - (w_add + w_imp + btn_gap) / 2.0;
    let add_r = Rect::from_min_size(
        Pos2::new(bx, area.top() + CARD_PAD),
        Vec2::new(w_add, ADD_BTN_H),
    );
    let imp_r = Rect::from_min_size(
        Pos2::new(bx + w_add + btn_gap, area.top() + CARD_PAD),
        Vec2::new(w_imp, ADD_BTN_H),
    );
    if accent_icon_button(ui, st, p, add_r, "plus", lbl_add) {
        list.push(HostProfile {
            name: t("新主机").to_owned(),
            ..Default::default()
        });
        *sel = Some(list.len() - 1);
        sync_sel(list, *sel, orig_host, port_text, ka_text);
    }
    if accent_icon_button(ui, st, p, imp_r, "arrow-line-right", lbl_imp) {

        super::import_dialog::open(st);
    }

    let grid_l = area.left() + CARD_PAD;
    let grid_t = area.top() + CARD_PAD + ADD_BTN_H + CARD_PAD;
    for (i, h) in list.iter().enumerate() {
        let col = i % 2;
        let row = i / 2;
        let cell = Rect::from_min_size(
            Pos2::new(grid_l + col as f32 * CELL.0, grid_t + row as f32 * CELL.1),
            Vec2::new(CELL.0, CELL.1),
        );
        let card = cell.shrink2(CELL_MARGIN);
        let active = *sel == Some(i);

        let lifted_here = matches!(drag.as_ref(), Some(d) if d.lifted && d.src == i);
        if lifted_here {
            painter.rect_filled(card, 6.0, host_card_bg(dark, active).gamma_multiply(0.3));
        } else {
            painter.rect_filled(card, 6.0, host_card_bg(dark, active));
            if let Some(b) = host_card_border(dark, active) {
                painter.rect_stroke(
                    card,
                    6.0,
                    theme::stroke(ui.ctx().pixels_per_point(), 1.0, b),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let resp = ui.allocate_rect(cell, Sense::click_and_drag());

        if resp.hovered() {
            cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::Default);
        }
        if resp.clicked() {
            *sel = Some(i);
            sync_sel(list, *sel, orig_host, port_text, ka_text);
        }
        if resp.secondary_clicked() {
            *menu = Some((i, ui.ctx().pointer_interact_pos().unwrap_or(cell.center())));
        }
        if resp.drag_started() {
            let pos = resp.interact_pointer_pos().unwrap_or(card.min);
            *drag = Some(CardDrag {
                src: i,
                grab: pos - card.min,
                press: pos,
                lifted: false,
            });
        }

        if lifted_here {
            continue;
        }

        let icon_c = Pos2::new(card.left() + 10.0 + 9.0, card.center().y);
        let icon_col = if active {
            p.text_primary
        } else {
            p.text_secondary
        };
        if let Some(img) = st.icons.get(ui.ctx(), "monitor", icon_col, 18.0) {
            img.paint_at(ui, Rect::from_center_size(icon_c, Vec2::splat(18.0)));
        }
        let tx = icon_c.x + 9.0 + 10.0;
        painter.text(
            Pos2::new(tx, card.center().y - 9.0),
            Align2::LEFT_CENTER,
            &h.name,
            theme::font_title(false),
            p.text_primary,
        );
        painter.text(
            Pos2::new(tx, card.center().y + 9.0),
            Align2::LEFT_CENTER,
            h.detail(),
            theme::font_small(),
            p.text_secondary,
        );

        let er = Rect::from_center_size(
            Pos2::new(card.right() - 10.0 - 11.5, card.center().y),
            Vec2::splat(23.0),
        );
        let eresp = ui.allocate_rect(er, Sense::click());
        if resp.hovered() {
            if eresp.hovered() {
                painter.rect_filled(er, 4.0, p.hover_bg);

                cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::PointingHand);
            }
            if let Some(img) = st.icons.get(ui.ctx(), "pencil", p.text_primary, 13.0) {
                img.paint_at(ui, Rect::from_center_size(er.center(), Vec2::splat(13.0)));
            }
        }
        if eresp.clicked() {
            *sel = Some(i);
            sync_sel(list, *sel, orig_host, port_text, ka_text);
        }
    }

    let released = ui.ctx().input(|i| i.pointer.any_released());
    if released {
        *drag = None;
    } else if let Some(d) = drag.as_mut()
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        if !d.lifted {
            d.lifted = pos.distance(d.press) > 6.0;
        }
        if d.lifted {
            let cols = 2usize;
            let max_row = list.len().saturating_sub(1) / cols;
            let col = (((pos.x - grid_l) / CELL.0).floor() as usize).min(cols - 1);
            let row =
                (((pos.y - grid_t) / CELL.1).floor() as i64).clamp(0, max_row as i64) as usize;
            let t = (row * cols + col).min(list.len() - 1);
            if t != d.src {
                let it = list.remove(d.src);
                list.insert(t, it);
                *sel = Some(t);
                sync_sel(list, *sel, orig_host, port_text, ka_text);
                d.src = t;
            }
            cursor_intent(ui.ctx(), CursorLayer::Drag, egui::CursorIcon::Grabbing);
        }
    }

    if let Some(d) = drag.as_ref()
        && d.lifted
        && let Some(pos) = ui.ctx().pointer_interact_pos()
        && let Some(h) = list.get(d.src)
    {
        let cell = Rect::from_min_size(
            Pos2::new(
                grid_l + (d.src % 2) as f32 * CELL.0,
                grid_t + (d.src / 2) as f32 * CELL.1,
            ),
            Vec2::new(CELL.0, CELL.1),
        );
        painter.rect_stroke(
            cell,
            8.0,
            theme::stroke(ui.ctx().pixels_per_point(), 1.5, theme::ACCENT),
            egui::StrokeKind::Outside,
        );
        let card = Rect::from_min_size(pos - d.grab, cell.shrink2(CELL_MARGIN).size());
        painter.rect_filled(
            card.translate(Vec2::new(0.0, 4.0)),
            6.0,
            Color32::from_black_alpha(60),
        );
        painter.rect_filled(card, 6.0, host_card_bg(dark, true));
        painter.rect_stroke(
            card,
            6.0,
            theme::stroke(ui.ctx().pixels_per_point(), 1.5, theme::ACCENT),
            egui::StrokeKind::Inside,
        );

        let icon_c = Pos2::new(card.left() + 10.0 + 9.0, card.center().y);
        if let Some(img) = st.icons.get(ui.ctx(), "monitor", p.text_primary, 18.0) {
            img.paint_at(ui, Rect::from_center_size(icon_c, Vec2::splat(18.0)));
        }
        let tx = icon_c.x + 9.0 + 10.0;
        painter.text(
            Pos2::new(tx, card.center().y - 9.0),
            Align2::LEFT_CENTER,
            &h.name,
            theme::font_title(false),
            p.text_primary,
        );
        painter.text(
            Pos2::new(tx, card.center().y + 9.0),
            Align2::LEFT_CENTER,
            h.detail(),
            theme::font_small(),
            p.text_secondary,
        );
    }

    if list.is_empty() {
        painter.text(
            Pos2::new(area.center().x, grid_t + 24.0),
            Align2::CENTER_CENTER,
            t("暂无主机，点 + 新增"),
            theme::font_small(),
            p.text_tertiary,
        );
    }
}

fn accent_icon_button(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    r: Rect,
    icon: &str,
    label: &str,
) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());

    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let bg = dialogs::accent_bg(resp.is_pointer_button_down_on(), resp.hovered(), true, p);
    let painter = ui.painter().clone();
    painter.rect_filled(r, 4.0, bg);
    let tw = painter
        .layout_no_wrap(label.to_owned(), theme::font_small(), Color32::WHITE)
        .size()
        .x;
    let left = r.center().x - (16.0 + 6.0 + tw) / 2.0;
    if let Some(img) = st.icons.get(ui.ctx(), icon, theme::ON_ACCENT_FG, 16.0) {
        img.paint_at(
            ui,
            Rect::from_min_size(Pos2::new(left, r.center().y - 8.0), Vec2::splat(16.0)),
        );
    }
    painter.text(
        Pos2::new(left + 16.0 + 6.0, r.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::font_small(),
        theme::ON_ACCENT_FG,
    );
    resp.clicked()
}

#[expect(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn draw_right(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    dark: bool,
    now: f64,
    area: Rect,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    tab: &mut usize,
    port_text: &mut String,
    ka_text: &mut String,
    alert: &mut Option<(bool, String, String, f64, bool)>,
    test_rx: &mut Option<Receiver<TestReceipt>>,
    fp_rx: &mut Option<Receiver<crate::ssh::Ev>>,
    testing: &mut bool,
    close: &mut bool,

    fresh: bool,
) {
    let painter = ui.painter().clone();

    painter.rect_filled(area, 6.0, theme::palette(ui.ctx()).card_bg);
    let enabled = sel.is_some();

    let strip = Rect::from_min_max(area.min, Pos2::new(area.right(), area.top() + TAB_H));

    let mut x = strip.left() + 9.0;
    let mut spans = [(0.0, 0.0); 3];
    for (i, lb) in [t("常规"), t("同步"), t("传输")].iter().enumerate() {

        let tw = painter
            .layout_no_wrap((*lb).to_owned(), theme::font_small(), Color32::WHITE)
            .size()
            .x
            + 16.0;
        let tr = Rect::from_min_size(Pos2::new(x, strip.top() + 8.0), Vec2::new(tw, TAB_H - 8.0));
        spans[i] = (tr.left(), tr.right());
        let tresp = ui.allocate_rect(tr, Sense::click());
        if enabled && tresp.clicked() {
            *tab = i;
        }
        let is_sel = *tab == i;
        if is_sel && enabled {
            painter.rect_filled(tr, 4.0, tab_bg(p, dark, true));

            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(tr.left(), tr.bottom() - 4.0),
                    Pos2::new(tr.right(), tr.bottom()),
                ),
                0.0,
                tab_bg(p, dark, true),
            );
        } else if enabled && tresp.is_pointer_button_down_on() {

            painter.rect_filled(tr, 4.0, tab_bg_pressed(p, dark));
        } else if enabled && tresp.hovered() {
            painter.rect_filled(tr, 4.0, tab_bg(p, dark, false));
        }

        let fg = if !enabled {
            p.text_disabled
        } else if is_sel {
            tab_fg(dark)
        } else if tresp.is_pointer_button_down_on() {
            p.text_tertiary
        } else {
            p.text_secondary
        };
        painter.text(
            tr.center(),
            Align2::CENTER_CENTER,
            *lb,
            theme::font_small(),
            fg,
        );
        x += tw;
    }

    let y = strip.bottom() - 0.5;
    let (l0, r0) = spans[*tab];
    for (a, b) in [(strip.left(), l0), (r0, strip.right())] {
        if b > a {
            painter.line_segment(
                [Pos2::new(a, y), Pos2::new(b, y)],
                theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.tab_border),
            );
        }
    }

    let area = Rect::from_min_max(
        Pos2::new(area.left(), strip.bottom()),
        Pos2::new(area.right(), area.bottom() - FOOT_H),
    );
    let pad = 10.0;

    let tab_labels: &[&str] = match *tab {
        0 => &[
            t("项目名称"),
            t("IP 地址"),
            t("端口"),
            t("登录方式"),
            t("用户名"),
            t("密码"),
            t("密钥文件路径"),
        ],
        1 => &[
            t("开启同步"),
            t("开启比较"),
            t("本地同步路径"),
            t("远程同步路径"),
        ],
        _ => &[
            "keep-alive",
            t("间隔秒数"),
            t("连接超时（秒）"),
            t("最大重试次数"),
            t("登录重试延迟（秒）"),
        ],
    };
    let label_w = tab_labels
        .iter()
        .map(|s| {
            painter
                .layout_no_wrap((*s).to_owned(), theme::font_small(), p.text_primary)
                .size()
                .x
        })
        .fold(LABEL_W, f32::max);
    let in_x = area.left() + pad + label_w + COL_GAP;
    let in_w = area.right() - pad - in_x;
    let mut y = area.top() + pad;

    let label_clip = Rect::from_min_max(
        Pos2::new(in_x - COL_GAP - label_w, area.top()),
        Pos2::new(in_x - COL_GAP, area.bottom()),
    );

    let label = |y: f32, text: &str| {
        painter.with_clip_rect(label_clip).text(
            Pos2::new(in_x - COL_GAP, y + ROW_H / 2.0),
            Align2::RIGHT_CENTER,
            text,
            theme::font_small(),
            p.text_primary,
        );
    };

    if let Some(i) = *sel
        && let Some(h) = list.get_mut(i)
    {
        match *tab {
            0 => {

                label(y, t("项目名称"));
                text_row(ui, in_x, y, in_w, &mut h.name, false, true, "hp-name");
                y += ROW_STEP;

                label(y, t("IP 地址"));
                text_row(ui, in_x, y, in_w, &mut h.host, false, true, "hp-host");
                y += ROW_STEP;

                label(y, t("端口"));
                text_row(ui, in_x, y, 150.0, port_text, false, true, "hp-port");
                let port = port_text.parse::<i32>().unwrap_or(h.port);
                h.port = port.clamp(1, 65535);
                y += ROW_STEP;

                label(y, t("登录方式"));
                let mut auth_i = usize::from(h.auth_mode == AuthMode::KeyFile);
                dialogs::card_combo(
                    ui,
                    Rect::from_min_size(Pos2::new(in_x, y), Vec2::new(150.0, ROW_H)),
                    "host-auth",
                    &mut auth_i,
                    &[t("密码"), t("密钥文件")],
                    150.0,
                    theme::FONT_SMALL,
                    true,
                );
                h.auth_mode = if auth_i == 1 {
                    AuthMode::KeyFile
                } else {
                    AuthMode::Password
                };
                y += ROW_STEP;

                label(y, t("用户名"));
                text_row(ui, in_x, y, in_w, &mut h.username, false, true, "hp-user");
                y += ROW_STEP;

                if h.auth_mode == AuthMode::Password {

                    label(y, t("密码"));
                    text_row(ui, in_x, y, in_w, &mut h.password, true, true, "hp-pass");
                    y += ROW_STEP;
                }

                label(y, t("密钥文件路径"));
                let old = h.key_file_path.clone();
                let bw_key = painter
                    .layout_no_wrap(t("浏览").to_owned(), theme::font_small(), Color32::WHITE)
                    .size()
                    .x
                    + 22.0;
                text_row(
                    ui,
                    in_x,
                    y,
                    in_w - bw_key - 8.0,
                    &mut h.key_file_path,
                    false,
                    true,
                    "hp-key",
                );
                h.key_file_path = old;
                let br = Rect::from_min_size(
                    Pos2::new(in_x + in_w - bw_key, y),
                    Vec2::new(bw_key, ROW_H),
                );
                if std_button_fs(ui, br, t("浏览"), p.text_primary, p, theme::font_small())
                    && let Some(path) = pick_key_file()
                    && h.key_file_path != path
                {
                    h.key_file_path = path;
                }
            }
            1 => {

                label(y, t("开启同步"));
                if dialogs::toggle(ui, toggle_rect(in_x, y), h.sync_enabled, true, p) {
                    h.sync_enabled = !h.sync_enabled;
                }
                y += ROW_STEP;

                label(y, t("开启比较"));

                if dialogs::toggle(
                    ui,
                    toggle_rect(in_x, y),
                    h.compare_enabled,
                    h.sync_enabled,
                    p,
                ) {
                    h.compare_enabled = !h.compare_enabled;
                }
                y += ROW_STEP;

                label(y, t("本地同步路径"));
                text_row(
                    ui,
                    in_x,
                    y,
                    in_w,
                    &mut h.local_sync_path,
                    false,
                    h.sync_enabled,
                    "hp-sync-local",
                );
                y += ROW_STEP;

                label(y, t("远程同步路径"));
                text_row(
                    ui,
                    in_x,
                    y,
                    in_w,
                    &mut h.remote_sync_path,
                    false,
                    h.sync_enabled,
                    "hp-sync-remote",
                );
            }
            _ => {

                label(y, "keep-alive");
                if dialogs::toggle(ui, toggle_rect(in_x, y), h.keep_alive_enabled, true, p) {
                    h.keep_alive_enabled = !h.keep_alive_enabled;
                }
                y += ROW_STEP;

                label(y, t("间隔秒数"));
                text_row(
                    ui,
                    in_x,
                    y,
                    150.0,
                    ka_text,
                    false,
                    h.keep_alive_enabled,
                    "hp-ka",
                );
                let ka = ka_text
                    .parse::<i32>()
                    .unwrap_or(h.keep_alive_interval_seconds);
                h.keep_alive_interval_seconds = ka.clamp(1, 3600);
                y += ROW_STEP;

                y = slider_row(
                    ui,
                    &painter,
                    y,
                    label_clip,
                    in_x,
                    t("连接超时（秒）"),
                    "{0} 秒",
                    &mut h.timeout_seconds,
                    15.0,
                    30.0,
                    5.0,
                    p,
                );
                y = slider_row(
                    ui,
                    &painter,
                    y,
                    label_clip,
                    in_x,
                    t("最大重试次数"),
                    "{0} 次",
                    &mut h.max_retries,
                    1.0,
                    3.0,
                    1.0,
                    p,
                );
                slider_row(
                    ui,
                    &painter,
                    y,
                    label_clip,
                    in_x,
                    t("登录重试延迟（秒）"),
                    "{0} 秒",
                    &mut h.retry_delay_seconds,
                    3.0,
                    10.0,
                    1.0,
                    p,
                );
            }
        }
    } else {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            t("选择左侧主机查看与编辑"),
            theme::font_small(),
            p.text_tertiary,
        );
    }

    let foot_top = area.bottom();
    painter.line_segment(
        [
            Pos2::new(area.left(), foot_top),
            Pos2::new(area.right(), foot_top),
        ],
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
    );
    let by = foot_top + 9.0;
    let text_w = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), theme::font_small(), Color32::WHITE)
            .size()
            .x
            + 28.0
    };
    let mut x = area.left() + pad;
    let save_label = t("保存");
    let test_label = if *testing {
        t("连接中…")
    } else {
        t("测试连接")
    };
    let save_r = Rect::from_min_size(Pos2::new(x, by), Vec2::new(text_w(save_label), ROW_H));
    x += save_r.width() + 8.0;
    let test_r = Rect::from_min_size(Pos2::new(x, by), Vec2::new(text_w(test_label), ROW_H));
    x += test_r.width() + 8.0;
    let close_r = Rect::from_min_size(Pos2::new(x, by), Vec2::new(text_w(t("关闭")), ROW_H));

    let save_id = ui.next_auto_id();
    if accent_text_button(ui, p, save_r, save_label) {

        for h in &mut *list {
            h.name = h.name.trim().to_owned();
            if h.name.is_empty() {
                h.name = t("新主机").to_owned();
            }
            h.host = h.host.trim().to_owned();
            h.username = h.username.trim().to_owned();
            h.key_file_path = h.key_file_path.trim().to_owned();
            h.local_sync_path = h.local_sync_path.trim().to_owned();
            h.remote_sync_path = h.remote_sync_path.trim().to_owned();
            h.port = h.port.clamp(1, 65535);
            h.keep_alive_interval_seconds = h.keep_alive_interval_seconds.clamp(1, 3600);
            h.timeout_seconds = h.timeout_seconds.clamp(15, 30);
            h.max_retries = h.max_retries.clamp(1, 3);
            h.retry_delay_seconds = h.retry_delay_seconds.clamp(3, 10);
        }
        if let Some(h) = sel.and_then(|i| list.get_mut(i)) {
            if h.host != *orig_host {

                h.host_key_fingerprint.clear();
            }
            *orig_host = h.host.clone();
            *port_text = h.port.to_string();
            *ka_text = h.keep_alive_interval_seconds.to_string();
        }

        if unsavable_password_on_this_platform(list) {

            crate::hosts::replace_working_set(list.clone());
            *alert = Some((
                false,
                t("本平台不保存密码").to_owned(),
                t("请改用密钥文件，或清空密码后在连接时输入；未清空前本平台不会写入存档")
                    .to_owned(),
                now,
                true,
            ));
        } else if let Err(e) = crate::hosts::replace_all(list.clone()) {

            *alert = Some((
                false,
                tfmt("保存失败：{0}（存档未改动，修复后重试）", &[&e]),
                String::new(),
                now,
                true,
            ));
        } else {
            *alert = Some((true, t("保存成功").to_owned(), String::new(), now, false));
        }
    }
    if fresh {
        ui.ctx().memory_mut(|m| m.request_focus(save_id));
    }
    if std_button_fs(
        ui,
        test_r,
        test_label,
        p.text_primary,
        p,
        theme::font_small(),
    ) && !*testing
        && let Some(h) = sel.and_then(|i| list.get(i).cloned())
    {
        if h.host.trim().is_empty() || h.username.trim().is_empty() {
            *alert = Some((
                false,
                t("请先填写 IP 地址和登录名").to_owned(),
                String::new(),
                now,
                true,
            ));
        } else if h.auth_mode == AuthMode::KeyFile && h.key_file_path.trim().is_empty() {
            *alert = Some((
                false,
                t("密钥登录方式需要先选择密钥文件").to_owned(),
                String::new(),
                now,
                true,
            ));
        } else if h.credential_unavailable && h.password.is_empty() {

            *alert = Some((
                false,
                t("凭据需重新录入").to_owned(),
                t("请先在密码框输入本次连接密码；该密码仅保存在内存中").to_owned(),
                now,
                true,
            ));
        } else {

            let (tx, rx) = std::sync::mpsc::channel();
            let (etx, erx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let auth = match h.auth_mode {
                    AuthMode::KeyFile => crate::ssh::Auth::KeyFile(h.key_file_path.clone()),
                    AuthMode::Password => crate::ssh::Auth::Password(h.password.clone()),
                };
                let port = h.port.clamp(1, 65535) as u16;
                let trusted = (!h.host_key_fingerprint.trim().is_empty())
                    .then(|| h.host_key_fingerprint.clone());
                let res = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(async {
                        crate::ssh::connect_sftp_interactive(
                            &h.host,
                            port,
                            &h.username,
                            &auth,
                            trusted,
                            etx,
                        )
                        .await
                    }),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send((res, h));
            });
            *fp_rx = Some(erx);
            *test_rx = Some(rx);
            *testing = true;
        }
    }
    if std_button_fs(
        ui,
        close_r,
        t("关闭"),
        p.text_primary,
        p,
        theme::font_small(),
    ) {
        *close = true;
    }
}

fn toggle_rect(in_x: f32, y: f32) -> Rect {
    Rect::from_center_size(
        Pos2::new(in_x + 20.0, y + ROW_H / 2.0),
        Vec2::new(40.0, 20.0),
    )
}

fn text_row(
    ui: &mut egui::Ui,
    in_x: f32,
    y: f32,
    w: f32,
    val: &mut String,
    secret: bool,
    on: bool,
    key: &'static str,
) {
    dialogs::winui_text_box(
        ui,
        Rect::from_min_size(Pos2::new(in_x, y), Vec2::new(w, ROW_H)),
        egui::Id::unique(key),
        val,
        secret,
        theme::FONT_SMALL,
        on,
        &theme::palette(ui.ctx()),
    );
}

#[expect(clippy::too_many_arguments)]
fn slider_row(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    y: f32,
    label_r: Rect,
    in_x: f32,
    label: &str,
    suffix: &'static str,
    val: &mut i32,
    min: f32,
    max: f32,
    step: f64,
    p: &theme::Palette,
) -> f32 {

    *val = (*val as f32).clamp(min, max) as i32;
    let mut v = *val as f32;
    painter.with_clip_rect(label_r).text(
        Pos2::new(in_x - COL_GAP, y + ROW_H / 2.0),
        Align2::RIGHT_CENTER,
        label,
        theme::font_small(),
        p.text_primary,
    );

    let rect = Rect::from_center_size(
        Pos2::new(in_x + 70.0, y + ROW_H / 2.0),
        Vec2::new(140.0, 22.5),
    );
    dialogs::winui_slider(ui, rect, &mut v, min, max, step, true, p);
    *val = v as i32;
    painter.text(
        Pos2::new(in_x + 150.0, y + ROW_H / 2.0),
        Align2::LEFT_CENTER,
        tfmt(suffix, &[&v]),
        theme::font_small(),
        p.text_primary,
    );
    y + ROW_STEP
}

fn accent_text_button(ui: &mut egui::Ui, p: &theme::Palette, r: Rect, label: &str) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    let bg = dialogs::accent_bg(resp.is_pointer_button_down_on(), resp.hovered(), true, p);
    ui.painter().rect_filled(r, 4.0, bg);

    if resp.has_focus() {
        ui.painter().rect_stroke(
            r,
            4.0,
            theme::stroke(ui.ctx().pixels_per_point(), 2.0, p.focus_stroke),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        theme::font_small(),
        theme::ON_ACCENT_FG,
    );
    resp.clicked()
}

#[expect(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn draw_card_menu(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    now: f64,
    pos: Pos2,
    mi: usize,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    port_text: &mut String,
    ka_text: &mut String,
    alert: &mut Option<(bool, String, String, f64, bool)>,
    menu: &mut Option<(usize, Pos2)>,
) -> bool {
    let full = ui.max_rect();
    let painter = ui.painter().clone();
    let ppp = ui.ctx().pixels_per_point();

    let mw = dialogs::MENU_W;
    let mh = dialogs::MENU_ITEM_H;
    let mut mr = Rect::from_min_size(pos, Vec2::new(mw, mh * 4.0 + dialogs::MENU_BODY_PAD * 2.0));

    let dx = (mr.right() - full.right()).max(0.0) + (full.left() - mr.left()).max(0.0);
    let dy = (mr.bottom() - full.bottom()).max(0.0) + (full.top() - mr.top()).max(0.0);
    mr = mr.translate(egui::vec2(-dx, -dy));

    theme::draw_shadow(ui, mr, 8.0, theme::ShadowKind::Flyout);
    painter.rect_filled(mr, 8.0, p.menu_bg);
    painter.rect_stroke(
        theme::snap_rect(mr, ppp),
        8.0,
        Stroke::new(theme::border_width(ppp), p.combo_popup_stroke),
        egui::StrokeKind::Inside,
    );

    if ui.allocate_rect(full, Sense::click()).clicked() {
        *menu = None;
    }
    let items = [
        (t_ctx("连接", "Connect"), false),
        (t("编辑"), false),
        (t("复制"), false),
        (t("删除"), true),
    ];
    for (idx, (text, danger)) in items.iter().enumerate() {
        let ir = Rect::from_min_size(

            Pos2::new(
                mr.left() + 4.0,
                mr.top() + dialogs::MENU_BODY_PAD + idx as f32 * mh,
            ),
            Vec2::new(mw - 8.0, mh),
        );
        let resp = ui.allocate_rect(ir, Sense::click());

        if resp.is_pointer_button_down_on() {
            painter.rect_filled(ir, 4.0, p.row_pressed);
        } else if resp.hovered() {
            painter.rect_filled(ir, 4.0, p.row_hover);
        }

        let fg = if *danger && !resp.hovered() {
            p.critical
        } else {
            p.text_primary
        };
        painter.text(

            Pos2::new(ir.left() + 11.0, ir.center().y - 0.5),
            Align2::LEFT_CENTER,
            *text,
            theme::font_body(),
            fg,
        );
        if resp.clicked() {
            *menu = None;
            match idx {
                0 => {

                    let h = &list[mi];
                    if h.host.trim().is_empty() || h.username.trim().is_empty() {
                        *alert = Some((
                            false,
                            t("请先填写 IP 地址和登录名").to_owned(),
                            String::new(),
                            now,
                            true,
                        ));
                        *sel = Some(mi);
                    } else if h.auth_mode == AuthMode::KeyFile && h.key_file_path.trim().is_empty()
                    {
                        *alert = Some((
                            false,
                            t("密钥登录方式需要先选择密钥文件").to_owned(),
                            String::new(),
                            now,
                            true,
                        ));
                        *sel = Some(mi);
                    } else {
                        *sel = Some(mi);
                        sync_sel(list, *sel, orig_host, port_text, ka_text);
                        return true;
                    }
                }
                1 => {
                    *sel = Some(mi);
                    sync_sel(list, *sel, orig_host, port_text, ka_text);
                }
                2 => {

                    let mut c = list[mi].clone();
                    c.name = format!("{} {}", c.name, t("副本"));
                    list.insert(mi + 1, c.clone());
                    *sel = Some(mi + 1);
                    sync_sel(list, *sel, orig_host, port_text, ka_text);
                    if let Err(e) = crate::hosts::append_to_disk(c) {
                        *alert = Some((
                            false,
                            tfmt("复制落盘失败：{0}（副本已加入列表，保存可重试）", &[&e]),
                            String::new(),
                            now,
                            true,
                        ));
                    }
                }
                _ => {

                    list.remove(mi);
                    *sel = if list.is_empty() {
                        None
                    } else {
                        Some(mi.min(list.len() - 1))
                    };
                    sync_sel(list, *sel, orig_host, port_text, ka_text);
                }
            }
        }
    }
    false
}

fn draw_alert(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    right: Rect,
    alert: &mut Option<(bool, String, String, f64, bool)>,
) {
    let Some((ok, title, msg, _, _)) = alert.as_ref() else {
        return;
    };
    let h = if msg.is_empty() { 40.0 } else { 56.0 };
    let r = Rect::from_min_max(
        Pos2::new(right.left() + 20.0, right.bottom() - 61.0 - h),
        Pos2::new(right.right() - 20.0, right.bottom() - 61.0),
    );
    let (bg, fg) = info_colors(*ok, p);
    let painter = ui.painter().clone();
    painter.rect_filled(r, 4.0, bg);

    let ic = Pos2::new(r.left() + 20.0, r.center().y);
    painter.circle_filled(ic, 8.0, fg);
    painter.text(
        ic,
        Align2::CENTER_CENTER,
        if *ok { "✓" } else { "!" },
        theme::font(theme::TextRole::Caption),
        Color32::WHITE,
    );
    painter.text(
        Pos2::new(
            r.left() + 38.0,
            r.top() + if msg.is_empty() { h / 2.0 } else { 14.0 },
        ),
        Align2::LEFT_CENTER,
        title,
        theme::font_body(),
        p.text_primary,
    );
    if !msg.is_empty() {
        let line: String = msg.chars().take(72).collect();
        painter.text(
            Pos2::new(r.left() + 38.0, r.top() + 32.0),
            Align2::LEFT_CENTER,
            line,
            theme::font_small(),
            p.text_secondary,
        );
    }

    let xr = Rect::from_center_size(Pos2::new(r.right() - 18.0, r.center().y), Vec2::splat(24.0));
    let xresp = ui.allocate_rect(xr, Sense::click());
    if xresp.hovered() {
        ui.painter().rect_filled(xr, 4.0, p.hover_bg);
    }
    if let Some(img) = st.icons.get(ui.ctx(), "x", p.text_primary, 10.0) {
        img.paint_at(ui, Rect::from_center_size(xr.center(), Vec2::splat(10.0)));
    }
    if xresp.clicked() {
        *alert = None;
    }
}

#[cfg(windows)]
#[repr(C)]
struct OpenFileNameW {
    lstruct_size: u32,
    owner: *mut core::ffi::c_void,
    instance: *mut core::ffi::c_void,
    filter: *const u16,
    custom_filter: *mut u16,
    max_custom_filter: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_file_title: u32,
    initial_dir: *const u16,
    title: *const u16,
    flags: u32,
    file_offset: u16,
    file_extension: u16,
    def_ext: *const u16,
    cust_data: usize,
    hook: *mut core::ffi::c_void,
    template_name: *const u16,
    reserved: *mut core::ffi::c_void,
    reserved_flags: u32,
    flags_ex: u32,
}

#[cfg(windows)]
#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn pick_key_file() -> Option<String> {
    let filter = wide(&format!(
        "{}\0*.ppk;*.pem;*.key\0{}\0*.*\0",
        t("密钥文件 (*.ppk;*.pem;*.key)"),
        t("所有文件 (*.*)")
    ));
    let title = wide(t("选择密钥文件"));
    let mut buf = [0u16; 1024];
    let mut ofn = OpenFileNameW {
        lstruct_size: std::mem::size_of::<OpenFileNameW>() as u32,
        owner: std::ptr::null_mut(),
        instance: std::ptr::null_mut(),
        filter: filter.as_ptr(),
        custom_filter: std::ptr::null_mut(),
        max_custom_filter: 0,
        filter_index: 1,
        file: buf.as_mut_ptr(),
        max_file: buf.len() as u32,
        file_title: std::ptr::null_mut(),
        max_file_title: 0,
        initial_dir: std::ptr::null(),
        title: title.as_ptr(),
        flags: 0x1804,
        file_offset: 0,
        file_extension: 0,
        def_ext: std::ptr::null(),
        cust_data: 0,
        hook: std::ptr::null_mut(),
        template_name: std::ptr::null(),
        reserved: std::ptr::null_mut(),
        reserved_flags: 0,
        flags_ex: 0,
    };
    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(not(windows))]
fn pick_key_file() -> Option<String> {
    None
}
