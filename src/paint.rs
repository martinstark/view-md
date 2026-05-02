use cosmic_text::{Buffer, Color, Cursor, FontSystem, SwashCache};
use tiny_skia::{Color as SkColor, FillRule, Path, PathBuilder, Pixmap, Rect, Transform};

use crate::doc::CellAlign;
use crate::layout::{LaidBlock, LaidDoc, LaidKind, TableCellLayout, TableRowLayout, UnderlineRun};
use crate::theme::Theme;

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

  pub fn paint_doc(&mut self, pixmap: &mut Pixmap, doc: &LaidDoc, theme: &Theme, scroll_y: f32) {
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

  pub fn paint_selection(
    &mut self,
    pixmap: &mut Pixmap,
    doc: &LaidDoc,
    sel: &crate::app::Selection,
    theme: &Theme,
    scroll_y: f32,
  ) {
    let (start, end) = sel.ordered();
    let bg = if theme.is_dark {
      SkColor::from_rgba8(0x40, 0x60, 0x90, 0x70)
    } else {
      SkColor::from_rgba8(0xa0, 0xc0, 0xff, 0x80)
    };

    if start.block_idx == end.block_idx {
      paint_block_selection(
        pixmap,
        &doc.blocks[start.block_idx],
        scroll_y,
        Some(&start.cursor),
        Some(&end.cursor),
        bg,
      );
      return;
    }
    paint_block_selection(
      pixmap,
      &doc.blocks[start.block_idx],
      scroll_y,
      Some(&start.cursor),
      None,
      bg,
    );
    for i in (start.block_idx + 1)..end.block_idx {
      paint_block_selection(pixmap, &doc.blocks[i], scroll_y, None, None, bg);
    }
    paint_block_selection(
      pixmap,
      &doc.blocks[end.block_idx],
      scroll_y,
      None,
      Some(&end.cursor),
      bg,
    );
  }

  pub fn paint_help_overlay(&mut self, pixmap: &mut Pixmap, theme: &Theme) {
    // Dim the doc behind
    let mut paint = tiny_skia::Paint::default();
    paint.set_color(SkColor::from_rgba8(0, 0, 0, 0x8c));
    paint.anti_alias = false;
    if let Some(rect) = Rect::from_xywh(0.0, 0.0, pixmap.width() as f32, pixmap.height() as f32) {
      pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }

    let entries: &[(&str, &str)] = &[
      ("q / Esc", "quit"),
      ("t", "toggle theme"),
      ("+ / -", "zoom in / out"),
      ("0", "reset zoom"),
      ("j / k", "line down / up"),
      ("d / u", "half page down / up"),
      ("f / b / Space", "full page down / up"),
      ("g / G", "top / bottom"),
      ("] / [", "next / prev heading"),
      ("} / {", "next / prev block"),
      ("y", "yank visible code block"),
      ("?", "toggle this help"),
    ];

    let card_w = 420.0_f32;
    let row_h = 22.0_f32;
    let pad = 28.0_f32;
    let title_h = 32.0_f32;
    let card_h = title_h + pad + entries.len() as f32 * row_h + pad;

    let cx = (pixmap.width() as f32 - card_w) / 2.0;
    let cy = (pixmap.height() as f32 - card_h) / 2.0;

    if let Some(path) = rounded_rect(cx, cy, card_w, card_h, 10.0) {
      let mut bg = tiny_skia::Paint::default();
      bg.set_color(if theme.is_dark {
        SkColor::from_rgba8(0x16, 0x1b, 0x22, 0xff)
      } else {
        SkColor::from_rgba8(0xff, 0xff, 0xff, 0xff)
      });
      bg.anti_alias = true;
      pixmap.fill_path(&path, &bg, FillRule::Winding, Transform::identity(), None);

      let mut border_paint = tiny_skia::Paint::default();
      border_paint.set_color(theme.border);
      border_paint.anti_alias = true;
      let stroke = tiny_skia::Stroke {
        width: 1.0,
        ..Default::default()
      };
      pixmap.stroke_path(&path, &border_paint, &stroke, Transform::identity(), None);
    }

    let title = crate::layout::make_plain_buffer(
      &mut self.fs,
      "KEYBINDS",
      12.0,
      14.0,
      card_w - pad * 2.0,
      crate::text::FONT_SANS,
    );
    draw_buffer(
      pixmap,
      &title,
      &mut self.fs,
      &mut self.swash,
      cx + pad,
      cy + pad - 6.0,
      theme.muted,
    );

    let mut row_y = cy + pad + title_h;
    for (key, desc) in entries {
      let key_buf = crate::layout::make_plain_buffer(
        &mut self.fs,
        key,
        13.0,
        row_h,
        160.0,
        crate::text::FONT_MONO,
      );
      let desc_buf = crate::layout::make_plain_buffer(
        &mut self.fs,
        desc,
        13.0,
        row_h,
        card_w - 200.0,
        crate::text::FONT_SANS,
      );
      draw_buffer(
        pixmap,
        &key_buf,
        &mut self.fs,
        &mut self.swash,
        cx + pad,
        row_y,
        theme.link,
      );
      draw_buffer(
        pixmap,
        &desc_buf,
        &mut self.fs,
        &mut self.swash,
        cx + pad + 170.0,
        row_y,
        theme.fg,
      );
      row_y += row_h;
    }
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
    LaidKind::Text {
      buffer,
      color,
      underlines,
      strikes,
      code_runs,
      ..
    } => {
      for c in code_runs {
        draw_run_pills(pixmap, buffer, block.x, y, c, theme.inline_code_bg);
      }
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
    LaidKind::TaskBox { checked } => paint_task_box(pixmap, block.x, y, block.h, *checked, theme),
    LaidKind::CodeBlock {
      buffer,
      bg,
      width,
      pad_x,
      pad_y,
      lang_label,
      lang_label_color,
      source: _,
    } => {
      if let Some(path) = rounded_rect(block.x, y, *width, block.h, 6.0) {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(*bg);
        paint.anti_alias = true;
        pixmap.fill_path(
          &path,
          &paint,
          FillRule::Winding,
          Transform::identity(),
          None,
        );
      }
      if let Some(label) = lang_label {
        let label_w = label
          .layout_runs()
          .map(|r| r.line_w)
          .fold(0.0_f32, f32::max);
        let lx = block.x + *width - label_w - *pad_x;
        let ly = y + 6.0;
        draw_buffer(pixmap, label, fs, swash, lx, ly, *lang_label_color);
      }
      draw_buffer(
        pixmap,
        buffer,
        fs,
        swash,
        block.x + *pad_x,
        y + *pad_y,
        cosmic_text::Color::rgb(0xe6, 0xed, 0xf3),
      );
    }
    LaidKind::Table {
      block_w,
      rows,
      border,
      header_bg,
      alt_bg: _,
    } => paint_table(
      pixmap, fs, swash, block.x, y, *block_w, rows, *border, *header_bg, theme,
    ),
  }
}

fn paint_table(
  pixmap: &mut Pixmap,
  fs: &mut FontSystem,
  swash: &mut SwashCache,
  x0: f32,
  y0: f32,
  block_w: f32,
  rows: &[TableRowLayout],
  border: SkColor,
  header_bg: SkColor,
  theme: &Theme,
) {
  let total_h = rows.last().map(|r| r.y_top + r.h).unwrap_or(0.0);

  // Header background
  if let Some(first) = rows.first() {
    if first.is_header {
      let mut paint = tiny_skia::Paint::default();
      paint.set_color(header_bg);
      paint.anti_alias = false;
      if let Some(rect) = Rect::from_xywh(x0, y0, block_w, first.h) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
      }
    }
  }

  // Outer border + horizontal lines
  let mut paint = tiny_skia::Paint::default();
  paint.set_color(border);
  paint.anti_alias = false;

  // top
  if let Some(rect) = Rect::from_xywh(x0, y0, block_w, 1.0) {
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
  }
  // bottom
  if let Some(rect) = Rect::from_xywh(x0, y0 + total_h - 1.0, block_w, 1.0) {
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
  }
  // between rows
  for r in rows.iter().skip(1) {
    if let Some(rect) = Rect::from_xywh(x0, y0 + r.y_top, block_w, 1.0) {
      pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
  }
  // left + right
  if let Some(rect) = Rect::from_xywh(x0, y0, 1.0, total_h) {
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
  }
  if let Some(rect) = Rect::from_xywh(x0 + block_w - 1.0, y0, 1.0, total_h) {
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
  }
  // vertical column lines
  if let Some(first) = rows.first() {
    for cell in first.cells.iter().skip(1) {
      if let Some(rect) = Rect::from_xywh(x0 + cell.x, y0, 1.0, total_h) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
      }
    }
  }

  // Cell content
  for r in rows.iter() {
    for c in r.cells.iter() {
      paint_table_cell(pixmap, fs, swash, x0, y0 + r.y_top, c, theme);
    }
  }
}

