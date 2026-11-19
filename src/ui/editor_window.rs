use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egui::text::{LayoutJob, TextFormat};
use egui::widgets::text_edit::TextEditState;
use egui::{
    Align2, Color32, FontId, FontSelection, Id, Pos2, Rect, Sense, Stroke, TextBuffer, TextEdit,
    Vec2, ViewportBuilder, ViewportClass, ViewportId,
};

use crate::editor::{EditorDoc, EditorHub};
use crate::i18n::{t, t_ctx, tfmt};
use crate::theme;

use super::ctx_menu::CtxMenu;

#[cfg(test)]
mod probe_tests {
    use super::*;

    #[test]
    fn textedit_margin_yields_galley_offset() {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)));
        ctx.begin_pass(raw);
        let mut root = egui::Ui::new(
            ctx.clone(),
            Id::unique("probe-margin"),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(Rect::from_min_size(
                    Pos2::new(100.0, 50.0),
                    Vec2::new(300.0, 250.0),
                )),
        );
        let mut text = String::from("alpha\nbeta\n");
        let out = egui::TextEdit::multiline(&mut text)
            .margin(egui::Margin {
                left: 30,
                right: 12,
                top: 5,
                bottom: 5,
            })
            .show(&mut root);

        assert_eq!(out.galley_pos, Pos2::new(130.0, 55.0));
        ctx.end_pass().textures_delta.clear();
    }

    #[test]
    fn editor_line_numbers_visible_and_padding_applied() {
        let dir = std::env::temp_dir().join("ifreesftp-ew-probe");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("probe.txt");
        std::fs::write(&f, "alpha\nbeta\ngamma\n").unwrap();
        let mut hub = EditorHub::new();
        hub.open_local(&f, true);
        let doc = hub.docs[0].clone();
        assert!(doc.lock().unwrap().line_numbers, "默认开行号");
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0)));
        ctx.begin_pass(raw);
        let mut root = egui::Ui::new(
            ctx.clone(),
            Id::unique("probe-root"),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 720.0))),
        );
        draw_window(&mut root, &doc);
        let mut full = ctx.end_pass();
        let mut texts = Vec::new();
        for cs in &full.shapes {
            if let egui::Shape::Text(t) = &cs.shape {
                let s = t.galley.text().to_string();
                if !s.is_empty() {
                    texts.push((s, t.pos, cs.clip_rect));
                }
            }
        }

        let body = texts
            .iter()
            .find(|(s, _, _)| s.contains("alpha"))
            .expect("正文 galley 应存在");
        assert!(
            body.1.x > 43.0 && body.1.x < 50.0,
            "正文 x 应 ≈ 栏沟+10px（实测 {}）",
            body.1.x
        );
        assert!(
            body.2.min.x >= body.1.x - 1.0,
            "正文 clip 不再吃掉 padding（clip.min.x={}）",
            body.2.min.x
        );

        let nums: Vec<(String, Pos2)> = texts
            .iter()
            .filter(|(s, _, _)| matches!(s.as_str(), "1" | "2" | "3"))
            .map(|(s, p, _)| (s.clone(), *p))
            .collect();
        assert_eq!(nums.len(), 3, "行号 1/2/3 应各画一枚");
        for (s, pos) in &nums {
            assert!(
                pos.x > 24.0 && pos.x <= 37.0,
                "行号 {s} 应落在栏沟内（实测 x={}）",
                pos.x
            );
        }

        let mut ys: Vec<f32> = nums.iter().map(|(_, p)| p.y).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(ys[1] - ys[0] > 14.0 && ys[1] - ys[0] < 17.0);
        assert!(ys[2] - ys[1] > 14.0 && ys[2] - ys[1] < 17.0);

        full.textures_delta.clear();
    }
}

const WIN_SIZE: [f32; 2] = [960.0, 720.0];

const MIN_SIZE: [f32; 2] = [400.0, 300.0];

const TITLE_H: f32 = 32.0;

const TOOL_H: f32 = 30.0;

const STATUS_H: f32 = 22.0;

const TOOL_BTN: f32 = 26.0;

const TEXT_SIZE: f32 = 13.0;

struct Pal {
    back: Color32,
    fore: Color32,
    dim: Color32,
    line_no: Color32,
    tool_bg: Color32,
    hover: Color32,
    checked: Color32,
    sep: Color32,
    card: Color32,
    danger: Color32,
    ok: Color32,
    input_bg: Color32,

    hl_comment: Color32,

    hl_number: Color32,

    hl_string: Color32,
}

fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            back: Color32::from_rgb(0x1B, 0x24, 0x30),
            fore: Color32::from_rgb(0xD6, 0xDD, 0xE6),
            dim: Color32::from_rgb(0x8A, 0x97, 0xA5),
            line_no: Color32::from_rgb(0x66, 0x74, 0x82),
            tool_bg: Color32::from_rgb(0x2E, 0x39, 0x48),
            hover: Color32::from_rgb(0x3A, 0x46, 0x56),
            checked: Color32::from_rgb(0x46, 0x53, 0x64),
            sep: Color32::from_rgb(0x24, 0x2E, 0x3B),
            card: Color32::from_rgb(0x2E, 0x39, 0x48),
            danger: Color32::from_rgb(0xFF, 0x99, 0xA4),
            ok: Color32::from_rgb(0x6C, 0xCB, 0x7F),
            input_bg: Color32::from_rgb(0x1B, 0x24, 0x30),
            hl_comment: Color32::from_rgb(0x6A, 0x99, 0x55),
            hl_number: Color32::from_rgb(0xB5, 0xCE, 0xA8),
            hl_string: Color32::from_rgb(0xCE, 0x91, 0x78),
        }
    } else {
        Pal {
            back: Color32::from_rgb(0xF9, 0xF9, 0xF9),
            fore: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            dim: Color32::from_rgb(0x6D, 0x6D, 0x6D),
            line_no: Color32::from_rgb(0x90, 0x90, 0x90),
            tool_bg: Color32::from_rgb(0xDD, 0xDD, 0xDD),
            hover: Color32::from_rgb(0xCC, 0xCC, 0xCC),
            checked: Color32::from_rgb(0xBF, 0xBF, 0xBF),
            sep: Color32::from_rgb(0xC8, 0xC8, 0xC8),
            card: Color32::WHITE,
            danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
            ok: Color32::from_rgb(0x0F, 0x7B, 0x0F),
            input_bg: Color32::WHITE,
            hl_comment: Color32::from_rgb(0x00, 0x80, 0x00),
            hl_number: Color32::from_rgb(0x09, 0x86, 0x58),
            hl_string: Color32::from_rgb(0xA3, 0x15, 0x15),
        }
    }
}

fn icons() -> &'static Mutex<Option<crate::icons::Cache>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<crate::icons::Cache>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Some(crate::icons::Cache::default())))
}

fn app_icon() -> Option<std::sync::Arc<egui::IconData>> {
    static ICON: std::sync::OnceLock<Option<std::sync::Arc<egui::IconData>>> =
        std::sync::OnceLock::new();
    ICON.get_or_init(|| crate::window_icon().map(std::sync::Arc::new))
        .clone()
}

fn ctx_menus() -> &'static Mutex<HashMap<u64, CtxMenu>> {
    static M: std::sync::OnceLock<Mutex<HashMap<u64, CtxMenu>>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn show_all(ctx: &egui::Context, hub: &mut EditorHub, dark: bool) {

    let mut live_ids = std::collections::HashSet::new();
    hub.docs.retain(|d| {
        let mut d = d.lock().unwrap();
        if d.closed {
            if let Some(r) = &d.remote {
                crate::editor::remove_remote_copy(&r.host, &r.remote_path);
            }
            return false;
        }
        d.dark = dark;
        live_ids.insert(d.id);
        true
    });

    ctx_menus()
        .lock()
        .unwrap()
        .retain(|k, _| live_ids.contains(k));

    for doc in &hub.docs {
        let d = doc.lock().unwrap();
        let title = d.window_title();
        let id = d.id;
        drop(d);

        let mut builder = ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(WIN_SIZE)
            .with_min_inner_size(MIN_SIZE)

            .with_resizable(true)
            .with_decorations(false);
        if let Some(icon) = app_icon() {
            builder = builder.with_icon(icon);
        }
        ctx.show_viewport_deferred(ViewportId(Id::unique(("editor", id))), builder, {
            let doc = doc.clone();
            move |ui: &mut egui::Ui, _class: ViewportClass| draw_window(ui, &doc)
        });
    }
}

