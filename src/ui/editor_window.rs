//! 编辑器独立窗口（T7，对标 C# WpfEditorWindow）：主窗口每帧为每篇文档声明
//! 一个 deferred viewport（不声明即关闭）；窗口内 自绘标题栏 + 工具栏 + TextEdit
//! 编辑区（行号叠加）+ 查找替换浮层 + 状态栏 + 未保存确认。
//!
//! C# 语义对齐：
//! - 默认 960×720 / 最小 400×300；标题 `名 * - iFreeSftp`；
//! - 工具钮 保存/撤消/查找/自动换行(toggle)/行号(toggle)，明暗配色独立令牌；
//! - 撤消走 TextEditState.undoer 镜像（与 Ctrl+Z 同一撤销栈）；
//! - 关闭未保存 → 保存并关闭/不保存/取消（C# MessageBox YesNoCancel）；
//! - 保存按探测编码写回；远程文档保存成功 → 静态事件队列（主窗口回传上传）。

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

    /// 钉子（egui 0.37 语义备案）：TextEdit::margin() 生效时 galley_pos =
    /// response.min + margin（body 坐标语义）。同帧再设 .frame() 会令 margin
    /// 整个跳过（builder.rs:693-701 frame 分支优先）——编辑器 padding 因此
    /// 必须挂 frame.inner_margin，不能依赖 margin()。
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
        // response.min=(100,50) + margin=(30,5) → galley_pos=(130,55)。
        assert_eq!(out.galley_pos, Pos2::new(130.0, 55.0));
        ctx.end_pass().textures_delta.clear();
    }

    /// 钉子（行号可见 + 编辑工作区 padding 10px）：draw_window 一帧后——
    /// ① 正文 galley 起点 = 编辑区左缘 + 栏沟 + 10px（4 行提示 → ln_w=44.8，
    ///    正文 x=54.8；修复前 margin 失效正文贴 x=0）；
    /// ② 行号 1/2/3 画在栏沟内、正坐标（修复前 pos.x=-17.8 被 clip 全裁，
    ///    行号功能缺失）；③ 行右距恒定（纵向随行进）。
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
        // ① 正文起点：栏沟 ln_w=36.6（digits_hint 钳 3 位）+ padding 10 → x≈46
        //   （修复前 margin 失效正文贴 x=0）。
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
        // ② 行号 1/2/3 画在栏沟内（右对齐至 x≈ln_w=36.6，"1" 左上 ≈28.8），
        //    正坐标可见。
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
        // ③ 行号随行进（y 递增、行距 ≈ 行高 15.4）。
        let mut ys: Vec<f32> = nums.iter().map(|(_, p)| p.y).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(ys[1] - ys[0] > 14.0 && ys[1] - ys[0] < 17.0);
        assert!(ys[2] - ys[1] > 14.0 && ys[2] - ys[1] < 17.0);
        // headless 无渲染器：未消化的字体 delta 会在 Drop 时 debug_assert。
        full.textures_delta.clear();
    }
}

/// 窗口默认尺寸（C# Width=960 Height=720）。
const WIN_SIZE: [f32; 2] = [960.0, 720.0];
/// 最小尺寸（C# MinWidth/MinHeight）。
const MIN_SIZE: [f32; 2] = [400.0, 300.0];
/// 标题栏高。
const TITLE_H: f32 = 32.0;
/// 工具栏高。
const TOOL_H: f32 = 30.0;
/// 状态栏高。
const STATUS_H: f32 = 22.0;
/// 工具钮边长。
const TOOL_BTN: f32 = 26.0;
/// 正文字号（C# EditorFontSize 13）。
const TEXT_SIZE: f32 = 13.0;

/// 编辑器独立配色（C# WpfEditorWindow 令牌，不随主面板令牌表）。
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
    /// 行注释色（简易高亮，VSCode 风格）。
    hl_comment: Color32,
    /// 数字字面量色（简易高亮）。
    hl_number: Color32,
    /// 字符串字面量色（简易高亮）。
    hl_string: Color32,
}

/// 编辑器独立色板（F-11 范围外记录，2026-11-11 拍板「只具名、值一律不动」）：
/// 本文件不走 theme:: 令牌、且被门禁 theme_literals_all_use_tokens 豁免。
/// 已知差异仅记录不改值——ok 绿（亮 0x0F7B0F / 暗 0x6CCB7F）与
/// Palette.success_fg（亮 0x107C10）不同源；danger 亮值 0xC42B1C 同
/// theme::CAPTION_CLOSE_HOVER 字面但语义独立（编辑器内联红，非 caption 钮）。
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

