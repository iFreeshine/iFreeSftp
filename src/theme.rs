use egui::{Color32, Context, FontFamily, FontId, Style, TextStyle, Theme};

pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0x78, 0xD4);

pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x10, 0x6E, 0xBE);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x00, 0x5A, 0x9E);

pub const SUCCESS: Color32 = Color32::from_rgb(0x10, 0x7C, 0x10);

pub const WARNING: Color32 = Color32::from_rgb(0xD4, 0xA0, 0x17);

pub const UBUNTU: Color32 = Color32::from_rgb(0xE9, 0x54, 0x20);

pub const MENU_DANGER: Color32 = Color32::from_rgb(0xE8, 0x11, 0x23);

pub const CAPTION_CLOSE_HOVER: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);

pub const TOOLTIP_FG: Color32 = Color32::WHITE;

pub const MEASURE_ONLY: Color32 = Color32::WHITE;

pub const CHECK_FG: Color32 = Color32::WHITE;

pub const ON_ACCENT_FG: Color32 = Color32::WHITE;

pub const TEXT_ON_ACCENT_DISABLED: Color32 =
    Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 0x5D);

pub fn selection() -> Color32 {
    Color32::from_rgba_unmultiplied(0x00, 0x78, 0xD4, 0x80)
}

pub fn mask() -> Color32 {
    Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x80)
}

pub fn warning_border() -> Color32 {
    Color32::from_rgba_unmultiplied(0xD4, 0xA0, 0x17, 0x66)
}

pub fn dim50(c: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a() / 2)
}

pub fn border_width(ppp: f32) -> f32 {
    1.0 / ppp
}

pub fn stroke(ppp: f32, w: f32, color: Color32) -> egui::Stroke {
    egui::Stroke::new(w / ppp, color)
}

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

