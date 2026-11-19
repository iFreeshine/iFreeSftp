use std::path::{Component, PathBuf};

use egui::{Align2, Color32, CursorIcon, FontId, LayerId, Order, Pos2, Rect, Sense, Stroke, Vec2};

use super::dialogs::{Dialog, Hooks, MenuState};
use super::{CursorLayer, cursor_intent};
use crate::i18n::{t, tfmt};
use crate::theme;

const ICON_CELL: f32 = 22.0;

const COL_GAP: f32 = 12.0;

const TREE_W: f32 = 200.0;

const TREE_ROW: f32 = 20.0;
const TREE_INDENT: f32 = 14.0;

const ROW_ICON: f32 = 18.0;

const ROW_FONT: f32 = 12.0;

const ROW_H: f32 = 24.0;

const HEADER_FONT: f32 = 11.0;

const CRUMB_H: f32 = 30.0;

const TOOL_SIZE: f32 = 24.0;

pub(crate) const REFRESH_ANIM: f64 = 0.45;

const REFRESH_SLIDE: f32 = 16.0;

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

    pub size_bytes: u64,

    pub mtime_secs: i64,
}

impl Entry {

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

pub struct RubberState {

    origin: Pos2,

    additive: bool,

    base: Vec<usize>,

    button: egui::PointerButton,
}

pub(crate) struct DragPayload {
    pub side: usize,
    pub rows: Vec<usize>,
}

pub(crate) fn can_transfer_to_oppo(
    src_nav: bool,
    src_conn: bool,
    dst_nav: bool,
    dst_conn: bool,
) -> bool {
    src_nav && src_conn && dst_nav && dst_conn
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ColVisibility {
    pub size: bool,
    pub ty: bool,
    pub modified: bool,
    pub perms: bool,
}

impl Default for ColVisibility {
    fn default() -> Self {
        Self {
            size: true,
            ty: true,
            modified: true,
            perms: true,
        }
    }
}

static COL_VIS: std::sync::Mutex<ColVisibility> = std::sync::Mutex::new(ColVisibility {
    size: true,
    ty: true,
    modified: true,
    perms: true,
});

pub fn col_vis() -> ColVisibility {
    *COL_VIS.lock().unwrap()
}

pub fn set_col_vis(v: ColVisibility) {
    *COL_VIS.lock().unwrap() = v;
}

pub(crate) fn toggle_col(key: &str) {
    let mut g = COL_VIS.lock().unwrap();
    match key {
        "大小" => g.size = !g.size,
        "类型" => g.ty = !g.ty,
        "修改时间" => g.modified = !g.modified,
        "权限" => g.perms = !g.perms,
        _ => {}
    }
}

pub struct FilePane {
    pub name_w: f32,
    pub size_w: f32,
    pub type_w: f32,
    pub modified_w: f32,
    pub perms_w: f32,

    pub name_def_w: f32,
    pub size_def_w: f32,
    pub type_def_w: f32,
    pub modified_def_w: f32,
    pub perms_def_w: f32,

    pub dragging_col: Option<Col>,
    pub tree_open: bool,

    pub tree_w: f32,
    pub show_hidden: bool,
    pub sort_col: SortKey,
    pub sort_asc: bool,

    pub selected: Vec<usize>,

    pub sel_anchor: Option<usize>,

    pub rubber: Option<RubberState>,
    pub empty: bool,

    pub path: Option<PathBuf>,
    pub entries: Vec<Entry>,

    pub remote_path: Option<String>,

    pub empty_title: String,
    pub empty_sub: String,

    pub connecting: bool,

    pub loading: bool,

    pub pending_local: Option<crate::local::LocalPaneReq>,

    pub loading_label: Option<&'static str>,

    pub msg: Option<String>,

    pub tree: Vec<TreeNode>,

    pub tree_synced_for: Option<String>,

    pub tree_reexpand: Vec<String>,

    pub compare_marks: Option<Vec<u8>>,

    pub refresh_at: Option<f64>,

    pub crumb_ov: Option<(usize, usize)>,
}

pub struct TreeNode {

    pub name: String,

    pub path: String,
    pub expanded: bool,

    pub loading: bool,

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

            name_def_w: 160.0,
            size_def_w: 90.0,
            type_def_w: 100.0,
            modified_def_w: 120.0,
            perms_def_w: 90.0,
            dragging_col: None,
            tree_open: false,

            tree_w: TREE_W,
            show_hidden: false,
            sort_col: SortKey::Name,
            sort_asc: true,
            selected: Vec::new(),
            sel_anchor: None,
            rubber: None,
            empty: false,
            path: None,
            entries: Vec::new(),
            remote_path: None,
            empty_title: t("先选择一个主机").to_owned(),
            empty_sub: t("选择要浏览的本地或远端文件系统").to_owned(),
            connecting: false,
            tree: Vec::new(),
            tree_synced_for: None,
            tree_reexpand: Vec::new(),
            compare_marks: None,
            refresh_at: None,
            crumb_ov: None,
            loading: false,
            pending_local: None,
            loading_label: None,
            msg: None,
        }
    }
}

