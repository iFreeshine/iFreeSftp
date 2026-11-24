use egui::{
    Align, Align2, Area, Color32, FontId, Id, Layout, Margin, Order, Pos2, Rect, RichText, Sense,
    Stroke, Vec2,
};

use crate::i18n::{t, t_ctx, tfmt};
use crate::theme::{self, BTN_H, CLOSE_SIZE, CONTENT_INSET, CONTENT_PAD, GROUP_GAP, TITLE_BAR_H};

use super::State;
use super::{CursorLayer, cursor_intent};

static COMBO_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const TITLE_PAD: (f32, f32, f32, f32) = (
    CONTENT_INSET + CONTENT_PAD.0,
    5.0,
    CONTENT_INSET + CONTENT_PAD.0,
    2.0,
);

pub(super) const MENU_W: f32 = 200.0;

pub(super) const MENU_ITEM_H: f32 = 36.0;

const MENU_SEP_H: f32 = 3.0;

pub(super) const MENU_BODY_PAD: f32 = 2.0;

const TBL_HEAD_SIZE: f32 = 12.0;

const TBL_BODY_SIZE: f32 = 11.0;

const TBL_CELL_PAD: i8 = 2;

const TBL_VAL_GAP: f32 = 12.0;

pub enum Dialog {
    None,

    Rename {
        pane: usize,
        orig: String,
        name: String,

        fresh: bool,
    },

    Delete {
        pane: usize,
        indices: Vec<usize>,
    },

    Checksum {
        name: String,
        path: String,
        side: Option<usize>,
        algo: usize,
        copied: Option<f64>,
    },

    Permission {
        pane: usize,
        name: String,
        is_dir: bool,
        bits: [bool; 9],
    },

    HostPicker {
        target: usize,
        new_tab: bool,
    },

    TransferConfirm {

        rows: Vec<ConflictRow>,

        note: Option<String>,
    },

    Settings {
        win_drag: Vec2,
    },

    HostPanel {

        list: Vec<crate::hosts::HostProfile>,

        sel: Option<usize>,

        orig_host: String,

        tab: usize,

        port_text: String,

        ka_text: String,

        menu: Option<(usize, Pos2)>,

        alert: Option<(bool, String, String, f64, bool)>,

        test_rx: Option<
            std::sync::mpsc::Receiver<(Result<Option<String>, String>, crate::hosts::HostProfile)>,
        >,

        fp_rx: Option<std::sync::mpsc::Receiver<crate::ssh::Ev>>,

        testing: bool,

        drag: Option<super::host_panel::CardDrag>,

        win_drag: Vec2,

        confirm_del: Option<usize>,
    },

    Import {

        stage: u8,

        consent: bool,

        sources: [bool; 6],

        scanning: bool,

        scan_rx: Option<std::sync::mpsc::Receiver<crate::import::ScanResult>>,

        sessions: Vec<crate::import::ImportedSession>,

        stats: Vec<crate::import::SourceStat>,

        rows: Vec<super::import_dialog::ImportRow>,

        summary: String,

        missing: Vec<crate::import::SourceKind>,

        errors: Vec<(crate::import::SourceKind, &'static str)>,

        truncated: usize,

        imported: usize,
    },

    SyncMismatch {
        msg: String,
    },

    TabClose {
        side: usize,

        tab: u32,

        n: usize,
    },

    ExtEditChanged {
        side: usize,

        tab: u32,
        name: String,
        remote_path: String,
    },

    ExtEditConflict {
        side: usize,

        tab: u32,
        name: String,
        remote_path: String,

        src: ExtConflictSrc,
    },

    ExtEditLockWait {
        side: usize,

        tab: u32,
        name: String,
        remote_path: String,
    },

    Msg {
        title: String,
        msg: String,
    },
}

impl Dialog {

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
            Dialog::Msg { .. } => 15,
        }
    }
}

#[derive(Clone)]
pub enum ExtConflictSrc {

    Internal {
        doc_id: u64,
        local_path: std::path::PathBuf,
    },

    External,
}

pub struct ConflictRow {
    pub name: String,

    pub src_size: u64,
    pub src_mtime: i64,

    pub dst_size: Option<u64>,
    pub dst_mtime: Option<i64>,
}

#[derive(Clone, Copy)]
pub enum QueueMenu {

    Batch,

    Item(u64),
}

pub struct MenuState {
    pub pos: Pos2,
    pub pane: usize,
    pub row: Option<usize>,
    pub queue: Option<QueueMenu>,
    pub terminal: bool,

    pub cols: bool,
}

pub struct FpAsk {

    pub first: bool,
    pub host: String,

    pub stored: Option<String>,

    pub presented: String,
    pub reply: std::sync::mpsc::Sender<bool>,
}

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

pub struct Settings {

    pub category: usize,

    pub theme_picked: usize,

    pub theme_loaded: String,

    pub theme_touched: bool,
    pub lang_en: bool,

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

    pub dnd_stream_batch: i32,
    pub editor_builtin: bool,

    pub editor_path: String,

    pub reg_code: String,

    pub update_check: bool,

    pub reg_hint: Option<(String, bool)>,

    pub update_checking: bool,

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
            tar_accel: false,
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

fn lang_write_back(loaded: &str, lang_en: bool) -> String {
    if lang_en == (loaded == "en-US") {
        loaded.to_owned()
    } else if lang_en {
        "en-US".into()
    } else {
        "zh-CN".into()
    }
}

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

    #[test]
    fn language_empty_preserved_until_explicit() {
        assert_eq!(lang_write_back("", false), "");
        assert_eq!(lang_write_back("", true), "en-US");
        assert_eq!(lang_write_back("zh-CN", false), "zh-CN");
        assert_eq!(lang_write_back("zh-CN", true), "en-US");
        assert_eq!(lang_write_back("en-US", false), "zh-CN");
        assert_eq!(lang_write_back("en-US", true), "en-US");
    }

    #[test]
    fn theme_untouched_keeps_loaded_snapshot() {
        assert_eq!(theme_write_back("", 0, false), "");
        assert_eq!(theme_write_back("Dark", 1, false), "Dark");
        assert_eq!(theme_write_back("Light", 2, false), "Light");
    }

    #[test]
    fn theme_follow_system_writes_empty_and_follows_on_restart() {
        assert_eq!(theme_write_back("Dark", 0, true), "");
        assert_eq!(theme_write_back("Light", 0, true), "");
        let follow = theme_write_back("Dark", 0, true);
        assert!(!crate::theme::resolve_dark(&follow, true));
        assert!(crate::theme::resolve_dark(&follow, false));
    }

    #[test]
    fn theme_explicit_pick_writes_dark_or_light() {
        assert_eq!(theme_write_back("", 1, true), "Dark");
        assert_eq!(theme_write_back("", 2, true), "Light");
        assert_eq!(theme_write_back("Dark", 2, true), "Light");
        assert_eq!(theme_write_back("Light", 1, true), "Dark");
    }
}

impl Settings {

    pub fn from_store() -> Self {
        crate::settings::with(|s| Self {
            category: 0,
            theme_picked: match s.theme.as_str() {
                "Dark" => 1,
                "Light" => 2,
                _ => 0,
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

            last_attempt: None,
        });
    }
}

pub struct Hooks<'a> {
    pub menu: &'a mut Option<MenuState>,

    pub ctx_menu: &'a mut super::ctx_menu::CtxMenu,
    pub dialog: &'a mut Dialog,
    pub crumb_edit: &'a mut Option<(usize, String)>,