fn paint_table_cell(
  pixmap: &mut Pixmap,
  fs: &mut FontSystem,
  swash: &mut SwashCache,
  table_x0: f32,
  row_y0: f32,
  cell: &TableCellLayout,
  theme: &Theme,
) {
  // Use proportionally based on cell width: ~12 base or scaled.
  let pad_x = (cell.w * 0.04).clamp(6.0, 24.0);
  let pad_y = (pad_x * 0.7).max(6.0);

  let cell_text_w = (cell.w - pad_x * 2.0).max(0.0);
  let actual_text_w = cell
    .buffer
    .layout_runs()
    .map(|r| r.line_w)
    .fold(0.0_f32, f32::max);
  let extra = (cell_text_w - actual_text_w).max(0.0);
  let dx = match cell.align {
    CellAlign::Left => 0.0,
    CellAlign::Center => extra / 2.0,
    CellAlign::Right => extra,
  };
  let cx = table_x0 + cell.x + pad_x + dx;
  let cy = row_y0 + pad_y;

  for c in &cell.code_runs {
    draw_run_pills(pixmap, &cell.buffer, cx, cy, c, theme.inline_code_bg);
  }
  draw_buffer(pixmap, &cell.buffer, fs, swash, cx, cy, cell.color);
  for u in &cell.underlines {
    draw_run_lines(
      pixmap,
      &cell.buffer,
      cx,
      cy,
      u,
      cell.color,
      LinePos::Underline,
    );
  }
  for s in &cell.strikes {
    draw_run_lines(pixmap, &cell.buffer, cx, cy, s, cell.color, LinePos::Strike);
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
  let metrics = buf.metrics();
  let font_size = metrics.font_size;
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
        LinePos::Strike => baseline_y - font_size * 0.36,
      };
      let thickness = (font_size * 0.06).max(1.0).round();
      fill_line(pixmap, ox + xs, oy + underline_y, xe - xs, thickness, color);
    }
  }
}