fn visible_cols(p: &FilePane) -> Vec<Col> {
    let vis = col_vis();
    let mut v = vec![Col::Name];
    if vis.size {
        v.push(Col::Size);
    }
    if vis.ty {
        v.push(Col::Type);
    }
    if vis.modified {
        v.push(Col::Modified);
    }
    if vis.perms && p.remote_path.is_some() {
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

fn set_col_w(p: &mut FilePane, c: Col, w: f32) {
    match c {
        Col::Name => p.name_w = w,
        Col::Size => p.size_w = w,
        Col::Type => p.type_w = w,
        Col::Modified => p.modified_w = w,
        Col::Permissions => p.perms_w = w,
    }
}

fn col_bounds(p: &FilePane, c: Col) -> (f32, f32, f32) {
    let w = col_width(p, c);
    match c {
        Col::Name => (w, 60.0, 1200.0),
        _ => (w, 40.0, 400.0),
    }
}

fn set_col_def(p: &mut FilePane, c: Col, w: f32) {
    match c {
        Col::Name => p.name_def_w = w,
        Col::Size => p.size_def_w = w,
        Col::Type => p.type_def_w = w,
        Col::Modified => p.modified_def_w = w,
        Col::Permissions => p.perms_def_w = w,
    }
}

fn update_column_fit(p: &mut FilePane, avail: f32) {
    if avail <= 0.0 {
        return;
    }

    let vis = col_vis();
    let mut cols: Vec<(Col, f32, f32, f32)> = vec![(Col::Name, p.name_w, 60.0, p.name_def_w)];
    if vis.size {
        cols.push((Col::Size, p.size_w, 40.0, p.size_def_w));
    }
    if vis.ty {
        cols.push((Col::Type, p.type_w, 40.0, p.type_def_w));
    }
    if vis.modified {
        cols.push((Col::Modified, p.modified_w, 40.0, p.modified_def_w));
    }
    if vis.perms && p.remote_path.is_some() {
        cols.push((Col::Permissions, p.perms_w, 40.0, p.perms_def_w));
    }

    const FIXED_OVERHEAD: f32 = 42.0;
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
                break;
            }
            let mut squeezable = 0.0;
            for &(c, w, min, _) in &cols {
                let is_drag = Some(c) == dragging;
                if (round == 0) == is_drag {
                    continue;
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

fn col_rects(p: &FilePane, x0: f32) -> Vec<(Col, Rect)> {
    let mut x = x0;
    let icon = Rect::from_min_size(Pos2::new(x, 0.0), egui::vec2(ICON_CELL, 0.0));
    x += ICON_CELL + COL_GAP;
    let mut out = Vec::with_capacity(6);
    let _ = icon;
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

pub const TAB_BAR_H: f32 = 38.0;

const TAB_ITEM_H: f32 = 36.0;

const TAB_ICON_FONT: f32 = 11.0;

const TAB_TITLE_FONT: f32 = 12.0;

const TAB_CLOSE_FONT: f32 = 11.0;

const TAB_CLOSE_SIZE: f32 = 18.0;

const TAB_CLOSE_COL: f32 = 28.0;

const TAB_NEW_SIZE: f32 = 28.0;

const TAB_APPEAR_SECS: f32 = 0.25;

const TAB_APPEAR_OFFSET: f32 = 10.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    Local,
    Remote,
}

#[derive(Clone)]
pub struct TabItem {

    pub id: u32,
    pub title: String,
    pub kind: TabKind,
}

pub struct Tabs {
    pub items: Vec<TabItem>,
    pub active: usize,

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

    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 37.5),
            Pos2::new(rect.right(), rect.top() + 37.5),
        ],
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.ide_tab_border),
    );

    let y0 = rect.top() + 1.0;
    let mut x = rect.left();

    let ell_w = galley_w(ui, "…", TAB_TITLE_FONT);
    let tab_fixed = 8.0 + TAB_ICON_FONT + 8.0 + TAB_CLOSE_COL;
    let min_w = tab_fixed + ell_w;
    let avail = rect.width() - (TAB_NEW_SIZE + 4.0 + 1.0);
    let text_ws: Vec<f32> = tabs
        .items
        .iter()
        .map(|it| galley_w(ui, &it.title, TAB_TITLE_FONT))
        .collect();
    let natural: Vec<f32> = text_ws.iter().map(|&w| tab_fixed + w).collect();
    let total = natural.iter().sum::<f32>() + (tabs.items.len().saturating_sub(1)) as f32;
    let mut widths = natural.clone();
    if total > avail {
        let excess = total - avail;
        let budget: f32 = (0..tabs.items.len())
            .filter(|&i| i != tabs.active)
            .map(|i| natural[i] - min_w)
            .sum();
        if budget >= excess {

            for i in 0..tabs.items.len() {
                if i != tabs.active {
                    widths[i] -= excess * (natural[i] - min_w) / budget;
                }
            }
        } else {

            for (i, w) in widths.iter_mut().enumerate() {
                if i != tabs.active {
                    *w = min_w;
                }
            }
            if let Some(w) = widths.get_mut(tabs.active) {
                *w = (*w - (excess - budget)).max(min_w);
            }
        }
    }

    let tab_clip = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top()),
        Pos2::new(rect.right() - 4.0 - TAB_NEW_SIZE - 1.0, rect.bottom()),
    );
    let title_font = theme::font_small();
    let mut close_req: Option<usize> = None;
    let mode_icon = if remote { "desktop-tower" } else { "monitor" };

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
        let item_w = widths[i];
        let mut item = Rect::from_min_size(Pos2::new(x, y0), egui::vec2(item_w, TAB_ITEM_H));

        let mut painter = ui.painter().clone();
        let mut alpha = 1.0;
        if let Some((idx, e)) = appear {
            if idx == i {
                item = item.translate(egui::vec2(0.0, (1.0 - e) * TAB_APPEAR_OFFSET));
                alpha = e;
            }
        }
        painter.set_opacity(alpha);
        painter.set_clip_rect(tab_clip);

        let resp = ui.allocate_rect(item, egui::Sense::click());

        let resp = resp.on_hover_text(title.clone());

        let bg = if active {
            p.tab_selected_bg
        } else if resp.is_pointer_button_down_on() || resp.hovered() {
            p.tab_hover
        } else {
            p.tab_rest
        };
        painter.rect_filled(
            item,
            egui::CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            },
            bg,
        );
        if resp.clicked() {
            tabs.active = i;
        }

        if active {
            let ind = Rect::from_min_max(
                Pos2::new(item.left(), item.bottom() - 1.0),
                Pos2::new(item.right(), item.bottom() + 1.0),
            );
            painter.rect_filled(ind, 0.0, theme::ACCENT);
        }

        let cy = item.center().y - 1.0;

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

        let title_fg = if active {
            p.tab_active_fg
        } else if resp.is_pointer_button_down_on() {
            p.text_tertiary
        } else {
            p.text_secondary
        };

        let close_label = crate::i18n::tfmt("关闭 {0}", &[&title]);
        let title_x = Pos2::new(item.left() + 8.0 + TAB_ICON_FONT + 8.0, cy);
        if text_ws[i] > widths[i] - tab_fixed {
            let mut job = egui::text::LayoutJob::single_section(
                title,
                egui::TextFormat::simple(title_font.clone(), title_fg),
            );
            job.wrap.max_width = widths[i] - tab_fixed;
            job.wrap.max_rows = 1;
            job.wrap.overflow_character = Some('…');
            let galley = painter.layout_job(job);
            painter.galley(title_x, galley, title_fg);
        } else {
            painter.text(
                title_x,
                Align2::LEFT_CENTER,
                title,
                title_font.clone(),
                title_fg,
            );
        }

        if active || resp.hovered() || resp.is_pointer_button_down_on() {
            let close_r = Rect::from_center_size(
                Pos2::new(item.right() - 4.0 - TAB_CLOSE_SIZE / 2.0, item.center().y),
                egui::vec2(TAB_CLOSE_SIZE, TAB_CLOSE_SIZE),
            );
            let close_resp = ui.allocate_rect(close_r, egui::Sense::click());
            close_resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, close_label.as_str())
            });
            if close_resp.hovered() {

                painter.rect_filled(close_r, 3.0, p.control_hover);
            }
            if close_resp.clicked() {

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

        x += item_w + 1.0;
    }

    let new_r = Rect::from_min_size(
        Pos2::new(rect.right() - 4.0 - TAB_NEW_SIZE, rect.top() + 5.0),
        egui::vec2(TAB_NEW_SIZE, TAB_NEW_SIZE),
    );
    let resp = ui.allocate_rect(new_r, egui::Sense::click());

    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, t("新建标签")));
    if resp.hovered() {

        ui.painter().rect_filled(new_r, 3.0, p.tab_new_hover);
    }
    if resp.clicked() {

        *hooks.dialog = Dialog::HostPicker {
            target: side,
            new_tab: true,
        };
    }

    if let Some(img) = st_icons.get(ui.ctx(), "plus", p.text_primary, 15.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(new_r.center(), egui::vec2(15.0, 15.0)),
        );
    }
    close_req
}

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

    let inner_full = rect.shrink2(egui::vec2(1.0, 1.0));

    if pane.empty && !pane.connecting {
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

    if pane.empty && pane.connecting {
        let top = inner.top() + inner.height() * 0.40;
        let cx = inner.center().x;
        let spin = Rect::from_center_size(Pos2::new(cx, top), egui::vec2(18.0, 18.0));
        egui::Spinner::new()
            .size(18.0)
            .color(p.accent)
            .paint_at(ui, spin);
        ui.painter().text(
            Pos2::new(cx, top + 30.0),
            Align2::CENTER_CENTER,
            &pane.empty_title,
            theme::font_body(),
            p.text_secondary,
        );
        return;
    }

    let y0 = inner.top();
    let crumb = Rect::from_min_max(inner.left_top(), Pos2::new(inner.right(), y0 + CRUMB_H));
    breadcrumb(ui, crumb, pane, st_icons, &p, hooks, side);

    let toolbar_h = TOOL_SIZE + 2.0;
    let toolbar = Rect::from_min_max(
        Pos2::new(inner.left(), crumb.bottom()),
        Pos2::new(inner.right(), crumb.bottom() + toolbar_h),
    );
    toolbar_row(ui, toolbar, pane, st_icons, &p, hooks, side);

    let header_h = HEADER_FONT + 16.0 + 1.0;

    let status_h = ROW_FONT + 12.0 + 1.0;
    let status = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - status_h),
        inner.right_bottom(),
    );

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

    tree_sync_step(pane, hooks, side);

    if pane.tree_open {
        let line_x = list_left + 0.5;
        ui.painter().line_segment(
            [
                Pos2::new(line_x, toolbar.bottom()),
                Pos2::new(line_x, status.top()),
            ],
            theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
        );
        let thumb = Rect::from_min_size(
            Pos2::new(list_left - 6.0, toolbar.bottom()),
            egui::vec2(13.0, status.top() - toolbar.bottom()),
        );
        let resp = ui.allocate_rect(thumb, egui::Sense::drag());
        if resp.hovered() || resp.dragged() {
            cursor_intent(ui.ctx(), CursorLayer::Hover, CursorIcon::ResizeHorizontal);
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

    header_row(ui, header, pane, st_icons, &p, hooks, side);
    file_rows(ui, list, pane, st_icons, sys_icons, &p, hooks, side);

    let content_x = list_left + 4.0;
    let rects = col_rects(pane, content_x);
    for (idx, (c, r)) in rects.iter().enumerate() {
        if idx + 1 == rects.len() {
            break;
        }
        let x = r.right() + 6.0;
        if x < inner.right() - 8.0 {
            ui.painter().line_segment(
                [Pos2::new(x, header.top()), Pos2::new(x, list.bottom())],
                theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
            );
            let thumb = Rect::from_min_size(
                Pos2::new(x - 8.0, header.top()),
                egui::vec2(16.0, header.height()),
            );
            let thumb_resp = ui.allocate_rect(thumb, Sense::click_and_drag());
            if thumb_resp.hovered() || thumb_resp.dragged() {
                cursor_intent(ui.ctx(), CursorLayer::Hover, CursorIcon::ResizeHorizontal);
            }
            if thumb_resp.dragged() {

                pane.dragging_col = Some(*c);
                let (_, min, max) = col_bounds(pane, *c);
                let nw = col_width(pane, *c) + thumb_resp.drag_delta().x;
                set_col_w(pane, *c, nw.clamp(min, max));
            }
            if thumb_resp.drag_stopped() && pane.dragging_col == Some(*c) {

                let w = col_width(pane, *c);
                set_col_def(pane, *c, w);
                pane.dragging_col = None;
            }
        }
    }

    update_column_fit(pane, list.width());

    status_bar(ui, status, pane, &p, hooks, side);
}

fn crumb_fold(
    widths: &[f32],
    ov_w: f32,
    sep_w: f32,
    house_w: f32,
    avail: f32,
) -> Option<(usize, usize)> {
    let n = widths.len();
    let nc = n + 1;

    let full = house_w
        + widths.iter().sum::<f32>()
        + (nc - 1) as f32 * sep_w
        + 2.0 * ((2 * nc - 2) as f32);
    if n <= 1 || avail <= 0.0 || full <= avail {
        return None;
    }

    let mid_count = nc - 2;
    let (mut fold_a, mut fold_b) = (0usize, mid_count - 1);
    for cnt in 1..=mid_count {
        let i = 1 + (mid_count - cnt) / 2;
        let j = i + cnt - 1;
        let (a, b) = (i - 1, j - 1);
        if collapsed_w(widths, a, b, ov_w, sep_w, house_w) <= avail {
            fold_a = a;
            fold_b = b;
            break;
        }
    }
    Some((fold_a, fold_b))
}

fn collapsed_w(widths: &[f32], a: usize, b: usize, ov_w: f32, sep_w: f32, house_w: f32) -> f32 {
    let n = widths.len();

    let vis_buttons = a + 2 + (n - 1 - b);
    let seps = vis_buttons - 1;
    let head: f32 = widths[..a].iter().sum();
    let tail: f32 = widths[b + 1..].iter().sum();
    head + house_w + ov_w + tail + seps as f32 * sep_w + 2.0 * ((vis_buttons + seps - 1) as f32)
}

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
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.row_separator),
    );

    let blank = ui.allocate_rect(rect, Sense::click());

    if matches!(&*hooks.crumb_edit, Some((s, _)) if *s == side) {
        let pad = 6.0;
        let edit_r = Rect::from_min_max(
            Pos2::new(rect.left() + pad, rect.top() + 3.0),
            Pos2::new(rect.right() - pad, rect.bottom() - 3.0),
        );
        if let Some((_, text)) = hooks.crumb_edit.as_mut() {
            let mut text = std::mem::take(text);

            let te_id = egui::Id::NULL.with(("crumb_edit_te", side));
            let open_id = te_id.with("open");

            let first_frame = !ui
                .ctx()
                .data(|d| d.get_temp::<bool>(open_id))
                .unwrap_or(false);
            let te = egui::TextEdit::singleline(&mut text)
                .id_salt(te_id)
                .font(theme::font_small())
                .desired_width(edit_r.width())
                .min_size(egui::vec2(edit_r.width(), edit_r.height()));
            let resp = ui.put(edit_r, te);
            if first_frame {
                ui.ctx().data_mut(|d| d.insert_temp(open_id, true));
                resp.request_focus();
            }

            hooks.ctx_menu.register(resp.id, edit_r);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));

            let suppress = hooks.ctx_menu.suppress_submit();

            if !suppress && esc && resp.has_focus() {

                *hooks.crumb_edit = None;
                ui.ctx().data_mut(|d| d.insert_temp(open_id, false));
            } else if !suppress && enter && resp.has_focus() {

                let t = text.trim().to_owned();
                if !t.is_empty() {
                    if pane.remote_path.is_some() {
                        hooks.list_req.push((side, t));
                    } else if crate::local::navigate(pane, std::path::Path::new(&t)) {
                        *hooks.sync_nav = Some((side, t));
                    }
                }
                *hooks.crumb_edit = None;

                ui.ctx().data_mut(|d| d.insert_temp(open_id, false));
            } else if !suppress && resp.lost_focus() {

                *hooks.crumb_edit = None;
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

    enum Nav {
        Local(PathBuf),
        Remote(String),
    }
    let mut segs: Vec<(String, Nav)> = Vec::new();
    if let Some(rp) = &pane.remote_path {

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

    let vpad = (CRUMB_H - 20.0) * 0.5;
    let house_r = Rect::from_min_size(Pos2::new(x, rect.top() + vpad), egui::vec2(24.0, 20.0));
    let house_resp = ui.allocate_rect(house_r, Sense::click());
    if house_resp.hovered() {
        ui.painter().rect_filled(house_r, 4.0, p.row_hover);
    }

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

    let brand_font = theme::font_brand_small();
    let reg_font = theme::font_small();
    let mut widths = Vec::with_capacity(segs.len());
    for (i, (text, _)) in segs.iter().enumerate() {
        let font = if i + 1 == segs.len() {
            &brand_font
        } else {
            &reg_font
        };
        let shown = ellipsize(ui, text, font, 160.0);
        let w = ui
            .painter()
            .layout_no_wrap(shown, font.clone(), Color32::WHITE)
            .size()
            .x
            + 12.0;
        widths.push(w);
    }

    let ov_w = ui
        .painter()
        .layout_no_wrap("..".to_owned(), brand_font.clone(), Color32::WHITE)
        .size()
        .x
        + 12.0;

    let fold = crumb_fold(&widths, ov_w, 10.0, 24.0, rect.width() - 12.0);

    if pane.crumb_ov.is_some() && pane.crumb_ov != fold {
        pane.crumb_ov = None;
    }

    enum CrumbItem {
        Seg(usize),
        Ov,
    }
    let items: Vec<CrumbItem> = match fold {
        None => (0..segs.len()).map(CrumbItem::Seg).collect(),
        Some((a, b)) => {
            let mut v: Vec<CrumbItem> = (0..a).map(CrumbItem::Seg).collect();
            v.push(CrumbItem::Ov);
            v.extend((b + 1..segs.len()).map(CrumbItem::Seg));
            v
        }
    };

    let ov_open_id = egui::Id::NULL.with(("crumb_ov_open", side));
    let mut ov_btn_r: Option<Rect> = None;
    let mut ov_clicked = false;

    for (ii, item) in items.iter().enumerate() {
        let last = ii == items.len() - 1;

        let is_last_seg = match item {
            CrumbItem::Seg(i) => *i + 1 == segs.len(),
            CrumbItem::Ov => false,
        };
        let (text, font, color) = match item {
            CrumbItem::Seg(i) => {
                let (seg_text, _) = &segs[*i];
                (
                    seg_text.as_str(),
                    if is_last_seg { &brand_font } else { &reg_font },
                    if is_last_seg {
                        p.text_primary
                    } else {
                        p.text_secondary
                    },
                )
            }
            CrumbItem::Ov => ("..", &brand_font, p.text_primary),
        };

        let shown = ellipsize(ui, text, font, 160.0);
        let galley = ui
            .painter()
            .layout_no_wrap(shown.clone(), font.clone(), color);

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

            let cx = x + 5.0;
            let crumb_stroke = theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.text_tertiary);
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
        if matches!(item, CrumbItem::Ov) {
            ov_btn_r = Some(seg_r);

            if pane.crumb_ov.is_some() {
                ui.painter().rect_filled(seg_r, 4.0, p.row_hover);
            }
            if resp.clicked() {
                ov_clicked = true;
            }
        } else if resp.clicked() {
            if let CrumbItem::Seg(i) = item {
                let (_, nav) = &segs[*i];
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
    }

    if ov_clicked {
        let opening = if pane.crumb_ov.is_some() { None } else { fold };
        pane.crumb_ov = opening;
        ui.ctx()
            .data_mut(|d| d.insert_temp(ov_open_id, opening.is_some()));
    }

    if pane.crumb_ov == fold && fold.is_some() {
        let fresh = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(ov_open_id))
            .unwrap_or(false);
        if fresh {

            ui.ctx().data_mut(|d| d.insert_temp(ov_open_id, false));
        } else if let (Some(btn_r), Some((a, b))) = (ov_btn_r, fold) {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                pane.crumb_ov = None;
            } else {
                let ctx = ui.ctx().clone();
                let screen = ctx.input(|i| i.raw.screen_rect).unwrap_or(rect);

                egui::Area::new(egui::Id::NULL.with(("crumb_ov_blocker", side)))
                    .order(egui::Order::Foreground)
                    .fixed_pos(screen.min)
                    .show(&ctx, |ui| {
                        let blk = ui.allocate_rect(screen, Sense::click());
                        if blk.clicked() {
                            pane.crumb_ov = None;
                        }
                    });

                let item_h = 26.0;
                let mut pop_w = 120.0f32;
                for (seg_text, _) in &segs[a..=b] {
                    let g = ui.painter().layout_no_wrap(
                        seg_text.clone(),
                        reg_font.clone(),
                        Color32::WHITE,
                    );
                    pop_w = pop_w.max(g.size().x + 24.0);
                }
                let pop_h = (b - a + 1) as f32 * item_h + 2.0;
                let pop_rect = Rect::from_min_size(
                    Pos2::new(btn_r.left(), btn_r.bottom() + 4.0),
                    egui::vec2(pop_w, pop_h),
                );
                egui::Area::new(egui::Id::NULL.with(("crumb_ov_popup", side)))
                    .order(egui::Order::Foreground)
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
                        for (k, (seg_text, seg_nav)) in segs[a..=b].iter().enumerate() {
                            let ir = Rect::from_min_size(
                                Pos2::new(
                                    pop_rect.left() + 1.0,
                                    pop_rect.top() + 1.0 + k as f32 * item_h,
                                ),
                                egui::vec2(pop_w - 2.0, item_h),
                            );
                            let row = ui.allocate_rect(ir, Sense::click());
                            if row.hovered() {
                                pt.rect_filled(ir, 0.0, p.row_hover);
                            }
                            let shown = ellipsize(ui, seg_text, &reg_font, pop_w - 24.0);
                            pt.text(
                                Pos2::new(ir.left() + 12.0, ir.center().y),
                                Align2::LEFT_CENTER,
                                shown,
                                theme::font_small(),
                                p.text_primary,
                            );
                            if row.clicked() {
                                match seg_nav {
                                    Nav::Local(pth) => {
                                        if crate::local::navigate(pane, pth) {
                                            *hooks.sync_nav =
                                                Some((side, pth.to_string_lossy().into_owned()));
                                        }
                                    }
                                    Nav::Remote(target) => {
                                        hooks.list_req.push((side, target.clone()))
                                    }
                                }
                                pane.crumb_ov = None;
                            }
                        }
                    });
            }
        }
    }

    if house_resp.clicked() {
        if pane.remote_path.is_some() {
            hooks.list_req.push((side, "/".to_owned()));
        } else {
            crate::local::navigate_root(pane);

            *hooks.sync_nav = Some((side, String::new()));
        }
    }
    if blank.clicked() {

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
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.row_separator),
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

    let is_remote = pane.remote_path.is_some();
    let can_new = pane.path.is_some() || pane.remote_path.is_some();
    let mut x = rect.left() + 4.0;
    let cy = rect.center().y;
    for (icon, id, label) in tools {
        let enabled = match id {

            "open_dir" => cfg!(windows) && !is_remote && hooks.pick.is_none(),
            "new_folder" | "new_file" => can_new,
            _ => true,
        };
        let r = Rect::from_min_size(
            Pos2::new(x, cy - TOOL_SIZE / 2.0),
            egui::vec2(TOOL_SIZE, TOOL_SIZE),
        );

        let resp = ui.allocate_rect(
            r,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );

        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, t(label)));
        let on = match id {
            "tree" => pane.tree_open,
            "hidden" => pane.show_hidden,
            _ => false,
        };

        if enabled {
            if on && resp.hovered() {
                ui.painter().rect_filled(r, 4.0, p.control_hover);
            } else if on {
                ui.painter().rect_filled(r, 4.0, p.control_pressed);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, p.hover_bg);
            }
        }

        let fg = if enabled {
            p.text_primary
        } else {
            p.text_disabled
        };
        if let Some(img) = st_icons.get(ui.ctx(), icon, fg, 11.0) {

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

        let resp = if enabled {
            super::accent_tip(resp, t(label))
        } else {
            resp
        };
        if resp.clicked() {
            match id {
                "tree" => {
                    pane.tree_open = !pane.tree_open;

                    if pane.tree_open {
                        pane.tree.clear();
                        pane.tree_synced_for = None;
                        pane.tree_reexpand.clear();
                    }
                }
                "hidden" => {
                    pane.show_hidden = !pane.show_hidden;

                    tree_mark_dirty(pane);
                    pane.refresh_at = Some(ui.ctx().input(|i| i.time));
                    if let Some(rp) = pane.remote_path.clone() {
                        hooks.list_req.push((side, rp));
                    } else {
                        crate::local::refresh(pane);
                    }
                }
                "up" => {

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

                    pane.refresh_at = Some(ui.ctx().input(|i| i.time));
                    if let Some(rp) = pane.remote_path.clone() {
                        hooks.list_req.push((side, rp));
                    } else {
                        crate::local::refresh(pane);
                    }
                }

                "open_dir" => {
                    if hooks.pick.is_none() {

                        *hooks.pick = Some(crate::local::spawn_pick_folder(
                            pane.path.clone(),
                            crate::local::owner_window(),
                            side,
                        ));
                    }
                }

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

fn header_row(
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
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.row_separator),
    );

    let font = theme::font_header();
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

        let hit = Rect::from_min_max(
            Pos2::new(r.left(), rect.top()),
            Pos2::new(r.right(), rect.bottom()),
        );
        let resp = ui.allocate_rect(hit, egui::Sense::click());
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

    if ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.secondary_clicked()) {
        let pos = ui.input(|i| i.pointer.interact_pos().unwrap_or(rect.left_top()));
        *hooks.menu = Some(MenuState {
            pos,
            pane: side,
            row: None,
            queue: None,
            terminal: false,
            cols: true,
        });
    }
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
    let row_font = theme::font_small();
    let content_x = rect.left() + 4.0;
    let row_h = ROW_H;

    let released = ui.input(|i| i.pointer.any_released());
    if let Some(pl) = egui::DragAndDrop::payload::<DragPayload>(ui.ctx()) {
        if pl.side == side {

            if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                let lp = ui
                    .ctx()
                    .layer_painter(LayerId::new(Order::Tooltip, egui::Id::unique("dnd_ghost")));
                let font = theme::font_small();
                let names: Vec<String> = pl
                    .rows
                    .iter()
                    .filter_map(|&r| pane.entries.get(r).map(|e| e.name.clone()))
                    .collect();
                let mut lines: Vec<String> = names.iter().take(3).cloned().collect();
                if names.len() > 3 {
                    lines.push(tfmt("等 {0} 项", &[&names.len().to_string()]));
                }
                let galleys: Vec<_> = lines
                    .iter()
                    .map(|s| {

                        let shown = if s.chars().count() > 28 {
                            format!("{}…", s.chars().take(28).collect::<String>())
                        } else {
                            s.clone()
                        };
                        lp.layout_no_wrap(shown, font.clone(), p.text_primary)
                    })
                    .collect();
                let row_gap = 2.0;
                let pad = egui::vec2(10.0, 5.0);
                let card_w =
                    galleys.iter().map(|g| g.size().x).fold(56.0_f32, f32::max) + pad.x * 2.0;
                let card_h = galleys.iter().map(|g| g.size().y).sum::<f32>()
                    + row_gap * (galleys.len().saturating_sub(1)) as f32
                    + pad.y * 2.0;
                let card =
                    Rect::from_min_size(pos + egui::vec2(10.0, 10.0), Vec2::new(card_w, card_h));
                lp.rect_filled(
                    card.translate(egui::vec2(2.0, 3.0)),
                    6.0,
                    Color32::from_black_alpha(38),
                );
                lp.rect_filled(card, 6.0, theme::palette(ui.ctx()).card_bg);
                lp.rect_stroke(
                    card,
                    6.0,
                    theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.accent),
                    egui::StrokeKind::Inside,
                );
                let mut y = card.top() + pad.y;
                for g in &galleys {
                    lp.galley(Pos2::new(card.left() + pad.x, y), g.clone(), p.text_primary);
                    y += g.size().y + row_gap;
                }
            }
        } else if !hooks.dnd_ready {

            if ui.rect_contains_pointer(rect) {
                cursor_intent(ui.ctx(), CursorLayer::Hover, CursorIcon::NotAllowed);
            }
        } else if ui.rect_contains_pointer(rect) {
            if released {

                egui::DragAndDrop::clear_payload(ui.ctx());
                *hooks.transfer = Some((pl.side, pl.rows.clone()));
            } else {

                cursor_intent(ui.ctx(), CursorLayer::Hover, CursorIcon::Default);
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

    if pane.remote_path.is_some() {
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        let internal_dnd = egui::DragAndDrop::payload::<DragPayload>(ui.ctx()).is_some();
        if hovering && !internal_dnd && ui.rect_contains_pointer(rect) {
            ui.painter()
                .rect_filled(rect, 2.0, p.accent.gamma_multiply(0.08));
            ui.painter().rect_stroke(
                rect,
                2.0,
                Stroke::new(2.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        let dropped = ui.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() && ui.rect_contains_pointer(rect) {
            let shift = ui.input(|i| i.modifiers.shift);

            let paths: Vec<PathBuf> = dropped
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
            if !paths.is_empty() {
                *hooks.drop_upload = Some((side, paths, shift));
            }
        }
    }

    if pane
        .compare_marks
        .as_ref()
        .is_some_and(|m| m.len() != pane.entries.len())
    {
        *hooks.compare_req = true;
    }

    let n = pane.entries.len();

    let ctrl_a = ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::A));
    if ctrl_a && n > 0 && ui.rect_contains_pointer(rect) && ui.memory(|m| m.focused().is_none()) {
        pane.selected = (0..n)
            .filter(|&i| !pane.entries[i].name.is_empty())
            .collect();
        pane.sel_anchor = Some(0);
    }

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

            let mut pending_rubber = false;
            let mut pending_button = egui::PointerButton::Primary;
            for (i, e) in pane.entries.iter().enumerate() {
                let mut row = Rect::from_min_max(
                    Pos2::new(rect.left(), top + i as f32 * row_h),
                    Pos2::new(rect.right(), top + (i + 1) as f32 * row_h),
                );

                if row.bottom() < clip.top() {
                    continue;
                }
                if row.top() > clip.bottom() {
                    break;
                }

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

                let ph = e.name.is_empty();

                let resp = ui.allocate_rect(row, Sense::click_and_drag());

                let selected = !ph && pane.selected.contains(&i);
                if selected {
                    ui.painter().rect_filled(row, 0.0, theme::selection());
                } else if !ph {

                    if resp.is_pointer_button_down_on() {
                        ui.painter().rect_filled(row, 0.0, p.row_pressed);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(row, 0.0, p.row_hover);
                    }
                }

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

                    let mods = ui.input(|i| i.modifiers);
                    if mods.shift
                        && let Some(a) = pane.sel_anchor
                    {
                        let last = pane.entries.len().saturating_sub(1);
                        let hi = a.max(i).min(last);
                        let lo = a.min(i).min(last);
                        pane.selected = if lo <= hi {
                            (lo..=hi).collect()
                        } else {
                            vec![i]
                        };
                    } else if mods.ctrl {
                        if let Some(p) = pane.selected.iter().position(|&s| s == i) {
                            pane.selected.remove(p);
                        } else {
                            pane.selected.push(i);
                        }
                        pane.sel_anchor = Some(i);
                    } else {
                        pane.selected = vec![i];
                        pane.sel_anchor = Some(i);
                    }
                }

                if !ph && e.is_dir && resp.double_clicked() {
                    open_idx = Some(i);
                }

                if !ph && resp.drag_started_by(egui::PointerButton::Primary) {
                    let mods = ui.input(|i| i.modifiers);
                    if mods.shift {
                        pending_rubber = true;
                    } else {
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
                }

                if resp.drag_started_by(egui::PointerButton::Secondary) {
                    pending_rubber = true;
                    pending_button = egui::PointerButton::Secondary;
                }

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
                        cols: false,
                    });
                }

                if ph {
                    continue;
                }

                let cy = row.center().y;

                let cell = Rect::from_center_size(
                    Pos2::new(content_x + ICON_CELL / 2.0, cy),
                    egui::vec2(ICON_CELL, ICON_CELL),
                );

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

                    let text = match c {
                        Col::Name | Col::Type => ellipsize(ui, &text, &row_font, cell_r.width()),
                        _ => text,
                    };
                    ui.painter()
                        .text(Pos2::new(tx, cy), anchor, text, row_font.clone(), color);
                }
            }

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
                        cols: false,
                    });
                }

                if resp.drag_started_by(egui::PointerButton::Primary) {
                    rubber_begin(ui, pane, egui::PointerButton::Primary);
                }

                if resp.drag_started_by(egui::PointerButton::Secondary) {
                    rubber_begin(ui, pane, egui::PointerButton::Secondary);
                }
            }

            if pending_rubber {
                rubber_begin(ui, pane, pending_button);
            }

            rubber_apply(ui, pane, top, rect, row_h, clip);
        });

    if let Some(t0) = pane.refresh_at {
        let now = ui.ctx().input(|i| i.time);
        let el = now - t0;
        if el >= REFRESH_ANIM {
            pane.refresh_at = None;
        } else {
            let k = (1.0 - el / REFRESH_ANIM) as f32;
            let a = (k * k * 200.0) as u8;
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

fn rubber_begin(ui: &egui::Ui, pane: &mut FilePane, button: egui::PointerButton) {
    let additive = ui.input(|i| i.modifiers.ctrl);
    pane.rubber = Some(RubberState {
        origin: ui.input(|i| {
            i.pointer
                .latest_pos()
                .unwrap_or_else(|| i.pointer.press_origin().unwrap_or_default())
        }),
        additive,
        base: if additive {
            pane.selected.clone()
        } else {
            Vec::new()
        },
        button,
    });
}

fn rubber_apply(ui: &egui::Ui, pane: &mut FilePane, top: f32, list: Rect, row_h: f32, clip: Rect) {

    let Some(btn) = pane.rubber.as_ref().map(|r| r.button) else {
        return;
    };
    if !ui.input(move |i| i.pointer.button_down(btn)) {
        pane.rubber = None;
        return;
    }
    let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else {
        return;
    };

    const EDGE: f32 = 28.0;
    const SPEED: f32 = 14.0;
    if pos.y > clip.bottom() - EDGE {
        let target = Pos2::new(pos.x, clip.bottom() + SPEED);
        ui.scroll_to_rect(Rect::from_min_size(target, Vec2::ZERO), None);
    } else if pos.y < clip.top() + EDGE {
        let target = Pos2::new(pos.x, clip.top() - SPEED);
        ui.scroll_to_rect(Rect::from_min_size(target, Vec2::ZERO), None);
    }
    let Some(rub) = pane.rubber.as_ref() else {
        return;
    };
    let band = Rect::from_two_pos(rub.origin, pos);
    let (additive, base) = (rub.additive, rub.base.clone());

    let mut hit = Vec::new();
    for (i, e) in pane.entries.iter().enumerate() {
        if e.name.is_empty() {
            continue;
        }
        let row = Rect::from_min_max(
            Pos2::new(list.left(), top + i as f32 * row_h),
            Pos2::new(list.right(), top + (i + 1) as f32 * row_h),
        );
        if row.bottom() < clip.top() || row.top() > clip.bottom() {
            continue;
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

    let p = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(band, 2.0, p.accent.gamma_multiply(0.35));
    ui.painter().rect_stroke(
        band,
        2.0,
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.accent),
        egui::StrokeKind::Inside,
    );
}

#[cfg(test)]
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

fn status_bar(
    ui: &mut egui::Ui,
    rect: Rect,
    pane: &FilePane,
    p: &theme::Palette,
    hooks: &mut Hooks,
    side: usize,
) {
    ui.painter().rect_filled(rect, 0.0, p.folder_area);
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 0.5),
            Pos2::new(rect.right(), rect.top() + 0.5),
        ],
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.row_separator),
    );
    let dirs = pane.entries.iter().filter(|e| e.is_dir).count();
    let files = pane.entries.len() - dirs;

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

        format!(
            "  ·  {}",
            pane.loading_label.unwrap_or_else(|| t("加载中…"))
        )
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
        theme::font_small(),
        p.text_secondary,
    );

    let mut chip_w = 0.0;
    if let Some(n) = hooks.drop_stash[side] {
        chip_w = draw_stash_chip(ui, rect, n, p, hooks);
    }
    if let Some(m) = &pane.msg {
        ui.painter().text(
            Pos2::new(rect.right() - 8.0 - chip_w, rect.center().y),
            Align2::RIGHT_CENTER,
            m,
            theme::font_small(),
            p.text_secondary,
        );
    }
}