    pub list_req: &'a mut Vec<(usize, String)>,

    pub new_req: &'a mut Vec<(usize, String, bool)>,

    pub tree_req: &'a mut Option<(usize, String)>,

    pub sync_nav: &'a mut Option<(usize, String)>,

    pub compare_req: &'a mut bool,

    pub transfer: &'a mut Option<(usize, Vec<usize>)>,

    pub dnd_ready: bool,

    pub drop_upload: &'a mut Option<(usize, Vec<std::path::PathBuf>, bool)>,

    pub drop_stash: &'a [Option<usize>; 2],

    pub drop_resume: &'a mut bool,

    pub drop_clear: &'a mut bool,

    pub pick: &'a mut Option<crate::local::PickJob>,

    pub tab_close: Option<(usize, u32)>,
}

pub(crate) fn fp_ask_pending(st: &State) -> bool {
    st.remotes
        .iter()
        .any(|rs| rs.sessions.iter().any(|s| s.fp_ask.is_some()))
        || st.fp_ask_test.is_some()
}

pub fn draw(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {

    if st.scroll_stash != egui::Vec2::ZERO {
        let d = std::mem::take(&mut st.scroll_stash);
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = d);
    }

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

                st.dialog = Dialog::Settings { win_drag };
            }
        }
        Dialog::HostPanel { .. } => super::host_panel::draw(ui, st, p),
        Dialog::Import { .. } => super::import_dialog::draw(ui, st, p),
        Dialog::SyncMismatch { .. } => sync_mismatch_dialog(ui, st, p),
        Dialog::Msg { .. } => msg_dialog(ui, st, p),
        Dialog::TabClose { .. } => tab_close_dialog(ui, st, p),
        Dialog::ExtEditChanged { .. } => ext_edit_changed_dialog(ui, st, p),
        Dialog::ExtEditConflict { .. } => ext_edit_conflict_dialog(ui, st, p),
        Dialog::ExtEditLockWait { .. } => ext_edit_lock_wait_dialog(ui, st, p),
    }

    if fp_ask_pending(st) {
        fp_confirm_overlay(ui, st, p);
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) enum CloseAction {

    #[default]
    None,

    CancelPendingStream,
}

#[derive(Clone, Copy)]
pub(crate) struct ShellOpts {

    pub mask_close: bool,

    pub on_close: CloseAction,
}

impl Default for ShellOpts {

    fn default() -> Self {
        Self {
            mask_close: true,
            on_close: CloseAction::None,
        }
    }
}

pub(crate) fn shell(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    w: f32,
    h: f32,
    title: &str,
    large: bool,

    row_gap: f32,

    content_card: bool,

    mut win_drag: Option<&mut Vec2>,

    opts: ShellOpts,

    body: impl FnOnce(&mut egui::Ui, Rect, &mut State),
) -> bool {
    let full = ui.max_rect();

    let (w, h) = (w.min(full.width()), h.min(full.height()));

    let combo_open = COMBO_OPEN.swap(false, std::sync::atomic::Ordering::Relaxed);
    let esc = !fp_ask_pending(st)
        && !combo_open
        && ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

    st.shell_enter = !fp_ask_pending(st)
        && !combo_open
        && (ui.ctx().memory(|m| m.focused().is_none()) || ui.ctx().text_edit_focused())
        && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));

    Area::new(Id::unique("dialog-shell"))
        .order(Order::Foreground)
        .fixed_pos(full.min)
        .constrain_to(full)
        .show(ui.ctx(), |ui| {

            ui.ctx().memory_mut(|m| m.set_modal_layer(ui.layer_id()));

            if st.dialog_fresh {
                ui.ctx().memory_mut(|m| m.stop_text_input());
            }

            let mask = ui.allocate_rect(full, Sense::click() - Sense::FOCUSABLE);
            ui.painter().rect_filled(full, 0.0, theme::mask());

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

            ui.allocate_rect(card, Sense::click() - Sense::FOCUSABLE);

            let inner = card.shrink(1.0);

            let title_bar = Rect::from_min_max(
                inner.min,
                Pos2::new(inner.right(), inner.top() + TITLE_BAR_H),
            );
            let drag = ui.allocate_rect(title_bar, Sense::click_and_drag() - Sense::FOCUSABLE);
            if let Some(off) = win_drag.as_deref_mut() {
                let delta = drag.drag_delta();
                if delta != Vec2::ZERO {
                    *off += delta;

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

                theme::font_title(large),
                p.text_primary,
            );

            let close_r = Rect::from_center_size(
                Pos2::new(
                    title_bar.right() - TITLE_PAD.2 - CLOSE_SIZE / 2.0,
                    title_bar.center().y,
                ),
                egui::vec2(CLOSE_SIZE, CLOSE_SIZE),
            );
            let close = ui.allocate_rect(close_r, Sense::click());

            close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, t("关闭")));
            if close.hovered() {
                let bg = if close.is_pointer_button_down_on() {
                    p.row_pressed
                } else {
                    p.row_hover
                };
                ui.painter().rect_filled(close_r, 4.0, bg);
            }

            if close.has_focus() {
                ui.painter().rect_stroke(
                    close_r,
                    4.0,
                    theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.focus_stroke),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(img) = st.icons.get(ui.ctx(), "x", p.text_primary, icon_size) {
                img.paint_at(
                    ui,
                    Rect::from_center_size(close_r.center(), egui::vec2(icon_size, icon_size)),
                );
            }

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

                match opts.on_close {
                    CloseAction::CancelPendingStream => super::cancel_pending_stream(st),
                    CloseAction::None => {}
                }
                st.dialog = Dialog::None;
                return false;
            }

            body(&mut content_ui(ui, content), content, st);
            true
        })
        .inner
}

fn content_ui(ui: &mut egui::Ui, content: Rect) -> egui::Ui {
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content)
            .layout(Layout::top_down(Align::Min)),
    )
}

pub(crate) fn std_button(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &str,
    fg: Color32,
    p: &theme::Palette,
) -> bool {
    std_button_fs(ui, rect, label, fg, p, theme::font_body())
}

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

pub(crate) fn std_button_en(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &str,
    fg: Color32,
    p: &theme::Palette,
    font: FontId,
    enabled: bool,
) -> bool {

    let rect = theme::snap_rect(rect, ui.ctx().pixels_per_point());
    let resp = ui.allocate_rect(rect, Sense::click());

    if enabled && resp.hovered() {
        cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::PointingHand);
    }

    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let (bg, stroke_c, text_c) = if !enabled {

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

    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect,
            4.0,
            theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.focus_stroke),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, label, font, text_c);
    enabled && resp.clicked()
}

fn danger_button(ui: &mut egui::Ui, rect: Rect, label: &str, p: &theme::Palette) -> bool {
    std_button(ui, rect, label, p.critical, p)
}

pub(crate) fn btn_w(ui: &egui::Ui, label: &str, font: &FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), font.clone(), theme::MEASURE_ONLY)
        .size()
        .x
        + 22.0
}

pub(crate) enum TextMode {

    Wrap,

    Truncate,

    Single,
}

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

pub(crate) fn elide22(s: &str) -> String {
    let mut out: String = s.chars().take(21).collect();
    if s.chars().count() > 21 {
        out.push('…');
    }
    out
}

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

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        f(a.r(), b.r()),
        f(a.g(), b.g()),
        f(a.b(), b.b()),
        f(a.a(), b.a()),
    )
}

