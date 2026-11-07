//! Windows 系统真实文件图标（C# SystemIconService 1:1，仅文件列表条目使用）。
//!
//! 链路：SHGetFileInfoW(SHGFI_ICON | SHGFI_LARGEICON) → HICON → GetIconInfo
//! → GetDIBits 32bpp 顶行在前 → 掩码 α 修复 → BGRA→RGBA 上传纹理。
//! α 直通不预乘：GetDIBits 输出本就是非预乘，egui `from_rgba_unmultiplied`
//! 恰好需要非预乘——C# XAML 走 SoftwareBitmap(Premultiplied) 的预乘步不迁移。
//!
//! 缓存键（C# GetBitmap）：盘符根（"C:\"）与自带图标的本地文件（exe/dll/ico/
//! msi/scr/cpl）按完整路径 "path:<大写路径>"；目录 "dir"；其余文件
//! "ext:<小写扩展名>"。取不到也缓存（None），避免反复失败查询。
//!
//! 本地条目传完整路径（文件真实存在，壳层按真实属性解析）；远程条目只传
//! 文件名并带 SHGFI_USEFILEATTRIBUTES（按扩展名解析，文件无需存在）。
//! 任意一步失败返回 None，由调用方回退通用字形（C# 同）。查询均在 UI 线程。

use std::collections::HashMap;

use egui::{Context, TextureHandle};

/// 缓存封顶（C# BitmapCacheCapacity = 512 LRU；Rust 键空间天然有限——
/// 目录/扩展名/盘符/自带图标文件，超限防御性清空重建，语义等价）。
const CAP: usize = 512;

/// 自带图标的扩展名（C# _perFileIconExtensions）：按完整路径取与缓存。
const PER_FILE_EXTS: [&str; 6] = [".exe", ".dll", ".ico", ".msi", ".scr", ".cpl"];

/// 系统图标纹理缓存。None = 取不到，也记住（C# null 缓存同语义）。
#[derive(Default)]
pub struct Cache {
    map: HashMap<String, Option<TextureHandle>>,
}

impl Cache {
    /// 取系统图标定尺寸纹理。is_local=条目来自本地文件系统；远程按扩展名解析。
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
        // 键：盘符根目录（各盘图标不同）与自带图标的本地文件按完整路径；其余按扩展名。
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

/// 扩展名（带点小写；无扩展名 / 以点结尾返回空，C# Path.GetExtension 同语义）。
fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => name[i..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// 由纹理句柄构造定尺寸 Image（同 icons::sized：0.37 需要 SizedTexture）。
fn sized(handle: &TextureHandle, size: f32) -> egui::Image<'static> {
    egui::Image::new(egui::load::SizedTexture::new(
        handle.id(),
        handle.size_vec2(),
    ))
    .fit_to_exact_size(egui::vec2(size, size))
}

/// SHGetFileInfo → HICON → 32bpp DIB → 纹理；任意一步失败 None（回字形）。
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

    // 本地条目传完整路径；远程条目传文件名并按属性解析（C# QueryBitmap 调用方约定）。
    let path = if is_local { full_path } else { name };
    let mut wide: Vec<u16> = path.encode_utf16().collect();
    wide.push(0);

    let mut flags = SHGFI_ICON; // SHGFI_LARGEICON = 0，无需并
    let attrs = if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    if !is_local {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }

    let mut sfi = ShFileInfo::default();
    // SAFETY: sfi 为合法可写缓冲，cb 传其字节数；path 为 NUL 结尾宽字符串。
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
    // SAFETY: hIcon 为 SHGetFileInfoW 返回的裸句柄，用完显式销毁（C# DestroyIcon 同）。
    unsafe { DestroyIcon(sfi.h_icon) };
    out
}

/// HICON → ICONINFO 色位图 → GetDIBits 32bpp 顶行在前 → α 修复 → 纹理。
#[cfg(windows)]
fn hicon_to_texture(ctx: &Context, hicon: usize) -> Option<TextureHandle> {
    let mut info = IconInfo::default();
    // SAFETY: info 为可写缓冲；GetIconInfo 成功时回填两个位图句柄，
    // 用完需 DeleteObject（对齐 C# ICONINFO.Dispose）。
    let ok = unsafe { GetIconInfo(hicon, &mut info) };
    if ok == 0 {
        return None;
    }
    let out = decode_dib(ctx, info.hbm_color);
    // SAFETY: 位图句柄由 GetIconInfo 回填，统一删除（掩码位图同样要删）。
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

/// GetDIBits 两次调用（第一次不取像素、由 GDI 回填真实宽高，随进程 DPI
/// "大图标"不一定是 32×32；第二次取 32bpp 顶行在前像素）→ α 修复 → BGRA→RGBA。
#[cfg(windows)]
fn decode_dib(ctx: &Context, hbm: usize) -> Option<TextureHandle> {
    if hbm == 0 {
        return None; // 纯掩码单色图标（色位图为空）→ 字形兜底（C# 同）
    }
    const DIB_RGB_COLORS: u32 = 0;
    // SAFETY: GetDC(null) 取屏幕 DC，用完 ReleaseDC。
    let hdc = unsafe { GetDC(0) };
    let mut bmi = BitmapInfo::default();
    bmi.header.bi_size = std::mem::size_of::<BitmapInfoHeader>() as u32;
    // 第一次调用 lpvBits=null：GDI 回填真实宽高。
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
        bmi.header.bi_height = -h; // 负高 = 顶行在前，与 egui 像素序一致
        bmi.header.bi_bit_count = 32;
        bmi.header.bi_compression = 0; // BI_RGB
        let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
        // SAFETY: px 为 width*height*4 字节可写缓冲，与请求行数匹配。
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
            // 旧式掩码图标全 α=0 → 视为全不透明（C# HiconToBitmap 同）。
            let has_alpha = px.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
            if !has_alpha {
                for p in px.as_chunks_mut::<4>().0 {
                    p[3] = 255;
                }
            }
            // GetDIBits 输出 BGRA → egui 需要 RGBA（非预乘）。
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
    // SAFETY: 屏幕 DC 用完即还。
    unsafe { ReleaseDC(0, hdc) };
    out
}

/// SHFILEINFOW（只消费 hIcon；其余字段占位保证与 API 布局一致）。
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

/// ICONINFO（位图句柄用 usize 占位便于 Default；GetIconInfo 回填）。
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

/// BITMAPINFOHEADER（GetDIBits 第一次调用回填真实宽高）。
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

/// BITMAPINFO（bmiColors 占位；32bpp BI_RGB 不用调色板，多留余量）。
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 3],
}

// 手写 FFI（零依赖，先例 local.rs GetLogicalDrives）：壳层图标链所需最小集。
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

/// 非 Windows 无壳层图标，恒 None（C# OperatingSystem.IsWindows 分支同）。
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