fn draw_stash_chip(
    ui: &mut egui::Ui,
    rect: Rect,
    n: usize,
    p: &theme::Palette,
    hooks: &mut Hooks,
) -> f32 {
    let label = tfmt("已暂存 {0} 项", &[&n.to_string()]);
    let lw = galley_w(ui, &label, ROW_FONT);
    let q_label = t("入队");
    let qw = galley_w(ui, q_label, ROW_FONT);
    let pad = 8.0;
    let btn_w = qw + 14.0;
    let close_w = 14.0;
    let inner_h = rect.height() - 6.0;
    let y = rect.top() + 3.0;
    let cy = rect.center().y;
    let x_close_r = rect.right() - pad;
    let x_close_l = x_close_r - close_w;
    let x_q_r = x_close_l - 4.0;
    let x_q_l = x_q_r - btn_w;
    let x_l_r = x_q_l - 6.0;
    let x_l_l = x_l_r - lw;
    let pill = Rect::from_min_max(
        Pos2::new(x_l_l - pad, y),
        Pos2::new(x_close_r + pad * 0.5, y + inner_h),
    );
    let radius = (inner_h / 2.0).min(9.0);
    ui.painter().rect_filled(pill, radius, p.card_bg);
    ui.painter().rect_stroke(
        pill,
        radius,
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.accent),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        Pos2::new(x_l_l, cy),
        Align2::LEFT_CENTER,
        &label,
        theme::font_small(),
        p.accent,
    );

    let qrect = Rect::from_min_max(
        Pos2::new(x_q_l, y + 1.0),
        Pos2::new(x_q_r, y + inner_h - 1.0),
    );
    let resp = ui.allocate_rect(qrect, egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, q_label));
    let bg = if resp.hovered() || resp.is_pointer_button_down_on() {
        p.accent
    } else {
        p.accent.gamma_multiply(0.8)
    };
    ui.painter().rect_filled(qrect, radius - 1.0, bg);
    ui.painter().text(
        qrect.center(),
        Align2::CENTER_CENTER,
        q_label,
        theme::font_small(),
        theme::TOOLTIP_FG,
    );
    if resp.clicked() {
        *hooks.drop_resume = true;
    }

    let crect = Rect::from_center_size(
        Pos2::new((x_close_l + x_close_r) / 2.0, cy),
        Vec2::new(close_w + 6.0, rect.height() - 4.0),
    );
    let resp2 = ui.allocate_rect(crect, egui::Sense::click());
    resp2.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, t("放弃暂存")));
    ui.painter().text(
        crect.center(),
        Align2::CENTER_CENTER,
        "×",
        theme::font_small(),
        if resp2.hovered() {
            p.text_primary
        } else {
            p.text_secondary
        },
    );
    if resp2.clicked() {
        *hooks.drop_clear = true;
    }
    rect.right() - pill.left()
}

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

    let top = rect.top() + rect.height() * 0.40;
    let cx = rect.center().x;

    let block = Rect::from_center_size(Pos2::new(cx, top + 28.0), egui::vec2(56.0, 56.0));
    ui.painter().rect_filled(block, 11.0, p.empty_card_bg);
    if let Some(img) = st_icons.get(ui.ctx(), "compass", theme::ACCENT, 23.0) {
        img.paint_at(
            ui,
            Rect::from_center_size(block.center(), egui::vec2(23.0, 23.0)),
        );
    }

    let title_font = theme::font_empty_title();
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
        theme::font_small(),
        p.text_secondary,
    );

    let y = top + 140.0;
    let label = t("选择主机");
    let btn_font = theme::font_body();
    let text_w = galley_w(ui, label, 14.0);
    let btn_w = 24.0 + 14.0 + 8.0 + text_w + 24.0;
    let btn_h = 8.0 * 2.0 + 20.0;
    let btn = Rect::from_min_size(Pos2::new(cx - btn_w / 2.0, y), egui::vec2(btn_w, btn_h));
    let resp = ui.allocate_rect(btn, egui::Sense::click());

    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
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

