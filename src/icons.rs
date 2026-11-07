//! 图标运行时（D-014）：消费 build.rs 生成的内嵌 PNG 清单。
//! Phosphor 源图为黑色字形，按需重着色后上传纹理，按 (名字, 颜色) 缓存。
//!
//! 清晰度（密度桶）：build.rs 已把 256px 源 CPU Lanczos3 下采样为
//! 16/20/24/32/48 五档（SCALED），运行时按「逻辑 size × 像素比」取
//! ≥ 物理需求的最小档，GPU 近 1:1 采样不发糊；无合适档回退 256px 原图。

use std::collections::HashMap;

use egui::{Color32, Context, TextureHandle};

include!(concat!(env!("OUT_DIR"), "/icons_gen.rs"));

/// 图标纹理缓存。key = "{name}-{color}-{档位边长}"。
#[derive(Default)]
pub struct Cache {
    map: HashMap<String, TextureHandle>,
}

impl Cache {
    /// 取单色图标纹理（黑色源图重着色为 `color`，保留 alpha）。
    pub fn get(
        &mut self,
        ctx: &Context,
        name: &str,
        color: Color32,
        size: f32,
    ) -> Option<egui::Image<'static>> {
        let handle = self.texture(ctx, name, Some(color), size)?;
        Some(sized(&handle, size))
    }

    /// 取原色纹理（品牌 logo 等多色图）。
    pub fn get_raw(
        &mut self,
        ctx: &Context,
        name: &str,
        size: f32,
    ) -> Option<egui::Image<'static>> {
        let handle = self.texture(ctx, name, None, size)?;
        Some(sized(&handle, size))
    }

    fn texture(
        &mut self,
        ctx: &Context,
        name: &str,
        color: Option<Color32>,
        size: f32,
    ) -> Option<TextureHandle> {
        // 密度桶选档：物理需求 = 逻辑尺寸 × 像素比；取 ≥ 需求的最小档
        // （GPU 缩放比 ≤ ~1.25 的近无损下采样），无合适档回退 256px 原图。
        let phys = (size * ctx.pixels_per_point()).ceil().max(1.0);
        let (side, bytes) = pick_bucket(name, phys)?;
        let key = format!(
            "{name}-{}-{side}",
            color.map(|c| c.to_hex()).unwrap_or_default()
        );
        if let Some(t) = self.map.get(&key) {
            return Some(t.clone());
        }

        let img = image::load_from_memory(bytes).ok()?.to_rgba8();
        let dim = [img.width() as usize, img.height() as usize];
        let mut px = img.into_raw();
        if let Some(c) = color {
            for px4 in px.as_chunks_mut::<4>().0 {
                px4[0] = c.r();
                px4[1] = c.g();
                px4[2] = c.b();
            }
        }
        let handle = ctx.load_texture(
            format!("icon:{key}"),
            egui::ColorImage::from_rgba_unmultiplied(dim, &px),
            Default::default(),
        );
        self.map.insert(key, handle.clone());
        Some(handle)
    }
}

/// 选档：SCALED（16/20/24/32/48）中取 ≥ phys 的最小边长；否则 ENTRIES 256px 兜底。
fn pick_bucket(name: &str, phys: f32) -> Option<(u32, &'static [u8])> {
    let mut best: Option<(u32, &'static [u8])> = None;
    for (n, side, bytes) in SCALED {
        if *n != name {
            continue;
        }
        if (*side as f32) >= phys && best.is_none_or(|(b, _)| *side < b) {
            best = Some((*side, *bytes));
        }
    }
    best.or_else(|| {
        ENTRIES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| (256, *b))
    })
}

/// 由纹理句柄构造定尺寸 Image（0.37 需要 SizedTexture，位于 egui::load）。
fn sized(handle: &TextureHandle, size: f32) -> egui::Image<'static> {
    egui::Image::new(egui::load::SizedTexture::new(
        handle.id(),
        handle.size_vec2(),
    ))
    .fit_to_exact_size(egui::vec2(size, size))
}
