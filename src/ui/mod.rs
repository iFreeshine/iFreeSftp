mod ctx_menu;
mod dialogs;
mod editor_window;
mod header;
mod host_panel;
mod import_dialog;

pub(crate) mod panes;
mod sync;

use egui::Rect;

use crate::i18n::{t, tfmt};
use crate::theme;

pub const HEADER_H: f32 = 43.0;

const SAVE_BAR_H: f32 = 26.0;

const SHELL_BORDER: (f32, f32, f32, f32) = (5.0, 0.0, 5.0, 5.0);

const PANE_MIN: f32 = 220.0;

const BOTTOM_MIN: f32 = 40.0;

const TOP_ROW_MIN: f32 = 80.0;

pub(crate) fn accent_tip(resp: egui::Response, text: impl Into<String>) -> egui::Response {
    accent_tip_sub(resp, text, None)
}

pub(crate) fn accent_tip_sub(
    resp: egui::Response,
    text: impl Into<String>,
    sub: Option<&str>,
) -> egui::Response {
    let [ar, ag, ab, _] = theme::ACCENT.to_array();
    let [tr, tg, tb, _] = theme::TOOLTIP_FG.to_array();
    let mut tip = egui::Tooltip::for_enabled(&resp);
    tip.popup = tip.popup.frame(
        egui::Frame::new()

            .fill(egui::Color32::from_rgba_unmultiplied(ar, ag, ab, 204))
            .corner_radius(theme::CONTROL_RADIUS)
            .inner_margin(egui::Margin {

                left: 10,
                right: 10,
                top: 5,
                bottom: 6,
            }),
    );
    tip.show(|ui| {

        ui.set_max_width(ui.spacing().tooltip_width);

        ui.label(
            egui::RichText::new(text.into())
                .color(egui::Color32::from_rgba_unmultiplied(tr, tg, tb, 204))
                .size(12.0),
        );
        if let Some(sub) = sub {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(sub.to_owned())
                    .color(egui::Color32::from_rgba_unmultiplied(tr, tg, tb, 179))
                    .size(12.0),
            );
        }
    });
    resp
}

pub struct PendingTx {

    pub seq: u64,

    pub profile_side: usize,

    pub src_pane: usize,

    pub at: std::time::Instant,
    pub items: Vec<PendingItem>,

    pub resolved: usize,

    pub ready: usize,

    pub batch: usize,

    pub batches: Vec<u64>,

    pub placed: usize,

    pub held: bool,
}

fn preparing_msg() -> String {
    t("正在准备传输（探测目标/统计文件）…").to_owned()
}

fn is_preparing_msg(m: &str, en: bool) -> bool {
    m == crate::i18n::t_for("正在准备传输（探测目标/统计文件）…", en)
}

fn clear_preparing_msg(st: &mut State, pane: usize) {
    let en = crate::i18n::is_en();
    let p = pane_view_mut(st, pane);
    if p.msg.as_deref().is_some_and(|m| is_preparing_msg(m, en)) {
        p.msg = None;
    }
}

fn settle_batch(st: &mut State, profile_side: usize, src_pane: usize, items: Vec<PendingItem>) {
    let (prev_batches, placed) = st
        .pending_tx
        .take()
        .map(|p| (p.batches, p.placed))
        .unwrap_or((Vec::new(), 0));
    st.pending_tx = None;
    clear_preparing_msg(st, src_pane);
    if items.iter().any(|pi| pi.conflict) {
        let conflicts = items.iter().filter(|pi| pi.conflict).count();
        let note = scope_note_for(placed, items.len(), conflicts);
        st.pending_tx = Some(PendingTx {
            seq: 0,
            profile_side,
            src_pane,
            at: std::time::Instant::now(),
            items,
            resolved: 0,
            ready: 0,
            batch: usize::MAX,
            batches: prev_batches,
            placed: 0,
            held: false,
        });
        let rows = match &st.pending_tx {
            Some(ptx) => confirm_data(&ptx.items),
            None => Vec::new(),
        };

        st.conflict_sel = Some(conflict_defaults(&rows));
        st.dialog = dialogs::Dialog::TransferConfirm { rows, note };
    } else {
        enqueue_tasks(
            st,
            profile_side,
            items.into_iter().map(into_task).collect(),
            0,
        );
    }
}

fn scope_note_for(placed: usize, remaining: usize, conflicts: usize) -> Option<String> {
    (placed > 0).then(|| scope_note(placed, remaining, conflicts))
}

fn scope_note(placed: usize, remaining: usize, conflicts: usize) -> String {
    tfmt(
        "已落位 {0} 项已开始传输，本次确认只处理剩余 {1} 项（其中冲突 {2} 项）",
        &[
            &placed.to_string(),
            &remaining.to_string(),
            &conflicts.to_string(),
        ],
    )
}

enum StreamStep {
    None,

    Place,

    Settle,
}

fn stream_step(ptx: &PendingTx) -> StreamStep {
    if ptx.resolved == ptx.items.len() {
        StreamStep::Settle
    } else if ptx.ready >= ptx.batch {
        StreamStep::Place
    } else {
        StreamStep::None
    }
}

fn stream_take(ptx: &mut PendingTx, need: usize) -> Vec<PendingItem> {
    let mut out = Vec::with_capacity(need);
    let mut rest = Vec::with_capacity(ptx.items.len() - need);
    for pi in ptx.items.drain(..) {
        if pi.stat_path.is_none() && !pi.conflict && out.len() < need {
            out.push(pi);
        } else {
            rest.push(pi);
        }
    }
    ptx.items = rest;
    ptx.ready -= need;
    ptx.resolved -= need;
    out
}

fn stream_place(st: &mut State) {
    let (profile_side, items) = {
        let Some(ptx) = &mut st.pending_tx else {
            return;
        };
        (ptx.profile_side, stream_take(ptx, ptx.ready))
    };
    let placed = items.len();
    let tasks: Vec<crate::transfer::NewTask> = items.into_iter().map(into_task).collect();
    let batch = enqueue_tasks(st, profile_side, tasks, 0);
    if let Some(ptx) = &mut st.pending_tx {
        ptx.batches.push(batch);
        ptx.placed += placed;
    }
}

fn stream_settle(st: &mut State) {
    let items = match st
        .pending_tx
        .as_mut()
        .filter(|ptx| ptx.resolved == ptx.items.len())
    {
        Some(ptx) => std::mem::take(&mut ptx.items),
        None => return,
    };
    let Some(ptx) = &st.pending_tx else {
        return;
    };
    let (side, pane) = (ptx.profile_side, ptx.src_pane);
    settle_batch(st, side, pane, items);
}

pub(crate) fn cancel_pending_stream(st: &mut State) {
    st.conflict_sel = None;
    let Some(ptx) = st.pending_tx.take() else {
        return;
    };
    let n = ptx.items.len();
    if !ptx.batches.is_empty()
        && let Some(q) = st.transfer.as_ref()
    {
        q.cancel_batches(&ptx.batches);
    }

    st.terminal.push((
        crate::local::now_hms(),
        tfmt("已取消传输确认：{0} 项未入队", &[&n.to_string()]),
    ));
}

const STAT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn force_settle_expired(ptx: &mut PendingTx, timeout: std::time::Duration) -> usize {
    if ptx.seq != 0 && ptx.at.elapsed() >= timeout {
        let mut n = 0;
        for pi in &mut ptx.items {
            if pi.stat_path.take().is_some() {
                pi.probe_incomplete = true;
                n += 1;
            }
        }
        n
    } else {
        0
    }
}

pub struct PendingItem {
    pub task: crate::transfer::NewTask,

    pub stat_path: Option<String>,
    pub conflict: bool,

    pub src_mtime: i64,

    pub dst_size: Option<u64>,
    pub dst_mtime: Option<i64>,

    pub probe_incomplete: bool,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SessionKey {
    pub host: String,
    pub port: u16,
    pub user: String,
}

pub struct SideSession {

    pub sid: u64,

    pub key: SessionKey,

    pub ssh: crate::ssh::Ssh,

    pub host: Option<crate::hosts::HostProfile>,

    pub profile: Option<crate::transfer::Profile>,

    pub state: RemoteState,

    pub root: Option<String>,

    pub fp_ask: Option<dialogs::FpAsk>,

    pub refs: usize,
}

pub struct RemoteTab {

    pub id: u32,

    pub sid: u64,

    pub title: String,

    pub pane: panes::FilePane,

    pub wait: Option<(u64, String)>,

    pub fallback: Option<String>,

    pub del_pending: u64,

    pub new_pending: u64,

    pub pending_ed: Option<(u64, String, String)>,

    pub ext_edits: Vec<ExtEdit>,

    pub ext_stat: Option<(u64, String)>,

    pub ext_write: Option<u64>,

    pub ext_poll: std::time::Instant,

    pub lock_msg_on: bool,
}

pub struct RemoteSide {
    pub tabs: Vec<RemoteTab>,

    pub active: Option<u32>,

    pub sessions: Vec<SideSession>,
}

pub struct LocalTab {

    pub id: u32,

    pub pane: panes::FilePane,
}

pub struct LocalSide {
    pub tabs: Vec<LocalTab>,
    pub active: Option<u32>,
}

pub struct ExtEdit {

    pub host_tag: String,

    pub remote_path: String,

    pub name: String,

    pub local: std::path::PathBuf,

    pub base_mtime: Option<i64>,

    pub baseline: (u64, i64),

    pub quiet_since: Option<std::time::Instant>,

    pub quiet_snap: Option<(u64, i64)>,

    pub lock_wait_since: Option<std::time::Instant>,

    pub lock_warned: bool,

    pub lock_dialog_done: bool,
}

fn placeholder_remote_tab(id: u32) -> RemoteTab {
    RemoteTab {
        id,
        sid: 0,
        title: t("远程").to_owned(),
        pane: panes::FilePane {
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
    }
}

pub struct LocalJob {
    pub side: usize,
    pub tab_id: u32,
    pub req: crate::local::LocalPaneReq,
    pub rx: std::sync::mpsc::Receiver<crate::local::LocalPaneReply>,
}

pub struct State {
    pub icons: crate::icons::Cache,

    pub sys_icons: crate::icons_sys::Cache,

    pub left_w: f32,

    pub bottom_h: f32,

    pub sync_browse: bool,

    pub compare: bool,

    pub locals: [LocalSide; 2],

    pub remotes: [RemoteSide; 2],

    pub next_tab_id: u32,

    pub next_sid: u64,

    pub tab_born: [Option<(u32, std::time::Instant)>; 2],

    pub dialog: dialogs::Dialog,

    pub menu: Option<dialogs::MenuState>,

    pub menu_rect: Option<egui::Rect>,

    pub reconnecting: u32,

    pub ctx_menu: ctx_menu::CtxMenu,

    pub crumb_edit: Option<(usize, String)>,

    pub settings: dialogs::Settings,

    pub list_reqs: Vec<(usize, u32, String)>,

    pub new_reqs: Vec<(usize, u32, String, bool)>,

    pub rename_reqs: Vec<(usize, u32, String, String)>,

    pub tree_req: Option<(usize, u32, u64, String)>,

    pub sync_base: Option<sync::SyncBase>,

    pub sync_mirror: Option<(usize, u32, String)>,

    pub sync_nav: Option<(usize, u32, String)>,

    pub compare_req: bool,

    pub dnd_transfer: Option<(usize, Vec<usize>)>,

    pub pick: Option<crate::local::PickJob>,

    pub transfer: Option<crate::transfer::TransferQueue>,

    pub queue_tab: usize,

    pub terminal: Vec<(String, String)>,

    pub pending_tx: Option<PendingTx>,

    pub conflict_sel: Option<Vec<bool>>,

    pub stat_seq: u64,

    pub inflight: std::collections::HashMap<u64, (usize, u32)>,

    pub q_owner_tabs: Vec<(u8, u32)>,

    pub q_active: bool,

    pub q_refresh: std::collections::HashMap<(u8, u32), (f64, u64)>,

    pub tab_close_wait: Option<(usize, u32, std::time::Instant)>,

    pub chk_local: Option<Result<String, String>>,

    pub chk_pending: Option<(u64, usize)>,

    pub chk_computing: bool,

    pub chk_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,

    pub local_jobs: Vec<LocalJob>,

    pub save_err_pending: bool,

    pub editors: crate::editor::EditorHub,

    pub doc_stat: Option<DocStatSlot>,

    pub fp_ask_test: Option<dialogs::FpAsk>,

    pub scroll_stash: egui::Vec2,

    pub dialog_kind: u8,

    pub dialog_fresh: bool,

    pub shell_enter: bool,
}

pub enum RemoteState {
    Off,
    Connecting,
    Failed(String),
}

pub(crate) fn next_seq(st: &mut State) -> u64 {
    st.stat_seq += 1;
    st.stat_seq
}

pub(crate) fn initial_remote_dir(
    h: &crate::hosts::HostProfile,
    root: &str,
) -> (String, Option<String>, Option<String>) {
    if h.sync_enabled && !h.remote_sync_path.trim().is_empty() {
        let p = normalize_remote(&h.remote_sync_path);
        (p.clone(), None, Some(p))
    } else if !h.last_remote_path.trim().is_empty() {
        let p = normalize_remote(&h.last_remote_path);
        (p.clone(), Some(root.to_owned()), Some(p))
    } else {
        (root.to_owned(), None, None)
    }
}

pub(crate) fn pane_is_local(st: &State, side: usize) -> bool {
    match (st.locals[side].active, st.remotes[side].active) {
        (Some(_), _) => true,
        (None, Some(_)) => false,

        (None, None) => true,
    }
}

pub(crate) fn pane_view(st: &State, side: usize) -> &panes::FilePane {
    match pane_is_local(st, side) {
        true => {
            &active_local_tab(st, side)
                .expect("local side keeps its active tab")
                .pane
        }
        false => {
            &active_remote_tab(st, side)
                .expect("remote side keeps its active tab")
                .pane
        }
    }
}

pub(crate) fn pane_view_mut(st: &mut State, side: usize) -> &mut panes::FilePane {
    match pane_is_local(st, side) {
        true => {
            &mut active_local_tab_mut(st, side)
                .expect("local side keeps its active tab")
                .pane
        }
        false => {
            &mut active_remote_tab_mut(st, side)
                .expect("remote side keeps its active tab")
                .pane
        }
    }
}

fn active_local_slot(st: &State, side: usize) -> Option<usize> {
    let ls = &st.locals[side];
    let id = ls.active?;
    ls.tabs.iter().position(|t| t.id == id)
}

fn active_remote_slot(st: &State, side: usize) -> Option<usize> {
    let rs = &st.remotes[side];
    let id = rs.active?;
    rs.tabs.iter().position(|t| t.id == id)
}

fn active_local_tab(st: &State, side: usize) -> Option<&LocalTab> {
    let ls = &st.locals[side];
    let id = ls.active?;
    ls.tabs.iter().find(|t| t.id == id)
}

fn active_local_tab_mut(st: &mut State, side: usize) -> Option<&mut LocalTab> {
    let ls = &mut st.locals[side];
    let id = ls.active?;
    ls.tabs.iter_mut().find(|t| t.id == id)
}

pub(crate) fn active_remote_tab(st: &State, side: usize) -> Option<&RemoteTab> {
    let rs = &st.remotes[side];
    let id = rs.active?;
    rs.tabs.iter().find(|t| t.id == id)
}

pub(crate) fn active_remote_tab_mut(st: &mut State, side: usize) -> Option<&mut RemoteTab> {
    let rs = &mut st.remotes[side];
    let id = rs.active?;
    rs.tabs.iter_mut().find(|t| t.id == id)
}

pub(crate) fn active_tab_id(st: &State, side: usize) -> Option<u32> {
    st.locals[side].active.or(st.remotes[side].active)
}

pub(crate) fn find_remote_tab_mut(st: &mut State, side: usize, id: u32) -> Option<&mut RemoteTab> {
    st.remotes[side].tabs.iter_mut().find(|t| t.id == id)
}

pub(crate) fn session_mut(st: &mut State, side: usize, sid: u64) -> Option<&mut SideSession> {
    st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid)
}

pub(crate) fn session_of_tab(st: &State, side: usize, tab_id: u32) -> Option<&SideSession> {
    let sid = st.remotes[side].tabs.iter().find(|t| t.id == tab_id)?.sid;
    st.remotes[side].sessions.iter().find(|s| s.sid == sid)
}

pub(crate) fn active_remote_session(st: &State, side: usize) -> Option<&SideSession> {
    let tab = active_remote_tab(st, side)?;
    session_of_tab(st, side, tab.id)
}

pub(crate) fn tab_pane(st: &State, side: usize, tab_id: u32) -> Option<&panes::FilePane> {
    st.locals[side]
        .tabs
        .iter()
        .find(|t| t.id == tab_id)
        .map(|t| &t.pane)
        .or_else(|| {
            st.remotes[side]
                .tabs
                .iter()
                .find(|t| t.id == tab_id)
                .map(|t| &t.pane)
        })
}

pub(crate) fn tab_pane_mut(
    st: &mut State,
    side: usize,
    tab_id: u32,
) -> Option<&mut panes::FilePane> {
    let local = st.locals[side]
        .tabs
        .iter_mut()
        .find(|t| t.id == tab_id)
        .map(|t| &mut t.pane);
    local.or_else(|| {
        st.remotes[side]
            .tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .map(|t| &mut t.pane)
    })
}

fn build_tabs_view(st: &State, side: usize) -> panes::Tabs {
    let mut items = Vec::new();
    for lt in &st.locals[side].tabs {

        items.push(panes::TabItem {
            id: lt.id,
            title: t("本地").to_owned(),
            kind: panes::TabKind::Local,
        });
    }
    for rt in &st.remotes[side].tabs {

        items.push(panes::TabItem {
            id: rt.id,
            title: rt.title.clone(),
            kind: panes::TabKind::Remote,
        });
    }
    let active_id = st.locals[side].active.or(st.remotes[side].active);
    let active = items
        .iter()
        .position(|i| Some(i.id) == active_id)
        .unwrap_or(0);
    let born = st.tab_born[side]
        .and_then(|(id, t0)| items.iter().position(|i| i.id == id).map(|p| (p, t0)));
    panes::Tabs {
        items,
        active,
        born,
    }
}

impl Default for State {
    fn default() -> Self {

        let mut root_local = panes::FilePane::default();

        crate::local::navigate_root(&mut root_local);
        Self {
            icons: crate::icons::Cache::default(),
            sys_icons: crate::icons_sys::Cache::default(),
            left_w: 0.0,
            bottom_h: theme::BOTTOM_ROW_MIN,
            sync_browse: false,
            compare: false,
            locals: [
                LocalSide {
                    tabs: vec![LocalTab {
                        id: 1,
                        pane: root_local,
                    }],
                    active: Some(1),
                },

                LocalSide {
                    tabs: Vec::new(),
                    active: None,
                },
            ],
            remotes: [
                RemoteSide {
                    tabs: Vec::new(),
                    active: None,
                    sessions: Vec::new(),
                },
                RemoteSide {
                    tabs: vec![placeholder_remote_tab(2)],
                    active: Some(2),
                    sessions: Vec::new(),
                },
            ],
            next_tab_id: 3,
            next_sid: 1,
            tab_born: [None, None],
            dialog: dialogs::Dialog::None,
            menu: None,
            menu_rect: None,
            reconnecting: 0,
            ctx_menu: ctx_menu::CtxMenu::default(),
            crumb_edit: None,

            settings: dialogs::Settings::from_store(),
            list_reqs: Vec::new(),
            q_refresh: std::collections::HashMap::new(),
            new_reqs: Vec::new(),
            rename_reqs: Vec::new(),
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
            conflict_sel: None,
            stat_seq: 0,
            inflight: std::collections::HashMap::new(),
            q_owner_tabs: Vec::new(),
            q_active: false,
            tab_close_wait: None,
            chk_local: None,
            chk_pending: None,
            chk_computing: false,
            chk_rx: None,
            local_jobs: Vec::new(),
            save_err_pending: false,
            editors: crate::editor::EditorHub::new(),
            doc_stat: None,
            fp_ask_test: None,
            scroll_stash: egui::Vec2::ZERO,
            dialog_kind: 0,
            dialog_fresh: false,
            shell_enter: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorLayer {
    Dialog,
    Drag,
    Edge,
    Hover,
}

impl CursorLayer {

    fn rank(self) -> u8 {
        match self {
            CursorLayer::Hover => 0,
            CursorLayer::Edge => 1,
            CursorLayer::Drag => 2,
            CursorLayer::Dialog => 3,
        }
    }
}

fn cursor_slot(ctx: &egui::Context) -> egui::Id {

    egui::Id::unique(("ifree.cursor_intent", ctx.viewport_id()))
}

pub(crate) fn cursor_intent(ctx: &egui::Context, layer: CursorLayer, icon: egui::CursorIcon) {
    let id = cursor_slot(ctx);
    ctx.data_mut(|d| {
        let cur: Option<(CursorLayer, egui::CursorIcon)> = d.get_temp(id).unwrap_or(None);
        if cur
            .map(|(prev, _)| layer.rank() > prev.rank())
            .unwrap_or(true)
        {
            d.insert_temp(id, Some((layer, icon)));
        }
    });
}

pub fn cursor_decide(ctx: &egui::Context) {
    let id = cursor_slot(ctx);
    let intent = ctx
        .data_mut(|d| d.remove_temp::<Option<(CursorLayer, egui::CursorIcon)>>(id))
        .flatten();
    ctx.set_cursor_icon(
        intent
            .map(|(_, icon)| icon)
            .unwrap_or(egui::CursorIcon::Default),
    );
}

pub fn frame_modal(st: &State) -> bool {
    !matches!(st.dialog, dialogs::Dialog::None) || st.menu.is_some() || dialogs::fp_ask_pending(st)
}

fn save_error_bar_rect(full: Rect, pending: bool) -> (Rect, Option<Rect>) {
    if !pending || full.height() <= HEADER_H + SAVE_BAR_H + TOP_ROW_MIN + BOTTOM_MIN {
        return (full, None);
    }
    let (rest, bar) = full.split_top_bottom_at_y(full.bottom() - SAVE_BAR_H);
    (rest, Some(bar))
}

fn draw_save_error_bar(ui: &mut egui::Ui, r: Rect, p: &theme::Palette) {
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(r)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.painter().rect_filled(r, 0.0, p.dialog_title_bar);
    bar.painter().line_segment(
        [r.left_top(), r.right_top()],
        egui::Stroke::new(1.0, theme::WARNING),
    );
    bar.add_space(10.0);
    bar.add(
        egui::Label::new(
            egui::RichText::new(t("设置未保存，正在自动重试"))
                .color(theme::WARNING)
                .size(12.0),
        )
        .truncate(),
    );
    bar.add_space(8.0);
    bar.add(
        egui::Label::new(
            egui::RichText::new(crate::settings::save_error().unwrap_or_default())
                .color(p.text_secondary)
                .size(12.0),
        )
        .truncate(),
    );
}

pub fn root(ui: &mut egui::Ui, st: &mut State) {

    if !matches!(st.dialog, dialogs::Dialog::None) || dialogs::fp_ask_pending(st) {
        st.scroll_stash = ui
            .ctx()
            .input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta));
    }

    let p = theme::palette(ui.ctx());

    crate::settings::maybe_flush(ui.ctx());

    if let Some(e) = crate::settings::take_save_error() {
        st.terminal.push((crate::local::now_hms(), e));
    }
    st.save_err_pending = crate::settings::save_error().is_some();

    if st.menu.is_some()
        && let Some(r) = st.menu_rect
    {
        let outside_press = ui.ctx().input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_none_or(|p| !r.contains(p))
        });
        if outside_press {
            st.menu = None;
        }
    }