use crate::transfer::{Status as TStatus, VerifyKind};

pub(crate) fn queue_status_text(i: &crate::transfer::Item, paused: bool) -> String {
    match i.status {
        TStatus::Waiting => {
            if paused {
                t("已暂停").to_owned()
            } else {
                t("等待中").to_owned()
            }
        }
        TStatus::Transferring => {

            if i.resumed && i.size > 0 {
                let pct = (i.transferred.saturating_mul(100) / i.size).clamp(0, 100);
                let mut s = format!("{} {}%", t("续传中"), pct);
                if i.speed > 0 {
                    s.push_str(&format!(" {}/s", crate::local::format_size(i.speed)));
                }
                s
            } else if i.speed > 0 {
                if i.resumed {
                    format!("{} {}/s", t("续传中"), crate::local::format_size(i.speed))
                } else {
                    format!("{}/s", crate::local::format_size(i.speed))
                }
            } else if i.phase == 1 {

                t("服务端打包中").to_owned()
            } else if i.phase == 2 {
                t("解包中").to_owned()
            } else if i.resumed {
                t("续传中").to_owned()
            } else {
                t("传输中").to_owned()
            }
        }

        TStatus::Scanning => t("扫描中").to_owned(),
        TStatus::Verifying => t("校验中").to_owned(),

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

pub fn queue(ui: &mut egui::Ui, rect: Rect, st: &mut super::State) {
    let p = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(rect, theme::CONTAINER_RADIUS, p.folder_area);
    let inner = rect.shrink2(egui::vec2(1.0, 1.0));

    let font = theme::font(theme::TextRole::Caption);
    let header_font = theme::font_small();

    let (items, paused) = match &st.transfer {
        Some(q) => (q.with_items(|all| all.to_vec()), q.paused()),
        None => (Vec::new(), false),
    };
    let in_active = |s: TStatus| {
        matches!(
            s,
            TStatus::Waiting
                | TStatus::Transferring
                | TStatus::Verifying
                | TStatus::Scanning
                | TStatus::Cancelled
        )
    };
    let n_active = items.iter().filter(|i| in_active(i.status)).count();
    let n_failed = items.iter().filter(|i| i.status == TStatus::Failed).count();
    let n_done = items.iter().filter(|i| i.status == TStatus::Done).count();

    let header_h = 5.0 + 16.0 + 5.0 + 1.0;
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
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
    );
    let hy = header.center().y;

    let star_total = (inner.width() - 24.0 - 180.0 - 48.0).max(120.0);
    let (half, reason_w) = if st.queue_tab == 1 {
        ((star_total * 0.3).max(40.0), (star_total * 0.4).max(40.0))
    } else {
        ((star_total / 2.0).max(40.0), 0.0)
    };
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
            header_font.clone(),
            p.text_secondary,
        );

        if k < 3 || (st.queue_tab == 1 && k == 3) {
            let dx = xs[k] + if k < 2 { half } else { 90.0 } + 6.0;
            ui.painter().line_segment(
                [Pos2::new(dx, header.top()), Pos2::new(dx, header.bottom())],
                theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.divider_stroke),
            );
        }
    }

    let row_h = 26.0;
    let list_bottom = inner.bottom() - 29.0;
    let rows_rect = Rect::from_min_max(
        Pos2::new(inner.left(), header.bottom()),
        Pos2::new(inner.right(), list_bottom),
    );
    let shown: Vec<&crate::transfer::Item> = items
        .iter()
        .filter(|i| match st.queue_tab {
            0 => in_active(i.status),
            1 => i.status == TStatus::Failed,
            _ => i.status == TStatus::Done,
        })
        .collect();
    let mut hit_row: Option<(Pos2, u64)> = None;
    let mut blank_hit: Option<Pos2> = None;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rows_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );

    child.style_mut().spacing.scroll.bar_outer_margin = theme::SCROLL_BAR_INNER_QUEUE;
    egui::ScrollArea::vertical()
        .id_salt(("queue-rows", st.queue_tab))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let (content_r, _) = ui.allocate_exact_size(
                egui::vec2(
                    rows_rect.width(),
                    (shown.len() as f32 * row_h).max(rows_rect.height()),
                ),
                Sense::hover(),
            );
            let top = content_r.top();
            let clip = ui.clip_rect();
            for (ri, i) in shown.iter().enumerate() {
                let row = Rect::from_min_max(
                    Pos2::new(rows_rect.left(), top + ri as f32 * row_h),
                    Pos2::new(rows_rect.right(), top + (ri + 1) as f32 * row_h),
                );

                if row.bottom() < clip.top() {
                    continue;
                }
                if row.top() > clip.bottom() {
                    break;
                }
                let resp = ui.allocate_rect(row, Sense::click());

                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!(
                            "{} → {} · {}",
                            i.source,
                            i.dest,
                            queue_status_text(i, paused)
                        ),
                    )
                });
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
                    ellipsize(ui, &i.reason, &font, reason_w),
                ]
                .into_iter()
                .enumerate()
                {
                    if k == 4 && st.queue_tab != 1 {
                        continue;
                    }

                    let color = p.text_primary;
                    ui.painter().text(
                        Pos2::new(xs[k], cy),
                        Align2::LEFT_CENTER,
                        text,
                        font.clone(),
                        color,
                    );
                }
            }

            let blank = Rect::from_min_max(
                Pos2::new(rows_rect.left(), top + shown.len() as f32 * row_h),
                Pos2::new(rows_rect.right(), content_r.bottom()),
            );
            if !items.is_empty() && blank.height() > 0.0 {
                let resp = ui.allocate_rect(blank, Sense::click());
                if resp.secondary_clicked() {
                    blank_hit = Some(resp.interact_pointer_pos().unwrap_or(blank.center()));
                }
            }
        });

    if !items.is_empty() {
        let resp = ui.allocate_rect(header, Sense::click());
        if resp.secondary_clicked() || blank_hit.is_some() {
            let pos =
                blank_hit.or_else(|| Some(resp.interact_pointer_pos().unwrap_or(header.center())));
            if let Some(pos) = pos {
                st.menu = Some(super::dialogs::MenuState {
                    pos,
                    pane: 0,
                    row: None,
                    queue: Some(super::dialogs::QueueMenu::Batch),
                    terminal: false,
                    cols: false,
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
            cols: false,
        });
    }

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
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
    );

    let mut foot_hit: Option<Pos2> = None;
    let foot_resp = ui.allocate_rect(foot, Sense::click());
    if foot_resp.secondary_clicked() {
        foot_hit = Some(foot_resp.interact_pointer_pos().unwrap_or(foot.center()));
    }
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

        let w = text_w + 4.0 + count_w + 20.0;
        let r = Rect::from_min_size(Pos2::new(tx, foot.top() + 3.0), egui::vec2(w, 24.0));
        let resp = ui.allocate_rect(r, Sense::click());
        if resp.hovered() {

            ui.painter().rect_filled(r, 4.0, p.row_hover);
        }
        if resp.secondary_clicked() {

            foot_hit = Some(resp.interact_pointer_pos().unwrap_or(r.center()));
        }

        let fg = if sel { p.accent } else { p.text_secondary };
        let cg = if sel { p.accent } else { p.text_tertiary };
        let tab_font = theme::font_small();
        let sel_font = theme::font_brand_small();
        ui.painter().text(
            Pos2::new(r.left() + 10.0, r.center().y),
            Align2::LEFT_CENTER,
            *label,
            if sel { sel_font } else { tab_font.clone() },
            fg,
        );
        ui.painter().text(
            Pos2::new(r.left() + 10.0 + text_w + 4.0, r.center().y),
            Align2::LEFT_CENTER,
            count_s,
            tab_font,
            cg,
        );
        if resp.clicked() {
            st.queue_tab = k;
        }
        tx += w + 4.0;
    }

    if !items.is_empty()
        && let Some(pos) = foot_hit
    {
        st.menu = Some(super::dialogs::MenuState {
            pos,
            pane: 0,
            row: None,
            queue: Some(super::dialogs::QueueMenu::Batch),
            terminal: false,
            cols: false,
        });
    }
}