/// 编辑器窗口图标缓存（viewport 闭包 Fn 不可持 &mut，全局 OnceLock 兜住；
/// 纹理按各 viewport 自己的 ctx 缓存，Mutex 只护 HashMap）。
fn icons() -> &'static Mutex<Option<crate::icons::Cache>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<crate::icons::Cache>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Some(crate::icons::Cache::default())))
}

/// 编辑器窗口图标（deferred viewport 的 builder 每帧重建，OnceLock 缓存
/// 一次解析结果，避免逐帧解 256px PNG）。
fn app_icon() -> Option<std::sync::Arc<egui::IconData>> {
    static ICON: std::sync::OnceLock<Option<std::sync::Arc<egui::IconData>>> =
        std::sync::OnceLock::new();
    ICON.get_or_init(|| crate::window_icon().map(std::sync::Arc::new))
        .clone()
}

/// 编辑器窗口各自的右键菜单状态：deferred viewport 是独立 egui 帧，主窗口
/// 的 CtxMenu 钩子不覆盖这里，按文档 id 各持一份（draw_window 帧内
/// frame_start → 编辑区注册 → frame_end；show_all 回收文档时同步清理）。
fn ctx_menus() -> &'static Mutex<HashMap<u64, CtxMenu>> {
    static M: std::sync::OnceLock<Mutex<HashMap<u64, CtxMenu>>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 主窗口每帧调用：同步主题 → 回收已关文档（清远程副本）→ 声明各编辑器 viewport。