    st.menu_rect = None;

    apply_ssh_events(st, ui.ctx());

    #[cfg(feature = "official")]
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

    if let Some(q) = &st.transfer {
        for line in q.drain_log() {
            st.terminal.push((crate::local::now_hms(), line));
        }
        for line in q.drain_batch_reports() {
            st.terminal.push((crate::local::now_hms(), line));
        }
        if st.terminal.len() > 200 {
            st.terminal.drain(0..st.terminal.len() - 200);
        }
        let (active, owners) = q.with_items(|items| {
            let active = items.iter().any(|i| {
                matches!(
                    i.status,
                    crate::transfer::Status::Waiting
                        | crate::transfer::Status::Transferring
                        | crate::transfer::Status::Verifying
                )
            });
            let tabs =
                crate::transfer::collect_owner_tabs(items.iter().map(|i| (&i.owner, i.status)));
            (active, tabs)
        });
        if st.q_active && !active {

            for (side, tab_id) in std::mem::take(&mut st.q_owner_tabs) {
                let side = side as usize;
                if let Some(lt) = st.locals[side].tabs.iter_mut().find(|t| t.id == tab_id) {
                    crate::local::refresh(&mut lt.pane);
                    lt.pane.refresh_at = Some(ui.ctx().time());
                    continue;
                }
                let rp = st.remotes[side]
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .and_then(|t| t.pane.remote_path.clone());
                if let Some(rp) = rp {
                    st.list_reqs.push((side, tab_id, rp));
                }
            }

            st.q_refresh.clear();
        } else {

            st.q_owner_tabs = owners;
            if active {

                let now = ui.ctx().time();
                let targets = q.with_items(|items| {
                    crate::transfer::refresh_targets(
                        items
                            .iter()
                            .map(|i| (&i.owner, i.dest_is_remote(), i.dest.as_str(), i.status)),
                    )
                });
                for ((side, tab_id), done, dests) in targets {
                    let side_idx = side as usize;

                    let snap = st.q_refresh.entry((side, tab_id)).or_insert((now, done));

                    let cur = st
                        .locals
                        .get(side_idx)
                        .and_then(|l| l.tabs.iter().find(|t| t.id == tab_id))
                        .map(|t| {
                            (
                                t.pane
                                    .path
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().into_owned()),
                                false,
                            )
                        })
                        .or_else(|| {
                            st.remotes
                                .get(side_idx)
                                .and_then(|r| r.tabs.iter().find(|t| t.id == tab_id))
                                .map(|t| (t.pane.remote_path.clone(), true))
                        });
                    let same_dir = cur.is_some_and(|(p, remote)| {
                        p.is_some_and(|p| {
                            dests
                                .iter()
                                .any(|d| crate::transfer::same_dest_dir(&p, d, remote))
                        })
                    });
                    if crate::transfer::refresh_due(
                        now,
                        snap.0,
                        done.saturating_sub(snap.1),
                        same_dir,
                    ) {

                        *snap = (now, done);

                        if let Some(lt) =
                            st.locals[side_idx].tabs.iter_mut().find(|t| t.id == tab_id)
                        {
                            crate::local::refresh(&mut lt.pane);
                            lt.pane.refresh_at = Some(now);
                        } else if let Some(rp) = st.remotes[side_idx]
                            .tabs
                            .iter()
                            .find(|t| t.id == tab_id)
                            .and_then(|t| t.pane.remote_path.clone())
                        {
                            st.list_reqs.push((side_idx, tab_id, rp));
                        }
                    }
                }
            } else {

                st.q_refresh.clear();
            }
        }
        st.q_active = active;
    }

    poll_ext_edits(st);

    if st.chk_computing
        && let Some(rx) = &st.chk_rx
        && let Ok(r) = rx.try_recv()
    {
        st.chk_local = Some(r);
        st.chk_computing = false;
        st.chk_rx = None;
    }

    for ev in crate::editor::EditorHub::drain_saved() {
        saved_upload_dispatch(st, ui.ctx(), ev);
    }

    editor_window::show_all(
        ui.ctx(),
        &mut st.editors,
        ui.ctx().theme() == egui::Theme::Dark,
    );

    for side in 0..2 {
        for ti in 0..st.remotes[side].tabs.len() {
            let sid = st.remotes[side].tabs[ti].sid;
            let snap = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == sid)
                .map(|s| match &s.state {
                    RemoteState::Off => (0u8, String::new()),
                    RemoteState::Connecting => (1, String::new()),
                    RemoteState::Failed(m) => (2, m.clone()),
                })
                .unwrap_or((0, String::new()));
            let tab = &mut st.remotes[side].tabs[ti];
            match snap.0 {
                1 => {
                    tab.pane.empty = true;

                    tab.pane.connecting = true;
                    tab.pane.empty_title = t("正在连接…").to_owned();
                    tab.pane.empty_sub = t("正在建立 SSH 连接").to_owned();
                }
                2 => {
                    tab.pane.empty = true;

                    tab.pane.connecting = false;
                    tab.pane.empty_title = t("连接失败").to_owned();
                    tab.pane.empty_sub = snap.1.chars().take(60).collect();
                }
                _ => {
                    if tab.pane.empty {
                        tab.pane.connecting = false;
                        tab.pane.empty_title = t("先选择一个主机").to_owned();
                        tab.pane.empty_sub = t("选择要浏览的本地或远端文件系统").to_owned();
                    }
                }
            }
        }
    }

    let full = ui.max_rect();

    let (full, save_bar) = save_error_bar_rect(full, st.save_err_pending);

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

    let mut list_tmp: Vec<(usize, String)> = Vec::new();
    let mut new_tmp: Vec<(usize, String, bool)> = Vec::new();
    let mut tree_tmp: Option<(usize, String)> = None;
    let mut sync_nav_tmp: Option<(usize, String)> = None;

    let active_ids = [active_tab_id(st, 0), active_tab_id(st, 1)];

    enum Slot {
        Local(usize),
        Remote(usize),
    }
    let mut views = [build_tabs_view(st, 0), build_tabs_view(st, 1)];
    let builts: [Vec<u32>; 2] = [
        views[0].items.iter().map(|i| i.id).collect(),
        views[1].items.iter().map(|i| i.id).collect(),
    ];
    let slots = [
        if pane_is_local(st, 0) {
            Slot::Local(active_local_slot(st, 0).expect("local side keeps its active tab"))
        } else {
            Slot::Remote(active_remote_slot(st, 0).expect("remote side keeps its active tab"))
        },
        if pane_is_local(st, 1) {
            Slot::Local(active_local_slot(st, 1).expect("local side keeps its active tab"))
        } else {
            Slot::Remote(active_remote_slot(st, 1).expect("remote side keeps its active tab"))
        },
    ];

    let side_ready = |s: usize| match &slots[s] {
        Slot::Local(i) => {
            let pn = &st.locals[s].tabs[*i].pane;
            (pn.path.is_some() || pn.remote_path.is_some(), true)
        }
        Slot::Remote(j) => {
            let t = &st.remotes[s].tabs[*j];
            (
                t.pane.remote_path.is_some(),
                st.remotes[s].sessions.iter().any(|ss| ss.sid == t.sid),
            )
        }
    };
    let (nav0, conn0) = side_ready(0);
    let (nav1, conn1) = side_ready(1);
    let dnd_ready = panes::can_transfer_to_oppo(nav0, conn0, nav1, conn1);

    let mut drop_upload: Option<(usize, Vec<std::path::PathBuf>, bool)> = None;
    let mut drop_resume = false;
    let mut drop_clear = false;
    let drop_stash = [held_stash_count(st, 0), held_stash_count(st, 1)];
    let mut hooks = dialogs::Hooks {
        menu: &mut st.menu,
        ctx_menu: &mut st.ctx_menu,
        dialog: &mut st.dialog,
        crumb_edit: &mut st.crumb_edit,
        list_req: &mut list_tmp,
        new_req: &mut new_tmp,
        tree_req: &mut tree_tmp,
        sync_nav: &mut sync_nav_tmp,
        compare_req: &mut st.compare_req,
        transfer: &mut st.dnd_transfer,
        dnd_ready,
        drop_upload: &mut drop_upload,
        drop_stash: &drop_stash,
        drop_resume: &mut drop_resume,
        drop_clear: &mut drop_clear,
        pick: &mut st.pick,
        tab_close: None,
    };

    let mut closed: Vec<(usize, u32)> = Vec::new();
    for side in 0..2 {
        let rect = if side == 0 { left_top } else { right_top };
        let built = &builts[side];
        match &slots[side] {
            Slot::Local(i) => panes::folder(
                ui,
                rect,
                &mut st.locals[side].tabs[*i].pane,
                &mut views[side],
                &mut st.icons,
                &mut st.sys_icons,
                &mut hooks,
                side,
            ),
            Slot::Remote(j) => panes::folder(
                ui,
                rect,
                &mut st.remotes[side].tabs[*j].pane,
                &mut views[side],
                &mut st.icons,
                &mut st.sys_icons,
                &mut hooks,
                side,
            ),
        }

        if let Some(item) = views[side].items.get(views[side].active) {
            let id = item.id;
            if item.kind == panes::TabKind::Local {
                st.locals[side].active = Some(id);
                st.remotes[side].active = None;
            } else {
                st.remotes[side].active = Some(id);
                st.locals[side].active = None;
            }
        }

        if views[side].items.len() < built.len()
            && let Some(gone) = built
                .iter()
                .find(|id| !views[side].items.iter().any(|v| v.id == **id))
        {
            closed.push((side, *gone));
        }

        if views[side].born.is_none() && st.tab_born[side].is_some() {
            st.tab_born[side] = None;
        }
    }

    let tab_close = hooks.tab_close.take();

    if let Some((nav_side, nav_path)) = sync_nav_tmp.take() {
        st.sync_nav = Some((nav_side, active_ids[nav_side].unwrap_or(0), nav_path));
    }
    for (side, tab_id) in closed {
        finish_close_tab(st, side, tab_id);
    }
    if let Some((side, tab_id)) = tab_close {
        request_close_tab(st, side, tab_id);
    }

    if let Some((side, tab_id, t0)) = st.tab_close_wait {
        let key = (side as u8, tab_id);
        let active = st.transfer.as_ref().is_some_and(|q| {
            q.with_items(|items| {
                crate::transfer::count_owned_active(items.iter().map(|i| (&i.owner, i.status)), key)
                    > 0
            })
        });
        if !active || t0.elapsed() >= std::time::Duration::from_secs(5) {
            st.tab_close_wait = None;
            finish_close_tab(st, side, tab_id);
        }
    }

    if let Some((side, rows)) = st.dnd_transfer.take() {
        begin_transfer_multi(st, side, &rows);
    }

    if let Some((side, paths, shift)) = drop_upload {
        begin_drop_upload(st, side, paths, shift);
    }

    if drop_resume {
        resume_drop_stash(st);
    }
    if drop_clear {
        discard_drop_stash(st);
    }

    for (side, path) in list_tmp {
        st.list_reqs
            .push((side, active_ids[side].unwrap_or(0), path));
    }
    for (side, path, is_dir) in new_tmp {
        st.new_reqs
            .push((side, active_ids[side].unwrap_or(0), path, is_dir));
    }
    if let Some((side, path)) = tree_tmp {

        let tag = next_seq(st);
        st.tree_req = Some((side, active_ids[side].unwrap_or(0), tag, path));
    }

    for (side, tab_id, path) in std::mem::take(&mut st.list_reqs) {
        let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) else {
            continue;
        };
        let sid = st.remotes[side].tabs[pos].sid;

        let tag = next_seq(st);
        let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) else {
            continue;
        };
        let tab = &mut st.remotes[side].tabs[pos];
        tab.wait = Some((tag, path.clone()));
        tab.pane.loading = true;
        ss.ssh.list(tag, path);
    }

    for (side, tab_id, path, is_dir) in std::mem::take(&mut st.new_reqs) {
        let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) else {
            continue;
        };
        let sid = st.remotes[side].tabs[pos].sid;

        let tag = next_seq(st);
        st.inflight.insert(tag, (side, tab_id));
        let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) else {
            continue;
        };
        if is_dir {
            ss.ssh.mkdir(tag, path);
        } else {
            ss.ssh.create_file(tag, path);
        }
        st.remotes[side].tabs[pos].new_pending += 1;
    }

    for (side, tab_id, path, new_path) in std::mem::take(&mut st.rename_reqs) {
        let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) else {
            continue;
        };
        let sid = st.remotes[side].tabs[pos].sid;

        let tag = next_seq(st);
        st.inflight.insert(tag, (side, tab_id));
        let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) else {
            continue;
        };
        ss.ssh.rename(tag, path, new_path);
        st.remotes[side].tabs[pos].new_pending += 1;
    }
    if let Some((side, tab_id, tag, path)) = st.tree_req.take()
        && let Some(pos) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id)
    {
        let sid = st.remotes[side].tabs[pos].sid;
        if let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) {

            ss.ssh.list(tag, path.clone());
            st.tree_req = Some((side, tab_id, tag, path));
        }
    }

    if let Some(job) = st.pick.take() {
        match job.rx.try_recv() {
            Ok(Some(dir)) => {
                let side = job.side;
                let pane = pane_view_mut(st, side);
                if crate::local::navigate(pane, &dir) {

                    let tab_id = active_tab_id(st, side).unwrap_or(0);
                    st.sync_nav = Some((side, tab_id, dir.to_string_lossy().into_owned()));
                }
            }
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => st.pick = Some(job),
        }

        if st.pick.is_none() {
            ui.ctx().request_repaint();
        }
    }

    if let Some((side, tab, path)) = st.sync_nav.take() {
        sync::handle_nav(st, side, tab, path);
    }

    if st.compare_req {
        st.compare_req = false;
        sync::compare_apply_both(st);
    }

    panes::queue(ui, bottom_left, st);
    panes::terminal(ui, bottom_right, st);

    header::draw(ui, header_rect, st, &p);

    if let Some(r) = save_bar {
        draw_save_error_bar(ui, r, &p);
    }

    dialogs::draw(ui, st, &p);

    if poll_local_jobs(st) {
        ui.ctx().request_repaint();
    }

    if let Some(d) = repaint_delay(st) {
        ui.ctx().request_repaint_after(d);
    }
}