pub(crate) fn toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    on: bool,
    enabled: bool,
    p: &theme::Palette,
) -> bool {
    let resp = ui.allocate_rect(rect, Sense::click());

    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, enabled, on, ""));
    let t = ui
        .ctx()
        .animate_value_with_time(resp.id, f32::from(on), 0.15);
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();

    let off_fill = if !enabled {
        Color32::TRANSPARENT
    } else if pressed {
        p.toggle_off_fill_pressed
    } else if hovered {
        p.toggle_off_fill_hover
    } else {
        p.toggle_off_fill
    };
    let on_fill = if !enabled {
        p.accent_disabled
    } else if pressed {
        theme::ACCENT_PRESSED
    } else if hovered {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    ui.painter()
        .rect_filled(rect, rect.height() / 2.0, mix(off_fill, on_fill, t));

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

    if resp.has_focus() {
        ui.painter().rect_stroke(
            r,
            4.0,
            theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.focus_stroke),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter()
        .text(r.center(), Align2::CENTER_CENTER, label, font, text_c);
    enabled && resp.clicked()
}

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

    super::ctx_menu::register_auto(te_resp.id, te_resp.interact_rect);

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

    let confirm_w = btn_w(ui, t("重命名"), &theme::font_body());
    let cancel_w = btn_w(ui, t("取消"), &theme::font_body());
    let bx = content.right() - CONTENT_PAD.0;

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

    let enter = ok && st.shell_enter;
    if confirm || enter {
        apply_rename(st, pane, &name);
        st.dialog = Dialog::None;

        ui.ctx().request_repaint();
    } else if cancel {
        st.dialog = Dialog::None;
    } else if let Dialog::Rename { name: n, .. } = &mut st.dialog {
        *n = name;
    }
}

fn menu_side(st: &State, pane: usize) -> Option<usize> {
    (!super::pane_is_local(st, pane)).then_some(pane)
}

fn pane_view(st: &State, pane: usize) -> &super::panes::FilePane {
    super::pane_view(st, pane)
}

fn pane_view_mut(st: &mut State, pane: usize) -> &mut super::panes::FilePane {
    super::pane_view_mut(st, pane)
}

fn other_view(st: &State, pane: usize) -> &super::panes::FilePane {
    super::pane_view(st, 1 - pane)
}

fn view_ready(st: &State, pane: usize) -> bool {
    match menu_side(st, pane) {
        Some(side) => super::active_remote_session(st, side).is_some(),
        None => pane_view(st, pane).path.is_some(),
    }
}

fn apply_rename(st: &mut State, pane: usize, name: &str) {

    let orig = match &st.dialog {
        Dialog::Rename { orig, .. } => orig.clone(),
        _ => String::new(),
    };
    match menu_side(st, pane) {
        Some(side) => {

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

            crate::ui::panes::tree_mark_dirty(entries);
        }
    }
}

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

        theme::dim50(p.text_primary),
    );

    let body_y = content.top() + CONTENT_PAD.1 + 20.0 + GROUP_GAP;

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

    let btn_y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let del_w = btn_w(ui, t("删除"), &theme::font_body());
    let cancel_w = btn_w(ui, t("取消"), &theme::font_body());

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

        ui.ctx().request_repaint();
    }
    if del || st.shell_enter || cancel {
        st.dialog = Dialog::None;
    }
}

pub(crate) fn confirm_delete_shell(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    name: &str,
) -> Option<bool> {
    let mut out: Option<bool> = None;
    let alive = shell(
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
        |ui, content, st| {
            let cui = content_ui(ui, content);
            let x = content.left() + CONTENT_PAD.0;
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
                &elide22(name),
                theme::font_body(),
                TextMode::Single,
                0.0,
                theme::dim50(p.text_primary),
            );
            let body_y = content.top() + CONTENT_PAD.1 + 20.0 + GROUP_GAP;
            dlg_text(
                cui.painter(),
                Pos2::new(x, body_y),
                t("该主机将从列表中移除，保存后生效。"),
                theme::font_body(),
                TextMode::Wrap,
                content.width() - CONTENT_PAD.0,
                p.text_primary,
            );
            let btn_y = content.bottom() - CONTENT_PAD.1 - BTN_H;
            let del_w = btn_w(ui, t("删除"), &theme::font_body());
            let cancel_w = btn_w(ui, t("取消"), &theme::font_body());
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
            if del || st.shell_enter {
                out = Some(true);
            }
            if cancel {
                out = Some(false);
            }
        },
    );
    if !alive {

        out = Some(false);
    }
    out
}