fn draw_run_pills(
  pixmap: &mut Pixmap,
  buf: &Buffer,
  ox: f32,
  oy: f32,
  range: &UnderlineRun,
  bg: SkColor,
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
      let pad_x = 3.0;
      let pad_y = 1.0;
      let pill_top = run.line_top;
      let pill_h = line_height - 2.0;
      let mut paint = tiny_skia::Paint::default();
      paint.set_color(bg);
      paint.anti_alias = false;
      if let Some(rect) = Rect::from_xywh(
        ox + xs - pad_x,
        oy + pill_top + pad_y,
        (xe - xs) + pad_x * 2.0,
        pill_h - pad_y * 2.0,
      ) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
      }
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

fn paint_task_box(pixmap: &mut Pixmap, x: f32, y: f32, size: f32, checked: bool, theme: &Theme) {
  let outline = ct_to_sk(theme.muted);
  let stroke_w = (size * 0.12).max(1.5).round();

  // Always: outlined box with bg interior.
  let mut border_paint = tiny_skia::Paint::default();
  border_paint.set_color(outline);
  border_paint.anti_alias = true;
  if let Some(path) = rounded_rect(x, y, size, size, size * 0.18) {
    let stroke = tiny_skia::Stroke {
      width: stroke_w,
      ..Default::default()
    };
    pixmap.stroke_path(&path, &border_paint, &stroke, Transform::identity(), None);
  }

  if checked {
    let mut fill = tiny_skia::Paint::default();
    fill.set_color(ct_to_sk(theme.link));
    fill.anti_alias = true;
    let pad = size * 0.28;
    if let Some(rect) = Rect::from_xywh(x + pad, y + pad, size - pad * 2.0, size - pad * 2.0) {
      pixmap.fill_rect(rect, &fill, Transform::identity(), None);
    }
  }
}

fn ct_to_sk(c: Color) -> SkColor {
  SkColor::from_rgba8(c.r(), c.g(), c.b(), c.a())
}