pub fn poll_local_jobs(st: &mut State) -> bool {
    for side in 0..2 {
        let pending: Vec<(u32, crate::local::LocalPaneReq, bool)> = st.locals[side]
            .tabs
            .iter_mut()
            .filter_map(|t| {
                let show_hidden = t.pane.show_hidden;
                t.pane
                    .pending_local
                    .take()
                    .map(|req| (t.id, req, show_hidden))
            })
            .collect();
        for (tab_id, req, show_hidden) in pending {
            let (tx, rx) = std::sync::mpsc::channel();
            let job_req = req.clone();
            std::thread::Builder::new()
                .name("local-io".into())
                .spawn(move || {
                    let _ = tx.send(crate::local::run_local(req, show_hidden));
                })
                .ok();
            st.local_jobs.push(LocalJob {
                side,
                tab_id,
                req: job_req,
                rx,
            });
        }
    }

    let mut replies: Vec<(usize, u32, crate::local::LocalPaneReply)> = Vec::new();
    st.local_jobs.retain(|job| match job.rx.try_recv() {
        Ok(reply) => {
            replies.push((job.side, job.tab_id, reply));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            replies.push((
                job.side,
                job.tab_id,
                crate::local::LocalPaneReply::worker_gone(job.req.clone()),
            ));
            false
        }
    });

    let consumed = !replies.is_empty();
    for (side, tab_id, reply) in replies {
        let deleted = if matches!(&reply.req, crate::local::LocalPaneReq::Delete { .. }) {
            Some(crate::local::delete_log_lines(&reply.deleted))
        } else {
            None
        };
        let logs = if let Some(tab) = st
            .locals
            .get_mut(side)
            .and_then(|ls| ls.tabs.iter_mut().find(|t| t.id == tab_id))
        {
            crate::local::apply_local(&mut tab.pane, reply)
        } else {

            deleted.unwrap_or_default()
        };
        for line in logs {
            st.terminal.push((crate::local::now_hms(), line));
        }
    }
    if st.terminal.len() > 200 {
        st.terminal.drain(0..st.terminal.len() - 200);
    }
    consumed
}

