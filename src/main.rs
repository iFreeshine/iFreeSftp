#![windows_subsystem = "windows"]

use std::sync::Arc;

mod app_paths;
mod editor;
mod hosts;
mod i18n;
mod icons;
mod icons_sys;
mod import;
mod local;

#[cfg(feature = "mcp")]
mod mcp_daemon;
mod persistence;
#[cfg(feature = "official")]
mod registration;
mod settings;
mod sm3;
mod ssh;
mod theme;
mod transfer;
mod ui;

#[cfg(feature = "official")]
#[path = "../../ifree-core/src/lib.rs"]
pub(crate) mod ifree_core;

const WINDOW_TITLE: &str = "iFreeSftp";
const DEFAULT_SIZE: [f32; 2] = [1280.0, 800.0];
const MIN_SIZE: [f32; 2] = [900.0, 600.0];

const LATIN_FONT: &[u8] = include_bytes!("assets/fonts/Inter-Regular.ttf");

const BRAND_FONT: &[u8] = include_bytes!("assets/fonts/Inter-SemiBold.ttf");

const MONO_FONT: &[u8] = include_bytes!("assets/fonts/JetBrainsMono-Regular.ttf");

const CJK_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\msyhbd.ttc",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
];

fn main() -> eframe::Result {

    #[cfg(feature = "mcp")]
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        return mcp_daemon::run();
    }

    #[cfg(debug_assertions)]
    sm3::debug_self_check();

    let saved = persistence::load();

    settings::load();

    hosts::load();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_inner_size(saved.map(|s| [s.w, s.h]).unwrap_or(DEFAULT_SIZE))
        .with_min_inner_size(MIN_SIZE)

        .with_resizable(true)
        .with_decorations(false);
    if let Some(s) = saved {
        viewport = viewport.with_position([s.x as f32, s.y as f32]);
        if s.maximized {
            viewport = viewport.with_maximized(true);
        }
    }

    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            let app = App::new(cc, saved);
            Ok(Box::new(app))
        }),
    )
}

struct App {
    st: ui::State,

    saved: Option<persistence::WinState>,

    last_save: f64,
}

pub fn window_icon() -> Option<egui::IconData> {
    const ICO: &[u8] = include_bytes!("../assets/AppIcon.ico");

    let count = u16::from_le_bytes([ICO[4], ICO[5]]) as usize;
    let (mut off, mut len, mut best) = (0usize, 0usize, 0u32);
    for i in 0..count {
        let e = 6 + 16 * i;

        let w = match *ICO.get(e)? {
            0 => 256,
            w => u32::from(w),
        };
        let entry_len = u32::from_le_bytes(ICO.get(e + 8..e + 12)?.try_into().ok()?) as usize;
        let entry_off = u32::from_le_bytes(ICO.get(e + 12..e + 16)?.try_into().ok()?) as usize;
        if !ICO.get(entry_off..entry_off + 4)?.starts_with(b"\x89PNG") {
            continue;
        }
        if w >= best {
            best = w;
            len = entry_len;
            off = entry_off;
        }
    }
    let img = image::load_from_memory(ICO.get(off..off + len)?)
        .ok()?
        .to_rgba8();
    Some(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, saved: Option<persistence::WinState>) -> Self {

        theme::install(&cc.egui_ctx, egui::Theme::Light);
        theme::install(&cc.egui_ctx, egui::Theme::Dark);
        if !settings::startup_theme_dark() {
            theme::install(&cc.egui_ctx, egui::Theme::Light);
        }

        install_fonts(&cc.egui_ctx);

        if let Some(c) = persistence::load_cols() {
            ui::panes::set_col_vis(ui::panes::ColVisibility {
                size: c[0],
                ty: c[1],
                modified: c[2],
                perms: c[3],
            });
        }

        #[cfg(feature = "official")]
        crate::registration::service::init(cc.egui_ctx.clone());

        Self {
            st: ui::State::default(),
            saved,
            last_save: 0.0,
        }
    }

    fn persist_window(&mut self, ctx: &egui::Context) {
        let (inner, maximized, minimized) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.inner_rect,
                v.maximized.unwrap_or(false),
                v.minimized.unwrap_or(false),
            )
        });
        let Some(rect) = inner else {
            return;
        };
        if minimized {
            return;
        }

        let dark = ctx.theme() == egui::Theme::Dark;

        let cv = ui::panes::col_vis();
        let base = self.saved.unwrap_or_default();
        let cand = if maximized {
            persistence::WinState {
                maximized: true,
                dark,
                col_size: cv.size,
                col_type: cv.ty,
                col_modified: cv.modified,
                col_perms: cv.perms,
                ..base
            }
        } else {
            persistence::WinState {
                x: rect.left() as i32,
                y: rect.top() as i32,
                w: rect.width(),
                h: rect.height(),
                maximized: false,
                dark,
                col_size: cv.size,
                col_type: cv.ty,
                col_modified: cv.modified,
                col_perms: cv.perms,
            }
        };
        if self.saved == Some(cand) {
            return;
        }
        let now = ctx.time();
        if now - self.last_save < 0.3 {

            ctx.request_repaint_after(std::time::Duration::from_millis(350));
            return;
        }
        persistence::save(&cand);
        self.saved = Some(cand);
        self.last_save = now;
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(egui::FontData::from_owned(LATIN_FONT.to_vec())),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    fonts.font_data.insert(
        "inter-semibold".to_owned(),
        Arc::new(egui::FontData::from_owned(BRAND_FONT.to_vec())),
    );
    fonts.families.insert(
        egui::FontFamily::Name("brand".into()),
        vec!["inter-semibold".to_owned()],
    );

    fonts.font_data.insert(
        "mono".to_owned(),
        Arc::new(egui::FontData::from_owned(MONO_FONT.to_vec())),
    );
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "mono".to_owned());

    for path in CJK_FONT_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };

        fonts.font_data.insert(
            "cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );

        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }

        ctx.set_fonts(fonts);
        return;
    }

    eprintln!(
        "warning: no CJK font found in {}",
        CJK_FONT_CANDIDATES.join(", ")
    );
    ctx.set_fonts(fonts);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {

        self.st.terminal.extend(crate::hosts::drain_pending_logs());

        self.st.ctx_menu.frame_start(ui.ctx());
        ui::root(ui, &mut self.st);
        self.st.ctx_menu.frame_end(ui.ctx());
        let modal = ui::frame_modal(&self.st);
        ui::edge_resize(ui, modal);

        ui::cursor_decide(ui.ctx());
        self.persist_window(ui.ctx());
    }
}