pub fn snap_rect(rect: egui::Rect, ppp: f32) -> egui::Rect {

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

pub(crate) enum ShadowKind {

    Flyout,

    Soft,

    Dialog,
}

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

    let rect = rect.round();

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

pub const GUTTER: f32 = 6.0;

pub const CONTAINER_RADIUS: f32 = 8.0;

pub const CONTROL_RADIUS: f32 = 4.0;

pub const TITLE_BAR_H: f32 = 35.0;

pub const CLOSE_SIZE: f32 = 28.0;

pub const CONTENT_PAD: (f32, f32) = (15.0, 15.0);

pub const CONTENT_INSET: f32 = 5.0;

pub const GROUP_GAP: f32 = 14.0;

pub const BTN_H: f32 = 32.0;

pub const SHELL_RADIUS: f32 = 6.0;

pub const PILL_RADIUS: f32 = 12.0;

pub const CONTROL_BORDER: bool = true;

pub const BOTTOM_ROW_MIN: f32 = 120.0;

pub const SCROLL_BAR_WIDTH: f32 = 6.0;

pub const SCROLL_BAR_FLOATING_WIDTH: f32 = 2.0;

pub const SCROLL_BAR_OUTER_OFFSET: f32 = 5.0;

pub const SCROLL_BAR_OPACITY: f32 = 0.3;

pub const SCROLL_BAR_INNER_QUEUE: f32 = 1.0;

#[derive(Clone, Copy)]
pub struct Palette {

    pub header_area: Color32,

    pub folder_area: Color32,

    pub dialog_title_bar: Color32,

    pub separator: Color32,

    pub row_separator: Color32,

    pub text_primary: Color32,

    pub text_secondary: Color32,

    pub text_tertiary: Color32,

    pub text_disabled: Color32,

    pub hover_bg: Color32,

    pub pressed_bg: Color32,

    pub terminal_bg: Color32,

    pub terminal_text: Color32,

    pub terminal_time: Color32,

    pub tab_border: Color32,

    pub ide_tab_border: Color32,

    pub tab_hover: Color32,

    pub tab_rest: Color32,

    pub tab_active_fg: Color32,

    pub row_hover: Color32,

    pub row_pressed: Color32,

    pub card_bg: Color32,

    pub card_stroke: Color32,

    pub control_bg: Color32,

    pub control_hover: Color32,

    pub control_pressed: Color32,

    pub control_stroke: Color32,

    pub control_disabled: Color32,

    pub control_stroke_disabled: Color32,

    pub control_stroke_default: Color32,

    pub control_bg_focused: Color32,

    pub focus_stroke: Color32,

    pub control_strong_fill: Color32,

    pub control_strong_fill_disabled: Color32,

    pub on_accent_fg: Color32,

    pub toggle_off_fill: Color32,

    pub toggle_off_fill_hover: Color32,

    pub toggle_off_fill_pressed: Color32,

    pub combo_popup_bg: Color32,

    pub combo_popup_stroke: Color32,

    pub critical: Color32,

    pub caution_bg: Color32,

    pub critical_bg: Color32,

    pub tab_selected_bg: Color32,

    pub tab_new_hover: Color32,

    pub nav_selected_bg: Color32,

    pub nav_rest_bg: Color32,

    pub accent: Color32,

    pub divider_stroke: Color32,

    pub accent_disabled: Color32,

    pub success_fg: Color32,

    pub success_bg: Color32,

    pub menu_bg: Color32,

    pub empty_card_bg: Color32,

    pub caption_hover: Color32,

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
    focus_stroke: Color32::from_black_alpha(0xA6),
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
        focus_stroke: Color32::from_white_alpha(0xC0),
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

pub fn palette(ctx: &Context) -> Palette {
    match ctx.theme() {
        Theme::Light => LIGHT,
        Theme::Dark => dark(),
    }
}

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

fn apply_native_controls(style: &mut Style, p: &Palette) {
    let s = &mut style.spacing;
    s.scroll.bar_width = SCROLL_BAR_WIDTH;
    s.scroll.floating_width = SCROLL_BAR_FLOATING_WIDTH;

    s.scroll.bar_outer_margin = -SCROLL_BAR_OUTER_OFFSET;
    s.scroll.active_handle_opacity = SCROLL_BAR_OPACITY;
    s.scroll.interact_handle_opacity = SCROLL_BAR_OPACITY;

    s.scroll.dormant_handle_opacity = SCROLL_BAR_OPACITY;
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

    v.widgets.inactive.weak_bg_fill = p.control_bg;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text_secondary);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.separator);
    v.widgets.inactive.corner_radius = CONTROL_RADIUS.into();

    v.widgets.hovered.bg_fill = p.hover_bg;
    v.widgets.hovered.weak_bg_fill = p.control_hover;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.separator);
    v.widgets.hovered.corner_radius = CONTROL_RADIUS.into();

    v.widgets.active.bg_fill = p.pressed_bg;
    v.widgets.active.weak_bg_fill = p.control_pressed;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
    v.widgets.active.corner_radius = CONTROL_RADIUS.into();

    v.widgets.open.bg_fill = p.hover_bg;
    v.widgets.open.weak_bg_fill = p.control_hover;
    v.widgets.open.corner_radius = CONTROL_RADIUS.into();

    v.selection.bg_fill = selection();
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
}

fn apply_type_scale(style: &mut Style) {
    let ts = &mut style.text_styles;
    ts.insert(TextStyle::Heading, FontId::proportional(16.0));
    ts.insert(TextStyle::Body, FontId::proportional(14.0));
    ts.insert(TextStyle::Button, FontId::proportional(14.0));
    ts.insert(TextStyle::Small, FontId::proportional(12.0));
    ts.insert(TextStyle::Monospace, FontId::monospace(13.0));
}

#[derive(Clone, Copy)]
pub enum TextRole {

    Caption,

    Compact,

    #[expect(dead_code)]
    EmptyTitle,

    #[expect(dead_code)]
    PageTitle,

    #[expect(dead_code)]
    EmptyIcon,
}

pub fn role_size(role: TextRole) -> f32 {
    match role {
        TextRole::Caption => 11.0,
        TextRole::Compact => 13.0,
        TextRole::EmptyTitle => 18.0,
        TextRole::PageTitle => 20.0,
        TextRole::EmptyIcon => 23.0,
    }
}

pub fn font(role: TextRole) -> FontId {
    FontId::proportional(role_size(role))
}