fn repaint_delay(st: &State) -> Option<std::time::Duration> {
    const FAST: std::time::Duration = std::time::Duration::from_millis(120);
    const POLL: std::time::Duration = std::time::Duration::from_millis(300);
    const SLOW: std::time::Duration = std::time::Duration::from_millis(1000);
    let in_flight = !st.list_reqs.is_empty()

        || !st.local_jobs.is_empty()
        || st
            .locals
            .iter()
            .any(|ls| ls.tabs.iter().any(|t| t.pane.pending_local.is_some()))
        || st.tree_req.is_some()

        || st.pending_tx.as_ref().is_some_and(|p| !p.held)
        || st.chk_pending.is_some()
        || st.chk_computing
        || st.doc_stat.is_some()
        || st
            .editors
            .docs
            .iter()
            .any(|d| d.lock().unwrap().pending_upload)
        || st.remotes.iter().any(|rs| {
            rs.sessions
                .iter()
                .any(|s| matches!(s.state, RemoteState::Connecting) || s.fp_ask.is_some())
                || rs.tabs.iter().any(|t| {
                    t.wait.is_some()
                        || t.del_pending > 0
                        || t.new_pending > 0
                        || t.pending_ed.is_some()
                        || t.ext_stat.is_some()
                        || t.ext_write.is_some()
                })
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
    if in_flight {
        return Some(FAST);
    }

    if st.save_err_pending {
        return Some(SLOW);
    }

    let lock_wait = st.remotes.iter().any(|rs| {
        rs.tabs.iter().any(|t| {
            t.ext_edits.iter().any(|e| {
                e.lock_wait_since
                    .is_some_and(|s| s.elapsed().as_millis() >= LOCK_TIMEOUT_MS)
            })
        })
    });
    if lock_wait {
        return Some(SLOW);
    }

    if st
        .remotes
        .iter()
        .any(|rs| rs.tabs.iter().any(|t| !t.ext_edits.is_empty()))
    {
        return Some(POLL);
    }
    None
}

fn apply_ssh_events(st: &mut State, ctx: &egui::Context) {

    if let Some(ptx) = &mut st.pending_tx {
        let n = force_settle_expired(ptx, STAT_PROBE_TIMEOUT);
        if n > 0 {
            let items = std::mem::take(&mut ptx.items);
            let (side, pane) = (ptx.profile_side, ptx.src_pane);
            settle_batch(st, side, pane, items);
            st.terminal.push((
                crate::local::now_hms(),
                tfmt(
                    "预检超时：{0} 项未完成探测，已按远端不存在放行（传输时按续传决策兜底）",
                    &[&n.to_string()],
                ),
            ));
        }
    }
    for side in 0..2 {
        let sids: Vec<u64> = st.remotes[side].sessions.iter().map(|s| s.sid).collect();
        for sid in sids {
            let evs = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == sid)
                .map(|s| s.ssh.poll())
                .unwrap_or_default();
            for ev in evs {
                apply_ssh_event(st, ctx, side, sid, ev);
            }

            trim_terminal(&mut st.terminal);
        }
    }
}

fn trim_terminal(terminal: &mut Vec<(String, String)>) {
    if terminal.len() > 400 {
        terminal.drain(0..terminal.len() - 400);
    }
}

fn apply_ssh_event(st: &mut State, ctx: &egui::Context, side: usize, sid: u64, ev: crate::ssh::Ev) {

    let tag = match st.remotes[side]
        .sessions
        .iter()
        .find(|s| s.sid == sid)
        .and_then(|s| s.host.as_ref())
    {
        Some(h) => format!("[{}]", dialogs::host_tab_naming(&h.name, &h.host)),
        None => format!("[{}]", t("远程")),
    };
    match ev {
        crate::ssh::Ev::Connected {
            label,
            root,
            confirmed,
        } => {

            if st.reconnecting > 0 {
                st.reconnecting -= 1;
                st.terminal
                    .push((crate::local::now_hms(), tfmt("已重新连接：{0}", &[&label])));
            }

            let hopt = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == sid)
                .and_then(|s| s.host.clone());
            let (init, fallback, remote_base) = match &hopt {
                Some(h) => initial_remote_dir(h, &root),
                None => (root.clone(), None, None),
            };

            if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                ss.state = RemoteState::Off;
                ss.root = Some(root.clone());
            }

            let waiting: Vec<u32> = st.remotes[side]
                .tabs
                .iter()
                .filter(|t| t.sid == sid && t.pane.empty)
                .map(|t| t.id)
                .collect();
            for tab_id in waiting {
                let ltag = next_seq(st);
                if let Some(ss) = st.remotes[side].sessions.iter().find(|s| s.sid == sid) {
                    ss.ssh.list(ltag, init.clone());
                }
                if let Some(tab) = st.remotes[side].tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.pane = panes::FilePane {
                        remote_path: Some(init.clone()),
                        ..Default::default()
                    };
                    tab.wait = Some((ltag, init.clone()));
                    tab.pane.loading = true;
                    tab.fallback = fallback.clone();
                    tab.title = label.clone();
                }
            }

            st.dialog = dialogs::Dialog::None;

            sync::attach_connect(st, side, sid, remote_base);

            if let Some(fp) = &confirmed {
                let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) else {
                    return;
                };
                if ss
                    .host
                    .as_ref()
                    .is_some_and(|h| h.host_key_fingerprint != *fp)
                {

                    if let Some(p) = &mut ss.profile {
                        p.fingerprint = Some(fp.clone());
                    }
                    let was_empty = ss
                        .host
                        .as_ref()
                        .is_some_and(|h| h.host_key_fingerprint.trim().is_empty());
                    let (host, port, user, last_path) = {
                        let h = ss.host.as_mut().expect("checked above");
                        (
                            h.host.clone(),
                            h.port,
                            h.username.clone(),
                            h.last_remote_path.clone(),
                        )
                    };

                    crate::hosts::update_fingerprint(&host, port, &user, fp);
                    let saved =
                        crate::hosts::save_single_session(&host, port, &user, fp, &last_path);
                    if let Some(h) = &mut ss.host {
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
        }
        crate::ssh::Ev::Failed { what, msg, tag } => {

            let tree_kill = match (&tag, st.tree_req.as_ref()) {
                (Some(tg), Some((_, _, ttag, _))) => *ttag == *tg,
                (None, Some(_)) => what == "连接",
                _ => false,
            };
            if tree_kill {
                let (tside, ttab) = st
                    .tree_req
                    .take()
                    .map(|(s, t, _, _)| (s, t))
                    .unwrap_or((0, 0));
                for s2 in 0..2 {
                    for lt in &mut st.locals[s2].tabs {
                        crate::ui::panes::reset_tree_loading(&mut lt.pane);
                    }
                    for rt in &mut st.remotes[s2].tabs {
                        crate::ui::panes::reset_tree_loading(&mut rt.pane);
                    }
                }
                if let Some(tab) = find_remote_tab_mut(st, tside, ttab) {
                    crate::ui::panes::tree_sync_give_up(&mut tab.pane);
                }
            }
            if what == "连接" {
                if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                    ss.state = RemoteState::Failed(tfmt("连接失败：{0}", &[&msg]));
                }

                if st.reconnecting > 0 {
                    st.reconnecting -= 1;
                    st.terminal
                        .push((crate::local::now_hms(), tfmt("重连失败：{0}", &[&msg])));
                }

                st.dialog = dialogs::Dialog::None;
            } else if st
                .sync_mirror
                .as_ref()
                .is_some_and(|(ms, _, _)| *ms == side)
            {

                st.sync_mirror = None;
                if let Some(tg) = tag
                    && let Some(tab) = st.remotes[side]
                        .tabs
                        .iter_mut()
                        .find(|t| t.wait.as_ref().is_some_and(|w| w.0 == tg))
                {
                    tab.wait = None;
                    tab.pane.loading = false;
                }
                sync::report_mismatch(st, "SyncMismatchMsg");
            } else {

                let hit = tag.and_then(|tg| {
                    st.remotes[side]
                        .tabs
                        .iter()
                        .find(|t| t.wait.as_ref().is_some_and(|w| w.0 == tg))
                        .map(|t| t.id)
                });
                let Some(tab_id) = hit else {
                    return;
                };
                let fb = find_remote_tab_mut(st, side, tab_id).and_then(|tab| tab.fallback.take());
                if let Some(home) = fb {
                    let ltag = next_seq(st);
                    if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                        tab.wait = Some((ltag, home.clone()));
                        tab.pane.loading = true;
                        tab.pane.msg = None;
                    }
                    if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                        ss.ssh.list(ltag, home);
                    }
                } else if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                    tab.wait = None;
                    tab.pane.loading = false;
                    tab.pane.msg = Some(format!("{what}失败：{msg}"));
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

            if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
                ss.fp_ask = Some(dialogs::FpAsk {
                    first,
                    host,
                    stored,
                    presented,
                    reply,
                });
            }
        }
        crate::ssh::Ev::Listing { tag, path, entries } => {

            let mut tree_hit = false;
            if let Some((tside, ttab, ttag, _)) = st.tree_req.as_ref()
                && *ttag == tag
            {
                let (tside, ttab) = (*tside, *ttab);
                st.tree_req = None;
                if let Some(tab) = find_remote_tab_mut(st, tside, ttab) {
                    crate::ui::panes::apply_tree_listing(&mut tab.pane, &path, &entries);
                    tree_hit = true;
                }
            }

            let mirror_hit = {
                let mut hit = None;
                if !tree_hit {
                    let tab = st.remotes[side]
                        .tabs
                        .iter_mut()
                        .find(|t| t.wait.as_ref().is_some_and(|w| w.0 == tag));
                    if let Some(tab) = tab {
                        tab.wait = None;
                        tab.pane.loading = false;
                        tab.pane.msg = None;

                        let same_dir = tab.pane.remote_path.as_deref() == Some(path.as_str());
                        let old_sel: Vec<String> = if same_dir {
                            tab.pane
                                .selected
                                .iter()
                                .filter_map(|&i| tab.pane.entries.get(i).map(|e| e.name.clone()))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        tab.pane.remote_path = Some(path.clone());

                        let mut entries = entries;
                        if !tab.pane.show_hidden {
                            entries.retain(|e| !e.name.starts_with('.'));
                        }
                        tab.pane.entries = entries;
                        tab.pane.selected = tab
                            .pane
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| old_sel.iter().any(|n| n == &e.name))
                            .map(|(i, _)| i)
                            .collect();
                        crate::local::apply_sort(&mut tab.pane);

                        let now = ctx.time();
                        if tab
                            .pane
                            .refresh_at
                            .map_or(true, |t0| now - t0 >= panes::REFRESH_ANIM)
                        {
                            tab.pane.refresh_at = Some(now);
                        }

                        tab.fallback = None;
                        if let Some(h) = st.remotes[side]
                            .sessions
                            .iter()
                            .find(|s| s.sid == sid)
                            .and_then(|s| s.host.clone())
                        {
                            let _ = crate::hosts::save_single_session(
                                &h.host,
                                h.port,
                                &h.username,
                                &h.host_key_fingerprint,
                                &path,
                            );
                        }

                        if st
                            .sync_mirror
                            .as_ref()
                            .is_some_and(|(ms, mt, _)| *ms == side && *mt == tab.id)
                        {
                            hit = Some(tab.id);
                        }
                    }
                }
                hit
            };
            if let Some(nav_tab) = mirror_hit {
                st.sync_mirror = None;
                sync::handle_nav(st, side, nav_tab, path);
            }
        }
        crate::ssh::Ev::Disconnected => {

        }
        crate::ssh::Ev::Log { msg } => {

            st.terminal.push((crate::local::now_hms(), msg));
        }
        crate::ssh::Ev::Deleted { tag } => {

            let owner = st.inflight.remove(&tag).filter(|&(bs, _)| bs == side);
            let act = owner.and_then(|(_, tab_id)| {
                st.remotes[side]
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == tab_id)
                    .map(|tab| {
                        tab.del_pending = tab.del_pending.saturating_sub(1);
                        if tab.del_pending == 0 && tab.pane.loading {
                            tab.pane.loading = false;

                            crate::ui::panes::tree_mark_dirty(&mut tab.pane);
                            tab.pane.remote_path.clone().map(|rp| (tab.id, rp))
                        } else {
                            None
                        }
                    })
            });
            if let Some(Some((tab_id, rp))) = act {
                st.list_reqs.push((side, tab_id, rp));
            }
        }
        crate::ssh::Ev::Created { tag } => {

            let owner = st.inflight.remove(&tag).filter(|&(bs, _)| bs == side);
            let act = owner.and_then(|(_, tab_id)| {
                st.remotes[side]
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == tab_id)
                    .map(|tab| {
                        tab.new_pending = tab.new_pending.saturating_sub(1);
                        crate::ui::panes::tree_mark_dirty(&mut tab.pane);
                        tab.pane.remote_path.clone().map(|rp| (tab.id, rp))
                    })
            });
            if let Some(Some((tab_id, rp))) = act {
                st.list_reqs.push((side, tab_id, rp));
            }
        }
        crate::ssh::Ev::Exec {
            req,
            exit,
            out,
            err,
        } => {

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

            let hit = st.remotes[side]
                .tabs
                .iter_mut()
                .find(|t| t.sid == sid)
                .filter(|t| t.pending_ed.as_ref().is_some_and(|(w, _, _)| *w == req))
                .map(|t| (t.id, t.pending_ed.take()));
            if let Some((tab_id, Some((_, remote_path, name)))) = hit {
                match result {
                    Ok(fd) => {

                        if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                            tab.ext_edits.retain(|e| e.remote_path != remote_path);
                        }
                        let host = st.remotes[side]
                            .sessions
                            .iter()
                            .find(|s| s.sid == sid)
                            .and_then(|s| s.host.as_ref().map(|h| h.host.clone()));
                        if let Some(host) = host {

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

                                    let baseline = ext_snapshot(&copy);
                                    if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                                        tab.ext_edits.push(ExtEdit {
                                            host_tag: tag.clone(),
                                            remote_path: remote_path.clone(),
                                            name: name.clone(),
                                            local: copy.clone(),
                                            base_mtime: fd.mtime,
                                            baseline,
                                            quiet_since: None,
                                            quiet_snap: None,
                                            lock_wait_since: None,
                                            lock_warned: false,
                                            lock_dialog_done: false,
                                        });
                                    }
                                    if let Err(e) =
                                        std::process::Command::new(&editor_path).arg(&copy).spawn()
                                    {
                                        let msg =
                                            tfmt("外部编辑器启动失败：{0}", &[&e.to_string()]);
                                        st.terminal.push((crate::local::now_hms(), msg.clone()));

                                        if matches!(st.dialog, dialogs::Dialog::None) {
                                            st.dialog = dialogs::Dialog::Msg {
                                                title: crate::i18n::t("外部编辑器").to_owned(),
                                                msg,
                                            };
                                        }
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

            let ext_hit = st.remotes[side]
                .tabs
                .iter_mut()
                .find(|t| t.sid == sid)
                .filter(|t| t.ext_write.is_some_and(|w| w == req))
                .map(|t| {
                    t.ext_write = None;
                    t.id
                });
            if let Some(tab_id) = ext_hit {
                match result {
                    Ok(mtime) => {

                        if let Some(tab) = find_remote_tab_mut(st, side, tab_id)
                            && let Some(e) =
                                tab.ext_edits.iter_mut().find(|e| e.remote_path == path)
                        {
                            e.base_mtime = mtime;
                            e.baseline = ext_snapshot(&e.local);
                            e.quiet_since = None;
                            e.quiet_snap = None;
                        }

                        let fname = std::path::Path::new(&path)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt("\"{0}\" 已回传到远端", &[&fname]),
                        ));

                        if let Some(rp) = st.remotes[side]
                            .tabs
                            .iter()
                            .find(|t| t.id == tab_id)
                            .and_then(|t| t.pane.remote_path.clone())
                        {
                            st.list_reqs.push((side, tab_id, rp));
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

                match result {
                    Ok(mtime) => {
                        let len = st
                            .editors
                            .docs
                            .iter()
                            .find(|doc| doc.lock().unwrap().id == req)
                            .map(|doc| {
                                let mut d = doc.lock().unwrap();
                                if let Some(r) = &mut d.remote {
                                    r.base_mtime = mtime;
                                }
                                let n = std::fs::metadata(&d.path).map(|m| m.len()).unwrap_or(0);
                                d.pending_upload = false;
                                d.dirty = false;
                                d.save_err = None;
                                d.last_receipt = Some(tfmt("已回传 {0} 字节 · {1}", &[&n, &path]));
                                n
                            })
                            .unwrap_or(0);

                        ctx.request_repaint_of(editor_viewport(req));
                        st.terminal.push((
                            crate::local::now_hms(),
                            tfmt(
                                "{0} 已回传 {1} 字节，远端 mtime {2}：{3}",
                                &[&tag, &len, &mtime.unwrap_or(0), &path],
                            ),
                        ));

                        if let Some((tid, rp)) = st.remotes[side]
                            .tabs
                            .iter()
                            .find(|t| t.sid == sid)
                            .and_then(|t| t.pane.remote_path.clone().map(|rp| (t.id, rp)))
                        {
                            st.list_reqs.push((side, tid, rp));
                        }
                    }
                    Err(e) => {
                        for doc in &st.editors.docs {
                            let mut d = doc.lock().unwrap();
                            if d.id == req {
                                d.pending_upload = false;
                                d.dirty = true;
                                d.save_err = Some(tfmt("回传未完成：{0}", &[&e]));
                            }
                        }

                        ctx.request_repaint_of(editor_viewport(req));
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

            let mut ext_handled = false;
            let ext_tab = st.remotes[side]
                .tabs
                .iter_mut()
                .find(|t| t.sid == sid && t.ext_stat.as_ref().is_some_and(|(w, _)| *w == req))
                .map(|t| {
                    t.ext_stat = None;
                    t.id
                });
            if let Some(tab_id) = ext_tab {
                ext_handled = true;
                let session = st.remotes[side]
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .and_then(|t| {
                        t.ext_edits
                            .iter()
                            .find(|e| e.remote_path == path)
                            .map(|e| (e.name.clone(), e.base_mtime))
                    });
                if let Some((name, base)) = session {
                    let conflict = match (base, mtime) {
                        (Some(a), Some(b)) => exists && a.abs_diff(b) >= 1,
                        _ => false,
                    };
                    if conflict {
                        st.dialog = dialogs::Dialog::ExtEditConflict {
                            side,
                            tab: tab_id,
                            name,
                            remote_path: path.clone(),
                            src: dialogs::ExtConflictSrc::External,
                        };
                    } else {
                        ext_upload(st, side, tab_id, &path);
                    }
                }
            }
            if !ext_handled {

                let mut doc_handled = false;
                if let Some((req_no, doc_id, rpath, lpath, data, base)) = st.doc_stat.take() {
                    if req_no == req {
                        doc_handled = true;
                        let doc = st
                            .editors
                            .docs
                            .iter()
                            .find(|doc| doc.lock().unwrap().id == doc_id)
                            .cloned();
                        let conflict = mtime_conflict(base, mtime, exists);
                        if conflict {
                            let name = doc
                                .as_ref()
                                .map(|doc| doc.lock().unwrap().title.clone())
                                .unwrap_or_default();
                            st.dialog = dialogs::Dialog::ExtEditConflict {
                                side,
                                tab: 0,
                                name,
                                remote_path: rpath,
                                src: dialogs::ExtConflictSrc::Internal {
                                    doc_id,
                                    local_path: lpath,
                                },
                            };
                        } else {

                            if let Some(ss) =
                                st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid)
                            {
                                ss.ssh.write_file(rpath, doc_id, data);
                            } else {
                                doc_upload_failed(st, ctx, doc_id, t("未找到主机会话"));
                            }
                        }
                    } else {
                        st.doc_stat = Some((req_no, doc_id, rpath, lpath, data, base));
                    }
                }
                if !doc_handled {

                    let mut step = StreamStep::None;
                    if let Some(ptx) = &mut st.pending_tx
                        && ptx.seq == req
                    {

                        ptx.at = std::time::Instant::now();
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
                            ptx.resolved += 1;
                            if !exists {
                                ptx.ready += 1;
                            }
                            step = stream_step(ptx);
                        }
                    }
                    match step {
                        StreamStep::Place => stream_place(st),
                        StreamStep::Settle => stream_settle(st),
                        StreamStep::None => {}
                    }
                }
            }
        }
    }
}

const LOCK_SOFT_MS: u128 = 3000;

const LOCK_TIMEOUT_MS: u128 = 10000;

pub(crate) type DocStatSlot = (u64, u64, String, std::path::PathBuf, Vec<u8>, Option<i64>);

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

#[cfg(windows)]
fn ext_write_lock_released(path: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    #[allow(non_snake_case)]
    mod extlock {

        pub const FILE_SHARE_READ: u32 = 0x1;
        pub const GENERIC_READ: u32 = 0x8000_0000;
        pub const OPEN_EXISTING: u32 = 3;
        pub const INVALID_HANDLE_VALUE: isize = -1;

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

        unsafe { GetLastError() != ERROR_SHARING_VIOLATION }
    } else {
        unsafe { CloseHandle(h) };
        true
    }
}

#[cfg(not(windows))]
fn ext_write_lock_released(_path: &std::path::Path) -> bool {
    true
}

fn mtime_conflict(base_mtime: Option<i64>, remote_mtime: Option<i64>, exists: bool) -> bool {
    match (base_mtime, remote_mtime) {
        (Some(a), Some(b)) => exists && a.abs_diff(b) >= 1,
        _ => false,
    }
}

fn doc_upload_failed(st: &mut State, ctx: &egui::Context, doc_id: u64, reason: &str) {
    for doc in &st.editors.docs {
        let mut d = doc.lock().unwrap();
        if d.id == doc_id {
            d.pending_upload = false;
            d.dirty = true;
            d.save_err = Some(tfmt("回传未完成：{0}", &[&reason.to_string()]));
        }
    }
    ctx.request_repaint_of(editor_viewport(doc_id));
}

fn editor_viewport(doc_id: u64) -> egui::ViewportId {
    egui::ViewportId(egui::Id::unique(("editor", doc_id)))
}

fn session_for_host(st: &State, host: &str) -> Option<(usize, u64)> {
    st.remotes.iter().enumerate().find_map(|(side, rs)| {
        rs.sessions
            .iter()
            .find(|s| s.host.as_ref().is_some_and(|x| x.host == host))
            .map(|s| (side, s.sid))
    })
}

fn saved_upload_dispatch(st: &mut State, ctx: &egui::Context, ev: crate::editor::SavedEvent) {
    if st.doc_stat.is_some() {
        crate::editor::EditorHub::push_saved(ev);
        return;
    }
    st.terminal.push((
        crate::local::now_hms(),
        tfmt("[{0}] 上传编辑副本…", &[&ev.remote_path]),
    ));

    let data = match std::fs::read(&ev.local_path) {
        Ok(d) => d,
        Err(e) => {
            let msg = e.to_string();
            st.terminal.push((
                crate::local::now_hms(),
                tfmt("[{0}] 回传未完成：{1}", &[&ev.remote_path, &msg]),
            ));
            doc_upload_failed(st, ctx, ev.doc_id, &msg);
            return;
        }
    };
    let Some((side, sid)) = session_for_host(st, &ev.host) else {
        let msg = t("未找到主机会话").to_owned();
        st.terminal.push((
            crate::local::now_hms(),
            tfmt("[{0}] 回传未完成：{1}", &[&ev.remote_path, &msg]),
        ));
        doc_upload_failed(st, ctx, ev.doc_id, &msg);
        return;
    };
    st.stat_seq += 1;
    let req = st.stat_seq;
    st.doc_stat = Some((
        req,
        ev.doc_id,
        ev.remote_path.clone(),
        ev.local_path.clone(),
        data,
        ev.base_mtime,
    ));
    if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
        ss.ssh.stat(ev.remote_path.clone(), req);
    } else {
        st.doc_stat = None;
        doc_upload_failed(st, ctx, ev.doc_id, t("未找到主机会话"));
    }
}

pub(super) fn doc_upload(
    st: &mut State,
    ctx: &egui::Context,
    doc_id: u64,
    local_path: &std::path::Path,
    remote_path: &str,
) {
    let data = match std::fs::read(local_path) {
        Ok(d) => d,
        Err(e) => {
            let msg = e.to_string();
            doc_upload_failed(st, ctx, doc_id, &msg);
            return;
        }
    };
    let host = st.editors.docs.iter().find_map(|doc| {
        let d = doc.lock().unwrap();
        (d.id == doc_id)
            .then(|| d.remote.as_ref().map(|r| r.host.clone()))
            .flatten()
    });
    let Some((side, sid)) = host.as_deref().and_then(|h| session_for_host(st, h)) else {
        doc_upload_failed(st, ctx, doc_id, t("未找到主机会话"));
        return;
    };
    if let Some(ss) = st.remotes[side].sessions.iter_mut().find(|s| s.sid == sid) {
        ss.ssh.write_file(remote_path.to_owned(), doc_id, data);
    } else {
        doc_upload_failed(st, ctx, doc_id, t("未找到主机会话"));
    }
}

fn ext_upload(st: &mut State, side: usize, tab_id: u32, remote_path: &str) {
    let pre = find_remote_tab_mut(st, side, tab_id).and_then(|t| {
        t.ext_edits
            .iter()
            .find(|e| e.remote_path == remote_path)
            .map(|e| (e.local.clone(), t.sid))
    });
    let Some((local, sid)) = pre else {
        return;
    };
    if let Ok(data) = std::fs::read(&local) {
        st.stat_seq += 1;
        let req = st.stat_seq;
        if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
            tab.ext_write = Some(req);
        }
        if let Some(ssh) = session_mut(st, side, sid).map(|s| &s.ssh) {
            ssh.write_file(remote_path.to_owned(), req, data);
        } else {

            if let Some(tab) = find_remote_tab_mut(st, side, tab_id) {
                tab.ext_write = None;
            }
        }
    }
}

fn poll_ext_edits(st: &mut State) {

    const QUIET_MS: u128 = POLL_MS;
    const POLL_MS: u128 = 300;
    let now = std::time::Instant::now();
    for side in 0..2 {
        for ti in 0..st.remotes[side].tabs.len() {

            let tab_sid = st.remotes[side].tabs[ti].sid;
            let host_tag = st.remotes[side]
                .sessions
                .iter()
                .find(|s| s.sid == tab_sid)
                .and_then(|s| s.host.clone())
                .map(|h| format!("[{}]", dialogs::host_tab_naming(&h.name, &h.host)))
                .unwrap_or_else(|| format!("[{}]", t("远程")));
            let tab_id = st.remotes[side].tabs[ti].id;
            let tab = &mut st.remotes[side].tabs[ti];
            tab.ext_edits.retain(|e| e.host_tag == host_tag);
            if now.duration_since(tab.ext_poll).as_millis() < POLL_MS {
                continue;
            }
            tab.ext_poll = now;
            for e in tab.ext_edits.iter_mut() {
                let snap = ext_snapshot(&e.local);
                if snap == e.baseline {

                    if e.lock_wait_since.take().is_some() {
                        e.lock_warned = false;
                        e.lock_dialog_done = false;
                        if tab.lock_msg_on {
                            tab.pane.msg = None;
                            tab.lock_msg_on = false;
                        }
                    }
                    e.quiet_since = None;
                    e.quiet_snap = None;
                    continue;
                }
                match e.quiet_since {

                    Some(since)
                        if e.quiet_snap == Some(snap)
                            && since.elapsed().as_millis() >= QUIET_MS
                            && ext_write_lock_released(&e.local) =>
                    {

                        if e.lock_wait_since.take().is_some() {
                            e.lock_warned = false;
                            e.lock_dialog_done = false;
                            st.terminal.push((
                                crate::local::now_hms(),
                                t("写锁已释放，继续回传").to_owned(),
                            ));
                            if tab.lock_msg_on {
                                tab.pane.msg = None;
                                tab.lock_msg_on = false;
                            }
                        }

                        if let dialogs::Dialog::None = st.dialog {

                            e.baseline = snap;
                            e.quiet_since = None;
                            e.quiet_snap = None;
                            st.dialog = dialogs::Dialog::ExtEditChanged {
                                side,
                                tab: tab_id,
                                name: e.name.clone(),
                                remote_path: e.remote_path.clone(),
                            };
                        }
                    }

                    Some(since)
                        if e.quiet_snap == Some(snap)
                            && since.elapsed().as_millis() >= QUIET_MS =>
                    {
                        let started = e.lock_wait_since.get_or_insert(now);
                        let wait = started.elapsed().as_millis();
                        if wait >= LOCK_TIMEOUT_MS {
                            if !e.lock_warned {
                                e.lock_warned = true;
                                st.terminal.push((
                                    crate::local::now_hms(),
                                    tfmt(
                                        "写锁未释放，已暂缓回传：{0}（{1}s）——不会上传半写文件",
                                        &[&e.name, &wait],
                                    ),
                                ));
                            }

                            if !e.lock_dialog_done && matches!(st.dialog, dialogs::Dialog::None) {
                                st.dialog = dialogs::Dialog::ExtEditLockWait {
                                    side,
                                    tab: tab_id,
                                    name: e.name.clone(),
                                    remote_path: e.remote_path.clone(),
                                };
                                e.lock_dialog_done = true;
                            }
                        }
                    }

                    _ => {
                        e.quiet_since = Some(now);
                        e.quiet_snap = Some(snap);
                    }
                }
            }

            let lockmsg = tab.ext_edits.iter().find_map(|e| {
                let s = e.lock_wait_since.as_ref()?;
                let wait = s.elapsed().as_millis();
                (wait >= LOCK_SOFT_MS).then(|| {
                    if wait >= LOCK_TIMEOUT_MS {
                        tfmt("写锁未释放，已暂缓回传：{0}（{1}s）", &[&e.name, &wait])
                    } else {
                        t("等待编辑器完成保存…").to_owned()
                    }
                })
            });
            match lockmsg {
                Some(m) => {
                    tab.pane.msg = Some(m);
                    tab.lock_msg_on = true;
                }
                None if tab.lock_msg_on => {
                    tab.pane.msg = None;
                    tab.lock_msg_on = false;
                }
                None => {}
            }
        }
    }
}

fn build_transfer(
    st: &State,
    pane: usize,
    row: usize,
) -> Option<(crate::transfer::NewTask, usize, Option<String>, i64)> {
    let dst = 1 - pane;

    let owner = crate::transfer::TaskOwner {
        src: (pane as u8, active_tab_id(st, pane).unwrap_or(0)),
        dst: (dst as u8, active_tab_id(st, dst).unwrap_or(0)),
    };
    let src_remote = !pane_is_local(st, pane);
    let dst_remote = !pane_is_local(st, dst) && pane_view(st, dst).remote_path.is_some();
    let (src_name, src_is_dir, size, src_mtime) = match pane_view(st, pane).entries.get(row) {
        Some(e) => (e.name.clone(), e.is_dir, e.size_bytes, e.mtime_secs),
        None => return None,
    };
    match (src_remote, dst_remote) {

        (false, true) => {
            let ldir = pane_view(st, pane).path.clone()?;
            let src = ldir.join(&src_name);
            let rdir = pane_view(st, dst).remote_path.clone()?;
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
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                dst,
                Some(remote),
                src_mtime,
            ))
        }

        (true, true) => {
            let rdir = pane_view(st, pane).remote_path.clone()?;
            let remote_src = join_remote_path(&rdir, &src_name);
            let rdir_d = pane_view(st, dst).remote_path.clone()?;
            let remote_dst = join_remote_path(&rdir_d, &src_name);
            let temp = relay_temp_dir(&remote_src, &remote_dst);

            let (src_prof, dst_prof) = (
                active_remote_session(st, pane).and_then(|s| s.profile.clone())?,
                active_remote_session(st, dst).and_then(|s| s.profile.clone())?,
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
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                dst,
                Some(remote_dst),
                src_mtime,
            ))
        }

        (true, false) => {
            let ldir = pane_view(st, dst).path.clone()?;
            let rdir = pane_view(st, pane).remote_path.clone()?;
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
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                pane,
                None,
                src_mtime,
            ))
        }

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
                    probe_incomplete: false,
                    kind,
                    owner,
                },
                pane,
                None,
                src_mtime,
            ))
        }
    }
}

pub(crate) fn begin_transfer_multi(st: &mut State, pane: usize, rows: &[usize]) {
    let built: Vec<(crate::transfer::NewTask, usize, Option<String>, i64)> = rows
        .iter()
        .filter_map(|&row| build_transfer(st, pane, row))
        .collect();
    if built.is_empty() {
        return;
    }

    discard_stale_pending(st);
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
            probe_incomplete: false,
        })
        .collect();
    if items[0].stat_path.is_some() {

        st.stat_seq += 1;
        let seq = st.stat_seq;
        if let Some(ss) = active_remote_session(st, profile_side) {
            let paths: Vec<String> = items.iter().filter_map(|pi| pi.stat_path.clone()).collect();
            ss.ssh.stat_batch(paths, seq);
        }
        pane_view_mut(st, pane).msg = Some(preparing_msg());
        st.pending_tx = Some(PendingTx {
            seq,
            profile_side,
            src_pane: pane,
            at: std::time::Instant::now(),
            resolved: 0,
            ready: 0,
            batch: dnd_stream_batch(),
            batches: Vec::new(),
            items,
            placed: 0,
            held: false,
        });
    } else {

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

            st.conflict_sel = Some(conflict_defaults(&rows));
            st.pending_tx = Some(PendingTx {
                seq: 0,
                profile_side,
                src_pane: pane,
                at: std::time::Instant::now(),

                resolved: 0,
                ready: 0,
                batch: usize::MAX,
                batches: Vec::new(),
                items,
                placed: 0,
                held: false,
            });
            st.dialog = dialogs::Dialog::TransferConfirm { rows, note: None };
        } else {
            enqueue_tasks(
                st,
                profile_side,
                items.into_iter().map(|pi| pi.task).collect(),
                0,
            );
        }
    }
}