pub fn show_all(ctx: &egui::Context, hub: &mut EditorHub, dark: bool) {
    // 回收：closed 文档移出并清远程临时副本目录（C# onClosed 同语义）。
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
    // 右键菜单实例随文档回收（键 = 文档 id）。
    ctx_menus()
        .lock()
        .unwrap()
        .retain(|k, _| live_ids.contains(k));

    for doc in &hub.docs {
        let d = doc.lock().unwrap();
        let title = d.window_title();
        let id = d.id;
        drop(d);
        // with_icon 只收 IconData 本体，Option 在外层判（解析失败保持默认图标）。
        let mut builder = ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(WIN_SIZE)
            .with_min_inner_size(MIN_SIZE)
            // 自绘标题栏：边缘缩放由 ui::edge_resize 命中区接手（同主窗口）。
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

    // 右键菜单（复制/剪切/粘贴）：本 viewport 帧首 → 编辑区注册 → 帧尾。
    let mut menus = ctx_menus().lock().unwrap();
    let cm = menus.entry(d.id).or_default();
    cm.frame_start(&ctx);

    // OS 关闭请求（标题栏 X / Alt+F4）：未保存先确认（吞请求即窗口保持）。
    if ctx.input(|i| i.viewport().close_requested()) {
        if d.dirty && !d.close_ask && d.load_error.is_none() {
            d.close_ask = true;
        } else if !d.dirty || d.load_error.is_some() {
            d.closed = true;
        }
    }

    // 快捷键（C# AvalonEdit 内置对齐）：Ctrl+S 保存 / Ctrl+F 查找 / Ctrl+H 替换 / Esc 关查找。
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

    // 工具栏「撤消」置位，draw_editor 里 TextEdit show 后执行（需要 galley 读选区）。
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

    // 帧尾画浮层菜单（编辑区本帧未注册如加载失败则自动收掉）。
    cm.frame_end(&ctx);
    // 边缘缩放命中区：须在一切控件之后注册（命中优先级最高，见 edge_resize 注释）。
    // 子视口自己的关闭确认框即弹层，期间整层跳过。
    super::edge_resize(ui, d.close_ask);
    // F-8：子视口帧尾也要裁决一次，否则它的光标恒为 Default。
    super::cursor_decide(ui.ctx());

    // closed 生效后（✕ / 不保存 / 保存并关闭 / 干净文档 Alt+F4），窗口回收依赖
    // 主窗口 show_all「不再重声明」；而 eframe 里子视口重绘只跑子闭包，主窗口
    // 闲置时不会自行重绘，窗口就会永远留在屏幕上。对齐 eframe 原生 Alt+F4 的
    // 处理（request_repaint_of 父视口），主动请求主窗口补一帧完成回收。
    if d.closed {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

// ── 标题栏 ──────────────────────────────────────────────────

fn draw_title(ui: &mut egui::Ui, d: &mut EditorDoc, r: Rect, p: &Pal) {
    ui.painter().rect_filled(r, 0.0, p.back);

    // 图标 + 标题（带 * 由 window_title 生成）。
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

    // 最小化 / 最大化 / 关闭（hover 变色；C# 标题栏钮）。
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

    // 拖动窗口（按钮区除外）。
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

// ── 工具栏 ──────────────────────────────────────────────────

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

    // (图标名, 钮面文字 zh, 钮面文字 en, 动作)；tooltip 另表（带快捷键）。
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
            x += 6.0; // toggle 组前小间隔
        }
        // 图标 + 文字钮：钮宽按当前语言文案实测（每帧 5 次小布局，开销可忽略）。
        let label = t_ctx(zh, en);
        // egui 0.37 起 FontsView::layout_no_wrap 需 &mut；Painter 版内部持锁，&self 即可。
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

/// 保存（Ctrl+S / 工具钮 / 保存并关闭三入口共用）。保存成功且为远程文档时
/// 请求主窗口（ROOT）补一帧——编辑器是独立视口，主窗口闲置不会自行重绘，
/// 不唤醒则「上传编辑副本…」要等用户碰主窗口才出现（首帧唤醒，项1）。
fn save_doc(d: &mut EditorDoc, ctx: &egui::Context) {
    d.save_err = None;
    if let Err(e) = d.save() {
        d.save_err = Some(e);
    } else if d.remote.is_some() {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

// ── 编辑区（TextEdit + 行号叠加）─────────────────────────────

fn draw_editor(
    ui: &mut egui::Ui,
    d: &mut EditorDoc,
    r: Rect,
    p: &Pal,
    want_undo: bool,
    cm: &mut CtxMenu,
) {
    // 行号列宽按打开/保存时缓存的行数估位（避免每帧 O(n) 扫描）。
    let text_id = Id::unique(("editor-text", d.id));
    let digits_hint = d.lines_hint.to_string().len().max(3);
    let ln_w = if d.line_numbers {
        12.0 + digits_hint as f32 * 8.2
    } else {
        0.0
    };

    // 编辑区放进滚动域：TextEdit multiline 会随内容长高、自身既不裁剪也不滚动，
    // child ui 的 max_rect 只约束布局游标、paint 仍会越过状态栏（溢出根因）。
    // wrap 开 → 仅纵向；wrap 关 → 双向滚动（C# AvalonEdit 同款行为）。
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let area = if d.wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    let (wrap, line_numbers) = (d.wrap, d.line_numbers);
    let lang = lang_of(&d.title);
    // 简易高亮（注释/字符串/数字，按扩展名分流语言）走官方 layouter 钩子：
    // 同字体同字号只改色，不影响游标/选区映射；wrap 关时解除折行约束。
    let mut layouter =
        |ui: &egui::Ui, buf: &dyn TextBuffer, wrap_width: f32| -> std::sync::Arc<egui::Galley> {
            let job = highlight_job(p, buf.as_str(), wrap_width, wrap, lang);
            ui.fonts_mut(|f| f.layout_job(job))
        };
    let result = area
        .auto_shrink(false)
        .id_salt(text_id.with("scroll"))
        // 底部横向滚动条上移一点（2026-11-12 用户定）：与状态栏留 6px 空隙，
        // 不再贴死编辑区下缘。scroll_bar_rect = 屏幕空间矩形。
        .scroll_bar_rect(Rect::from_min_max(
            r.min,
            Pos2::new(r.right(), r.bottom() - 6.0),
        ))
        .show(&mut body_ui, |sui| {
            // 编辑工作区四周 padding 10px（2026-11-12 用户定）；行号开时左缘
            // 先让出栏沟（ln_w）再加 padding。
            // egui 0.37 坑：TextEdit::margin() 只在未设置 .frame() 时生效——
            // builder.rs:693-701 frame 分支优先，margin 被整个跳过。此前
            // .frame(Frame::new()) 令 padding 全灭（正文贴边）、行号按
            // galley_pos.x-10 画进负坐标被 clip 裁掉（行号缺失根因）。
            // padding 须挂 frame 的 inner_margin。
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

    // 行号叠加（固着左侧栏沟：横向不随内容滚走、纵向随滚动行进；裁剪到
    // 编辑区可视范围）。wrap 续行不画（AvalonEdit 同款）。galley_pos 是
    // body 坐标（含 r.min 与 margin），scroll 内容的绘制 transform 已含
    // -offset，固着栏沟须手动减回 offset.y。
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

    // 注册右键菜单（无「全选」；rect = TextEdit 内容区）。
    cm.register_plain(out.response.id, out.response.rect);

    // 选区 + 状态栏行列缓存（char 序变化才重算 O(n) 扫描）。
    if let Some(rng) = out.cursor_range {
        let [a, b] = rng.sorted_cursors();
        d.sel_cache = Some((a.index.0, b.index.0));
        let cur = rng.primary.index.0;
        if d.cursor_cache.0 != cur {
            let (row, col) = line_col(&d.text, cur);
            d.cursor_cache = (cur, row, col);
        }
    }

    // 工具栏「撤消」：TextEdit show 后执行（out.state 与内部 feed 的一致才能命中撤销栈）。
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

/// 高亮语言（按扩展名探测）。
#[derive(Clone, Copy, PartialEq)]
enum Lang {
    /// 通用（未知扩展名）：`//` 行注释（scheme:// 除外）+ `#` 行首 + 数字。
    Plain,
    Python,
    Js,
    Css,
    Shell,
}

/// 按文件名扩展名探测语言（title = 本地/远程统一的文件名）。
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

/// 着色类别。
#[derive(Clone, Copy)]
enum Tok {
    Comment,
    Str,
    Num,
}

/// 简易语法高亮（方案 B 轻量版）：注释 + 字符串 + 数字。纯 ASCII 字节扫描，
/// 命中区间必落 ASCII 边界（多字节 UTF-8 序列不可能含 ASCII 字节），切片安全。
/// 规则：
/// - 注释：JS `//` 与 `/* */`、CSS `/* */`（可跨行）、Python `#`、Shell `#`
///   （行首或前置空白）；通用（未知扩展名）`//` + `#` 行首。`//` 前一字节为
///   `:` 时视为 `scheme://` 连接符不着色（排除 https:// 等 URL）；
/// - 字符串：' "（JS 另认 `），`\` 跳过下一字节，行尾未闭合按行截断；
///   Python 三引号跨行到闭合序号或文末；
/// - 数字：标识符中间不着色；`0x` 十六进制；十进制可含小数点，尾部孤点回退。
fn highlight_job(p: &Pal, text: &str, wrap_width: f32, wrap: bool, lang: Lang) -> LayoutJob {
    let font = FontId::monospace(TEXT_SIZE);
    let base = TextFormat::simple(font.clone(), p.fore);
    let b = text.as_bytes();
    let mut spans: Vec<(usize, usize, Tok)> = Vec::new(); // (字节起, 字节止, 类别)
    let (mut i, mut line_head, mut prev_word) = (0usize, true, false);
    while i < b.len() {
        let c = b[i];
        let n1 = b.get(i + 1).copied();
        // 字符串起始（通用规则不着色字符串）。
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
                // 三引号：跨行扫描到闭合序号，未闭合到文末。
                i += 3;
                while i + 3 <= b.len() && !(b[i] == q && b[i + 1] == q && b[i + 2] == q) {
                    i += 1;
                }
                i = if i + 3 <= b.len() { i + 3 } else { b.len() };
            } else {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2; // 转义跳下一字节（越界由下方钳制兜底）
                    } else if b[i] == q || b[i] == b'\n' {
                        break;
                    } else {
                        i += 1;
                    }
                }
                if i < b.len() && b[i] == q {
                    i += 1; // 收进行内闭合引号
                }
                i = i.min(b.len());
            }
            spans.push((start, i, Tok::Str));
            prev_word = false;
            line_head = false;
            continue;
        }
        // 块注释 /* */（JS / CSS，可跨行；未闭合到文末）。
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
        // 行注释 `//`（JS / 通用）：前一字节为 `:` 是 scheme:// 连接符，跳过。
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
        // 行注释 `#`：Python 任意位置；Shell 行首或前置空白；通用仅行首。
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
        // 数字字面量：非标识符中间；0x 十六进制 / 十进制含小数点、尾点回退。
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
    // append 自动按拼接后的 text.len() 推导每段 byte_range（顺序切段即可），
    // leading_space 恒 0；同格式相邻段框架自动合并，保证整段 shaping 一致。
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

/// 光标 char 序 → (行,列)（1-based；列按字符计 —— C# 同为字符列）。
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

// ── 查找/替换浮层（右上角卡片）───────────────────────────────

fn draw_search(ui: &mut egui::Ui, d: &mut EditorDoc, body: Rect, p: &Pal, cm: &mut CtxMenu) {
    let w = 372.0;
    let h = if d.search.show_replace { 74.0 } else { 42.0 };
    let card = Rect::from_min_size(
        Pos2::new(body.right() - w - 8.0, body.top() + 8.0),
        Vec2::new(w, h),
    );
    // 重算匹配（签名不变跳过：查找词/大小写/文本字节数）。
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
    // 卡片命中面：吸收点击，防止穿透到编辑区误选。
    ui.allocate_rect(card, Sense::click());

    let x0 = card.left() + 10.0;
    let y0 = card.top() + 8.0;
    let input_w = 168.0;
    let small = 30.0;

    // 第一行：查找输入 + Aa + ↑↓ + ×。
    let find_r = Rect::from_min_size(Pos2::new(x0, y0), Vec2::new(input_w, small));
    let find_id = Id::unique(("editor-find", d.id));
    input_box(ui, find_r, find_id, &mut d.search.find, p, cm);
    let mut cx = find_r.right() + 6.0;

    // Aa（大小写 toggle）。
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

    // 计数 "n/m"。
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

    // × 关闭。
    let x_r = Rect::from_min_size(Pos2::new(card.right() - 28.0, y0), Vec2::new(22.0, small));
    if small_btn(ui, x_r, false, p, "✕") {
        d.search.open = false;
    }

    // 第二行：替换输入 + 替换 + 全部替换。
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

/// 跳到当前匹配（设选区；TextEdit 有焦点时自动滚动到光标 —— egui 内置）。
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

/// 替换当前：仅当光标选区恰为当前匹配时原地替换，否则先跳转（C# SearchPanel 同语义）。
fn replace_current(d: &mut EditorDoc, ctx: &egui::Context, text_id: Id) {
    let n = d.search.matches.len();
    if n == 0 {
        return;
    }
    let (s, e) = d.search.matches[d.search.current.min(n - 1)];
    let selected = d.sel_cache.unwrap_or((usize::MAX, usize::MAX));
    if selected != (s, e) {
        // 选区不在当前匹配上：先跳转，再按一次替换。
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

/// 全部替换（从后往前替换避开偏移失效）。
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

/// char 区间 → byte 区间（起点 O(n) 定位；替换频度低可接受）。
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

/// 非重叠匹配全找（char 偏移；大小写不敏感 = ASCII 折叠，1:1 char 映射保偏移不变）。
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
    let mut sb = 0usize; // 匹配起点 byte
    let mut sc = 0usize; // 对应 char 序
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

/// 查找输入框（单行；首帧获焦由 Ctrl+F 的 request_focus 处理）。
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
    // 输入区内边距 6。
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
        // 右键菜单（复制/剪切/粘贴/全选）：注册进本文档 viewport 实例
        // （编辑器窗口是独立 egui 帧，主窗口 st.ctx_menu 管不到这里）。
        // 仅聚焦帧注册：未聚焦时只有点击命中面，选区复制只发生在聚焦态。
        cm.register(out.response.id, out.response.rect);
        // 输入变化即重算匹配（下一帧 find_all 分支处理）。
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

/// 小方钮（toggle 态底色）。
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

// ── 状态栏 ──────────────────────────────────────────────────

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
    // 右侧四级优先（CR-6）：save_err（红）> 正在回传… > 成功回执 > 编码标签。
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

// ── 打开失败 / 未保存确认 ────────────────────────────────────

fn draw_load_error(ui: &mut egui::Ui, d: &mut EditorDoc, body: Rect, p: &Pal, err: &str) {
    let card = Rect::from_center_size(body.center(), Vec2::new(380.0, 130.0));
    // 软投影对照 C# 系统阴影，统一走 theme::draw_shadow。
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
    // 软投影对照 C# 系统阴影，统一走 theme::draw_shadow。
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
            d.close_ask = false; // 保存失败回编辑区看状态栏错误
        }
    }
    if c.clicked() {
        d.closed = true;
    }
    if x.clicked() {
        d.close_ask = false;
    }
}