pub fn terminal(ui: &mut egui::Ui, rect: Rect, st: &mut super::State) {
    let p = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(rect, theme::CONTAINER_RADIUS, p.terminal_bg);
    let inner = rect.shrink2(egui::vec2(12.0, 8.0));
    let mono = theme::font_mono_caption();
    let lh = 15.0;
    let slot_pad = 7.0;

    let mut menu_hit: Option<Pos2> = None;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("terminal-log")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(&mut child, |ui| {

            let mut laid: Vec<(
                f32,
                std::sync::Arc<egui::Galley>,
                f32,
                std::sync::Arc<egui::Galley>,
            )> = Vec::with_capacity(st.terminal.len());
            let mut off = 0.0;
            for (time, text) in &st.terminal {
                let time_g =
                    ui.painter()
                        .layout_no_wrap(format!("[{time}]"), mono.clone(), p.terminal_time);
                let text_x = inner.left() + time_g.size().x + 8.0;
                let wrap_w = (inner.width() - time_g.size().x - 8.0).max(40.0);
                let body_g =
                    ui.painter()
                        .layout(text.clone(), mono.clone(), p.terminal_text, wrap_w);
                let y0 = off;
                off += body_g.size().y.max(lh) + slot_pad;
                laid.push((y0, time_g, text_x, body_g));
            }
            let (content_r, resp) = ui.allocate_exact_size(
                egui::vec2(inner.width(), off.max(inner.height())),
                Sense::click(),
            );
            let top = content_r.top();
            let clip = ui.clip_rect();
            for (dy, time_g, text_x, body_g) in &laid {
                let y = top + dy;
                let slot = body_g.size().y.max(lh);

                if y + slot < clip.top() {
                    continue;
                }
                if y > clip.bottom() {
                    break;
                }
                ui.painter()
                    .galley(Pos2::new(inner.left(), y), time_g.clone(), p.terminal_time);
                ui.painter()
                    .galley(Pos2::new(*text_x, y), body_g.clone(), p.terminal_text);
            }

            if resp.secondary_clicked() {
                menu_hit = Some(resp.interact_pointer_pos().unwrap_or(inner.center()));
            }
        });

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

    if let Some(pos) = menu_hit {
        st.menu = Some(super::dialogs::MenuState {
            pos,
            pane: 0,
            row: None,
            queue: None,
            terminal: true,
            cols: false,
        });
    }
}

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