fn discard_stale_pending(st: &mut State) {
    let Some(old) = st.pending_tx.take() else {
        return;
    };
    let n = old.items.len();
    let key = if old.held {
        "已放弃上一批暂存的 {0} 项（未入队）"
    } else {
        "已放弃上一批待确认的 {0} 项（未入队）"
    };
    st.terminal
        .push((crate::local::now_hms(), tfmt(key, &[&n.to_string()])));
    if matches!(st.dialog, dialogs::Dialog::TransferConfirm { .. }) {
        st.dialog = dialogs::Dialog::None;
        st.conflict_sel = None;
    }
}

fn build_drop_item(
    rdir: &str,
    path: &std::path::Path,
    owner: crate::transfer::TaskOwner,
) -> Option<PendingItem> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let md = std::fs::metadata(path).ok();
    let is_dir = md
        .as_ref()
        .map(|m| m.is_dir())
        .unwrap_or_else(|| path.is_dir());
    let size = if is_dir {
        0
    } else {
        md.as_ref().map(|m| m.len()).unwrap_or(0)
    };
    let src_mtime = md
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let remote = join_remote_path(rdir, &name);
    let kind = if is_dir {
        crate::transfer::Kind::UploadDir {
            local: path.to_path_buf(),
            remote: remote.clone(),
        }
    } else {
        crate::transfer::Kind::UploadFile {
            local: path.to_path_buf(),
            remote: remote.clone(),
        }
    };
    Some(PendingItem {
        task: crate::transfer::NewTask {
            source: path.to_string_lossy().into_owned(),
            dest: remote.clone(),
            size,
            force: false,
            probe_incomplete: false,
            kind,
            owner,
        },
        stat_path: Some(remote),
        conflict: false,
        src_mtime,
        dst_size: None,
        dst_mtime: None,
        probe_incomplete: false,
    })
}

fn drop_ignored(st: &mut State, reason: &str) {
    st.terminal.push((
        crate::local::now_hms(),
        tfmt("外部拖入已忽略：{0}", &[&reason]),
    ));
}

pub(crate) fn begin_drop_upload(
    st: &mut State,
    side: usize,
    paths: Vec<std::path::PathBuf>,
    shift: bool,
) {
    let Some(rdir) = pane_view(st, side).remote_path.clone() else {
        drop_ignored(st, t("目标面板不在远程目录"));
        return;
    };
    let Some(_) = active_remote_session(st, side) else {
        drop_ignored(st, t("目标远程会话未连接"));
        return;
    };

    let tab_id = active_tab_id(st, side).unwrap_or(0);
    let owner = crate::transfer::TaskOwner {
        src: (side as u8, tab_id),
        dst: (side as u8, tab_id),
    };
    let items: Vec<PendingItem> = paths
        .iter()
        .filter_map(|p| build_drop_item(&rdir, p, owner))
        .collect();
    if items.is_empty() {
        drop_ignored(st, t("无可上传的条目"));
        return;
    }
    if shift {

        if let Some(ptx) = &mut st.pending_tx {
            if ptx.held && ptx.profile_side == side {
                let add = items.len();
                ptx.items.extend(items);
                let total = ptx.items.len();
                st.terminal.push((
                    crate::local::now_hms(),
                    tfmt(
                        "已追加 {0} 项到暂存（共 {1} 项）——点击面板底部「入队」开始传输",
                        &[&add.to_string(), &total.to_string()],
                    ),
                ));
                return;
            }
        }
        discard_stale_pending(st);
        let n = items.len();
        st.pending_tx = Some(PendingTx {
            seq: 0,
            profile_side: side,
            src_pane: side,
            at: std::time::Instant::now(),
            resolved: 0,
            ready: 0,
            batch: usize::MAX,
            batches: Vec::new(),
            placed: 0,
            items,
            held: true,
        });
        st.terminal.push((
            crate::local::now_hms(),
            tfmt(
                "已暂存 {0} 项（Shift 拖入）——点击面板底部「入队」开始传输",
                &[&n.to_string()],
            ),
        ));
    } else {

        discard_stale_pending(st);
        st.stat_seq += 1;
        let seq = st.stat_seq;
        if let Some(ss) = active_remote_session(st, side) {
            let stat_paths: Vec<String> =
                items.iter().filter_map(|pi| pi.stat_path.clone()).collect();
            ss.ssh.stat_batch(stat_paths, seq);
        }
        pane_view_mut(st, side).msg = Some(preparing_msg());
        st.pending_tx = Some(PendingTx {
            seq,
            profile_side: side,
            src_pane: side,
            at: std::time::Instant::now(),
            resolved: 0,
            ready: 0,
            batch: dnd_stream_batch(),
            batches: Vec::new(),
            placed: 0,
            items,
            held: false,
        });
    }
}

pub(crate) fn resume_drop_stash(st: &mut State) {
    let Some(mut ptx) = st.pending_tx.take() else {
        return;
    };
    if !ptx.held {
        st.pending_tx = Some(ptx);
        return;
    }
    if active_remote_session(st, ptx.profile_side).is_none() {
        let n = ptx.items.len();
        st.terminal.push((
            crate::local::now_hms(),
            tfmt("目标远程会话已断开，暂存 {0} 项已放弃", &[&n.to_string()]),
        ));
        return;
    }
    let n = ptx.items.len();
    st.stat_seq += 1;
    let seq = st.stat_seq;
    let stat_paths: Vec<String> = ptx
        .items
        .iter()
        .filter_map(|pi| pi.stat_path.clone())
        .collect();
    if let Some(ss) = active_remote_session(st, ptx.profile_side) {
        ss.ssh.stat_batch(stat_paths, seq);
    }
    ptx.seq = seq;
    ptx.held = false;
    ptx.batch = dnd_stream_batch();
    ptx.at = std::time::Instant::now();
    pane_view_mut(st, ptx.src_pane).msg = Some(preparing_msg());
    st.pending_tx = Some(ptx);
    st.terminal.push((
        crate::local::now_hms(),
        tfmt("暂存 {0} 项已入队（探测目标/统计文件）", &[&n.to_string()]),
    ));
}

pub(crate) fn discard_drop_stash(st: &mut State) {
    let Some(ptx) = st.pending_tx.take() else {
        return;
    };
    if !ptx.held {
        st.pending_tx = Some(ptx);
        return;
    }
    let n = ptx.items.len();
    st.terminal.push((
        crate::local::now_hms(),
        tfmt("已放弃暂存的 {0} 项（未入队）", &[&n.to_string()]),
    ));
}

fn held_stash_count(st: &State, side: usize) -> Option<usize> {
    st.pending_tx
        .as_ref()
        .filter(|p| p.held && p.profile_side == side)
        .map(|p| p.items.len())
}

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

pub(crate) fn conflict_defaults(rows: &[dialogs::ConflictRow]) -> Vec<bool> {
    rows.iter()
        .map(|r| !r.dst_mtime.is_some_and(|m| m > r.src_mtime))
        .collect()
}

fn item_default(pi: &PendingItem) -> bool {
    !pi.dst_mtime.is_some_and(|m| m > pi.src_mtime)
}

fn into_task(mut pi: PendingItem) -> crate::transfer::NewTask {
    pi.task.probe_incomplete = pi.probe_incomplete;
    pi.task
}

fn settle_tasks(
    items: Vec<PendingItem>,
    sel: Option<&[bool]>,
) -> (Vec<crate::transfer::NewTask>, u64) {
    let mut j = 0usize;
    let mut skipped = 0u64;
    let tasks: Vec<crate::transfer::NewTask> = items
        .into_iter()
        .filter_map(|mut pi| {
            if pi.conflict {
                let upload = sel
                    .and_then(|s| s.get(j).copied())
                    .unwrap_or_else(|| item_default(&pi));
                j += 1;
                if !upload {
                    skipped += 1;
                    return None;
                }
                pi.task.force = true;
            }
            Some(into_task(pi))
        })
        .collect();
    (tasks, skipped)
}

pub(crate) fn enqueue_pending(st: &mut State) {
    let Some(ptx) = st.pending_tx.take() else {
        return;
    };
    let sel = st.conflict_sel.take();
    let profile_side = ptx.profile_side;
    let (tasks, skipped) = settle_tasks(ptx.items, sel.as_deref());
    if !tasks.is_empty() {
        enqueue_tasks(st, profile_side, tasks, skipped);
    } else if skipped > 0 {

        st.terminal.push((
            crate::local::now_hms(),
            tfmt(
                "已跳过 {0} 个冲突项，无剩余任务入队",
                &[&skipped.to_string()],
            ),
        ));
    }
}

fn dnd_stream_batch() -> usize {
    crate::settings::with(|s| s.dnd_stream_batch).max(1) as usize
}

fn enqueue_tasks(
    st: &mut State,
    profile_side: usize,
    tasks: Vec<crate::transfer::NewTask>,
    skipped: u64,
) -> u64 {

    let (probe, profile) = match active_remote_session(st, profile_side) {
        Some(ss) => (ss.ssh.probe_handle(), ss.profile.clone()),
        None => (
            std::sync::Arc::new(std::sync::Mutex::new((0u64, None))),
            None,
        ),
    };
    let q = st
        .transfer
        .get_or_insert_with(crate::transfer::TransferQueue::new);
    q.enqueue(profile, probe, tasks, skipped)
}

fn request_close_tab(st: &mut State, side: usize, tab_id: u32) {
    let key = (side as u8, tab_id);
    let n = st
        .transfer
        .as_ref()
        .map(|q| {
            q.with_items(|items| {
                crate::transfer::count_owned_active(items.iter().map(|i| (&i.owner, i.status)), key)
            })
        })
        .unwrap_or(0);
    if n > 0 {
        st.dialog = dialogs::Dialog::TabClose {
            side,
            tab: tab_id,
            n,
        };
    } else {
        finish_close_tab(st, side, tab_id);
    }
}

