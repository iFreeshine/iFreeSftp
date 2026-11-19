use std::collections::HashMap;

use egui::{Color32, Context, TextureHandle};

include!(concat!(env!("OUT_DIR"), "/icons_gen.rs"));

#[derive(Default)]
pub struct Cache {
    map: HashMap<String, TextureHandle>,
}

impl Cache {

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

fn sized(handle: &TextureHandle, size: f32) -> egui::Image<'static> {
    egui::Image::new(egui::load::SizedTexture::new(
        handle.id(),
        handle.size_vec2(),
    ))
    .fit_to_exact_size(egui::vec2(size, size))
}