fn apply_delete(st: &mut State) {
    let Dialog::Delete { pane, indices } = &st.dialog else {
        return;
    };
    let mut indices = indices.clone();
    indices.sort_unstable();
    indices.dedup();

    let Some(side) = menu_side(st, *pane) else {

        let view = pane_view(st, *pane);
        let Some(dir) = view.path.clone() else {
            return;
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

const ALGOS: [&str; 3] = ["SHA-256", "SHA-512", "SM3"];

fn hash_local(path: &str, algo: usize) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    use std::io::Read;
    if algo == 2 {

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

fn checksum_start(st: &mut State, algo: usize, path: &str, remote_side: Option<usize>) {
    use crate::ssh::shell_quote;
    if let Some(side) = remote_side {
        let cmd = match algo {
            0 => format!("sha256sum -- {}", shell_quote(path)),
            1 => format!("sha512sum -- {}", shell_quote(path)),

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

        theme::dim50(p.text_primary),
    );
    y += 20.0 + GROUP_GAP;

    cui.painter().text(
        Pos2::new(x, y + 15.0),
        Align2::LEFT_CENTER,
        t("算法"),
        theme::font_body(),
        p.text_primary,
    );

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

    if st.dialog_fresh {
        resp.response.request_focus();
    }

    if algo != prev_algo {
        copied = None;
        checksum_start(st, algo, &path, side);
    }
    y += 32.0 + GROUP_GAP;

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

        _ => t_ctx("复制", "Copy"),
    };
    let copy_w = btn_w(ui, copy_label, &theme::font_body());

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

    let mut val_ui =
        cui.new_child(egui::UiBuilder::new().max_rect(value_r.shrink2(egui::vec2(8.0, 8.0))));
    egui::ScrollArea::vertical()
        .id_salt("chk-value")
        .auto_shrink(false)

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

        theme::dim50(p.text_primary),
    );
    y += 20.0 + GROUP_GAP;

    let rows = [t("所有者"), t("群组"), t("其他")];
    let cols = [t("读"), t("写"), t("执行")];
    let grid_w = content.width() - CONTENT_PAD.0 * 2.0;

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

    let value = perm_value(&bits, is_dir);
    cui.painter().text(
        Pos2::new(content.center().x, y + 8.0),
        Align2::CENTER_CENTER,
        value.as_str(),
        theme::font_small(),
        p.text_secondary,
    );
    y += 16.0 + GROUP_GAP;

    let apply_y = y;
    let apply_w = btn_w(ui, t("应用"), &theme::font_body());
    let cancel_w = btn_w(ui, t("取消"), &theme::font_body());

    let apply_id = ui.next_auto_id();

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

fn host_list_snapshot() -> Vec<crate::hosts::HostProfile> {
    crate::hosts::with(|hs| hs.to_vec())
}

fn host_picker(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (target, new_tab) = match &st.dialog {
        Dialog::HostPicker { target, new_tab } => (*target, *new_tab),
        _ => return,
    };
    let hosts = host_list_snapshot();

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

    let local_clicked = {

        cui.painter().text(
            Pos2::new(content.left() + pad_x, y + 10.0 + 8.0),
            Align2::LEFT_CENTER,
            t("本地"),
            theme::font_small(),
            p.text_secondary,
        );
        y += 30.0;

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

    y += 6.0;
    ui.painter().line_segment(
        [
            Pos2::new(content.left() + pad_x, y),
            Pos2::new(content.right() - pad_x, y),
        ],
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.row_separator),
    );
    y += 6.0;

    cui.painter().text(
        Pos2::new(content.left() + pad_x, y + 12.0 + 8.0),
        Align2::LEFT_CENTER,
        t_ctx("主机", "Hosts"),
        theme::font_small(),
        p.text_secondary,
    );
    y += 32.0;

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

                let row = Rect::from_min_max(
                    Pos2::new(content.left(), y + i as f32 * 48.0),
                    Pos2::new(content.right(), y + (i + 1) as f32 * 48.0),
                );
                let resp = s.allocate_rect(row, Sense::click());
                if resp.hovered() {
                    s.painter().rect_filled(row, 4.0, p.row_hover);
                }

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

        let h = hosts[i].clone();
        st.dialog = Dialog::None;
        connect_profile(st, &h, target, new_tab);
    }
}

pub(crate) fn connect_profile(
    st: &mut State,
    h: &crate::hosts::HostProfile,
    side: usize,
    new_tab: bool,
) {
    let label = host_tab_naming(&h.name, &h.host);

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

    let tab_id = if new_tab || st.remotes[side].tabs.is_empty() {
        let id = st.next_tab_id;
        st.next_tab_id += 1;
        st.remotes[side].tabs.push(super::RemoteTab {
            id,
            sid: 0,
            title: label.clone(),
            pane: super::panes::FilePane {
                empty: true,

                connecting: true,
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

        if side == 1 {
            st.tab_born[side] = Some((tid, std::time::Instant::now()));
        }
        tid
    };

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

    if st.remotes[side].sessions[si].host.is_none() {
        st.remotes[side].sessions[si].host = Some(h.clone());
    }
    let connected = matches!(st.remotes[side].sessions[si].state, super::RemoteState::Off);

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

            super::panes::FilePane {
                empty: true,
                connecting: true,
                ..Default::default()
            }
        };
    }
    if connected {

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

        super::sync::attach_connect(st, side, sid, remote_base);
    }
}

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

    let fp = (!h.host_key_fingerprint.trim().is_empty()).then(|| h.host_key_fingerprint.clone());

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

    let sid = st.next_sid;
    st.next_sid += 1;
    let ssh = crate::ssh::Ssh::spawn();

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

        host: Some(h.clone()),

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

    if let Some(tab) = st.remotes[side].tabs.iter_mut().find(|t| t.id == tab_id) {
        tab.sid = sid;
        tab.pane = super::panes::FilePane {
            empty: true,
            connecting: true,
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

    activate_remote_tab(st, side, tab_id);
    st.sync_browse = false;
    st.sync_base = None;
    st.sync_mirror = None;
}

fn activate_remote_tab(st: &mut State, side: usize, tab_id: u32) {
    st.locals[side].active = None;
    st.remotes[side].active = Some(tab_id);
}

fn activate_local_tab(st: &mut State, side: usize, tab_id: u32) {
    st.remotes[side].active = None;
    st.locals[side].active = Some(tab_id);
}

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

fn open_target(st: &mut State, target: usize, new_tab: bool) {
    let mut pane = super::panes::FilePane::default();
    crate::local::navigate_root(&mut pane);
    if new_tab {

        let id = st.next_tab_id;
        st.next_tab_id += 1;
        st.locals[target].tabs.push(super::LocalTab { id, pane });
        activate_local_tab(st, target, id);
        st.tab_born[target] = Some((id, std::time::Instant::now()));
    } else {

        let rids: Vec<u32> = st.remotes[target].tabs.iter().map(|t| t.id).collect();
        for rid in rids {
            discard_remote_tab(st, target, rid);
        }
        let id = st.next_tab_id;
        st.next_tab_id += 1;
        st.locals[target].tabs = vec![super::LocalTab { id, pane }];
        activate_local_tab(st, target, id);

        st.tab_born[target] = Some((id, std::time::Instant::now()));
    }
}

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

    let has_clear = enabled && focus && !val.is_empty();

    let clear_r = Rect::from_center_size(
        Pos2::new(rect.right() - 14.0, rect.center().y),
        egui::vec2(20.0, 20.0),
    );
    if has_clear {

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

    super::ctx_menu::register_auto(resp.id, resp.interact_rect);

    if has_clear && resp.clicked() {
        let on_x = ui
            .input(|i| i.pointer.latest_pos())
            .is_some_and(|pos| clear_r.contains(pos));
        if on_x {
            val.clear();
        }
    }
}

#[cfg(feature = "mcp")]
const CATS: [&str; 7] = ["外观", "连接", "文件编辑", "传输", "注册", "MCP", "关于"];
#[cfg(not(feature = "mcp"))]
const CATS: [&str; 6] = ["外观", "连接", "文件编辑", "传输", "注册", "关于"];

fn settings_dialog(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    win_drag: &mut Vec2,
) -> bool {

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

fn settings_body(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette, content: Rect) {

    let body = content;

    let nav = Rect::from_min_max(body.min, Pos2::new(body.left() + 150.0, body.bottom()));
    ui.painter().rect_filled(nav, 6.0, p.card_bg);
    let mut y = nav.top() + 8.0;
    for (i, cat) in CATS.iter().enumerate() {
        let item = Rect::from_min_size(
            Pos2::new(nav.left() + 8.0, y),
            egui::vec2(nav.width() - 16.0, 35.0),
        );
        let resp = ui.allocate_rect(item, Sense::click());

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

        if resp.has_focus() {
            ui.painter().rect_stroke(
                item,
                4.0,
                theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.focus_stroke),
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

            card_ui.add_space(16.0);
            match st.settings.category {
                0 => appearance_page(&mut card_ui, st, p),
                1 => connection_page(&mut card_ui, st, p),
                2 => file_edit_page(&mut card_ui, st, p),
                3 => transfer_page(&mut card_ui, st, p),
                #[cfg(feature = "official")]
                4 => registration_page(&mut card_ui, st, p),
                #[cfg(feature = "mcp")]
                5 => mcp_page(&mut card_ui, st, p),
                _ => about_page(&mut card_ui, st, p),
            }

            card_ui.add_space(16.0);

            let measured = card_ui.min_rect();
            s.expand_to_include_rect(measured);
        });

    st.settings.sync_to_store();
}

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

    let card_w = w - indent;
    let pad_l = 16.0;

    let text_w = card_w - pad_l - 16.0 - if ctrl_w > 0.0 { ctrl_w + 8.0 } else { 0.0 };
    let name_h = ui
        .painter()
        .layout_no_wrap(name.to_owned(), name_font.clone(), p.text_primary)
        .size()
        .y;

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

    let text_h = name_h + if desc_h > 0.0 { 7.0 + desc_h } else { 0.0 };
    let h = 10.0 + text_h.max(if ctrl_w > 0.0 { ctrl_h } else { 0.0 }) + 10.0;

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

fn card_toggle(ui: &mut egui::Ui, r: Rect, enabled: bool, val: &mut bool) {
    let rect = Rect::from_min_size(
        Pos2::new(r.right() - 40.0, r.center().y - 10.0),
        egui::vec2(40.0, 20.0),
    );
    if toggle(ui, rect, *val, enabled, &theme::palette(ui.ctx())) {
        *val = !*val;
    }
}

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

    resp.widget_info(|| egui::WidgetInfo::slider(enabled, f64::from(*val), ""));

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

    let rail = Rect::from_min_max(
        Pos2::new(rect.left(), cy - 2.0),
        Pos2::new(rect.right(), cy + 2.0),
    );
    pt.rect_filled(rail, 2.0, rail_c);

    if t > 0.0 {
        pt.rect_filled(
            Rect::from_min_max(rail.min, Pos2::new(thumb_x, rail.max.y)),
            2.0,
            fill_c,
        );
    }

    pt.circle_filled(Pos2::new(thumb_x, cy), 9.0, thumb_c);
}

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

    let rect = Rect::from_min_size(
        Pos2::new(r.left(), r.center().y - 11.25),
        egui::vec2(140.0, 22.5),
    );
    if enabled {
        winui_slider(ui, rect, val, min, max, step, true, &p);
    }

    ui.painter().text(
        Pos2::new(r.right(), r.center().y),
        Align2::RIGHT_CENTER,
        tfmt(suffix, &[&format!("{:.0}", *val)]),
        theme::font(theme::TextRole::Compact),
        p.text_primary,
    );
}

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

    let open = enabled && ctx.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    let rect = Rect::from_center_size(r.center(), egui::vec2(w, 32.0));

    let rect = theme::snap_rect(rect, ctx.pixels_per_point());
    let resp = ui.allocate_rect(
        rect,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );

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

    if resp.has_focus() {
        pt.rect_stroke(
            rect,
            theme::CONTROL_RADIUS,
            theme::stroke(ctx.pixels_per_point(), 1.0, p.focus_stroke),
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

        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            close = true;
        }

        let item_h = 40.0;
        let pop_h = options.len() as f32 * item_h + 2.0;
        let pop_w = rect.width();

        let screen = ctx.input(|i| i.raw.screen_rect).unwrap_or(rect);
        let top = if rect.bottom() + 4.0 + pop_h <= screen.bottom() - 8.0 {
            rect.bottom() + 4.0
        } else {
            rect.top() - 4.0 - pop_h
        };
        let pop_rect = Rect::from_min_size(Pos2::new(rect.left(), top), egui::vec2(pop_w, pop_h));

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

    if open && !close {
        COMBO_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn appearance_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("外观"), p);
    let w = ui.max_rect().width();

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

#[cfg(windows)]
fn pick_exe_file() -> Option<String> {

    #[repr(C)]
    #[allow(non_snake_case)]
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
        Flags: 0x1804,
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
            let mut sel = usize::from(!s.editor_builtin);
            let pc = theme::palette(c.ctx());

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

    settings_card(
        ui,
        w,
        t("编辑器路径"),
        t("外部编辑器可执行文件（如 Sublime Text、VS Code）；留空使用内置编辑器。"),
        300.0,
        !builtin,
        8.0,
        24.0,
        |c, r| {
            let pc = theme::palette(c.ctx());

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

            winui_text_box(
                c,
                box_r,
                egui::Id::unique("settings.editor_path"),
                &mut s.editor_path,
                false,
                theme::role_size(theme::TextRole::Compact),
                !builtin,
                &pc,
            );
            let btn_r = Rect::from_min_size(
                Pos2::new(box_r.right() + 6.0, r.center().y - 16.0),
                egui::vec2(bw, 32.0),
            );

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

#[cfg(feature = "official")]
fn registration_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("注册"), p);
    let w = ui.max_rect().width();
    let s = crate::registration::service::global().state();

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

    let badge_on = s.registered && !s.revoked && s.badge_enabled;
    let badge_text = if s.badge_text.trim().is_empty() {
        t("支持者")
    } else {
        s.badge_text.as_str()
    };
    let status_desc = if s.revoked {
        t("注册已被吊销，请重新激活").to_owned()
    } else if s.registered {

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

    let ctrl_font = theme::font(theme::TextRole::Compact);
    let act_bw = ui
        .painter()
        .layout_no_wrap(t("激活").to_owned(), ctrl_font.clone(), Color32::WHITE)
        .size()
        .x
        + 24.0;
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

            let row_top = r.center().y - 36.0;
            let box_r = Rect::from_min_size(
                Pos2::new(r.right() - 250.0 - 8.0 - act_bw, row_top),
                egui::vec2(250.0, 32.0),
            );
            winui_text_box(
                c,
                box_r,
                egui::Id::unique("settings.reg_code"),
                &mut st.settings.reg_code,
                false,
                theme::role_size(theme::TextRole::Compact),
                true,
                p,
            );
            let btn_r = Rect::from_min_size(
                Pos2::new(box_r.right() + 8.0, row_top),
                egui::vec2(act_bw, 32.0),
            );

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

                                crate::registration::core::ErrorKind::None => {
                                    t("注册码格式错误").to_owned()
                                }
                            },
                            false,
                        ),
                        crate::registration::service::ActivateResult::SaveFailed(e) => (

                            crate::i18n::tfmt("激活未完成：{0}（注册码未能保存，请重试）", &[&e]),
                            false,
                        ),
                    },
                );
            }

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

fn badge_pill_w(ui: &egui::Ui, text: &str) -> f32 {
    let tw = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::font_small(), theme::MEASURE_ONLY)
        .size()
        .x;
    12.0 + 13.0 + 6.0 + tw + 12.0
}

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

fn card_link(ui: &mut egui::Ui, r: Rect, label: &str, url: &str, p: &theme::Palette) {
    let font = theme::font(theme::TextRole::Compact);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), p.accent);

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

#[cfg(feature = "mcp")]
fn mcp_page(ui: &mut egui::Ui, _st: &mut State, p: &theme::Palette) {
    page_title(ui, t("MCP"), p);
    let w = ui.max_rect().width();

    settings_card(
        ui,
        w,
        t("MCP 接口"),
        t(
            "发布版内置 MCP 服务器：MCP 客户端以子命令 ifreesftp mcp（stdio JSON-RPC）拉起本程序，即可浏览目录与收发文件。",
        ),
        56.0,
        true,
        8.0,
        0.0,
        |c, r| {
            card_link(
                c,
                r,
                t("查看文档"),
                "https://github.com/iFreeshine/iFreeSftp/blob/main/docs/mcp.md",
                p,
            )
        },
    );

    let path = crate::mcp_daemon::config_path();
    let desc = tfmt(
        "目录白名单（allow_roots）在本文件中声明，未声明时路径类工具不可用：{0}",
        &[&path.display().to_string()],
    );
    settings_card(
        ui,
        w,
        t("配置文件"),
        &desc,
        64.0,
        true,
        8.0,
        0.0,
        |c, r| {
            let open_w = btn_w(c, t("打开"), &theme::font(theme::TextRole::Compact));
            let btn_r = Rect::from_min_size(
                Pos2::new(r.right() - 16.0 - open_w, r.center().y - 16.0),
                egui::vec2(open_w, 32.0),
            );
            if std_button_en(
                c,
                btn_r,
                t("打开"),
                p.text_primary,
                p,
                theme::font(theme::TextRole::Compact),
                true,
            ) {
                if !path.exists() {
                    let _ = std::fs::write(&path, "{\n  \"allow_roots\": []\n}");
                }

                #[cfg(windows)]
                let cmd = "explorer";
                #[cfg(target_os = "macos")]
                let cmd = "open";
                #[cfg(all(unix, not(target_os = "macos")))]
                let cmd = "xdg-open";
                let _ = std::process::Command::new(cmd).arg(&path).spawn();
            }
        },
    );
}

fn about_page(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    page_title(ui, t("关于 iFreeSftp"), p);
    let w = ui.max_rect().width();

    settings_card_fs(
        ui,
        w,
        "iFreeSftp",
        &tfmt("版本 {0}（Rust 迁移版）", &[&env!("CARGO_PKG_VERSION")]),
        56.0,
        32.0,
        true,
        8.0,
        0.0,
        theme::font_brand_compact(),
        |c, r| card_link(c, r, t("访问官网"), "https://ifrees.top", p),
    );

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

                st.settings.update_checking = true;
                st.settings.update_check_result = t("正在检查…").to_owned();
                #[cfg(feature = "official")]
                crate::registration::service::global().refresh_remote(true, c.ctx().clone());
            }

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
        !paused && transferring > 0,
        paused,
        waiting + transferring > 0,
        finished,

        waiting + transferring + done + failed + cancelled > 0,

        waiting + transferring == 0 && finished,
    ]
}