fn finish_close_tab(st: &mut State, side: usize, tab_id: u32) {

    if st
        .tree_req
        .as_ref()
        .is_some_and(|(s, t, _, _)| *s == side && *t == tab_id)
    {
        st.tree_req = None;
    }

    st.inflight.retain(|_, v| !(v.0 == side && v.1 == tab_id));

    let was_active =
        st.locals[side].active == Some(tab_id) || st.remotes[side].active == Some(tab_id);
    let pos = if let Some(i) = st.locals[side].tabs.iter().position(|t| t.id == tab_id) {
        st.locals[side].tabs.remove(i);

        if st
            .sync_base
            .as_ref()
            .is_some_and(|b| b.l_side == side as u8 && b.l_tab == tab_id)
        {
            st.sync_browse = false;
            st.sync_base = None;
            st.sync_mirror = None;
        }
        Some(i)
    } else if let Some(i) = st.remotes[side].tabs.iter().position(|t| t.id == tab_id) {
        let sid = st.remotes[side].tabs[i].sid;
        if let Some(si) = st.remotes[side].sessions.iter().position(|s| s.sid == sid) {
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
        let merged_pos = st.locals[side].tabs.len() + i;
        st.remotes[side].tabs.remove(i);

        if st
            .pending_tx
            .as_ref()
            .is_some_and(|p| p.held && p.profile_side == side)
        {
            discard_drop_stash(st);
        }

        if st.sync_base.as_ref().is_some_and(|b| {
            (b.r_side == side as u8 && b.r_tab == tab_id)
                || (b.l_side == side as u8 && b.l_tab == tab_id)
        }) {
            st.sync_browse = false;
            st.sync_base = None;
            st.sync_mirror = None;
        }
        Some(merged_pos)
    } else {
        None
    };
    if was_active {
        st.locals[side].active = None;
        st.remotes[side].active = None;

        let merged: Vec<(u32, panes::TabKind)> = st.locals[side]
            .tabs
            .iter()
            .map(|t| (t.id, panes::TabKind::Local))
            .chain(
                st.remotes[side]
                    .tabs
                    .iter()
                    .map(|t| (t.id, panes::TabKind::Remote)),
            )
            .collect();
        if merged.is_empty() {

            if side == 0 {
                let mut pane = panes::FilePane::default();
                crate::local::navigate_root(&mut pane);
                let id = st.next_tab_id;
                st.next_tab_id += 1;
                st.locals[side] = LocalSide {
                    tabs: vec![LocalTab { id, pane }],
                    active: Some(id),
                };
            } else {
                let id = st.next_tab_id;
                st.next_tab_id += 1;
                let mut rt = placeholder_remote_tab(id);
                rt.pane.empty_title = t("先选择一个主机").to_owned();
                rt.pane.empty_sub = t("选择要浏览的本地或远端文件系统").to_owned();
                st.remotes[side] = RemoteSide {
                    tabs: vec![rt],
                    active: Some(id),
                    sessions: Vec::new(),
                };
            }
        } else {
            let new_active = match pos {
                Some(p) => {
                    let i = p.min(merged.len() - 1);
                    match side {
                        0 => merged
                            .iter()
                            .find(|(_, k)| *k == panes::TabKind::Local)
                            .map(|(id, _)| *id),
                        _ => merged.get(i).map(|(id, _)| *id),
                    }
                }
                None => st.locals[side].active.or(st.remotes[side].active),
            };
            if let Some(id) = new_active {
                if st.locals[side].tabs.iter().any(|t| t.id == id) {
                    st.locals[side].active = Some(id);
                } else {
                    st.remotes[side].active = Some(id);
                }
            }
        }
    }
}

pub(crate) use crate::transfer::join_remote as join_remote_path;

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

fn splitter(ui: &mut egui::Ui, rect: Rect, vertical: bool, st: &mut State) {
    let resp = ui.allocate_rect(rect, egui::Sense::drag());
    let d = resp.drag_delta();
    if vertical {
        st.left_w += d.x;
    } else {
        st.bottom_h -= d.y;
    }
    if resp.hovered() {
        cursor_intent(
            ui.ctx(),
            CursorLayer::Hover,
            if vertical {
                egui::CursorIcon::ResizeHorizontal
            } else {
                egui::CursorIcon::ResizeVertical
            },
        );
    }
}

pub fn edge_resize(ui: &mut egui::Ui, modal: bool) {

    if modal {
        return;
    }
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
        return;
    }
    let r = ctx.input(|i| i.viewport_rect());
    let edge = 6.0;
    let corner = 16.0;
    use egui::{CursorIcon, Pos2, ResizeDirection, Sense, Vec2, ViewportCommand};

    let zones: [(Rect, ResizeDirection, CursorIcon); 8] = [
        (
            Rect::from_min_max(r.left_top(), Pos2::new(r.right(), r.top() + edge)),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(Pos2::new(r.left(), r.bottom() - edge), r.right_bottom()),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(r.left_top(), Pos2::new(r.left() + edge, r.bottom())),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(Pos2::new(r.right() - edge, r.top()), r.right_bottom()),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
        (
            Rect::from_min_size(r.min, Vec2::splat(corner)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        (
            Rect::from_min_size(Pos2::new(r.right() - corner, r.top()), Vec2::splat(corner)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            Rect::from_min_size(
                Pos2::new(r.left(), r.bottom() - corner),
                Vec2::splat(corner),
            ),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (
            Rect::from_min_size(r.max - Vec2::splat(corner), Vec2::splat(corner)),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
    ];
    for (rect, dir, icon) in zones {
        let resp = ui.allocate_rect(rect, Sense::drag());
        if resp.hovered() {
            cursor_intent(&ctx, CursorLayer::Edge, icon);
        }
        if resp.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

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

    #[test]
    fn join_trailing_slash_normalized() {
        assert_eq!(join_remote_path("/home/user/", "a.txt"), "/home/user/a.txt");
    }

    #[test]
    fn join_multiple_trailing_slashes_normalized() {
        assert_eq!(join_remote_path("/home//", "a.txt"), "/home/a.txt");
    }
}

#[cfg(test)]
mod initial_dir_tests {
    use super::initial_remote_dir;
    use crate::hosts::HostProfile;

    #[test]
    fn sync_dir_wins() {
        let h = HostProfile {
            sync_enabled: true,
            remote_sync_path: "/srv/deploy".into(),
            last_remote_path: "/tmp".into(),
            ..Default::default()
        };
        let (init, fallback, base) = initial_remote_dir(&h, "/root");
        assert_eq!(init, "/srv/deploy");
        assert_eq!(fallback, None);
        assert_eq!(base, Some("/srv/deploy".to_owned()));
    }

    #[test]
    fn last_path_second_fallback_root() {
        let h = HostProfile {
            sync_enabled: true,
            remote_sync_path: "  ".into(),
            last_remote_path: "home/u".into(),
            ..Default::default()
        };
        let (init, fallback, base) = initial_remote_dir(&h, "/root");
        assert_eq!(init, "/home/u");
        assert_eq!(fallback, Some("/root".to_owned()));
        assert_eq!(base, Some("/home/u".to_owned()));
    }

    #[test]
    fn root_when_nothing_configured() {
        let h = HostProfile {
            last_remote_path: " ".into(),
            ..Default::default()
        };
        let (init, fallback, base) = initial_remote_dir(&h, "/root");
        assert_eq!(init, "/root");
        assert_eq!(fallback, None);
        assert_eq!(base, None);
    }
}

#[cfg(test)]
mod p4_tests {
    use super::SessionKey;

    #[test]
    fn session_key_same_triple_matches() {
        let a = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "root".into(),
        };
        let b = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "root".into(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn session_key_differs_on_port_or_user() {
        let a = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "root".into(),
        };
        let port = SessionKey {
            host: "srv1".into(),
            port: 2222,
            user: "root".into(),
        };
        let user = SessionKey {
            host: "srv1".into(),
            port: 22,
            user: "admin".into(),
        };
        assert_ne!(a, port);
        assert_ne!(a, user);
    }
}

#[cfg(test)]
mod t20g_tests {
    use super::*;

    fn pend_item(stat_path: Option<String>) -> PendingItem {
        PendingItem {
            task: crate::transfer::NewTask {
                source: "/src/a.bin".into(),
                dest: "/dst/a.bin".into(),
                size: 1,
                force: false,
                probe_incomplete: false,
                owner: crate::transfer::TaskOwner {
                    src: (0, 1),
                    dst: (1, 2),
                },
                kind: crate::transfer::Kind::UploadFile {
                    local: "/src/a.bin".into(),
                    remote: "/dst/a.bin".into(),
                },
            },
            stat_path,
            conflict: false,
            src_mtime: 0,
            dst_size: None,
            dst_mtime: None,
            probe_incomplete: false,
        }
    }

    fn pend_tx(seq: u64, items: Vec<PendingItem>) -> PendingTx {
        PendingTx {
            seq,
            profile_side: 1,
            src_pane: 0,
            at: std::time::Instant::now(),
            resolved: 0,
            ready: 0,
            batch: 4,
            batches: Vec::new(),
            placed: 0,
            items,
            held: false,
        }
    }

    #[test]
    fn held_stash_never_force_settles() {
        let mut ptx = pend_tx(0, vec![pend_item(Some("/dst/a.bin".into()))]);
        ptx.held = true;
        ptx.at = std::time::Instant::now() - std::time::Duration::from_secs(3600);
        assert_eq!(
            force_settle_expired(&mut ptx, STAT_PROBE_TIMEOUT),
            0,
            "暂存批次不受超窗兜底影响（seq=0 门）"
        );
    }

    #[test]
    fn held_stash_count_filters_by_side_and_held() {
        let mut st = State::default();
        assert_eq!(held_stash_count(&st, 1), None, "无批次不出 chip");
        let mut ptx = pend_tx(0, vec![pend_item(None), pend_item(None)]);
        ptx.held = true;
        st.pending_tx = Some(ptx);
        assert_eq!(held_stash_count(&st, 1), Some(2), "同侧暂存出条数");
        assert_eq!(held_stash_count(&st, 0), None, "异侧不出 chip");
        let mut ptx = pend_tx(9, vec![pend_item(None)]);
        ptx.held = false;
        st.pending_tx = Some(ptx);
        assert_eq!(
            held_stash_count(&st, 1),
            None,
            "常规预检批次（held=false）不出 chip"
        );
    }

    #[test]
    fn drop_item_builds_upload_task_for_file_and_dir() {
        let base = std::env::temp_dir().join(format!("ifree_drop_pin_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        let f = base.join("样本.txt");
        std::fs::write(&f, b"hello").unwrap();
        let d = base.join("子目录");
        std::fs::create_dir_all(&d).unwrap();
        let owner = crate::transfer::TaskOwner::ZERO;
        let pi = build_drop_item("/up", &f, owner).expect("文件条目可构造");
        assert!(
            matches!(pi.task.kind, crate::transfer::Kind::UploadFile { .. }),
            "文件 → UploadFile"
        );
        assert_eq!(pi.task.dest, "/up/样本.txt", "目标 = rdir + 文件名");
        assert_eq!(pi.task.size, 5, "size = 实长");
        assert_eq!(pi.stat_path.as_deref(), Some("/up/样本.txt"), "挂起待探测");
        let pd = build_drop_item("/up", &d, owner).expect("目录条目可构造");
        assert!(
            matches!(pd.task.kind, crate::transfer::Kind::UploadDir { .. }),
            "目录 → UploadDir"
        );
        assert_eq!(pd.task.size, 0, "目录 size 恒 0（面板条目同口径）");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn stat_probe_timeout_forces_settle() {
        let mut ptx = pend_tx(
            7,
            vec![pend_item(None), pend_item(Some("/dst/a.bin".into()))],
        );
        assert_eq!(
            force_settle_expired(&mut ptx, std::time::Duration::ZERO),
            1,
            "仅未回流条目计数 probe_incomplete"
        );
        assert!(ptx.items.iter().all(|pi| pi.stat_path.is_none()));
        assert!(ptx.items[1].probe_incomplete, "未回流条目带标记");
        assert!(!ptx.items[0].probe_incomplete, "已回流条目不带标记");
    }

    #[test]
    fn stat_probe_timeout_spares_local_confirm_batch() {
        let mut local = pend_tx(0, vec![pend_item(Some("/dst/b.bin".into()))]);
        assert_eq!(
            force_settle_expired(&mut local, std::time::Duration::ZERO),
            0
        );
        assert!(local.items[0].stat_path.is_some());
    }

    #[test]
    fn stream_places_by_threshold_and_settles_remainder() {
        let mut ptx = pend_tx(
            9,
            (0..10)
                .map(|i| pend_item(Some(format!("/dst/f{i:03}.bin"))))
                .collect(),
        );
        let n = ptx.items.len();
        let mut places = 0;
        let mut settled = false;
        let mut placed_total = 0;
        for _ in 0..n {

            let pi = ptx
                .items
                .iter_mut()
                .find(|pi| pi.stat_path.is_some())
                .unwrap();
            pi.stat_path = None;
            ptx.resolved += 1;
            ptx.ready += 1;
            match stream_step(&ptx) {
                StreamStep::Place => {
                    places += 1;
                    let need = ptx.ready;
                    let batch = stream_take(&mut ptx, need);
                    assert_eq!(batch.len(), ptx.batch);
                    placed_total += batch.len();
                }
                StreamStep::Settle => {
                    settled = true;
                    assert_eq!(ptx.resolved, ptx.items.len());
                    assert_eq!(
                        ptx.ready,
                        ptx.items.len(),
                        "尾批余量（无冲突测试：全部可落位）"
                    );
                    placed_total += ptx.ready;
                }
                StreamStep::None => {}
            }
            assert_eq!(
                ptx.resolved,
                ptx.items.iter().filter(|p| p.stat_path.is_none()).count(),
                "不变式：resolved == 余下已回流未落位条数"
            );
            assert_eq!(
                ptx.ready,
                ptx.items
                    .iter()
                    .filter(|p| p.stat_path.is_none() && !p.conflict)
                    .count(),
                "不变式：ready == 余下无冲突已回流未落位条数"
            );
        }
        assert_eq!(places, 2, "阈值 4 × 10 条：恰在 4/8 两次落位");
        assert!(settled, "第 10 条全量结算统一收尾");
        assert_eq!(placed_total, 10, "全部条目逐批落位无遗漏");
    }

    #[test]
    fn stream_conflicts_do_not_stop_placing_and_settle_at_complete() {
        let mut ptx = pend_tx(
            9,
            (0..12)
                .map(|i| pend_item(Some(format!("/dst/g{i}.bin"))))
                .collect(),
        );
        ptx.batch = 4;

        let pattern = [
            true, false, false, true, false, false, false, false, true, false, false, false,
        ];
        let mut places = 0;
        let mut placed_total = 0;
        let mut settled = false;
        for (i, conflict) in pattern.iter().enumerate() {
            let pi = ptx
                .items
                .iter_mut()
                .find(|pi| pi.stat_path.is_some())
                .unwrap();
            pi.stat_path = None;
            ptx.resolved += 1;
            if *conflict {
                pi.conflict = true;
            } else {
                ptx.ready += 1;
            }
            match stream_step(&ptx) {
                StreamStep::Place => {
                    places += 1;
                    let need = ptx.ready;
                    let batch = stream_take(&mut ptx, need);
                    assert!(!batch.is_empty() && !batch.iter().any(|p| p.conflict));
                    placed_total += batch.len();
                }
                StreamStep::Settle => {
                    settled = true;
                    assert_eq!(i, pattern.len() - 1, "Settle 仅在集齐（最后一条回流）");
                }
                StreamStep::None => {}
            }
            assert_eq!(ptx.seq, 9, "E 策略无中止，seq 恒保留");
        }
        assert_eq!(
            places, 2,
            "12 条（含 3 冲突）阈值 4：恰两次落位（4+4 无冲突）"
        );
        assert_eq!(placed_total, 8, "仅无冲突项落位，冲突项不占批");
        assert!(settled, "集齐统一 Settle");
        assert_eq!(
            ptx.ready, 1,
            "尾批 1 条无冲突未走 Place（不足阈值，随 Settle 交接）"
        );
        assert_eq!(
            ptx.items.iter().filter(|p| p.conflict).count(),
            3,
            "冲突项全数留置（弹窗数据源）"
        );
        assert_eq!(ptx.items.len(), 4, "尾批 1 无冲突 + 3 冲突 → 统一确认");
    }

    #[test]
    fn sliding_window_refreshes_on_arrival_and_marks_probe_incomplete() {
        let mut ptx = pend_tx(
            9,
            (0..8)
                .map(|i| pend_item(Some(format!("/dst/s{i}.bin"))))
                .collect(),
        );

        for _ in 0..4 {
            let pi = ptx
                .items
                .iter_mut()
                .find(|pi| pi.stat_path.is_some())
                .unwrap();
            pi.stat_path = None;
            ptx.resolved += 1;
            ptx.at = std::time::Instant::now();
        }
        assert_eq!(
            force_settle_expired(&mut ptx, std::time::Duration::from_secs(60)),
            0,
            "刚回流过（滑动窗重置）不触发兜底"
        );

        ptx.at = std::time::Instant::now() - std::time::Duration::from_secs(61);
        let n = force_settle_expired(&mut ptx, std::time::Duration::from_secs(60));
        assert_eq!(n, 4, "仅未回流条目计数");
        assert!(ptx.items.iter().all(|pi| pi.stat_path.is_none()));
        assert_eq!(
            ptx.items.iter().filter(|p| p.probe_incomplete).count(),
            4,
            "标记恰落在未回流条目上"
        );
    }

    #[test]
    fn scope_note_covers_placed_items_only_when_streaming_placed() {
        assert!(
            scope_note_for(0, 5, 2).is_none(),
            "placed=0 不挂范围行（整批在手，无不可逆前置）"
        );
        let note = scope_note_for(512, 100, 3).expect("placed>0 必挂范围行");
        for frag in ["512", "100", "3"] {
            assert!(
                note.contains(frag),
                "范围行须含 M/N/C 实数：{frag} 缺失于「{note}」"
            );
        }
    }

    #[test]
    fn stream_place_then_settle_enqueues_every_item() {
        let mut st = State::default();

        let mut ptx = pend_tx(
            9,
            (0..10)
                .map(|i| {
                    let dest = format!("/dst/e{i:03}.bin");
                    let mut pi = pend_item(Some(dest.clone()));
                    pi.task.dest = dest.clone();
                    pi.task.kind = crate::transfer::Kind::UploadFile {
                        local: format!("/src/e{i:03}.bin").into(),
                        remote: dest,
                    };
                    pi
                })
                .collect(),
        );
        ptx.batch = 4;
        st.pending_tx = Some(ptx);
        let mut places = 0;
        let mut settled = false;
        for _ in 0..10 {
            {
                let ptx = st.pending_tx.as_mut().unwrap();
                let pi = ptx
                    .items
                    .iter_mut()
                    .find(|pi| pi.stat_path.is_some())
                    .unwrap();
                pi.stat_path = None;
                ptx.resolved += 1;
                ptx.ready += 1;
            }
            match stream_step(st.pending_tx.as_ref().unwrap()) {
                StreamStep::Place => {
                    places += 1;
                    stream_place(&mut st);
                }
                StreamStep::Settle => {
                    settled = true;
                    stream_settle(&mut st);
                }
                StreamStep::None => {}
            }
        }
        assert_eq!(places, 2, "阈值 4 × 10 条：4/8 两次 Place");
        assert!(settled, "第 10 条集齐走 stream_settle");
        assert!(st.pending_tx.is_none(), "结算后无悬挂 pending");
        assert!(matches!(st.dialog, dialogs::Dialog::None), "无冲突不出弹窗");
        let q = st.transfer.as_ref().expect("逐批入队");
        assert_eq!(
            q.with_items(|items| items.len()),
            10,
            "落位 8 + 尾批 2 = 10"
        );
        for k in 0..10 {
            let needle = format!("/dst/e{k:03}.bin");
            assert_eq!(
                q.with_items(|items| items
                    .iter()
                    .filter(|i| i.dest.starts_with(needle.as_str()))
                    .count()),
                1,
                "{needle} 恰入队一次（零缺项 / 零重复）"
            );
        }
    }

    #[test]
    fn ten_thousand_stream_batches_keep_every_item() {
        const N: usize = 10_000;
        let mut items: Vec<PendingItem> = (0..N)
            .map(|i| pend_item(Some(format!("/dst/f{i:05}.bin"))))
            .collect();

        for i in [5000usize, 5001, 5002] {
            items[i].conflict = true;
        }
        let mut ptx = pend_tx(9, items);
        ptx.batch = 256;
        let mut placed = 0usize;
        loop {
            let mut newly = 0usize;
            let mut newly_ready = 0usize;
            for pi in ptx.items.iter_mut() {
                if newly == 256 {
                    break;
                }
                if pi.stat_path.take().is_some() {
                    newly += 1;
                    if !pi.conflict {
                        newly_ready += 1;
                    }
                }
            }
            if newly == 0 {
                break;
            }
            ptx.resolved += newly;
            ptx.ready += newly_ready;
            loop {
                match stream_step(&ptx) {
                    StreamStep::Place => {
                        let need = ptx.ready;
                        let batch = stream_take(&mut ptx, need);
                        assert!(!batch.iter().any(|p| p.conflict), "冲突项不进落位批");
                        placed += batch.len();
                    }
                    StreamStep::Settle | StreamStep::None => break,
                }
            }
        }
        let remaining = ptx.items.len();
        assert_eq!(
            placed + remaining,
            N,
            "落位数 {placed} + 剩余入队数 {remaining} == 10000，零缺项"
        );
        assert!(
            placed >= N - 3 - 256,
            "绝大多数按 256 逐批落位，未被卡住（placed={placed}）"
        );
        assert!(
            remaining <= 256 + 3,
            "尾批不超过一个单位 + 冲突留置项（remaining={remaining}）"
        );
        assert_eq!(
            ptx.items.iter().filter(|p| p.conflict).count(),
            3,
            "中段 3 条冲突全数留置到集齐（无提前弹窗）"
        );
        assert_eq!(
            ptx.items.iter().filter(|p| p.stat_path.is_some()).count(),
            0,
            "全数回流完毕，无挂起 STAT"
        );
    }

    #[test]
    fn stream_batch_threshold_floor_and_disabled() {
        assert_eq!(0i32.max(1) as usize, 1);
        let mut ptx = pend_tx(9, vec![pend_item(Some("/dst/h.bin".into()))]);
        ptx.batch = usize::MAX;
        let pi = ptx
            .items
            .iter_mut()
            .find(|pi| pi.stat_path.is_some())
            .unwrap();
        pi.stat_path = None;
        ptx.resolved += 1;
        ptx.ready += 1;

        assert!(matches!(stream_step(&ptx), StreamStep::Settle));
    }

    #[test]
    fn probe_timeout_settle_terminal_notice_and_task_passthrough() {

        let mut st = State::default();
        let mut ptx = pend_tx(
            9,
            vec![pend_item(None), pend_item(Some("/dst/a.bin".into()))],
        );
        ptx.at = std::time::Instant::now() - std::time::Duration::from_secs(61);
        st.pending_tx = Some(ptx);
        apply_ssh_events(&mut st, &egui::Context::default());
        assert!(st.pending_tx.is_none(), "超窗批结算清空");
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains("预检超时")),
            "终端落对账提示：{:?}",
            st.terminal
        );
        let q = st.transfer.as_ref().expect("放行项直接入队");
        assert_eq!(q.with_items(|items| items.len()), 2, "整批放行入队");

        let mut ptx2 = pend_tx(
            9,
            vec![pend_item(None), pend_item(Some("/dst/a.bin".into()))],
        );
        ptx2.at = std::time::Instant::now() - std::time::Duration::from_secs(61);
        force_settle_expired(&mut ptx2, std::time::Duration::from_secs(60));
        let (tasks, _) = settle_tasks(ptx2.items, None);
        assert!(!tasks[0].probe_incomplete, "已回流项不带透传位");
        assert!(tasks[1].probe_incomplete, "超时放行项 NewTask 透传位为真");
    }
}

#[cfg(test)]
mod t20f1_tests {
    use super::*;

    fn conf_items(n_ok: usize, n_conflict: usize) -> Vec<PendingItem> {
        (0..n_ok + n_conflict)
            .map(|i| {
                let conflict = i >= n_ok;
                let dest = if conflict {
                    format!("/dst/c{i}.bin")
                } else {
                    format!("/dst/ok{i}.bin")
                };
                PendingItem {
                    task: crate::transfer::NewTask {
                        source: format!("/src/f{i}.bin"),
                        dest: dest.clone(),
                        size: 1,
                        force: false,
                        probe_incomplete: false,
                        owner: crate::transfer::TaskOwner {
                            src: (0, 1),
                            dst: (1, 2),
                        },
                        kind: crate::transfer::Kind::UploadFile {
                            local: format!("/src/f{i}.bin").into(),
                            remote: dest,
                        },
                    },
                    stat_path: None,
                    conflict,
                    src_mtime: 0,
                    dst_size: None,
                    dst_mtime: None,
                    probe_incomplete: false,
                }
            })
            .collect()
    }

    #[test]
    fn settle_tasks_selection_splits_per_item() {
        let (tasks, skipped) = settle_tasks(conf_items(2, 2), Some(&[true, true]));
        assert_eq!(skipped, 0, "全勾不计 skipped");
        assert_eq!(tasks.len(), 4, "全勾 = 全量入队");
        for (i, task) in tasks.iter().enumerate() {
            assert_eq!(
                task.force,
                i >= 2,
                "勾选冲突项 force（C# forceOverwrite），非冲突项不置"
            );
        }
        let (tasks, skipped) = settle_tasks(conf_items(2, 2), Some(&[false, false]));
        assert_eq!(skipped, 2, "全不勾 = 冲突项计数");
        assert_eq!(tasks.len(), 2, "全不勾 = 剔除冲突项后入队其余");
        assert!(tasks.iter().all(|t| !t.force), "非冲突项不带 force");

        let (tasks, skipped) = settle_tasks(conf_items(2, 2), Some(&[false, true]));
        assert_eq!(skipped, 1, "未勾冲突项计入 skipped");
        assert_eq!(tasks.len(), 3, "非冲突 2 项 + 勾选冲突 1 项");
        let forced: Vec<_> = tasks.iter().filter(|t| t.force).collect();
        assert_eq!(forced.len(), 1, "仅勾选的冲突项带 force");
        assert!(
            forced[0].dest.starts_with("/dst/c3"),
            "勾选的是第 2 个冲突项：{:?}",
            forced[0].dest
        );
    }

    #[test]
    fn conflict_defaults_newer_dst_unchecked() {
        let row = |src_mtime: i64, dst_mtime: Option<i64>| dialogs::ConflictRow {
            name: "a.bin".into(),
            src_size: 1,
            src_mtime,
            dst_size: Some(1),
            dst_mtime,
        };
        let rows = vec![
            row(100, Some(200)),
            row(200, Some(100)),
            row(100, Some(100)),
            row(100, None),
        ];
        assert_eq!(
            conflict_defaults(&rows),
            vec![false, true, true, true],
            "目标较新默认不勾，其余默认勾选"
        );
    }

    #[test]
    fn conflict_settle_overwrite_enqueues_all() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(3, 2));
        assert!(
            matches!(st.dialog, dialogs::Dialog::TransferConfirm { .. }),
            "冲突批出确认弹窗"
        );
        assert_eq!(
            st.conflict_sel.as_deref(),
            Some([true, true].as_slice()),
            "弹窗创建即填勾选默认值（夹具无目标 mtime → 全勾）"
        );
        let ptx = st.pending_tx.as_ref().expect("弹窗批次暂存");
        assert_eq!(ptx.seq, 0, "弹窗批次 seq 清零（迟到回流落空匹配）");
        assert_eq!(
            ptx.items.len(),
            5,
            "完整任务项保留在 pending_tx（NewTask 数据源）"
        );
        assert_eq!(ptx.items.iter().filter(|p| p.conflict).count(), 2);
        enqueue_pending(&mut st);
        assert!(st.pending_tx.is_none(), "应用后无悬挂 pending");
        assert!(st.conflict_sel.is_none(), "勾选态随应用 take 清空");
        let q = st.transfer.as_ref().expect("应用 = 入队");
        assert_eq!(q.with_items(|items| items.len()), 5, "队列任务数 = 全量");
        assert_eq!(
            q.with_items(|items| items
                .iter()
                .filter(|i| i.dest.starts_with("/dst/c"))
                .count()),
            2,
            "冲突任务照常入队（force 由 settle_tasks 钉子断言）"
        );
    }

    #[test]
    fn conflict_settle_skip_enqueues_rest_only() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(3, 2));
        st.conflict_sel = Some(vec![false, false]);
        enqueue_pending(&mut st);
        assert!(st.pending_tx.is_none(), "应用后无悬挂 pending");
        assert!(st.conflict_sel.is_none(), "勾选态随应用 take 清空");
        let q = st.transfer.as_ref().expect("非冲突余项仍入队");
        assert_eq!(
            q.with_items(|items| items.len()),
            3,
            "队列任务数 = 非冲突数"
        );
        assert!(
            q.with_items(|items| items.iter().all(|i| i.dest.starts_with("/dst/ok"))),
            "未勾冲突项被剔除"
        );
    }

    #[test]
    fn all_conflict_skip_aborts_with_terminal_log_and_zero_enqueue() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(0, 3));
        st.conflict_sel = Some(vec![false, false, false]);
        enqueue_pending(&mut st);
        assert!(st.pending_tx.is_none());
        assert!(st.transfer.is_none(), "全冲突 + 全不勾 = 零入队");
        assert_eq!(st.terminal.len(), 1, "对账日志可见（skipped 计数不丢）");
        assert!(
            st.terminal[0].1.contains('3'),
            "日志含 skipped 实数：{:?}",
            st.terminal[0].1
        );
    }

    #[test]
    fn cancel_clears_pending_without_enqueue() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(3, 2));
        cancel_pending_stream(&mut st);
        assert!(st.pending_tx.is_none(), "取消后暂存清空");
        assert!(st.conflict_sel.is_none(), "取消后勾选态清空");
        assert!(st.transfer.is_none(), "队列零新增");
        assert_eq!(st.terminal.len(), 1, "明确放弃留对账日志");
        assert!(
            st.terminal[0].1.contains('5'),
            "日志含未入队条数：{:?}",
            st.terminal[0].1
        );
    }

    #[test]
    fn local_settle_without_conflict_enqueues_directly() {
        let mut st = State::default();
        settle_batch(&mut st, 0, 0, conf_items(4, 0));
        assert!(matches!(st.dialog, dialogs::Dialog::None), "无冲突不出弹窗");
        assert!(st.pending_tx.is_none());
        let q = st.transfer.as_ref().expect("无冲突直接入队");
        assert_eq!(q.with_items(|items| items.len()), 4);
    }

    #[test]
    fn shell_close_esc_settles_pending() {
        let mut st = State::default();
        settle_batch(&mut st, 1, 0, conf_items(2, 1));
        assert!(st.pending_tx.is_some());
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        let p = crate::theme::palette(&ctx);
        let mut out = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = dialogs::shell(
                    ui,
                    &mut st,
                    &p,
                    640.0,
                    360.0,
                    t("传输确认"),
                    false,
                    0.0,
                    false,
                    None,

                    dialogs::ShellOpts {
                        on_close: dialogs::CloseAction::CancelPendingStream,
                        ..Default::default()
                    },
                    |_, _, _| {},
                );
                assert!(!r, "Esc 关闭");
            });
        });

        out.textures_delta.clear();
        assert!(matches!(st.dialog, dialogs::Dialog::None));
        assert!(
            st.pending_tx.is_none(),
            "关闭路径结算悬挂 pending（busy 判据 pending_tx 项复位）"
        );
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains('3')),
            "关闭 = 明确放弃，终端留对账日志"
        );
    }

    #[test]
    fn preparing_msg_clears_in_both_languages() {
        let zh = "正在准备传输（探测目标/统计文件）…";
        let en = "Preparing transfer (checking target/counting files)…";
        assert_eq!(crate::i18n::t_for(zh, false), zh);
        assert_eq!(crate::i18n::t_for(zh, true), en, "英文键活且逐字一致");
        assert!(is_preparing_msg(zh, false), "中文态识别中文准备提示");
        assert!(!is_preparing_msg(en, false), "中文态不误清英文提示");
        assert!(!is_preparing_msg(zh, true), "英文态不误清中文提示");
        assert!(is_preparing_msg(en, true), "英文态能识别英文准备提示");
        assert!(
            !en.starts_with("正在准备传输"),
            "旧前缀实现在英文态恒不匹配（回归证据）"
        );

        let mut st = State::default();
        pane_view_mut(&mut st, 0).msg = Some(preparing_msg());
        clear_preparing_msg(&mut st, 0);
        assert!(
            pane_view_mut(&mut st, 0).msg.is_none(),
            "准备提示结算后清除"
        );
        pane_view_mut(&mut st, 0).msg = Some("目录读取失败".into());
        clear_preparing_msg(&mut st, 0);
        assert_eq!(
            pane_view_mut(&mut st, 0).msg.as_deref(),
            Some("目录读取失败"),
            "期间被错误覆写则保留"
        );
    }

    #[test]
    fn cancel_settles_already_placed_batches() {
        let mut st = State::default();

        let first: Vec<crate::transfer::NewTask> =
            conf_items(2, 0).into_iter().map(into_task).collect();
        enqueue_tasks(&mut st, 1, first, 0);
        let before = st
            .transfer
            .as_ref()
            .unwrap()
            .with_items(|items| items.len());
        assert_eq!(before, 2, "已落位批次在队列里");
        settle_batch(&mut st, 1, 0, conf_items(2, 1));
        if let Some(ptx) = st.pending_tx.as_mut() {
            ptx.batches.push(1);
            ptx.placed = before;
            ptx.resolved = ptx.items.len();
        }
        cancel_pending_stream(&mut st);
        assert!(st.pending_tx.is_none(), "取消后暂存清空");
        assert!(st.conflict_sel.is_none(), "勾选态清空");
        assert_eq!(
            st.transfer
                .as_ref()
                .unwrap()
                .with_items(|items| items.len()),
            before,
            "取消不新增队列行（已落位批次按 cancel_batches 结算，不删行）"
        );
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains('3')),
            "明确放弃留对账日志（未入队条数）"
        );
    }
}