type TreeAction = (String, bool);

fn tree_apply(pane: &mut FilePane, hooks: &mut Hooks, side: usize, path: &str, toggle: bool) {

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
                n.expanded = false;
            }
        }
    }
}

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

pub(crate) fn apply_tree_listing(pane: &mut FilePane, path: &str, entries: &[Entry]) {
    let mut dirs: Vec<TreeNode> = entries
        .iter()
        .filter(|e| e.is_dir && (pane.show_hidden || !e.name.starts_with('.')))
        .map(|e| {

            let full = crate::transfer::join_remote(&path, &e.name);
            TreeNode::new(&e.name, &full)
        })
        .collect();
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(n) = find_node_mut(&mut pane.tree, path) {
        n.children = dirs;
        n.loading = false;
        n.loaded = true;
        n.expanded = true;
    }
}

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

#[derive(PartialEq)]
enum Walk {

    Ok,

    Dead,

    Wait,
}

fn pane_pos(pane: &FilePane) -> Option<String> {
    pane.remote_path
        .clone()
        .or_else(|| pane.path.as_ref().map(|p| p.to_string_lossy().into_owned()))
}

fn collect_expanded(nodes: &[TreeNode], out: &mut Vec<String>) {
    for n in nodes {
        if n.expanded {
            out.push(n.path.clone());
            collect_expanded(&n.children, out);
        }
    }
}

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

