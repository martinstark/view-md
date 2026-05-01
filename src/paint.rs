use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use tiny_skia::{Color as SkColor, Pixmap, PremultipliedColorU8};

use crate::text::FONT_SANS;

pub struct Painter {
    pub fs: FontSystem,
    pub swash: SwashCache,
}

impl Painter {
    pub fn new(fs: FontSystem) -> Self {
        Self {
            fs,
            swash: SwashCache::new(),
        }
    }

    pub fn paint_placeholder(&mut self, pixmap: &mut Pixmap, dark: bool) {
        let (bg, fg) = if dark {
            (
                SkColor::from_rgba8(0x0d, 0x11, 0x17, 0xff),
                Color::rgb(0xe6, 0xed, 0xf3),
            )
        } else {
            (
                SkColor::from_rgba8(0xff, 0xff, 0xff, 0xff),
                Color::rgb(0x1f, 0x23, 0x28),
            )
        };
        pixmap.fill(bg);

        let metrics = Metrics::new(28.0, 36.0);
        let mut buf = Buffer::new(&mut self.fs, metrics);
        buf.set_size(
            &mut self.fs,
            Some(pixmap.width() as f32),
            Some(pixmap.height() as f32),
        );
        let attrs = Attrs::new().family(Family::Name(FONT_SANS));
        buf.set_text(&mut self.fs, "mdv", attrs, Shaping::Advanced);
        buf.shape_until_scroll(&mut self.fs, false);

        let text_w: f32 = buf
            .layout_runs()
            .map(|r| r.line_w)
            .fold(0.0_f32, f32::max);
        let text_h = metrics.line_height;
        let ox = (pixmap.width() as f32 - text_w) / 2.0;
        let oy = (pixmap.height() as f32 - text_h) / 2.0;

        draw_buffer(pixmap, &buf, &mut self.fs, &mut self.swash, ox, oy, fg);
    }
}

pub fn draw_buffer(
    pixmap: &mut Pixmap,
    buf: &Buffer,
    fs: &mut FontSystem,
    swash: &mut SwashCache,
    ox: f32,
    oy: f32,
    color: Color,
) {
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    buf.draw(fs, swash, color, |x, y, w, h, c| {
        if c.a() == 0 || w == 0 || h == 0 {
            return;
        }
        let px = x as f32 + ox;
        let py = y as f32 + oy;
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                let fx = px as i32 + dx;
                let fy = py as i32 + dy;
                if fx < 0 || fy < 0 || fx >= pw || fy >= ph {
                    continue;
                }
                blend_pixel(pixmap, fx as u32, fy as u32, c);
            }
        }
    });
}

fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, c: Color) {
    let w = pixmap.width();
    let idx = ((y * w + x) * 4) as usize;
    let data = pixmap.data_mut();
    let dst_r = data[idx];
    let dst_g = data[idx + 1];
    let dst_b = data[idx + 2];
    let dst_a = data[idx + 3];

    let sr = c.r();
    let sg = c.g();
    let sb = c.b();
    let sa = c.a();
    let inv = 255 - sa as u16;

    let r = (sr as u16 + (dst_r as u16 * inv) / 255) as u8;
    let g = (sg as u16 + (dst_g as u16 * inv) / 255) as u8;
    let b = (sb as u16 + (dst_b as u16 * inv) / 255) as u8;
    let a = (sa as u16 + (dst_a as u16 * inv) / 255) as u8;

    let pre = PremultipliedColorU8::from_rgba(
        ((r as u16 * a as u16) / 255) as u8,
        ((g as u16 * a as u16) / 255) as u8,
        ((b as u16 * a as u16) / 255) as u8,
        a,
    )
    .unwrap_or_else(|| PremultipliedColorU8::from_rgba(0, 0, 0, 0).unwrap());
    data[idx] = pre.red();
    data[idx + 1] = pre.green();
    data[idx + 2] = pre.blue();
    data[idx + 3] = pre.alpha();
}

pub fn pixmap_to_softbuffer(pixmap: &Pixmap, buffer: &mut [u32]) {
    let data = pixmap.data();
    for (i, px) in buffer.iter_mut().enumerate() {
        let off = i * 4;
        let r = data[off] as u32;
        let g = data[off + 1] as u32;
        let b = data[off + 2] as u32;
        *px = (r << 16) | (g << 8) | b;
    }
}
