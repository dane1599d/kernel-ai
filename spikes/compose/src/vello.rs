//! Scenes (a) and (b) drawn by vello_cpu 0.3.0 instead of `Desk::compose`: the
//! same wallpaper, the windows' layers as image paints in rounded rectangles,
//! and the focused window's shadow as vello's blurred rounded rectangle. Only the
//! damaged region is rasterised, by a context sized to it and translated.

use crate::desk::{Desk, px};
use crate::frame::{Frame, Rect};
use alloc::sync::Arc;
use alloc::vec::Vec;
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::kurbo::{self, Affine, RoundedRect, Shape};
use vello_cpu::peniko::ImageSampler;
use vello_cpu::{Image, ImageSource, Pixmap, RenderContext, Resources};

fn rgba(c: u32, a: u8) -> AlphaColor<Srgb> {
    AlphaColor::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, a)
}

/// A premultiplied ARGB layer as a vello pixmap (premultiplied RGBA8).
fn pixmap(f: &Frame) -> Arc<Pixmap> {
    let mut p = Pixmap::new(f.w as u16, f.h as u16);
    for (out, &v) in p
        .data_as_u8_slice_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(&f.px)
    {
        out.copy_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]);
    }
    Arc::new(p)
}

pub struct VelloDesk {
    layers: Vec<Arc<Pixmap>>,
    resources: Resources,
    pub out: Pixmap,
}

impl VelloDesk {
    pub fn new(desk: &Desk) -> VelloDesk {
        VelloDesk {
            layers: desk.windows.iter().map(|w| pixmap(&w.layer)).collect(),
            resources: Resources::new(),
            out: Pixmap::new(desk.frame.w as u16, desk.frame.h as u16),
        }
    }

    /// Rasterises `damage` of `desk`'s scene into a pixmap of its size.
    pub fn render(&mut self, desk: &Desk, damage: Rect) {
        let damage = damage.intersect(&desk.frame.bounds());
        if damage.is_empty() {
            return;
        }
        let mut ctx = RenderContext::new(damage.w as u16, damage.h as u16);
        ctx.set_transform(Affine::translate((
            f64::from(-damage.x),
            f64::from(-damage.y),
        )));
        let look = desk.look;
        ctx.set_paint(rgba(look.desk, 255));
        ctx.fill_rect(&kurbo::Rect::new(
            0.0,
            0.0,
            f64::from(desk.frame.w),
            f64::from(desk.frame.h),
        ));
        ctx.set_paint(rgba(look.window, 255));
        let bar = desk.topbar;
        ctx.fill_rect(&kurbo::Rect::new(
            0.0,
            0.0,
            f64::from(bar.w),
            f64::from(bar.h),
        ));
        let radius = f64::from(px(6.0, desk.scale));
        for (w, layer) in desk.windows.iter().zip(&self.layers) {
            let r = w.rect;
            let rect = kurbo::Rect::new(
                f64::from(r.x),
                f64::from(r.y),
                f64::from(r.right()),
                f64::from(r.bottom()),
            );
            if w.focused {
                ctx.set_paint(rgba(0, 120));
                ctx.fill_blurred_rounded_rect(
                    &rect,
                    radius as f32,
                    desk.shadow.m as f32 / 2.5,
                    false,
                );
            }
            ctx.set_paint_transform(Affine::translate((rect.x0, rect.y0)));
            ctx.set_paint(Image {
                image: ImageSource::Pixmap(layer.clone()),
                sampler: ImageSampler::default(),
            });
            ctx.fill_path(&RoundedRect::from_rect(rect, radius).to_path(0.1));
            ctx.reset_paint_transform();
        }
        ctx.set_paint(rgba(look.window, 255));
        let p = desk.panel;
        ctx.fill_rect(&kurbo::Rect::new(
            0.0,
            f64::from(p.y),
            f64::from(p.w),
            f64::from(p.bottom()),
        ));
        ctx.flush();
        let mut target = Pixmap::new(damage.w as u16, damage.h as u16);
        ctx.render(&mut target, &mut self.resources);
        self.out = target;
    }
}