#[cfg(test)]
mod cr6_tests {
    use super::*;
    use crate::ssh::Ev;

    fn sess(host: &str) -> SideSession {
        SideSession {
            sid: 1,
            key: SessionKey {
                host: host.into(),
                port: 22,
                user: "u".into(),
            },
            ssh: crate::ssh::Ssh::spawn(),
            host: None,
            profile: None,
            state: RemoteState::Off,
            root: None,
            fp_ask: None,
            refs: 1,
        }
    }

    fn has_file_written(ssh: &crate::ssh::Ssh) -> bool {
        for _ in 0..40 {
            if ssh
                .poll()
                .iter()
                .any(|e| matches!(e, Ev::FileWritten { .. }))
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn saved_upload_read_failure_sends_no_ssh_and_stays_retryable() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        let bad = std::env::temp_dir().join("ifreesftp-cr6-missing-copy.txt");
        let _ = std::fs::remove_file(&bad);
        st.editors
            .open_remote("h1", &bad, "/r.txt".into(), Some(1000), true);
        saved_upload_dispatch(
            &mut st,
            &egui::Context::default(),
            crate::editor::SavedEvent {
                doc_id: 1,
                local_path: bad.clone(),
                remote_path: "/r.txt".into(),
                host: "h1".into(),
                base_mtime: Some(1000),
            },
        );
        assert!(st.doc_stat.is_none(), "读失败不挂 STAT 槽（零 ssh 请求）");
        let d = st.editors.docs[0].lock().unwrap();
        assert!(d.dirty, "失败态 dirty 回 true：再按 Ctrl+S 可重试");
        assert!(!d.pending_upload, "在途态清零");
        assert!(
            d.save_err.as_deref().unwrap_or("").contains("回传未完成"),
            "save_err 记因：{}",
            d.save_err.as_deref().unwrap_or("")
        );
        drop(d);
        assert!(
            st.terminal.iter().any(|(_, m)| m.contains("回传未完成")),
            "终端留失败对账日志"
        );
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "write_file 调用数为 0（远端内容不变）"
        );
    }