fn paint_block_selection(
  pixmap: &mut Pixmap,
  block: &LaidBlock,
  scroll_y: f32,
  start: Option<&Cursor>,
  end: Option<&Cursor>,
  bg: SkColor,
) {
  let (buffer, ox, oy) = match &block.kind {
    LaidKind::Text { buffer, .. } => (buffer, block.x, block.y - scroll_y),
    LaidKind::CodeBlock {
      buffer,
      pad_x,
      pad_y,
      ..
    } => (buffer, block.x + *pad_x, block.y - scroll_y + *pad_y),
    _ => return,
  };
  let line_height = buffer.metrics().line_height;
  let lh = line_height;
  for run in buffer.layout_runs() {
    let line_idx = run.line_i;
    let after_start = start.map_or(true, |s| line_idx > s.line);
    let before_end = end.map_or(true, |e| line_idx < e.line);
    let on_start = start.map_or(false, |s| line_idx == s.line);
    let on_end = end.map_or(false, |e| line_idx == e.line);

    if !after_start && !on_start && !on_end {
      continue;
    }
    if !before_end && !on_end && !on_start {
      continue;
    }

    let x_start = if on_start {
      cursor_x_in_run(&run, start.unwrap().index).unwrap_or(0.0)
    } else {
      0.0
    };
    let x_end = if on_end {
      cursor_x_in_run(&run, end.unwrap().index).unwrap_or(run.line_w)
    } else {
      run.line_w.max(8.0)
    };
    let xs = x_start.min(x_end);
    let xe = x_start.max(x_end);
    if xe <= xs {
      continue;
    }

    let mut paint = tiny_skia::Paint::default();
    paint.set_color(bg);
    paint.anti_alias = false;
    if let Some(rect) = Rect::from_xywh(ox + xs, oy + run.line_top, xe - xs, lh) {
      pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
  }
}

fn cursor_x_in_run(run: &cosmic_text::LayoutRun, byte_idx: usize) -> Option<f32> {
  // Walk glyphs to find the x coordinate matching a byte offset within the run.
  if run.glyphs.is_empty() {
    return Some(0.0);
  }
  for g in run.glyphs.iter() {
    if byte_idx <= g.start {
      return Some(g.x);
    }
    if byte_idx <= g.end {
      // Mid-glyph: approximate by interpolation
      let span = (g.end - g.start).max(1);
      let frac = (byte_idx - g.start) as f32 / span as f32;
      return Some(g.x + g.w * frac);
    }
  }
  let last = run.glyphs.last().unwrap();
  Some(last.x + last.w)
}

pub(crate) fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
  let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
  let mut pb = PathBuilder::new();
  pb.move_to(x + r, y);
  pb.line_to(x + w - r, y);
  pb.quad_to(x + w, y, x + w, y + r);
  pb.line_to(x + w, y + h - r);
  pb.quad_to(x + w, y + h, x + w - r, y + h);
  pb.line_to(x + r, y + h);
  pb.quad_to(x, y + h, x, y + h - r);
  pb.line_to(x, y + r);
  pb.quad_to(x, y, x + r, y);
  pb.close();
  pb.finish()
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
  let ox = ox as i32;
  let oy = oy as i32;
  // 0.19's Buffer::draw takes &mut self because it auto-shapes; we
  // already shape at layout time, so iterate layout_runs directly with
  // an immutable reference and rasterize each glyph through swash.
  for run in buf.layout_runs() {
    for glyph in run.glyphs.iter() {
      let physical = glyph.physical((0., run.line_y), 1.0);
      let glyph_color = glyph.color_opt.unwrap_or(color);
      swash.with_pixels(fs, physical.cache_key, glyph_color, |x, y, c| {
        if c.a() == 0 {
          return;
        }
        let fx = physical.x + x + ox;
        let fy = physical.y + y + oy;
        if fx < 0 || fy < 0 || fx >= pw || fy >= ph {
          return;
        }
        blend_pixel(pixmap, fx as u32, fy as u32, c);
      });
    }
  }
}

#[inline]
fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, c: Color) {
  let sa = c.a() as u32;
  if sa == 0 {
    return;
  }

  let w = pixmap.width();
  let idx = ((y * w + x) * 4) as usize;
  let data = pixmap.data_mut();

  let dst_r = data[idx] as u32;
  let dst_g = data[idx + 1] as u32;
  let dst_b = data[idx + 2] as u32;
  let dst_a = data[idx + 3];

  let sr = c.r() as u32;
  let sg = c.g() as u32;
  let sb = c.b() as u32;
  let inv = 255 - sa;

  // cosmic-text delivers straight color with `alpha` as coverage:
  // src-over-dst is `src*a + dst*(1-a)`. Our pixmap is premultiplied
  // but the bg fill is opaque, so dst_r etc. read back as straight.
  let r = (sr * sa + dst_r * inv) / 255;
  let g = (sg * sa + dst_g * inv) / 255;
  let b = (sb * sa + dst_b * inv) / 255;

  if dst_a == 255 {
    // Fast path (~99% of glyph pixels): dst opaque, result opaque,
    // premultiplied storage == straight value.
    data[idx] = r as u8;
    data[idx + 1] = g as u8;
    data[idx + 2] = b as u8;
    return;
  }

  // General path: dst has partial alpha (overlay/help-card pixels).
  let a = (sa + (dst_a as u32 * inv) / 255).min(255);
  data[idx] = ((r * a) / 255) as u8;
  data[idx + 1] = ((g * a) / 255) as u8;
  data[idx + 2] = ((b * a) / 255) as u8;
  data[idx + 3] = a as u8;
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