pub const FONT_SMALL: f32 = 12.0;

pub fn font_small() -> FontId {
    FontId::proportional(FONT_SMALL)
}

pub fn font_body() -> FontId {
    FontId::proportional(14.0)
}

pub fn font_mono_small() -> FontId {
    FontId::monospace(12.0)
}

pub fn font_title(large: bool) -> FontId {
    FontId::new(
        if large { 16.0 } else { 14.0 },
        FontFamily::Name("brand".into()),
    )
}

pub fn font_page_title() -> FontId {
    FontId::new(20.0, FontFamily::Name("brand".into()))
}

pub fn font_header() -> FontId {
    FontId::new(11.0, FontFamily::Name("brand".into()))
}

pub fn font_brand_small() -> FontId {
    FontId::new(12.0, FontFamily::Name("brand".into()))
}

pub fn font_brand_compact() -> FontId {
    FontId::new(13.0, FontFamily::Name("brand".into()))
}

pub fn font_empty_title() -> FontId {
    FontId::new(18.0, FontFamily::Name("brand".into()))
}

pub fn font_mono_caption() -> FontId {
    FontId::monospace(11.0)
}

pub fn font_result_glyph() -> FontId {
    FontId::proportional(20.0)
}

fn apply_spacing(style: &mut Style) {
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 4.0);
    s.button_padding = egui::vec2(11.0, 5.0);
}

pub(crate) fn resolve_dark(theme: &str, system_light: bool) -> bool {
    match theme {
        "Dark" => true,
        "Light" => false,
        _ => !system_light,
    }
}

#[cfg(windows)]
pub(crate) fn system_prefers_light() -> bool {
    unsafe extern "system" {
        fn RegGetValueW(
            hkey: isize,
            lpsubkey: *const u16,
            lpvalue: *const u16,
            dwflags: u32,
            pdwtype: *mut u32,
            pvdata: *mut std::ffi::c_void,
            pcbdata: *mut u32,
        ) -> i32;
    }
    const HKEY_CURRENT_USER: isize = 0x8000_0001;
    const RRF_RT_REG_DWORD: u32 = 0x0010;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    let key = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<u16>>();
    let value = "AppsUseLightTheme"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<u16>>();
    let mut data: u32 = 0;
    let mut size = 4u32;

    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    match code {
        ERROR_SUCCESS => data == 1,
        ERROR_FILE_NOT_FOUND => true,
        _ => false,
    }
}

#[cfg(not(windows))]
pub(crate) fn system_prefers_light() -> bool {
    false
}

#[cfg(test)]
mod theme_key_tests {
    use super::resolve_dark;

    #[test]
    fn resolve_dark_matches_csharp() {
        assert!(resolve_dark("Dark", false));
        assert!(resolve_dark("Dark", true));
        assert!(!resolve_dark("Light", false));
        assert!(!resolve_dark("Light", true));

        assert!(resolve_dark("", false));
        assert!(!resolve_dark("", true));
        assert!(resolve_dark("whatever", false));
        assert!(!resolve_dark("whatever", true));
    }
}

#[cfg(test)]
mod font_token_guard {

    const CTORS: [&str; 3] = [
        "FontId::proportional(",
        "FontId::monospace(",
        "FontId::new(",
    ];

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read src dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                collect_rs_files(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }

    fn strip_line_comment(line: &str) -> &str {
        let bytes = line.as_bytes();
        let mut quotes = 0usize;
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'"' {
                quotes += 1;
            }
            if bytes[i] == b'/' && bytes[i + 1] == b'/' && quotes % 2 == 0 {
                return &line[..i];
            }
            i += 1;
        }
        line
    }

    fn starts_with_number(s: &str) -> bool {
        s.trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == '.')
    }

    #[test]
    fn theme_literals_all_use_tokens() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let src = std::path::Path::new(manifest).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);

        assert!(
            files.len() > 10,
            "守卫哨兵：只扫到 {} 个 .rs，扫描器已失灵",
            files.len()
        );

        let mut hits: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for f in &files {

            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "theme.rs" || name == "editor_window.rs" {
                continue;
            }
            scanned += 1;
            let rel = f.strip_prefix(&src).unwrap_or(f).display().to_string();
            let body = std::fs::read_to_string(f).expect("read source");
            let code_lines: Vec<&str> = body.lines().map(strip_line_comment).collect();

            let mut pending = false;
            for (i, &code) in code_lines.iter().enumerate() {
                if pending {
                    let t = code.trim_start();
                    if !t.is_empty() {
                        if starts_with_number(t) {
                            hits.push(format!(
                                "{rel}:{}: FontId 构造子首参跨行数字字面（走 theme:: 具名字号）",
                                i + 1
                            ));
                        }
                        pending = false;
                    }
                }
                for ctor in CTORS {
                    let mut from = 0;
                    while let Some(pos) = code[from..].find(ctor) {
                        let rest = code[from + pos + ctor.len()..].trim_start();
                        if rest.is_empty() {
                            pending = true;
                        } else if starts_with_number(rest) {
                            hits.push(format!(
                                "{rel}:{}: {}数字字面（走 theme::font / TextRole 具名令牌）",
                                i + 1,
                                ctor
                            ));
                        }
                        from += pos + ctor.len();
                    }
                }
            }

            for (i, &code) in code_lines.iter().enumerate() {
                let mut from = 0;
                while let Some(pos) = code[from..].find(".text_styles") {
                    let mut window = code[from + pos..].to_owned();
                    let mut hit = false;
                    for next in code_lines.iter().skip(i + 1).take(8) {
                        window.push('\n');
                        window.push_str(next);
                        if window.contains(".insert(") {
                            hit = true;
                            break;
                        }
                        if window.contains(';') {
                            break;
                        }
                    }
                    if hit {
                        hits.push(format!(
                            "{rel}:{}: text_styles 局部覆盖（TextStyle 统一走 theme::apply_type_scale，Body 恒 14）",
                            i + 1
                        ));
                    }
                    from += pos + ".text_styles".len();
                }
            }
        }

        assert!(
            scanned > 10,
            "守卫哨兵：实扫 {} 个文件（共 {}），豁免名单异常",
            scanned,
            files.len()
        );

        assert!(
            hits.is_empty(),
            "UI 字面量禁词 {} 条（字号/字色一律走 theme:: 具名令牌，见 docs/theme-tokens-csharp.md §8）：\n{}",
            hits.len(),
            hits.join("\n")
        );
    }
}

#[cfg(test)]
mod focus_stroke_tests {
    use super::{LIGHT, dark};
    use egui::Color32;

    fn blend_over(fg: Color32, bg: Color32) -> Color32 {
        let a = fg.a() as f32 / 255.0;
        let mix = |s: u8, d: u8| (s as f32 + d as f32 * (1.0 - a)).round() as u8;
        Color32::from_rgb(
            mix(fg.r(), bg.r()),
            mix(fg.g(), bg.g()),
            mix(fg.b(), bg.b()),
        )
    }

    fn rel_lum(c: Color32) -> f64 {
        fn ch(v: u8) -> f64 {
            let v = v as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (l1, l2) = (rel_lum(a), rel_lum(b));
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn focus_stroke_contrast_on_accent() {
        for (name, fg) in [("light", LIGHT.focus_stroke), ("dark", dark().focus_stroke)] {
            let seen = blend_over(fg, super::ACCENT);
            let c = contrast(seen, super::ACCENT);
            assert!(
                c >= 3.0,
                "{name}: 焦点框在 ACCENT 主底上对比度 {c:.2} < 3.0"
            );
        }

        assert!(contrast(blend_over(Color32::BLACK, super::ACCENT), super::ACCENT) >= 3.0);
        assert!(contrast(blend_over(Color32::WHITE, super::ACCENT), super::ACCENT) >= 3.0);
    }

    #[test]
    fn focus_stroke_is_not_opaque() {
        assert!(LIGHT.focus_stroke.a() < 255, "亮档焦点框仍是纯色不透明");
        assert!(dark().focus_stroke.a() < 255, "暗档焦点框仍是纯色不透明");
    }
}