    #[test]
    fn saved_upload_session_mismatch_keeps_copy_and_marks_retryable() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h9"));
        let copy = std::env::temp_dir().join("ifreesftp-cr6-mismatch-copy.txt");
        std::fs::write(&copy, b"payload").unwrap();
        st.editors
            .open_remote("h1", &copy, "/r.txt".into(), Some(1000), true);
        saved_upload_dispatch(
            &mut st,
            &egui::Context::default(),
            crate::editor::SavedEvent {
                doc_id: 1,
                local_path: copy.clone(),
                remote_path: "/r.txt".into(),
                host: "h1".into(),
                base_mtime: Some(1000),
            },
        );
        assert!(st.doc_stat.is_none(), "失配不挂 STAT 槽（零 ssh 请求）");
        assert!(copy.exists(), "副本保留");
        let d = st.editors.docs[0].lock().unwrap();
        assert!(d.dirty, "失败态可重试");
        assert!(!d.pending_upload);
        assert!(d.save_err.is_some());
        drop(d);
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "write_file 调用数为 0（远端内容不变）"
        );
    }

    #[test]
    fn internal_conflict_verdict_matches_external_predicate() {
        let cases: [(Option<i64>, Option<i64>, bool); 7] = [
            (Some(1000), Some(1000), true),
            (Some(1000), Some(1001), true),
            (Some(1001), Some(1000), true),
            (Some(1000), Some(1002), true),
            (None, Some(1001), true),
            (Some(1000), None, true),
            (Some(1000), Some(1001), false),
        ];
        for (base, remote, exists) in cases {

            let external = match (base, remote) {
                (Some(a), Some(b)) => exists && a.abs_diff(b) >= 1,
                _ => false,
            };
            assert_eq!(
                mtime_conflict(base, remote, exists),
                external,
                "base={base:?} remote={remote:?} exists={exists}"
            );
        }
    }

    #[test]
    fn file_written_ok_receipt_bytes_mtime_match() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        let copy = std::env::temp_dir().join("ifreesftp-cr6-receipt-copy.txt");
        let content = b"hello world";
        std::fs::write(&copy, content).unwrap();
        st.editors
            .open_remote("h1", &copy, "/r.txt".into(), Some(1000), true);
        st.editors.docs[0].lock().unwrap().pending_upload = true;
        apply_ssh_event(
            &mut st,
            &egui::Context::default(),
            0,
            1,
            Ev::FileWritten {
                req: 1,
                path: "/r.txt".into(),
                result: Ok(Some(1720000001)),
            },
        );
        let d = st.editors.docs[0].lock().unwrap();
        assert_eq!(
            d.remote.as_ref().unwrap().base_mtime,
            Some(1720000001),
            "base mtime 回填 = 回执值（重新 STAT 将看到的 mtime）"
        );
        assert!(!d.dirty, "回执后清 dirty");
        assert!(!d.pending_upload, "回执后在途态清零");
        let rct = d.last_receipt.clone().unwrap();
        drop(d);
        assert!(rct.contains("11"), "回执带字节数：{rct}");
        assert!(rct.contains("/r.txt"), "回执带远端路径：{rct}");
        assert_eq!(
            std::fs::metadata(&copy).unwrap().len(),
            11,
            "回执字节数 = 副本实长（= 重新 STAT 的 size）"
        );
        assert!(
            st.terminal
                .iter()
                .any(|(_, m)| m.contains("11") && m.contains("1720000001")),
            "终端日志带字节数/mtime 实数"
        );
    }

    #[test]
    #[cfg(windows)]
    fn ext_write_lock_released_sharing_violation_waits() {
        let f = std::env::temp_dir().join("ifreesftp-extlock-probe.txt");
        std::fs::write(&f, b"probe").unwrap();

        assert!(ext_write_lock_released(&f), "无占用应判已释放");

        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_WRITE: u32 = 0x2;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_WRITE)
            .open(&f)
            .expect("持锁句柄");
        assert!(
            !ext_write_lock_released(&f),
            "共享冲突应判锁未释放（反判回归）"
        );

        drop(holder);
        assert!(ext_write_lock_released(&f), "释放后应判已释放");

        assert!(ext_write_lock_released(
            &std::env::temp_dir().join("ifreesftp-extlock-missing.txt")
        ));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn file_written_err_marks_doc_retryable_never_dirty_false() {
        let ctx = egui::Context::default();
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        st.editors.open_remote(
            "h1",
            &std::env::temp_dir().join("ifreesftp-fwerr.txt"),
            "/r.txt".into(),
            Some(1000),
            true,
        );
        {
            let mut d = st.editors.docs[0].lock().unwrap();
            d.pending_upload = true;
        }
        apply_ssh_event(
            &mut st,
            &ctx,
            0,
            1,
            Ev::FileWritten {
                req: 1,
                path: "/r.txt".into(),
                result: Err("permission denied".into()),
            },
        );
        let d = st.editors.docs[0].lock().unwrap();
        assert!(d.dirty, "写失败 dirty 必须回 true（不得 dirty=false）");
        assert!(!d.pending_upload, "在途态清零：再按 Ctrl+S 可重试");
        assert!(
            d.save_err
                .as_deref()
                .unwrap_or("")
                .contains("permission denied"),
            "save_err 记因：{:?}",
            d.save_err
        );
    }

    #[test]
    fn cr2_save_error_bar_geometry_does_not_overlap_panes() {
        let full = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 800.0));
        let (rest, none) = save_error_bar_rect(full, false);
        assert_eq!(rest, full, "无失败态时布局一字不变");
        assert!(none.is_none());
        let (rest, bar) = save_error_bar_rect(full, true);
        let bar = bar.expect("失败态应切出底部条");
        assert_eq!(bar.bottom(), full.bottom());
        assert_eq!(bar.height(), SAVE_BAR_H);
        assert_eq!(rest.bottom(), bar.top(), "主区不得与提示条重叠");
        assert_eq!(rest.top(), full.top());
        let tiny = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 100.0));
        let (rest, none) = save_error_bar_rect(tiny, true);
        assert_eq!(rest, tiny, "过矮窗口不切条");
        assert!(none.is_none());
    }

    #[test]
    fn cr2_save_failure_uses_slow_heartbeat_not_fast() {
        let mut st = State::default();
        st.remotes[0].tabs.push(placeholder_remote_tab(1));
        assert_eq!(repaint_delay(&st), None, "无失败态应自停");
        st.save_err_pending = true;
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(1000)),
            "失败在途应慢档兜底"
        );
        st.doc_stat = Some((
            1,
            1,
            "/r.txt".into(),
            std::path::PathBuf::new(),
            vec![],
            None,
        ));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "其它在途工作仍优先快帧"
        );
    }

    #[test]
    fn repaint_delay_four_tiers() {
        let mut st = State::default();
        st.remotes[0].tabs.push(placeholder_remote_tab(1));

        assert_eq!(repaint_delay(&st), None, "全清应自停");

        st.doc_stat = Some((
            1,
            1,
            "/r.txt".into(),
            std::path::PathBuf::new(),
            vec![],
            None,
        ));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "在途应 120ms"
        );
        st.doc_stat = None;

        st.remotes[0].tabs[0].ext_write = Some(9);
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "ext_write 在途应 120ms"
        );
        st.remotes[0].tabs[0].ext_write = None;

        let f = std::env::temp_dir().join("ifreesftp-rd-tier.txt");
        std::fs::write(&f, b"tier").unwrap();
        st.remotes[0].tabs[0].ext_edits.push(ExtEdit {
            host_tag: "[h1]".into(),
            remote_path: "/r.txt".into(),
            name: "r.txt".into(),
            local: f.clone(),
            base_mtime: Some(1000),
            baseline: (5, 1000),
            quiet_since: None,
            quiet_snap: None,
            lock_wait_since: None,
            lock_warned: false,
            lock_dialog_done: false,
        });
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(300)),
            "仅外部会话应 300ms"
        );

        st.remotes[0].tabs[0].ext_edits[0].lock_wait_since =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(11000));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(1000)),
            "超时兜底应 1000ms"
        );

        st.remotes[0].tabs[0].ext_stat = Some((7, "/r.txt".into()));
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "在途应优先于兜底"
        );
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    #[cfg(windows)]
    fn lock_wait_timeout_warns_once_recovers_with_single_info() {
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        st.remotes[0].tabs.push(placeholder_remote_tab(1));
        st.remotes[0].tabs[0].sid = 1;
        let copy = std::env::temp_dir().join("ifreesftp-lockwait-copy.txt");
        std::fs::write(&copy, b"payload").unwrap();
        let host_tag = format!("[{}]", t("远程"));
        let snap0 = ext_snapshot(&copy);
        st.remotes[0].tabs[0].ext_edits.push(ExtEdit {
            host_tag,
            remote_path: "/r.txt".into(),
            name: "r.txt".into(),
            local: copy.clone(),
            base_mtime: Some(1000),
            baseline: (snap0.0 + 1, snap0.1 + 1),
            quiet_since: Some(std::time::Instant::now() - std::time::Duration::from_millis(1000)),
            quiet_snap: Some(ext_snapshot(&copy)),
            lock_wait_since: Some(
                std::time::Instant::now() - std::time::Duration::from_millis(11000),
            ),
            lock_warned: false,
            lock_dialog_done: false,
        });

        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_WRITE: u32 = 0x2;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_WRITE)
            .open(&copy)
            .expect("持锁句柄");
        let back = |st: &mut State| {
            st.remotes[0].tabs[0].ext_poll =
                std::time::Instant::now() - std::time::Duration::from_millis(400);
        };

        back(&mut st);
        poll_ext_edits(&mut st);
        let warns = st
            .terminal
            .iter()
            .filter(|(_, m)| m.contains("不会上传半写文件"))
            .count();
        assert_eq!(warns, 1, "超时 WARN 恰 1 条");
        assert!(
            st.remotes[0].tabs[0]
                .pane
                .msg
                .as_deref()
                .unwrap_or("")
                .contains("写锁未释放"),
            "状态栏持久提示：{:?}",
            st.remotes[0].tabs[0].pane.msg
        );
        assert!(st.remotes[0].tabs[0].lock_msg_on, "独立清除标记置位");
        assert!(
            matches!(
                st.dialog,
                dialogs::Dialog::ExtEditLockWait { ref name, .. } if name == "r.txt"
            ),
            "超时应弹退出对话框"
        );
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "超时兜底绝不放行上传（零 write_file）"
        );
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(1000)),
            "兜底期间心跳 1000ms"
        );

        back(&mut st);
        poll_ext_edits(&mut st);
        assert_eq!(
            st.terminal
                .iter()
                .filter(|(_, m)| m.contains("不会上传半写文件"))
                .count(),
            1,
            "WARN 去重：不得每轮刷屏"
        );

        drop(holder);
        back(&mut st);
        poll_ext_edits(&mut st);
        let infos = st
            .terminal
            .iter()
            .filter(|(_, m)| m.contains("写锁已释放，继续回传"))
            .count();
        assert_eq!(infos, 1, "锁释放 INFO 恰 1 条");
        assert!(st.remotes[0].tabs[0].ext_edits[0].lock_wait_since.is_none());

        st.dialog = dialogs::Dialog::None;
        st.remotes[0].tabs[0].ext_edits[0].quiet_since =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(400));
        back(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditChanged { .. }),
            "锁释放后自动弹确认框"
        );

        back(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            st.remotes[0].tabs[0].pane.msg.is_none() && !st.remotes[0].tabs[0].lock_msg_on,
            "回基线后提示清除：{:?}",
            st.remotes[0].tabs[0].pane.msg
        );
        let _ = std::fs::remove_file(&copy);
    }

    #[cfg(windows)]
    fn lockwait_fixture() -> (State, std::fs::File, std::path::PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut st = State::default();
        st.remotes[0].sessions.push(sess("h1"));
        st.remotes[0].tabs.push(placeholder_remote_tab(1));
        st.remotes[0].tabs[0].sid = 1;
        let copy = std::env::temp_dir().join(format!(
            "ifreesftp-lockwait-dialog-copy-{}-{seq}.txt",
            std::process::id()
        ));
        std::fs::write(&copy, b"payload").unwrap();
        let host_tag = format!("[{}]", t("远程"));
        let snap0 = ext_snapshot(&copy);
        st.remotes[0].tabs[0].ext_edits.push(ExtEdit {
            host_tag,
            remote_path: "/r.txt".into(),
            name: "r.txt".into(),
            local: copy.clone(),
            base_mtime: Some(1000),
            baseline: (snap0.0 + 1, snap0.1 + 1),
            quiet_since: Some(std::time::Instant::now() - std::time::Duration::from_millis(1000)),
            quiet_snap: Some(ext_snapshot(&copy)),
            lock_wait_since: Some(
                std::time::Instant::now() - std::time::Duration::from_millis(11000),
            ),
            lock_warned: false,
            lock_dialog_done: false,
        });
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_WRITE: u32 = 0x2;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_WRITE)
            .open(&copy)
            .expect("持锁句柄");
        (st, holder, copy)
    }

    #[cfg(windows)]
    fn back400(st: &mut State) {
        st.remotes[0].tabs[0].ext_poll =
            std::time::Instant::now() - std::time::Duration::from_millis(400);
    }

    #[test]
    #[cfg(windows)]
    fn lock_wait_dialog_dismiss_does_not_reappear() {
        let (mut st, holder, copy) = lockwait_fixture();
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }),
            "首次超时弹框"
        );

        st.dialog = dialogs::Dialog::None;
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(matches!(st.dialog, dialogs::Dialog::None), "关闭后不再重现");
        let e = &st.remotes[0].tabs[0].ext_edits[0];
        assert!(e.lock_wait_since.is_some(), "等待态保留（继续计时）");
        assert!(e.lock_warned, "WARN 去重标记保持");
        assert!(e.lock_dialog_done, "弹框去重标记已置位（本次等锁不再弹）");
        assert!(
            st.remotes[0].tabs[0]
                .pane
                .msg
                .as_deref()
                .unwrap_or("")
                .contains("写锁未释放"),
            "状态栏持久提示仍在：{:?}",
            st.remotes[0].tabs[0].pane.msg
        );
        assert_eq!(
            st.terminal
                .iter()
                .filter(|(_, m)| m.contains("不会上传半写文件"))
                .count(),
            1,
            "WARN 恰 1 条（日志去重不变）"
        );
        assert!(
            !has_file_written(&st.remotes[0].sessions[0].ssh),
            "锁未释放期间零 write_file"
        );
        drop(holder);
        let _ = std::fs::remove_file(&copy);
    }

    #[test]
    #[cfg(windows)]
    fn lock_wait_dialog_defers_when_dialog_occupied() {
        let (mut st, holder, copy) = lockwait_fixture();
        st.dialog = dialogs::Dialog::SyncMismatch { msg: "busy".into() };
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::SyncMismatch { .. }),
            "占用框不被顶掉"
        );
        assert!(
            !st.remotes[0].tabs[0].ext_edits[0].lock_dialog_done,
            "占用轮不置位（否则永久不弹）"
        );
        assert!(
            st.remotes[0].tabs[0].ext_edits[0].lock_warned,
            "WARN 照常打（日志去重与弹框去重分离）"
        );

        st.dialog = dialogs::Dialog::None;
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }),
            "占用解除后补弹"
        );
        drop(holder);
        let _ = std::fs::remove_file(&copy);
    }

    #[test]
    #[cfg(windows)]
    fn lock_wait_dialog_next_wait_can_reopen() {
        let (mut st, holder, copy) = lockwait_fixture();
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }));
        st.dialog = dialogs::Dialog::None;

        let e0 = &mut st.remotes[0].tabs[0].ext_edits[0];
        e0.quiet_since = None;
        e0.quiet_snap = None;
        e0.lock_wait_since = None;
        e0.lock_warned = false;
        e0.lock_dialog_done = false;
        st.remotes[0].tabs[0].lock_msg_on = false;

        let e1 = &mut st.remotes[0].tabs[0].ext_edits[0];
        e1.quiet_since = Some(std::time::Instant::now() - std::time::Duration::from_millis(1000));
        e1.quiet_snap = Some(ext_snapshot(&e1.local));
        e1.lock_wait_since =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(11000));
        back400(&mut st);
        poll_ext_edits(&mut st);
        assert!(
            matches!(st.dialog, dialogs::Dialog::ExtEditLockWait { .. }),
            "复位后下一次等锁超时可再弹"
        );
        drop(holder);
        let _ = std::fs::remove_file(&copy);
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    fn frame(pointer: Option<egui::Pos2>, f: impl Fn(&mut egui::Ui, bool)) {
        let ctx = egui::Context::default();
        for pass in 0..2 {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            ));
            if let Some(pos) = pointer {
                input.events.push(egui::Event::PointerMoved(pos));
            }
            let check = pass == 1;
            let mut out = ctx.run_ui(input, |ui| f(ui, check));

            out.textures_delta.clear();
        }
    }

    fn icon(ui: &egui::Ui) -> egui::CursorIcon {
        ui.ctx().output(|o| o.cursor_icon)
    }

    fn expect(check: bool, ui: &egui::Ui, want: egui::CursorIcon, msg: &str) {
        if check {
            assert_eq!(icon(ui), want, "{}", msg);
        }
    }

    #[test]
    fn cursor_decide_priority() {
        use egui::CursorIcon as I;
        frame(None, |ui, check| {
            let ctx = ui.ctx();
            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_decide(ctx);
            expect(check, ui, I::PointingHand, "仅 Hover");

            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_intent(ctx, CursorLayer::Edge, I::ResizeEast);
            cursor_decide(ctx);
            expect(check, ui, I::ResizeEast, "Edge 压过 Hover");

            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_intent(ctx, CursorLayer::Edge, I::ResizeEast);
            cursor_intent(ctx, CursorLayer::Drag, I::Grabbing);
            cursor_decide(ctx);
            expect(check, ui, I::Grabbing, "Drag 压过 Edge / Hover");

            for (layer, ic) in [
                (CursorLayer::Hover, I::PointingHand),
                (CursorLayer::Edge, I::ResizeEast),
                (CursorLayer::Drag, I::Grabbing),
                (CursorLayer::Dialog, I::Default),
            ] {
                cursor_intent(ctx, layer, ic);
            }
            cursor_decide(ctx);
            expect(check, ui, I::Default, "四层正序：Dialog 最高");

            for (layer, ic) in [
                (CursorLayer::Dialog, I::Default),
                (CursorLayer::Drag, I::Grabbing),
                (CursorLayer::Edge, I::ResizeEast),
                (CursorLayer::Hover, I::PointingHand),
            ] {
                cursor_intent(ctx, layer, ic);
            }
            cursor_decide(ctx);
            expect(check, ui, I::Default, "四层逆序：低层不得反超");

            cursor_intent(ctx, CursorLayer::Hover, I::PointingHand);
            cursor_intent(ctx, CursorLayer::Hover, I::Grabbing);
            cursor_decide(ctx);
            expect(check, ui, I::PointingHand, "同层保留先写者");
        });
    }

    #[test]
    fn cursor_decide_empty_defaults() {
        frame(None, |ui, check| {
            let ctx = ui.ctx();
            cursor_intent(ctx, CursorLayer::Drag, egui::CursorIcon::Grabbing);
            cursor_decide(ctx);
            expect(check, ui, egui::CursorIcon::Grabbing, "有 Drag 意图");

            cursor_decide(ctx);
            expect(
                check,
                ui,
                egui::CursorIcon::Default,
                "无意图帧不得残留 Grabbing",
            );
            cursor_decide(ctx);
            expect(check, ui, egui::CursorIcon::Default, "连续空帧仍为 Default");
        });
    }

    #[test]
    fn edge_resize_modal_skips() {
        let top_edge = egui::Pos2::new(400.0, 2.0);
        frame(Some(top_edge), |ui, check| {
            edge_resize(ui, true);
            cursor_decide(ui.ctx());
            expect(
                check,
                ui,
                egui::CursorIcon::Default,
                "弹层期间贴边也不得写 Edge",
            );
        });
        frame(Some(top_edge), |ui, check| {
            edge_resize(ui, false);
            cursor_decide(ui.ctx());
            expect(
                check,
                ui,
                egui::CursorIcon::ResizeNorth,
                "非弹层贴边应上报 Edge",
            );
        });
    }

    #[test]
    fn std_button_hover_points() {
        let rect = egui::Rect::from_min_size(egui::Pos2::new(10.0, 10.0), egui::vec2(80.0, 28.0));
        let hover = egui::Pos2::new(50.0, 20.0);
        frame(Some(hover), |ui, check| {
            let p = theme::palette(ui.ctx());
            let _ = dialogs::std_button_en(
                ui,
                rect,
                "OK",
                egui::Color32::WHITE,
                &p,
                egui::FontId::default(),
                true,
            );
            cursor_decide(ui.ctx());
            expect(
                check,
                ui,
                egui::CursorIcon::PointingHand,
                "enabled + hover 应手型",
            );
        });
        frame(Some(hover), |ui, check| {
            let p = theme::palette(ui.ctx());
            let _ = dialogs::std_button_en(
                ui,
                rect,
                "OK",
                egui::Color32::WHITE,
                &p,
                egui::FontId::default(),
                false,
            );
            cursor_decide(ui.ctx());
            expect(check, ui, egui::CursorIcon::Default, "disabled 不得报手型");
        });
    }

    #[test]
    fn panes_copy_is_default_no_drop_hint_change() {
        frame(None, |ui, check| {
            cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::Default);
            cursor_decide(ui.ctx());
            expect(check, ui, egui::CursorIcon::Default, "合法放置为默认箭头");
        });
        assert_ne!(egui::CursorIcon::Copy, egui::CursorIcon::Default);
        let src = include_str!("panes.rs");
        assert!(
            src.contains("cursor_intent(ui.ctx(), CursorLayer::Hover, CursorIcon::Default)"),
            "file_rows 合法放置分支须上报 Default 而非 Copy"
        );
        assert!(
            src.contains("p.accent.gamma_multiply(0.08)"),
            "DropHint 高亮填充不得随光标改动删除"
        );
    }
}

#[cfg(test)]
mod cr1_jobs_tests {
    use super::*;
    use std::path::PathBuf;

    fn req_delete() -> crate::local::LocalPaneReq {
        crate::local::LocalPaneReq::Delete {
            dir: PathBuf::from("C:/cr1"),
            targets: vec!["a.txt".to_owned()],
            keep: Vec::new(),
        }
    }

    #[test]
    fn delete_reply_after_tab_close_keeps_log_and_drops_pane_update() {
        let mut st = State::default();
        let req = req_delete();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(crate::local::LocalPaneReply {
            req: req.clone(),
            entries: None,
            deleted: vec![("C:/cr1/a.txt".to_owned(), Ok(()))],
        })
        .unwrap();
        st.local_jobs.push(LocalJob {
            side: 0,
            tab_id: 999_999,
            req,
            rx,
        });
        assert!(poll_local_jobs(&mut st), "回执被消费");
        assert!(st.local_jobs.is_empty(), "在途槽清空");
        assert_eq!(st.terminal.len(), 1, "tab 已关仍留删除日志");
        assert!(st.terminal[0].1.contains("C:/cr1/a.txt"));
        assert!(!st.terminal[0].1.contains("已删除 1 项"));
    }

    #[test]
    fn worker_disconnect_clears_loading_and_reports_failure() {
        let mut st = State::default();
        let tab_id = st.locals[0].tabs[0].id;

        st.locals[0].tabs[0].pane.path = Some(PathBuf::from("C:/cr1"));
        st.locals[0].tabs[0].pane.loading = true;
        let (tx, rx) = std::sync::mpsc::channel::<crate::local::LocalPaneReply>();
        drop(tx);
        let req = crate::local::LocalPaneReq::Refresh {
            path: PathBuf::from("C:/cr1"),
            keep: Vec::new(),
        };
        st.local_jobs.push(LocalJob {
            side: 0,
            tab_id,
            req,
            rx,
        });
        assert!(poll_local_jobs(&mut st));
        assert!(!st.locals[0].tabs[0].pane.loading, "断开也须清 loading");
        assert!(
            st.locals[0].tabs[0].pane.msg.is_some(),
            "失败须显式提示，不静默"
        );
    }

    #[test]
    fn local_io_in_flight_keeps_heartbeat_busy() {
        let mut st = State::default();
        assert_eq!(repaint_delay(&st), None, "全清应自停");
        st.locals[0].tabs[0].pane.pending_local = Some(crate::local::LocalPaneReq::Refresh {
            path: PathBuf::from("C:/cr1"),
            keep: Vec::new(),
        });
        assert_eq!(
            repaint_delay(&st),
            Some(std::time::Duration::from_millis(120)),
            "登记待起线程即进 120ms 快帧"
        );
    }
}

#[cfg(test)]
mod cr7_terminal_tests {
    use super::*;
    use crate::ssh::Ev;

    #[test]
    fn single_deleted_reply_zeroes_pending_and_relists() {
        let mut st = State::default();
        let tab_id = st.remotes[1].tabs[0].id;
        st.remotes[1].tabs[0].pane.remote_path = Some("/r".to_owned());
        st.remotes[1].tabs[0].pane.loading = true;
        st.remotes[1].tabs[0].del_pending = 1;
        let tag = next_seq(&mut st);
        st.inflight.insert(tag, (1, tab_id));
        apply_ssh_event(
            &mut st,
            &egui::Context::default(),
            1,
            1,
            Ev::Deleted { tag },
        );
        assert_eq!(st.remotes[1].tabs[0].del_pending, 0, "批计数归零");
        assert!(st.inflight.is_empty(), "在途 tag 清空");
        assert!(!st.remotes[1].tabs[0].pane.loading, "busy 链解除");
        assert_eq!(st.list_reqs.len(), 1, "回零触发重列");
    }

    #[test]
    fn trim_terminal_keeps_last_400_in_one_pass() {
        let mut terminal: Vec<(String, String)> = (0..1000)
            .map(|i| (i.to_string(), format!("line-{i}")))
            .collect();
        trim_terminal(&mut terminal);
        assert_eq!(terminal.len(), 400);
        assert_eq!(terminal[0].1, "line-600");
        assert_eq!(terminal[399].1, "line-999");
        trim_terminal(&mut terminal);
        assert_eq!(terminal.len(), 400, "一次裁剪已收敛");
    }
}
