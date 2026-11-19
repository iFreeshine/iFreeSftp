use std::collections::HashMap;

use egui::{Context, TextureHandle};

const CAP: usize = 512;

const PER_FILE_EXTS: [&str; 6] = [".exe", ".dll", ".ico", ".msi", ".scr", ".cpl"];

#[derive(Default)]
pub struct Cache {
    map: HashMap<String, Option<TextureHandle>>,
}

impl Cache {

    pub fn get(
        &mut self,
        ctx: &Context,
        is_dir: bool,
        name: &str,
        full_path: &str,
        is_local: bool,
        size: f32,
    ) -> Option<egui::Image<'static>> {
        let ext = ext_of(name);

        let per_path = is_local
            && (full_path.ends_with(":\\") || (!is_dir && PER_FILE_EXTS.contains(&ext.as_str())));
        let key = if per_path {
            format!("path:{}", full_path.to_uppercase())
        } else if is_dir {
            "dir".to_owned()
        } else {
            format!("ext:{ext}")
        };
        if let Some(cached) = self.map.get(&key) {
            return cached.as_ref().map(|h| sized(h, size));
        }
        let handle = query_texture(ctx, is_dir, full_path, name, is_local);
        self.map.insert(key, handle.clone());
        if self.map.len() > CAP {
            self.map.clear();
        }
        handle.map(|h| sized(&h, size))
    }
}

fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => name[i..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn sized(handle: &TextureHandle, size: f32) -> egui::Image<'static> {
    egui::Image::new(egui::load::SizedTexture::new(
        handle.id(),
        handle.size_vec2(),
    ))
    .fit_to_exact_size(egui::vec2(size, size))
}

#[cfg(windows)]
fn query_texture(
    ctx: &Context,
    is_dir: bool,
    full_path: &str,
    name: &str,
    is_local: bool,
) -> Option<TextureHandle> {
    const SHGFI_ICON: u32 = 0x100;
    const SHGFI_USEFILEATTRIBUTES: u32 = 0x10;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

    let path = if is_local { full_path } else { name };
    let mut wide: Vec<u16> = path.encode_utf16().collect();
    wide.push(0);

    let mut flags = SHGFI_ICON;
    let attrs = if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    if !is_local {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }

    let mut sfi = ShFileInfo::default();

    let ok = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut sfi,
            std::mem::size_of::<ShFileInfo>() as u32,
            flags,
        )
    };
    if ok == 0 || sfi.h_icon == 0 {
        return None;
    }
    let out = hicon_to_texture(ctx, sfi.h_icon);

    unsafe { DestroyIcon(sfi.h_icon) };
    out
}

#[cfg(windows)]
fn hicon_to_texture(ctx: &Context, hicon: usize) -> Option<TextureHandle> {
    let mut info = IconInfo::default();

    let ok = unsafe { GetIconInfo(hicon, &mut info) };
    if ok == 0 {
        return None;
    }
    let out = decode_dib(ctx, info.hbm_color);

    unsafe {
        if info.hbm_mask != 0 {
            DeleteObject(info.hbm_mask);
        }
        if info.hbm_color != 0 {
            DeleteObject(info.hbm_color);
        }
    }
    out
}

#[cfg(windows)]
fn decode_dib(ctx: &Context, hbm: usize) -> Option<TextureHandle> {
    if hbm == 0 {
        return None;
    }
    const DIB_RGB_COLORS: u32 = 0;

    let hdc = unsafe { GetDC(0) };
    let mut bmi = BitmapInfo::default();
    bmi.header.bi_size = std::mem::size_of::<BitmapInfoHeader>() as u32;

    let ok = unsafe {
        GetDIBits(
            hdc,
            hbm,
            0,
            0,
            std::ptr::null_mut(),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    let (w, h) = (bmi.header.bi_width, bmi.header.bi_height.abs());
    let mut out = None;
    if ok != 0 && w > 0 && h > 0 {
        bmi.header.bi_height = -h;
        bmi.header.bi_bit_count = 32;
        bmi.header.bi_compression = 0;
        let mut px = vec![0u8; (w as usize) * (h as usize) * 4];

        let rows = unsafe {
            GetDIBits(
                hdc,
                hbm,
                0,
                h as u32,
                px.as_mut_ptr(),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };
        if rows == h {

            let has_alpha = px.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
            if !has_alpha {
                for p in px.as_chunks_mut::<4>().0 {
                    p[3] = 255;
                }
            }

            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            out = Some(ctx.load_texture(
                "sysicon",
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px),
                Default::default(),
            ));
        }
    }

    unsafe { ReleaseDC(0, hdc) };
    out
}

#[cfg(windows)]
#[repr(C)]
struct ShFileInfo {
    h_icon: usize,
    i_icon: i32,
    dw_attributes: u32,
    sz_display_name: [u16; 260],
    sz_type_name: [u16; 80],
}

#[cfg(windows)]
impl Default for ShFileInfo {
    fn default() -> Self {
        Self {
            h_icon: 0,
            i_icon: 0,
            dw_attributes: 0,
            sz_display_name: [0; 260],
            sz_type_name: [0; 80],
        }
    }
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct IconInfo {
    f_icon: i32,
    x_hotspot: u32,
    y_hotspot: u32,
    hbm_mask: usize,
    hbm_color: usize,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 3],
}

#[cfg(windows)]
#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetFileInfoW(
        path: *const u16,
        attrs: u32,
        sfi: *mut ShFileInfo,
        cb: u32,
        flags: u32,
    ) -> usize;
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn DestroyIcon(hicon: usize) -> i32;
    fn GetIconInfo(hicon: usize, info: *mut IconInfo) -> i32;
    fn GetDC(hwnd: usize) -> usize;
    fn ReleaseDC(hwnd: usize, hdc: usize) -> i32;
}

#[cfg(windows)]
#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetDIBits(
        hdc: usize,
        hbm: usize,
        start: u32,
        lines: u32,
        bits: *mut u8,
        bmi: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
    fn DeleteObject(obj: usize) -> i32;
}

#[cfg(not(windows))]
fn query_texture(
    _ctx: &Context,
    _is_dir: bool,
    _full_path: &str,
    _name: &str,
    _is_local: bool,
) -> Option<TextureHandle> {
    None
}
