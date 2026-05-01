use cosmic_text::{Buffer, Color, FontSystem, SwashCache};
use tiny_skia::{Color as SkColor, Pixmap, PremultipliedColorU8, Rect, Transform};

use crate::layout::{LaidBlock, LaidDoc, LaidKind, UnderlineRun};
use crate::theme::Theme;

pub struct Painter {
    pub fs: FontSystem,
    pub swash: SwashCache,
}

impl Painter {
    pub fn new(fs: FontSystem) -> Self {
        Self { fs, swash: SwashCache::new() }
    }

    pub fn paint_doc(
        &mut self,
        pixmap: &mut Pixmap,
        doc: &LaidDoc,
        theme: &Theme,
        scroll_y: f32,
    ) {
        pixmap.fill(theme.bg);
        let h = pixmap.height() as f32;

        for block in &doc.blocks {
            let by = block.y - scroll_y;
            if by + block.h < 0.0 || by > h {
                continue;
            }
            paint_block(pixmap, block, by, theme, &mut self.fs, &mut self.swash);
        }
    }

    pub fn paint_blank(&mut self, pixmap: &mut Pixmap, theme: &Theme) {
        pixmap.fill(theme.bg);
    }
}

fn paint_block(
    pixmap: &mut Pixmap,
    block: &LaidBlock,
    y: f32,
    theme: &Theme,
    fs: &mut FontSystem,
    swash: &mut SwashCache,
) {
    match &block.kind {
        LaidKind::Text { buffer, color, underlines, strikes, .. } => {
            draw_buffer(pixmap, buffer, fs, swash, block.x, y, *color);
            for u in underlines {
                draw_run_lines(pixmap, buffer, block.x, y, u, *color, LinePos::Underline);
            }
            for s in strikes {
                draw_run_lines(pixmap, buffer, block.x, y, s, *color, LinePos::Strike);
            }
        }
        LaidKind::Rule => {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(theme.rule);
            paint.anti_alias = false;
            let w = pixmap.width() as f32 - block.x * 2.0;
            if let Some(rect) = Rect::from_xywh(block.x, y, w, 1.0) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
        LaidKind::Bar { color, width } => {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(*color);
            paint.anti_alias = false;
            if let Some(rect) = Rect::from_xywh(block.x, y, *width, block.h) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
        LaidKind::TaskBox { checked, color, fg } => {
            let mut border = tiny_skia::Paint::default();
            border.set_color(*color);
            border.anti_alias = true;
            let size = block.h;
            // border (outline)
            if let Some(rect) = Rect::from_xywh(block.x, y, size, size) {
                pixmap.fill_rect(rect, &border, Transform::identity(), None);
            }
            // inner clear
            let inset = 1.0;
            let mut bg = tiny_skia::Paint::default();
            bg.set_color(theme.bg);
            if let Some(rect) = Rect::from_xywh(
                block.x + inset,
                y + inset,
                size - inset * 2.0,
                size - inset * 2.0,
            ) {
                pixmap.fill_rect(rect, &bg, Transform::identity(), None);
            }
            if *checked {
                let mut chk = tiny_skia::Paint::default();
                chk.set_color(*fg);
                chk.anti_alias = true;
                let pad = size * 0.20;
                if let Some(rect) =
                    Rect::from_xywh(block.x + pad, y + pad, size - pad * 2.0, size - pad * 2.0)
                {
                    pixmap.fill_rect(rect, &chk, Transform::identity(), None);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum LinePos {
    Underline,
    Strike,
}

fn draw_run_lines(
    pixmap: &mut Pixmap,
    buf: &Buffer,
    ox: f32,
    oy: f32,
    range: &UnderlineRun,
    color: Color,
    pos: LinePos,
) {
    let line_height = buf.metrics().line_height;
    for run in buf.layout_runs() {
        let mut x_start: Option<f32> = None;
        let mut x_end: Option<f32> = None;
        for g in run.glyphs.iter() {
            if g.end <= range.byte_start || g.start >= range.byte_end {
                continue;
            }
            let gx0 = g.x;
            let gx1 = g.x + g.w;
            x_start = Some(x_start.map(|s| s.min(gx0)).unwrap_or(gx0));
            x_end = Some(x_end.map(|s| s.max(gx1)).unwrap_or(gx1));
        }
        if let (Some(xs), Some(xe)) = (x_start, x_end) {
            let baseline_y = run.line_y;
            let underline_y = match pos {
                LinePos::Underline => baseline_y + 2.0,
                LinePos::Strike => baseline_y - line_height * 0.30,
            };
            fill_line(pixmap, ox + xs, oy + underline_y, xe - xs, 1.0, color);
        }
    }
}

fn fill_line(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Color) {
    let mut paint = tiny_skia::Paint::default();
    paint.set_color(SkColor::from_rgba8(c.r(), c.g(), c.b(), c.a()));
    paint.anti_alias = false;
    if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
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
        let bx = x + ox as i32;
        let by = y + oy as i32;
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                let fx = bx + dx;
                let fy = by + dy;
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