pub(crate) fn tree_sync_give_up(pane: &mut FilePane) {
    pane.tree_synced_for = pane_pos(pane);
    pane.tree_reexpand.clear();
}

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

fn tree_walk(
    pane: &mut FilePane,
    hooks: &mut Hooks,
    side: usize,
    target: &str,
    expand_target: bool,
) -> Walk {
    let remote = pane.remote_path.is_some();
    let sep = if remote { '/' } else { '\\' };

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
            return Walk::Ok;
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
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
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
        let font12 = theme::font_small();

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

        let icon_c = Pos2::new(x0 + 12.0 + 9.0, r.center().y);
        if let Some(img) = st_icons.get(ui.ctx(), "folder", p.text_secondary, 14.0) {
            img.paint_at(ui, Rect::from_center_size(icon_c, egui::vec2(14.0, 14.0)));
        }

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

#[cfg(test)]
mod dir_leaf_tests {
    use super::{FilePane, dir_leaf};

    fn pane(remote: Option<&str>, local: Option<&str>) -> FilePane {
        FilePane {
            remote_path: remote.map(str::to_owned),
            path: local.map(std::path::PathBuf::from),
            ..Default::default()
        }
    }

    #[test]
    fn remote_root_shows_slash() {
        assert_eq!(dir_leaf(&pane(Some("/"), None)), "/");
    }

    #[test]
    fn trailing_slash_keeps_leaf() {
        assert_eq!(dir_leaf(&pane(Some("/var/log/"), None)), "log");
    }

    #[test]
    fn no_remote_and_no_local_shows_slash() {
        assert_eq!(dir_leaf(&pane(None, None)), "/");
    }

    #[test]
    fn normal_path_shows_leaf() {
        assert_eq!(dir_leaf(&pane(Some("/var/log"), None)), "log");
    }
}

#[cfg(test)]
mod crumb_fold_tests {
    use super::{collapsed_w, crumb_fold};

    const OV: f32 = 24.0;
    const SEP: f32 = 10.0;
    const HOUSE: f32 = 24.0;

    #[test]
    fn fits_no_fold_boundary() {
        let w = [30.0, 30.0, 30.0, 30.0];
        assert_eq!(crumb_fold(&w, OV, SEP, HOUSE, 200.0), None);
    }

    #[test]
    fn minimal_fold_first_fit() {
        let w = [30.0, 30.0, 30.0, 30.0];

        assert_eq!(crumb_fold(&w, OV, SEP, HOUSE, 175.0), Some((0, 1)));

        assert_eq!(crumb_fold(&w, OV, SEP, HOUSE, 149.0), Some((0, 2)));

        assert_eq!(collapsed_w(&w, 0, 1, OV, SEP, HOUSE), 150.0);
    }

    #[test]
    fn fold_window_centered_left_biased() {
        let w = [10.0, 100.0, 100.0, 100.0, 10.0];

        assert_eq!(crumb_fold(&w, OV, SEP, HOUSE, 210.0), Some((0, 2)));
    }

    #[test]
    fn fallback_folds_all_middle() {
        let w = [10.0, 100.0, 100.0, 100.0, 10.0];

        assert_eq!(crumb_fold(&w, OV, SEP, HOUSE, 80.0), Some((0, 3)));
    }

    #[test]
    fn degenerate_no_fold() {
        assert_eq!(crumb_fold(&[30.0], OV, SEP, HOUSE, 10.0), None);
        let empty: [f32; 0] = [];
        assert_eq!(crumb_fold(&empty, OV, SEP, HOUSE, 200.0), None);
        assert_eq!(crumb_fold(&[30.0, 30.0], OV, SEP, HOUSE, 0.0), None);
    }
}

#[cfg(test)]
mod queue_status_tests {
    use super::queue_status_text;
    use crate::i18n::t;
    use crate::transfer::{Item, Status as TStatus};

    #[test]
    fn resumed_with_size_shows_percent() {
        let i = Item::fixture(TStatus::Transferring, 200, 90, 0, true);
        assert_eq!(queue_status_text(&i, false), format!("{} 45%", t("续传中")));
    }

    #[test]
    fn resumed_percent_appends_speed() {
        let i = Item::fixture(TStatus::Transferring, 200, 90, 1_048_576, true);
        assert_eq!(
            queue_status_text(&i, false),
            format!("{} 45% 1 MB/s", t("续传中"))
        );
    }

    #[test]
    fn resumed_zero_size_keeps_plain_form() {
        let i = Item::fixture(TStatus::Transferring, 0, 5, 0, true);
        assert_eq!(queue_status_text(&i, false), t("续传中"));
    }

    #[test]
    fn percent_clamped_to_bounds() {
        let over = Item::fixture(TStatus::Transferring, 100, 250, 0, true);
        assert_eq!(
            queue_status_text(&over, false),
            format!("{} 100%", t("续传中"))
        );
        let zero = Item::fixture(TStatus::Transferring, 100, 0, 0, true);
        assert_eq!(
            queue_status_text(&zero, false),
            format!("{} 0%", t("续传中"))
        );
    }

    #[test]
    fn non_resumed_rows_unchanged() {
        let i = Item::fixture(TStatus::Transferring, 200, 90, 1_048_576, false);
        assert_eq!(queue_status_text(&i, false), "1 MB/s");
        let j = Item::fixture(TStatus::Transferring, 200, 90, 0, false);
        assert_eq!(queue_status_text(&j, false), t("传输中"));
    }

    #[test]
    fn queue_status_scanning_text() {
        let i = Item::fixture(TStatus::Scanning, 0, 0, 0, false);
        assert_eq!(queue_status_text(&i, false), t("扫描中"));
        assert_eq!(queue_status_text(&i, true), t("扫描中"));
    }

    #[test]
    fn relay_doubled_size_resumed_percent() {

        let i = Item::fixture(TStatus::Transferring, 2000, 500, 0, true);
        assert_eq!(queue_status_text(&i, false), format!("{} 25%", t("续传中")));

        let j = Item::fixture(TStatus::Transferring, 2000, 1000, 1_048_576, true);
        assert_eq!(
            queue_status_text(&j, false),
            format!("{} 50% 1 MB/s", t("续传中"))
        );
    }
}

#[cfg(test)]
mod dnd_ready_tests {
    use super::can_transfer_to_oppo;

    #[test]
    fn all_ready() {
        assert!(can_transfer_to_oppo(true, true, true, true));
    }

    #[test]
    fn any_unnavigated_blocks() {
        assert!(!can_transfer_to_oppo(false, true, true, true));
        assert!(!can_transfer_to_oppo(true, true, false, true));
    }

    #[test]
    fn any_disconnected_blocks() {
        assert!(!can_transfer_to_oppo(true, false, true, true));
        assert!(!can_transfer_to_oppo(true, true, true, false));
    }

    #[test]
    fn combined_matrix() {
        assert!(!can_transfer_to_oppo(false, false, true, true));
        assert!(!can_transfer_to_oppo(true, true, false, false));
        assert!(!can_transfer_to_oppo(false, true, true, false));
    }
}

#[cfg(test)]
mod col_vis_tests {
    use super::{ColVisibility, col_vis, set_col_vis, toggle_col};

    #[test]
    fn toggle_by_key_and_set() {
        set_col_vis(ColVisibility::default());
        toggle_col("大小");
        let v = col_vis();
        assert!(!v.size && v.ty && v.modified && v.perms);
        toggle_col("权限");
        let v = col_vis();
        assert!(!v.size && v.ty && v.modified && !v.perms);
        toggle_col("不存在的键");
        assert_eq!(col_vis(), v);
        set_col_vis(ColVisibility::default());
        assert_eq!(col_vis(), ColVisibility::default());
    }
}