fn draw_window(ui: &mut egui::Ui, doc: &Arc<Mutex<EditorDoc>>) {
    let mut d = doc.lock().unwrap();
    let p = pal(d.dark);
    let ctx = ui.ctx().clone();

    let mut menus = ctx_menus().lock().unwrap();
    let cm = menus.entry(d.id).or_default();
    cm.frame_start(&ctx);

    if ctx.input(|i| i.viewport().close_requested()) {
        if d.dirty && !d.close_ask && d.load_error.is_none() {
            d.close_ask = true;
        } else if !d.dirty || d.load_error.is_some() {
            d.closed = true;
        }
    }

    let (ctrl_s, ctrl_f, ctrl_h, esc) = ctx.input(|i| {
        let c = i.modifiers.ctrl;
        (
            c && i.key_pressed(egui::Key::S),
            c && i.key_pressed(egui::Key::F),
            c && i.key_pressed(egui::Key::H),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if esc && d.search.open {
        d.search.open = false;
    }
    if ctrl_f && d.load_error.is_none() {
        d.search.open = true;
        d.search.show_replace = false;
        ctx.memory_mut(|m| m.request_focus(Id::unique(("editor-find", d.id))));
    }
    if ctrl_h && d.load_error.is_none() {
        d.search.open = true;
        d.search.show_replace = true;
        ctx.memory_mut(|m| m.request_focus(Id::unique(("editor-find", d.id))));
    }
    if ctrl_s && d.load_error.is_none() {
        save_doc(&mut d, &ctx);
    }

    let mut want_undo = false;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(p.back))
        .show(ui, |ui| {
            let full = ui.max_rect();
            let title_r = Rect::from_min_size(full.min, Vec2::new(full.width(), TITLE_H));
            let tool_r = Rect::from_min_size(
                Pos2::new(full.left(), full.top() + TITLE_H),
                Vec2::new(full.width(), TOOL_H),
            );
            let body = Rect::from_min_max(
                Pos2::new(full.left(), tool_r.bottom()),
                Pos2::new(full.right(), full.bottom() - STATUS_H),
            );
            let status_r =
                Rect::from_min_max(Pos2::new(full.left(), full.bottom() - STATUS_H), full.max);

            draw_title(ui, &mut d, title_r, &p);
            draw_toolbar(ui, &mut d, tool_r, &p, &mut want_undo);

            if let Some(err) = d.load_error.clone() {
                draw_load_error(ui, &mut d, body, &p, &err);
                return;
            }
            draw_editor(ui, &mut d, body, &p, want_undo, cm);
            if d.search.open {
                draw_search(ui, &mut d, body, &p, cm);
            }
            draw_status(ui, &mut d, status_r, &p);
            if d.close_ask {
                draw_close_ask(ui, &mut d, full, &p);
            }
        });

    cm.frame_end(&ctx);

    super::edge_resize(ui, d.close_ask);

    super::cursor_decide(ui.ctx());

    if d.closed {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn draw_title(ui: &mut egui::Ui, d: &mut EditorDoc, r: Rect, p: &Pal) {
    ui.painter().rect_filled(r, 0.0, p.back);

    let mut ic = icons().lock().unwrap();
    if let Some(img) = ic
        .as_mut()
        .and_then(|c| c.get_raw(ui.ctx(), "app-logo", 14.0))
    {
        img.paint_at(
            ui,
            Rect::from_center_size(Pos2::new(r.left() + 14.0, r.center().y), Vec2::splat(14.0)),
        );
    }
    drop(ic);
    ui.painter().text(
        Pos2::new(r.left() + 28.0, r.center().y),
        Align2::LEFT_CENTER,
        d.window_title(),
        FontId::proportional(12.5),
        p.fore,
    );

    let btn_w = 36.0;
    let mut close_hit = false;
    for (i, (glyph, act)) in [("─", 0), ("□", 1), ("✕", 2)].iter().enumerate() {
        let br = Rect::from_min_size(
            Pos2::new(r.right() - (3 - i) as f32 * btn_w, r.top()),
            Vec2::new(btn_w, r.height()),
        );
        let resp = ui.allocate_rect(br, Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(br, 0.0, p.hover);
        }
        ui.painter().text(
            br.center(),
            Align2::CENTER_CENTER,
            *glyph,
            FontId::proportional(11.0),
            if matches!(act, 2) && resp.hovered() {
                p.danger
            } else {
                p.fore
            },
        );
        if resp.clicked() {
            match act {
                0 => ui
                    .ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                1 => {
                    let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                }
                _ => close_hit = true,
            }
        }
    }

    let drag_r = Rect::from_min_max(r.min, Pos2::new(r.right() - 3.0 * btn_w, r.bottom()));
    let drag = ui.allocate_rect(drag_r, Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    if close_hit {
        if d.dirty && d.load_error.is_none() {
            d.close_ask = true;
        } else {
            d.closed = true;
        }
    }
}

fn draw_toolbar(ui: &mut egui::Ui, d: &mut EditorDoc, r: Rect, p: &Pal, want_undo: &mut bool) {
    ui.painter().rect_filled(r, 0.0, p.tool_bg);
    ui.painter().line_segment(
        [
            Pos2::new(r.left(), r.bottom() - 0.5),
            Pos2::new(r.right(), r.bottom() - 0.5),
        ],
        Stroke::new(1.0, p.sep),
    );

    let mut ic_lock = icons().lock().unwrap();
    let mut x = r.left() + 4.0;
    let cy = r.center().y;
    let label_font = FontId::proportional(12.0);

    let actions: [(&str, &str, &str, u8); 5] = [
        ("floppy-disk", "保存", "Save", 0),
        ("arrow-counter-clockwise", "撤消", "Undo", 1),
        ("magnifying-glass", "查找", "Find", 2),
        ("list", "自动换行", "Wrap", 3),
        ("check", "行号", "Lines", 4),
    ];
    let tips: [&str; 5] = [
        t_ctx("保存 (Ctrl+S)", "Save (Ctrl+S)"),
        t_ctx("撤消", "Undo"),
        t_ctx("查找 (Ctrl+F)", "Find (Ctrl+F)"),
        t_ctx("自动换行", "Wrap lines"),
        t_ctx("行号", "Line numbers"),
    ];
    for (i, (icon, zh, en, act)) in actions.iter().enumerate() {
        if *act == 3 {
            x += 6.0;
        }

        let label = t_ctx(zh, en);

        let tw = ui
            .painter()
            .layout_no_wrap(label.to_owned(), label_font.clone(), Color32::WHITE)
            .size()
            .x;
        let bw = 6.0 + 15.0 + 5.0 + tw + 8.0;
        let br = Rect::from_min_size(Pos2::new(x, cy - TOOL_BTN / 2.0), Vec2::new(bw, TOOL_BTN));
        let checked = matches!(act, 3) && d.wrap || matches!(act, 4) && d.line_numbers;
        let resp = ui.allocate_rect(br, Sense::click());
        if checked {
            ui.painter().rect_filled(br, 4.0, p.checked);
        } else if resp.hovered() {
            ui.painter().rect_filled(br, 4.0, p.hover);
        }
        let color = if checked || resp.hovered() {
            p.fore
        } else {
            p.dim
        };
        let img = ic_lock
            .as_mut()
            .and_then(|c| c.get(ui.ctx(), icon, color, 15.0));
        if let Some(img) = img {
            img.paint_at(
                ui,
                Rect::from_center_size(Pos2::new(br.left() + 6.0 + 7.5, cy), Vec2::splat(15.0)),
            );
        }
        ui.painter().text(
            Pos2::new(br.left() + 6.0 + 15.0 + 5.0, cy),
            Align2::LEFT_CENTER,
            label,
            label_font.clone(),
            color,
        );
        let clicked = resp.clicked();
        super::accent_tip(resp, tips[i]);
        if clicked {
            match act {
                0 => save_doc(d, ui.ctx()),
                1 => *want_undo = true,
                2 => {
                    d.search.open = true;
                    d.search.show_replace = false;
                    ui.ctx()
                        .memory_mut(|m| m.request_focus(Id::unique(("editor-find", d.id))));
                }
                3 => d.wrap = !d.wrap,
                _ => d.line_numbers = !d.line_numbers,
            }
        }
        x += bw + 2.0;
    }
}

fn save_doc(d: &mut EditorDoc, ctx: &egui::Context) {
    d.save_err = None;
    if let Err(e) = d.save() {
        d.save_err = Some(e);
    } else if d.remote.is_some() {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn draw_editor(
    ui: &mut egui::Ui,
    d: &mut EditorDoc,
    r: Rect,
    p: &Pal,
    want_undo: bool,
    cm: &mut CtxMenu,
) {

    let text_id = Id::unique(("editor-text", d.id));
    let digits_hint = d.lines_hint.to_string().len().max(3);
    let ln_w = if d.line_numbers {
        12.0 + digits_hint as f32 * 8.2
    } else {
        0.0
    };

    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let area = if d.wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    let (wrap, line_numbers) = (d.wrap, d.line_numbers);
    let lang = lang_of(&d.title);

    let mut layouter =
        |ui: &egui::Ui, buf: &dyn TextBuffer, wrap_width: f32| -> std::sync::Arc<egui::Galley> {
            let job = highlight_job(p, buf.as_str(), wrap_width, wrap, lang);
            ui.fonts_mut(|f| f.layout_job(job))
        };
    let result = area
        .auto_shrink(false)
        .id_salt(text_id.with("scroll"))

        .scroll_bar_rect(Rect::from_min_max(
            r.min,
            Pos2::new(r.right(), r.bottom() - 6.0),
        ))
        .show(&mut body_ui, |sui| {

            let pad = 10.0;
            let left_pad = if line_numbers { ln_w + pad } else { pad };
            TextEdit::multiline(&mut d.text)
                .id(text_id)
                .font(FontSelection::FontId(egui::FontId::monospace(TEXT_SIZE)))
                .text_color(p.fore)
                .frame(egui::Frame::new().inner_margin(egui::Margin {
                    left: left_pad as i8,
                    right: pad as i8,
                    top: pad as i8,
                    bottom: pad as i8,
                }))
                .desired_width(if wrap { r.width() } else { f32::INFINITY })
                .layouter(&mut layouter)
                .show(sui)
        });
    let out = result.inner;

    if line_numbers {
        let painter = body_ui.painter();
        let offset_y = result.state.clamped_offset().y;
        let mut logical = 1usize;
        let rows = &out.galley.rows;
        for (k, placed) in rows.iter().enumerate() {
            if k > 0 && rows[k - 1].ends_with_newline {
                logical += 1;
            }
            let top = out.galley_pos.y + placed.pos.y - offset_y;
            let h = placed.rect().height();
            if top + h < r.top() || top > r.bottom() {
                continue;
            }
            if k == 0 || rows[k - 1].ends_with_newline {
                painter.text(
                    Pos2::new(r.left() + ln_w, top + h / 2.0),
                    Align2::RIGHT_CENTER,
                    logical.to_string(),
                    FontId::monospace(TEXT_SIZE),
                    p.line_no,
                );
            }
        }
    }

    cm.register_plain(out.response.id, out.response.rect);

    if let Some(rng) = out.cursor_range {
        let [a, b] = rng.sorted_cursors();
        d.sel_cache = Some((a.index.0, b.index.0));
        let cur = rng.primary.index.0;
        if d.cursor_cache.0 != cur {
            let (row, col) = line_col(&d.text, cur);
            d.cursor_cache = (cur, row, col);
        }
    }

    if want_undo {
        let mut ts = out.state.clone();
        let cur = ts.cursor.range(&out.galley);
        let mut undoer = ts.undoer();
        if let Some((new_range, new_text)) = undoer.undo(&(
            cur.unwrap_or(egui::text::CCursorRange::one(egui::text::CCursor::new(0))),
            d.text.clone(),
        )) {
            d.text = new_text.clone();
            d.dirty = true;
            d.search.matches.clear();
            ts.cursor.set_char_range(Some(*new_range));
            ts.set_undoer(undoer);
            ts.store(ui.ctx(), text_id);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Lang {

    Plain,
    Python,
    Js,
    Css,
    Shell,
}

fn lang_of(name: &str) -> Lang {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "py" | "pyw" => Lang::Python,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => Lang::Js,
        "css" => Lang::Css,
        "sh" | "bash" | "zsh" | "ksh" => Lang::Shell,
        _ => Lang::Plain,
    }
}

#[derive(Clone, Copy)]
enum Tok {
    Comment,
    Str,
    Num,
}

fn highlight_job(p: &Pal, text: &str, wrap_width: f32, wrap: bool, lang: Lang) -> LayoutJob {
    let font = FontId::monospace(TEXT_SIZE);
    let base = TextFormat::simple(font.clone(), p.fore);
    let b = text.as_bytes();
    let mut spans: Vec<(usize, usize, Tok)> = Vec::new();
    let (mut i, mut line_head, mut prev_word) = (0usize, true, false);
    while i < b.len() {
        let c = b[i];
        let n1 = b.get(i + 1).copied();

        let quote = if matches!(lang, Lang::Python | Lang::Js | Lang::Css | Lang::Shell)
            && (c == b'"' || c == b'\'' || (lang == Lang::Js && c == b'`'))
        {
            Some(c)
        } else {
            None
        };
        if let Some(q) = quote {
            let start = i;
            if lang == Lang::Python && n1 == Some(q) && b.get(i + 2) == Some(&q) {

                i += 3;
                while i + 3 <= b.len() && !(b[i] == q && b[i + 1] == q && b[i + 2] == q) {
                    i += 1;
                }
                i = if i + 3 <= b.len() { i + 3 } else { b.len() };
            } else {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == q || b[i] == b'\n' {
                        break;
                    } else {
                        i += 1;
                    }
                }
                if i < b.len() && b[i] == q {
                    i += 1;
                }
                i = i.min(b.len());
            }
            spans.push((start, i, Tok::Str));
            prev_word = false;
            line_head = false;
            continue;
        }

        if matches!(lang, Lang::Js | Lang::Css) && c == b'/' && n1 == Some(b'*') {
            let start = i;
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            spans.push((start, i, Tok::Comment));
            prev_word = false;
            line_head = false;
            continue;
        }

        if (lang == Lang::Js || lang == Lang::Plain)
            && c == b'/'
            && n1 == Some(b'/')
            && (i == 0 || b[i - 1] != b':')
        {
            let start = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            spans.push((start, i, Tok::Comment));
            line_head = true;
            prev_word = false;
            continue;
        }

        if c == b'#'
            && (lang == Lang::Python
                || (lang == Lang::Shell
                    && (line_head || (i > 0 && matches!(b[i - 1], b' ' | b'\t'))))
                || (lang == Lang::Plain && line_head))
        {
            let start = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            spans.push((start, i, Tok::Comment));
            line_head = true;
            prev_word = false;
            continue;
        }

        if c.is_ascii_digit() && !prev_word {
            let start = i;
            if c == b'0' && matches!(n1, Some(b'x') | Some(b'X')) {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                while i > start + 1 && b[i - 1] == b'.' {
                    i -= 1;
                }
            }
            spans.push((start, i, Tok::Num));
            prev_word = true;
            line_head = false;
            continue;
        }
        prev_word = c.is_ascii_alphanumeric() || c == b'_';
        match c {
            b'\n' => line_head = true,
            b' ' | b'\t' => {}
            _ => line_head = false,
        }
        i += 1;
    }
    let mut job = LayoutJob::default();
    job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };

    let mut bp = 0usize;
    for (s, e, tok) in spans {
        if s > bp {
            let seg = &text[bp..s];
            job.append(seg, 0.0, base.clone());
        }
        let color = match tok {
            Tok::Comment => p.hl_comment,
            Tok::Str => p.hl_string,
            Tok::Num => p.hl_number,
        };
        let seg = &text[s..e];
        job.append(seg, 0.0, TextFormat::simple(font.clone(), color));
        bp = e;
    }
    if bp < text.len() {
        job.append(&text[bp..], 0.0, base.clone());
    }
    job
}

fn line_col(text: &str, idx: usize) -> (usize, usize) {
    let byte_at = text
        .char_indices()
        .nth(idx)
        .map(|(b, _)| b)
        .unwrap_or(text.len());
    let head = &text[..byte_at];
    let row = head.matches('\n').count() + 1;
    let last_start = head.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let col = head[last_start..].chars().count() + 1;
    (row, col)
}

fn draw_search(ui: &mut egui::Ui, d: &mut EditorDoc, body: Rect, p: &Pal, cm: &mut CtxMenu) {
    let w = 372.0;
    let h = if d.search.show_replace { 74.0 } else { 42.0 };
    let card = Rect::from_min_size(
        Pos2::new(body.right() - w - 8.0, body.top() + 8.0),
        Vec2::new(w, h),
    );

    let sig = (d.search.find.clone(), d.search.case_sensitive, d.text.len());
    if d.search.sig != sig {
        d.search.sig = sig;
        d.search.matches = find_all(&d.text, &d.search.find, d.search.case_sensitive);
        d.search.current = 0;
    }

    theme::draw_shadow(ui, card, 6.0, theme::ShadowKind::Soft);
    ui.painter().rect_filled(card, 6.0, p.card);
    ui.painter()
        .rect_stroke(card, 6.0, Stroke::new(1.0, p.sep), egui::StrokeKind::Inside);

    ui.allocate_rect(card, Sense::click());

    let x0 = card.left() + 10.0;
    let y0 = card.top() + 8.0;
    let input_w = 168.0;
    let small = 30.0;

    let find_r = Rect::from_min_size(Pos2::new(x0, y0), Vec2::new(input_w, small));
    let find_id = Id::unique(("editor-find", d.id));
    input_box(ui, find_r, find_id, &mut d.search.find, p, cm);
    let mut cx = find_r.right() + 6.0;

    let aa_r = Rect::from_min_size(Pos2::new(cx, y0), Vec2::new(32.0, small));
    if small_btn(ui, aa_r, d.search.case_sensitive, p, "Aa") {
        d.search.case_sensitive = !d.search.case_sensitive;
    }
    cx += 36.0;

    let n = d.search.matches.len();
    let (prev_r, next_r) = (
        Rect::from_min_size(Pos2::new(cx, y0), Vec2::new(30.0, small)),
        Rect::from_min_size(Pos2::new(cx + 32.0, y0), Vec2::new(30.0, small)),
    );
    if n > 0 {
        if small_btn(ui, prev_r, false, p, "↑") {
            d.search.current = (d.search.current + n - 1) % n;
            goto_match(d, ui.ctx(), text_id_of(d.id));
        }
        if small_btn(ui, next_r, false, p, "↓") {
            d.search.current = (d.search.current + 1) % n;
            goto_match(d, ui.ctx(), text_id_of(d.id));
        }
    } else {
        small_btn(ui, prev_r, true, p, "↑");
        small_btn(ui, next_r, true, p, "↓");
    }
    cx += 68.0;

    ui.painter().text(
        Pos2::new(cx, y0 + small / 2.0),
        Align2::LEFT_CENTER,
        if d.search.find.is_empty() {
            String::new()
        } else {
            format!("{}/{}", if n == 0 { 0 } else { d.search.current + 1 }, n)
        },
        FontId::proportional(11.5),
        p.dim,
    );

    let x_r = Rect::from_min_size(Pos2::new(card.right() - 28.0, y0), Vec2::new(22.0, small));
    if small_btn(ui, x_r, false, p, "✕") {
        d.search.open = false;
    }

    if d.search.show_replace {
        let y1 = y0 + small + 4.0;
        let rep_r = Rect::from_min_size(Pos2::new(x0, y1), Vec2::new(input_w, small));
        input_box(
            ui,
            rep_r,
            Id::unique(("editor-replace", d.id)),
            &mut d.search.replace,
            p,
            cm,
        );
        let mut cx = rep_r.right() + 6.0;
        let rep_btn = |ui: &mut egui::Ui, r: Rect, label: &str, p: &Pal, en: bool| {
            ui.painter()
                .rect_filled(r, 4.0, if en { p.hover } else { p.card });
            ui.painter()
                .rect_stroke(r, 4.0, Stroke::new(1.0, p.sep), egui::StrokeKind::Inside);
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(11.5),
                if en { p.fore } else { p.dim },
            );
            en && ui.allocate_rect(r, Sense::click()).clicked()
        };
        let r1 = Rect::from_min_size(Pos2::new(cx, y1), Vec2::new(58.0, small));
        if rep_btn(ui, r1, t_ctx("替换", "Replace"), p, n > 0) {
            replace_current(d, ui.ctx(), text_id_of(d.id));
        }
        cx += 62.0;
        let r2 = Rect::from_min_size(Pos2::new(cx, y1), Vec2::new(72.0, small));
        if rep_btn(ui, r2, t_ctx("全部替换", "Replace All"), p, n > 0) {
            replace_all(d);
        }
    }
}

fn text_id_of(id: u64) -> Id {
    Id::unique(("editor-text", id))
}

fn goto_match(d: &mut EditorDoc, ctx: &egui::Context, text_id: Id) {
    if d.search.matches.is_empty() {
        return;
    }
    let (s, e) = d.search.matches[d.search.current.min(d.search.matches.len() - 1)];
    let mut ts = TextEditState::load(ctx, text_id).unwrap_or_default();
    ts.cursor.set_char_range(Some(egui::text::CCursorRange::two(
        egui::text::CCursor::new(s),
        egui::text::CCursor::new(e),
    )));
    ts.store(ctx, text_id);
}

fn replace_current(d: &mut EditorDoc, ctx: &egui::Context, text_id: Id) {
    let n = d.search.matches.len();
    if n == 0 {
        return;
    }
    let (s, e) = d.search.matches[d.search.current.min(n - 1)];
    let selected = d.sel_cache.unwrap_or((usize::MAX, usize::MAX));
    if selected != (s, e) {

        goto_match(d, ctx, text_id);
        return;
    }
    if let Some((bs, be)) = char_range_to_byte(&d.text, s, e) {
        let rep = d.search.replace.clone();
        d.text.replace_range(bs..be, &rep);
        d.dirty = true;
    }
    d.search.matches = find_all(&d.text, &d.search.find, d.search.case_sensitive);
    if !d.search.matches.is_empty() {
        d.search.current %= d.search.matches.len();
        goto_match(d, ctx, text_id);
    }
}

fn replace_all(d: &mut EditorDoc) {
    if d.search.matches.is_empty() {
        return;
    }
    let rep = d.search.replace.clone();
    let ranges: Vec<(usize, usize)> = d.search.matches.clone();
    let mut byte_ranges: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (s, e) in &ranges {
        if let Some(b) = char_range_to_byte(&d.text, *s, *e) {
            byte_ranges.push(b);
        }
    }
    for (bs, be) in byte_ranges.into_iter().rev() {
        d.text.replace_range(bs..be, &rep);
    }
    d.dirty = true;
    d.search.matches = find_all(&d.text, &d.search.find, d.search.case_sensitive);
    d.search.current = 0;
}

fn char_range_to_byte(text: &str, s: usize, e: usize) -> Option<(usize, usize)> {
    let mut bs = text.len();
    let mut be = text.len();
    let mut ci = 0usize;
    for (b, _) in text.char_indices() {
        if ci == s {
            bs = b;
        }
        if ci == e {
            be = b;
            break;
        }
        ci += 1;
    }
    if s > ci {
        return None;
    }
    Some((bs, be))
}

fn find_all(hay: &str, needle: &str, case: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    let (h, n): (String, String) = if case {
        (hay.to_owned(), needle.to_owned())
    } else {
        (
            hay.chars().map(|c| c.to_ascii_lowercase()).collect(),
            needle.chars().map(|c| c.to_ascii_lowercase()).collect(),
        )
    };
    let mut res = Vec::new();
    let mut sb = 0usize;
    let mut sc = 0usize;
    while let Some(off) = h[sb..].find(&n) {
        let bs = sb + off;
        let be = bs + n.len();
        let cs = sc + h[sb..bs].chars().count();
        let ce = cs + n.chars().count();
        res.push((cs, ce));
        sb = be;
        sc = ce;
    }
    res
}

fn input_box(ui: &mut egui::Ui, r: Rect, id: Id, text: &mut String, p: &Pal, cm: &mut CtxMenu) {
    ui.painter().rect_filled(r, 4.0, p.input_bg);
    ui.painter()
        .rect_stroke(r, 4.0, Stroke::new(1.0, p.sep), egui::StrokeKind::Inside);
    let resp = ui.allocate_rect(r, Sense::click());
    if resp.clicked() {
        ui.ctx().memory_mut(|m| m.request_focus(id));
    }
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    if focused {
        ui.painter()
            .rect_stroke(r, 4.0, Stroke::new(1.5, p.ok), egui::StrokeKind::Inside);
    }
    let hint = t_ctx("查找", "Find");
    let shown = if text.is_empty() && !focused {
        (hint.to_owned(), p.dim)
    } else {
        (text.clone(), p.fore)
    };

    let inner = r.shrink2(egui::vec2(6.0, 2.0));
    if focused {
        let te = TextEdit::singleline(text)
            .id(id)
            .desired_width(inner.width())
            .font(FontSelection::FontId(FontId::proportional(12.0)))
            .text_color(p.fore)
            .frame(egui::Frame::new())
            .margin(egui::Margin::same(4));
        let out = te.show(ui);

        cm.register(out.response.id, out.response.rect);

    } else {
        ui.painter().text(
            Pos2::new(inner.left(), r.center().y),
            Align2::LEFT_CENTER,
            shown.0,
            FontId::proportional(12.0),
            shown.1,
        );
    }
}

fn small_btn(ui: &mut egui::Ui, r: Rect, checked: bool, p: &Pal, label: &str) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    if checked {
        ui.painter().rect_filled(r, 4.0, p.checked);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, p.hover);
    }
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        p.fore,
    );
    resp.clicked()
}

fn draw_status(ui: &mut egui::Ui, d: &mut EditorDoc, r: Rect, p: &Pal) {
    ui.painter().line_segment(
        [
            Pos2::new(r.left(), r.top() + 0.5),
            Pos2::new(r.right(), r.top() + 0.5),
        ],
        Stroke::new(1.0, p.sep),
    );
    let (_, row, col) = d.cursor_cache;
    let left = tfmt("行 {0}, 列 {1}", &[&row, &col]);
    ui.painter().text(
        Pos2::new(r.left() + 10.0, r.center().y),
        Align2::LEFT_CENTER,
        left,
        FontId::proportional(11.0),
        p.dim,
    );

    let right = if let Some(err) = &d.save_err {
        (err.clone(), p.danger)
    } else if d.pending_upload {
        (t("正在回传…").to_owned(), p.dim)
    } else if let Some(rct) = &d.last_receipt {
        (rct.clone(), p.dim)
    } else {
        (d.enc.label().to_owned(), p.dim)
    };
    ui.painter().text(
        Pos2::new(r.right() - 10.0, r.center().y),
        Align2::RIGHT_CENTER,
        right.0,
        FontId::proportional(11.0),
        right.1,
    );
}

fn draw_load_error(ui: &mut egui::Ui, d: &mut EditorDoc, body: Rect, p: &Pal, err: &str) {
    let card = Rect::from_center_size(body.center(), Vec2::new(380.0, 130.0));

    theme::draw_shadow(ui, card, 8.0, theme::ShadowKind::Soft);
    ui.painter().rect_filled(card, 8.0, p.card);
    ui.painter()
        .rect_stroke(card, 8.0, Stroke::new(1.0, p.sep), egui::StrokeKind::Inside);
    ui.painter().text(
        Pos2::new(card.center().x, card.top() + 34.0),
        Align2::CENTER_CENTER,
        t_ctx("无法打开文件", "Cannot open file"),
        FontId::proportional(14.0),
        p.fore,
    );
    ui.painter().text(
        Pos2::new(card.center().x, card.top() + 62.0),
        Align2::CENTER_CENTER,
        err,
        FontId::proportional(11.5),
        p.danger,
    );
    let br = Rect::from_center_size(
        Pos2::new(card.center().x, card.bottom() - 30.0),
        Vec2::new(88.0, 28.0),
    );
    ui.painter().rect_filled(br, 4.0, p.hover);
    ui.painter().text(
        br.center(),
        Align2::CENTER_CENTER,
        t("关闭"),
        FontId::proportional(12.0),
        p.fore,
    );
    if ui.allocate_rect(br, Sense::click()).clicked() {
        d.closed = true;
    }
}

fn draw_close_ask(ui: &mut egui::Ui, d: &mut EditorDoc, full: Rect, p: &Pal) {
    ui.painter()
        .rect_filled(full, 0.0, Color32::from_black_alpha(80));
    let card = Rect::from_center_size(full.center(), Vec2::new(380.0, 148.0));

    theme::draw_shadow(ui, card, 8.0, theme::ShadowKind::Soft);
    ui.painter().rect_filled(card, 8.0, p.card);
    ui.painter()
        .rect_stroke(card, 8.0, Stroke::new(1.0, p.sep), egui::StrokeKind::Inside);

    ui.painter().text(
        Pos2::new(card.center().x, card.top() + 36.0),
        Align2::CENTER_CENTER,
        tfmt(
            t_ctx("是否保存对 {0} 的更改?", "Save changes to {0}?"),
            &[&d.title],
        ),
        FontId::proportional(14.0),
        p.fore,
    );

    let (save_label, discard_label, cancel_label) = (
        t_ctx("保存并关闭", "Save"),
        t_ctx("不保存", "Don't Save"),
        t("取消"),
    );
    let bw = 96.0;
    let y = card.bottom() - 38.0;
    let save_r = Rect::from_min_size(
        Pos2::new(card.center().x - bw * 1.5 - 8.0, y),
        Vec2::new(bw, 28.0),
    );
    let disc_r = Rect::from_min_size(
        Pos2::new(card.center().x - bw / 2.0, y),
        Vec2::new(bw, 28.0),
    );
    let cancel_r = Rect::from_min_size(
        Pos2::new(card.center().x + bw / 2.0 + 8.0, y),
        Vec2::new(bw, 28.0),
    );

    for (r, label) in [
        (save_r, save_label),
        (disc_r, discard_label),
        (cancel_r, cancel_label),
    ] {
        ui.painter().rect_filled(r, 4.0, p.hover);
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.0),
            p.fore,
        );
    }
    let (s, c, x) = (
        ui.allocate_rect(save_r, Sense::click()),
        ui.allocate_rect(disc_r, Sense::click()),
        ui.allocate_rect(cancel_r, Sense::click()),
    );
    if s.clicked() {
        save_doc(d, ui.ctx());
        if d.save_err.is_none() {
            d.closed = true;
        } else {
            d.close_ask = false;
        }
    }
    if c.clicked() {
        d.closed = true;
    }
    if x.clicked() {
        d.close_ask = false;
    }
}