fn draw_menu(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let Some(m) = st.menu.as_ref() else {
        return;
    };

    cursor_intent(ui.ctx(), CursorLayer::Dialog, egui::CursorIcon::Default);
    let pos = m.pos;
    let pane = m.pane;
    let row = m.row;
    let queue = m.queue;
    let term_menu = m.terminal;
    let cols_menu = m.cols;
    let ppp = ui.ctx().pixels_per_point();

    enum Item {

        Label(&'static str, bool, bool),

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

                let (paused, waiting, transferring, done, failed, cancelled) = match &st.transfer {
                    Some(q) => {
                        let (w, x, d, f, c) = q.with_items(|items| {
                            let (mut w, mut x, mut d, mut f, mut c) =
                                (0i64, 0i64, 0i64, 0i64, 0i64);
                            for i in items {
                                match i.status {
                                    crate::transfer::Status::Waiting => w += 1,

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

                let other = other_view(st, pane);
                let path_ok =
                    |pp: &super::panes::FilePane| pp.path.is_some() || pp.remote_path.is_some();
                let can_transfer = !other.empty
                    && path_ok(p)
                    && path_ok(other)
                    && view_ready(st, pane)
                    && view_ready(st, 1 - pane);
                if remote {

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

                let p = pane_view(st, pane);
                let side = menu_side(st, pane);

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

        st.menu = None;
        return;
    }

    let label_n = items.len() - sep_n;
    let h = label_n as f32 * MENU_ITEM_H + sep_n as f32 * MENU_SEP_H + MENU_BODY_PAD * 2.0;

    let full = ui.max_rect();
    let x = pos.x.min(full.right() - MENU_W - 4.0);
    let y = if pos.y + h > full.bottom() - 4.0 {
        full.bottom() - 4.0 - h
    } else {
        pos.y
    };
    let rect = Rect::from_min_size(Pos2::new(x, y), egui::vec2(MENU_W, h));

    ui.allocate_rect(rect, Sense::click());
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        st.menu = None;
        return;
    }

    st.menu_rect = Some(rect);

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

                if *enabled && resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(r, 4.0, p.row_pressed);
                } else if *enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }

                let fg = if !*enabled {
                    p.text_disabled
                } else if *danger && !resp.hovered() {
                    theme::MENU_DANGER
                } else {
                    p.text_primary
                };
                ui.painter().text(

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

    if let Some(label) = action {
        st.menu = None;

        ui.ctx().request_repaint();

        if cols_menu {
            super::panes::toggle_col(label);
            return;
        }

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

                let snap = {
                    let p = pane_view(st, pane);
                    let side = menu_side(st, pane);
                    row.and_then(|i| p.entries.get(i))
                        .map(|e| (side, e.name.clone(), p.path.clone(), p.remote_path.clone()))
                };
                if let Some((side, name, local_dir, remote_dir)) = snap {
                    match side {
                        None => {

                            if let Some(dir) = local_dir {
                                let path = dir.join(&name);
                                let (external, editor_path) = crate::settings::with(|s| {
                                    (!s.editor_builtin, s.editor_path.trim().to_owned())
                                });
                                if external && !editor_path.is_empty() {
                                    if let Err(e) =
                                        std::process::Command::new(&editor_path).arg(&path).spawn()
                                    {
                                        let msg =
                                            tfmt("外部编辑器启动失败：{0}", &[&e.to_string()]);
                                        st.terminal.push((crate::local::now_hms(), msg.clone()));

                                        if matches!(st.dialog, Dialog::None) {
                                            st.dialog = Dialog::Msg {
                                                title: t("外部编辑器").to_owned(),
                                                msg,
                                            };
                                        }
                                    }
                                } else {
                                    let dark = ui.ctx().theme() == egui::Theme::Dark;
                                    st.editors.open_local(&path, dark);
                                }
                            }
                        }
                        Some(side) => {
                            if let Some(rp) = remote_dir {

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

                            if let Some(rp) = remote_dir {

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

                if !rows.is_empty() {
                    st.dialog = Dialog::Delete {
                        pane,
                        indices: rows,
                    };
                }
            }
            "权限" => {
                if let Some(i) = row {

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

                pane_view_mut(st, pane).refresh_at = Some(ui.ctx().input(|i| i.time));
                if let Some(rp) = pane_view(st, pane).remote_path.clone() {

                    st.list_reqs
                        .push((pane, super::active_tab_id(st, pane).unwrap_or(0), rp));
                } else {
                    crate::local::refresh(pane_view_mut(st, pane));
                }
            }
            "断开" => {

                if let Some(side) = menu_side(st, pane)
                    && let Some(tab_id) = super::active_tab_id(st, pane)
                    && let Some(ti) = st.remotes[side].tabs.iter().position(|x| x.id == tab_id)
                {

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

                if !rows.is_empty() {
                    super::begin_transfer_multi(st, pane, &rows);
                }
            }

            "暂停" => {
                if let Some(q) = &st.transfer {
                    q.pause();

                    st.terminal
                        .push((crate::local::now_hms(), t("传输已暂停").to_owned()));
                }
            }
            "继续传输" => {
                if let Some(q) = &st.transfer {
                    q.resume();

                    st.terminal
                        .push((crate::local::now_hms(), t("传输继续").to_owned()));
                }
            }
            "停止" => {
                if let Some(q) = &st.transfer {
                    q.stop_all();

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

type ConfirmRows = Vec<(String, (String, String), (String, String), bool)>;

fn transfer_confirm_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {

    let (rows, note, defaults): (ConfirmRows, Option<String>, Vec<bool>) = match &st.dialog {
        Dialog::TransferConfirm { rows, note } => {
            let defaults = super::conflict_defaults(rows);
            let rows = rows
                .iter()
                .map(|r| {

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

                    let newer = r.dst_mtime.is_some_and(|m| m > r.src_mtime);
                    (name, src, dst, newer)
                })
                .collect();
            (rows, note.clone(), defaults)
        }
        _ => return,
    };

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

        ShellOpts {
            on_close: CloseAction::CancelPendingStream,
            ..Default::default()
        },
        |ui, content, st| transfer_confirm_body(ui, st, p, content, rows, note),
    );
}

#[allow(clippy::too_many_arguments)]
fn transfer_confirm_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    rows: ConfirmRows,
    note: Option<String>,
) {

    let mut cui = content_ui(ui, content);
    cui.add_space(CONTENT_PAD.1);
    cui.horizontal(|row| {
        row.add_space(CONTENT_PAD.0);
        row.label(RichText::new(tfmt("目标已存在 {0} 个同名项目：", &[&rows.len()])).size(14.0));
    });

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

                        let gx0 = s.cursor().left();
                        let avail = s.available_width();
                        let gx1 = gx0 + avail;
                        let cell_w = 2.0 * TBL_CELL_PAD as f32;

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

                        let chk_w = 18.0 + cell_w;

                        let rest_w = ((avail - chk_w - name_w - 48.0) / 2.0).max(120.0);

                        let line_color = p.text_secondary.gamma_multiply(0.2);
                        let hline = |g: &mut egui::Ui| {
                            let y = g.cursor().top() - 2.5;
                            g.painter().rect_filled(
                                Rect::from_min_size(Pos2::new(gx0, y), egui::vec2(gx1 - gx0, 1.0)),
                                0.0,
                                line_color,
                            );
                        };

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

                                    let src_pair = [
                                        RichText::new(src.0.as_str())
                                            .size(TBL_BODY_SIZE)
                                            .color(p.text_secondary),
                                        RichText::new(src.1.as_str())
                                            .size(TBL_BODY_SIZE)
                                            .color(p.text_secondary),
                                    ];
                                    cell(g, &src_pair, Some(rest_w));

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

    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_apply = btn_w(ui, t("应用"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_apply = Rect::from_min_size(Pos2::new(x0 - w_apply, y), egui::vec2(w_apply, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_apply - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    ui.painter().text(
        Pos2::new(r_cancel.left(), y - 14.0),
        Align2::LEFT_CENTER,
        t("勾选的冲突项覆盖上传，未勾选的跳过"),
        theme::font(theme::TextRole::Caption),
        p.text_secondary,
    );

    let apply_id = ui.next_auto_id();

    let apply = accent_button(ui, r_apply, t("应用"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(apply_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {

        super::cancel_pending_stream(st);
        st.dialog = Dialog::None;
    } else if apply || st.shell_enter {

        super::enqueue_pending(st);
        st.dialog = Dialog::None;
    }
}

fn select_all(sel: &mut [bool], on: bool) {
    sel.iter_mut().for_each(|b| *b = on);
}

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

    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_retry = btn_w(ui, t("重新建立"), &theme::font_body());
    let w_close = btn_w(ui, t("关闭"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_retry = Rect::from_min_size(Pos2::new(x0 - w_retry, y), egui::vec2(w_retry, BTN_H));
    let r_close = Rect::from_min_size(
        Pos2::new(x0 - w_retry - 8.0 - w_close, y),
        egui::vec2(w_close, BTN_H),
    );

    let retry_id = ui.next_auto_id();
    let retry = accent_button(ui, r_retry, t("重新建立"), true, p, theme::font_body());
    let close = std_button(ui, r_close, t("关闭"), p.text_primary, p);
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(retry_id));
    }
    if retry || st.shell_enter {

        let ev = super::sync::evaluate(st);
        st.dialog = Dialog::None;
        if ev.is_ok() {
            st.sync_browse = true;
        } else {

            st.terminal.push((
                crate::local::now_hms(),
                t("同步浏览: 两侧目录仍不一致").to_owned(),
            ));
        }
    } else if close {
        st.dialog = Dialog::None;
    }
}

fn msg_dialog(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let (title, msg) = match &st.dialog {
        Dialog::Msg { title, msg } => (title.clone(), msg.clone()),
        _ => return,
    };
    shell(
        ui,
        st,
        p,
        400.0,
        172.0,
        &title,
        false,
        0.0,
        true,
        None,
        ShellOpts::default(),
        |ui, content, st| msg_body(ui, st, p, content, msg),
    );
}

fn msg_body(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette, content: Rect, msg: String) {
    let cui = content_ui(ui, content);
    dlg_text(
        cui.painter(),
        Pos2::new(
            content.left() + CONTENT_PAD.0,
            content.top() + CONTENT_PAD.1,
        ),
        &msg,
        theme::font_body(),
        TextMode::Wrap,
        content.width() - CONTENT_PAD.0 * 2.0,
        p.text_primary,
    );

    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_close = btn_w(ui, t("关闭"), &theme::font_body());
    let r_close = Rect::from_min_size(
        Pos2::new(content.right() - CONTENT_PAD.0 - w_close, y),
        egui::vec2(w_close, BTN_H),
    );
    let close_id = ui.next_auto_id();
    let close = std_button(ui, r_close, t("关闭"), p.text_primary, p);
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(close_id));
    }
    if close || st.shell_enter {
        st.dialog = Dialog::None;
    }
}

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

    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_save = btn_w(ui, t("保存编辑结果"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_save = Rect::from_min_size(Pos2::new(x0 - w_save, y), egui::vec2(w_save, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_save - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    let save_id = ui.next_auto_id();

    let save = accent_button(ui, r_save, t("保存编辑结果"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(save_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {

        st.dialog = Dialog::None;
    } else if save || st.shell_enter {

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

    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_over = btn_w(ui, t("覆盖"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_over = Rect::from_min_size(Pos2::new(x0 - w_over, y), egui::vec2(w_over, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_over - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    let over_id = ui.next_auto_id();

    let overwrite = accent_button(ui, r_over, t("覆盖"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(over_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消"), p.text_primary, p);
    if cancel {

        st.dialog = Dialog::None;
    } else if overwrite || st.shell_enter {

        match src {
            ExtConflictSrc::Internal { doc_id, local_path } => {
                super::doc_upload(st, ui.ctx(), *doc_id, local_path, &remote_path);
            }
            ExtConflictSrc::External => super::ext_upload(st, side, tab, &remote_path),
        }
        st.dialog = Dialog::None;
    }
}

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

    let y = content.bottom() - CONTENT_PAD.1 - BTN_H;
    let w_retry = btn_w(ui, t("立即重试探测"), &theme::font_body());
    let w_cancel = btn_w(ui, t("取消本次回传"), &theme::font_body());
    let x0 = content.right() - CONTENT_PAD.0;
    let r_retry = Rect::from_min_size(Pos2::new(x0 - w_retry, y), egui::vec2(w_retry, BTN_H));
    let r_cancel = Rect::from_min_size(
        Pos2::new(x0 - w_retry - 8.0 - w_cancel, y),
        egui::vec2(w_cancel, BTN_H),
    );

    let retry_id = ui.next_auto_id();
    let retry = accent_button(ui, r_retry, t("立即重试探测"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(retry_id));
    }
    let cancel = std_button(ui, r_cancel, t("取消本次回传"), p.text_primary, p);

    if cancel {

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
            e.lock_dialog_done = false;
            t.lock_msg_on = false;
        }
        st.terminal.push((
            crate::local::now_hms(),
            tfmt("已取消回传：{0}（本地副本保留）", &[&name]),
        ));
        st.dialog = Dialog::None;
    } else if retry || st.shell_enter {

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
            e.lock_dialog_done = false;
            t.lock_msg_on = false;
        }
        st.dialog = Dialog::None;
    }
}

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

    let stay_id = ui.next_auto_id();

    let stay = accent_button(ui, r_stay, t("不关了"), true, p, theme::font_body());
    if st.dialog_fresh {
        ui.ctx().memory_mut(|m| m.request_focus(stay_id));
    }
    if stay || st.shell_enter {
        st.dialog = Dialog::None;
    } else if cancel {

        if let Some(q) = &st.transfer {
            q.cancel_owned((side as u8, tab));
        }
        st.dialog = Dialog::None;
        st.tab_close_wait = Some((side, tab, std::time::Instant::now()));
    } else if bg {

        st.dialog = Dialog::None;
        super::finish_close_tab(st, side, tab);
    }
}

fn fp_confirm_overlay(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {

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

    let (pri, cancel) = Area::new(Id::unique("fp-overlay"))
        .order(Order::Foreground)
        .fixed_pos(full.min)
        .constrain_to(full)
        .show(ui.ctx(), |ui| {

            ui.ctx().memory_mut(|m| m.set_modal_layer(ui.layer_id()));
            ui.allocate_rect(full, Sense::click() - Sense::FOCUSABLE);
            ui.painter().rect_filled(full, 0.0, theme::mask());

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

            let pri_id = ui.next_auto_id();

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

    let decision = if pri {
        Some(true)
    } else if cancel || esc {
        Some(false)
    } else if enter {
        Some(first)
    } else {
        None
    };
    if let Some(ok) = decision {

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
        );
        assert_eq!(host_tab_naming("", "10.0.0.1"), "10.0.0.1");
        assert_eq!(host_tab_naming("", "10.0.0.1"), "10.0.0.1");
        assert_eq!(host_tab_naming("10.0.0.1", "10.0.0.1"), "10.0.0.1");
        assert_eq!(host_tab_naming("vps", ""), "vps");
        assert_eq!(host_tab_naming("", ""), "");
        assert_eq!(host_tab_naming("   ", ""), "");
    }
}

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

        assert!(hash_local(r"Z:\definitely\not\exists", 2).is_err());
    }
}

#[cfg(test)]
mod batch_menu_tests {
    use super::batch_menu_rules;

    #[test]
    fn batch_menu_rules_empty_queue_all_hidden() {

        assert_eq!(batch_menu_rules(false, 0, 0, 0, 0, 0), [false; 6]);
    }

    #[test]
    fn batch_menu_rules_waiting_only_shows_stop_and_clear_all() {

        assert_eq!(
            batch_menu_rules(false, 3, 0, 0, 0, 0),
            [false, false, true, false, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_waiting_with_finished_no_reset() {

        assert_eq!(
            batch_menu_rules(false, 2, 0, 0, 1, 1),
            [false, false, true, true, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_transferring_shows_pause_and_stop() {

        assert_eq!(
            batch_menu_rules(false, 1, 2, 0, 0, 0),
            [true, false, true, false, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_paused_shows_resume() {

        assert_eq!(
            batch_menu_rules(true, 1, 2, 0, 0, 0),
            [false, true, true, false, true, false]
        );
    }

    #[test]
    fn batch_menu_rules_all_finished_shows_reset() {

        assert_eq!(
            batch_menu_rules(false, 0, 0, 1, 1, 1),
            [false, false, false, true, true, true]
        );
    }
}

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

        out.textures_delta.clear();
    }
}

#[cfg(test)]
mod select_all_tests {
    use super::select_all;

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

#[cfg(test)]
mod dialog_shell_tests {
    use std::sync::Mutex;

    use super::*;

    static TEST_SER: Mutex<()> = Mutex::new(());

    fn plain_input() -> egui::RawInput {
        let mut i = egui::RawInput::default();
        i.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        i
    }

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

    #[test]
    fn textbox_clear_button_click_clears_text() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        let mut val = String::from("abc");
        let id = egui::Id::new("t-clear-pin");
        let box_r =
            egui::Rect::from_min_size(egui::Pos2::new(100.0, 100.0), egui::vec2(200.0, 32.0));
        let mut render = |ctx: &egui::Context, input: egui::RawInput, val: &mut String| {
            let mut out = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    winui_text_box(
                        ui,
                        box_r,
                        id,
                        val,
                        false,
                        13.0,
                        true,
                        &theme::palette(ui.ctx()),
                    );
                });
            });
            out.textures_delta.clear();
        };

        for input in click_frames(egui::Pos2::new(130.0, 116.0)) {
            render(&ctx, input, &mut val);
        }
        assert!(!val.is_empty(), "文字区聚焦点击不应清空");

        let clear_pos = egui::Pos2::new(box_r.right() - 14.0, 116.0);
        for input in click_frames(clear_pos) {
            render(&ctx, input, &mut val);
        }
        assert!(val.is_empty(), "点击 × 应清空文本");
    }

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

        out.textures_delta.clear();
    }

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

    fn sync_fixture() -> (egui::Context, State) {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::Theme::Dark);
        let mut st = State::default();
        st.dialog = Dialog::SyncMismatch {
            msg: "两侧目录不一致".into(),
        };
        (ctx, st)
    }

    #[test]
    fn enter_activates_fresh_primary_button() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        let p = theme::palette(&ctx);
        let mut main_btn = None;

        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        assert!(
            ctx.memory(|m| m.focused()).is_some(),
            "C3② 打开首帧初焦点必须落位（主钮「重新建立」）"
        );

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

    #[test]
    fn tab_focus_cannot_escape_modal_dialog() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();

        st.dialog = Dialog::None;
        let p = theme::palette(&ctx);
        let mut main_btn = None;

        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        let main = main_btn.expect("主界面锚点按钮 id");
        assert!(
            ctx.memory(|m| m.focused()).is_none(),
            "无弹窗无输入 = 无焦点"
        );

        frame(&ctx, key_input(egui::Key::Tab), &mut st, &mut main_btn, &p);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(main),
            "对照组：无弹窗时 Tab 应聚焦主界面按钮（机制可用）"
        );

        st.dialog = Dialog::SyncMismatch {
            msg: "两侧目录不一致".into(),
        };
        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);
        let f3 = ctx.memory(|m| m.focused());
        assert!(
            f3.is_some() && f3 != Some(main),
            "弹窗打开首帧焦点必须落弹窗内（C3②），实测 {f3:?}"
        );

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

    #[test]
    fn mask_click_respects_mask_close_opt() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let mask_pos = egui::pos2(10.0, 10.0);

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

    #[test]
    fn combo_open_esc_consumed_dialog_survives() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        let p = theme::palette(&ctx);
        COMBO_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);

        let open_id = Id::unique("test-combo").with("open");
        ctx.data_mut(|d| d.insert_temp(open_id, true));
        COMBO_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);

        let mut val = 0usize;
        let mut out = ctx.run_ui(key_input(egui::Key::Escape), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {

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

        run(plain_input(), &mut val);
        run(plain_input(), &mut val);
        let [press, release] = click_frames(r.center());
        run(press, &mut val);
        run(release, &mut val);

        run(plain_input(), &mut val);
        run(plain_input(), &mut val);

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

        COMBO_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
    }

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
            d.dirty = true;
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

        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);

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

    #[test]
    fn conflict_external_overwrite_routes_ext_upload() {
        let _g = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        let (ctx, mut st) = sync_fixture();
        let p = theme::palette(&ctx);
        st.remotes[0].sessions.push(cr6_sess("h1"));

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

        frame(&ctx, plain_input(), &mut st, &mut main_btn, &p);

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
