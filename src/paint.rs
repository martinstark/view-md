use cosmic_text::{Buffer, Color, Cursor, FontSystem, SwashCache, SwashContent};
use tiny_skia::{Color as SkColor, FillRule, Path, PathBuilder, Pixmap, Transform};

use crate::doc::CellAlign;
use crate::layout::{LaidBlock, LaidDoc, LaidKind, TableCellLayout, TableRowLayout, UnderlineRun};
use crate::theme::Theme;

/// u32-ARGB paint surface. Wraps the softbuffer slice directly so paint
/// writes go straight to the wl_shm-backed buffer with no BGRA→u32
/// conversion pass at the end. Format is `0x00RRGGBB` per pixel
/// (Wayland ignores the alpha byte). Coordinate origin is top-left.
pub struct Frame<'a> {
  pub data: &'a mut [u32],
  pub width: u32,
  pub height: u32,
}

impl<'a> Frame<'a> {
  pub fn new(data: &'a mut [u32], width: u32, height: u32) -> Self {
    debug_assert!(data.len() >= (width as usize) * (height as usize));
    Self {
      data,
      width,
      height,
    }
  }

  /// Solid-fill the entire buffer. Single u32 store loop — much cheaper
  /// than tiny-skia's `Pixmap::fill` because no premultiplication and
  /// no row-by-row Rect traversal.
  pub fn fill_solid(&mut self, color: u32) {
    self.data.fill(color);
  }

  /// Solid-color axis-aligned rect. Clips to frame bounds. No alpha.
  pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let fw = self.width as i32;
    let fh = self.height as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(fw);
    let y1 = (y + h).min(fh);
    if x0 >= x1 || y0 >= y1 {
      return;
    }
    let stride = self.width as usize;
    for ry in y0..y1 {
      let row_start = (ry as usize) * stride + (x0 as usize);
      let row_end = row_start + (x1 - x0) as usize;
      self.data[row_start..row_end].fill(color);
    }
  }

  /// Translucent axis-aligned rect fill. Blends `color_rgb` (top 24
  /// bits) onto the frame using `alpha` (0..=255) as coverage. Used for
  /// inline-code pills and selection rects whose theme colors carry a
  /// non-opaque alpha. Substantially cheaper than the `composite_pixmap`
  /// path for axis-aligned rects because no scratch allocation and no
  /// per-pixel premul reconstruction.
  pub fn fill_rect_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, color_rgb: u32, alpha: u8) {
    if alpha == 0 {
      return;
    }
    if alpha == 255 {
      self.fill_rect(x, y, w, h, color_rgb);
      return;
    }
    let fw = self.width as i32;
    let fh = self.height as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(fw);
    let y1 = (y + h).min(fh);
    if x0 >= x1 || y0 >= y1 {
      return;
    }
    let sa = alpha as u32;
    let inv = 255 - sa;
    // Pre-multiply src once, blend per-pixel.
    let sr = ((color_rgb >> 16) & 0xFF) * sa;
    let sg = ((color_rgb >> 8) & 0xFF) * sa;
    let sb = (color_rgb & 0xFF) * sa;
    let stride = self.width as usize;
    for ry in y0..y1 {
      let row_start = (ry as usize) * stride;
      for rx in x0..x1 {
        let idx = row_start + rx as usize;
        let dst = self.data[idx];
        let dr = (dst >> 16) & 0xFF;
        let dg = (dst >> 8) & 0xFF;
        let db = dst & 0xFF;
        let r = (sr + dr * inv) / 255;
        let g = (sg + dg * inv) / 255;
        let b = (sb + db * inv) / 255;
        self.data[idx] = (r << 16) | (g << 8) | b;
      }
    }
  }

  /// Composite a tiny-skia `Pixmap` (RGBA8 premultiplied) into the frame
  /// at `(x, y)`. Used for the few elements that go through tiny-skia's
  /// path rasterizer — rounded code-block backgrounds, task-box strokes,
  /// help-overlay card. Source covers a small bounding box; per-pixel
  /// blend cost is bounded by the path's footprint, not the whole frame.
  pub fn composite_pixmap(&mut self, x: i32, y: i32, pm: &Pixmap) {
    let pw = pm.width() as i32;
    let ph = pm.height() as i32;
    let src = pm.data();
    let fw = self.width as i32;
    let fh = self.height as i32;
    let stride = self.width as usize;
    for sy in 0..ph {
      let dy = y + sy;
      if dy < 0 || dy >= fh {
        continue;
      }
      let dst_row = (dy as usize) * stride;
      let src_row = (sy as usize) * (pw as usize) * 4;
      for sx in 0..pw {
        let dx = x + sx;
        if dx < 0 || dx >= fw {
          continue;
        }
        let si = src_row + (sx as usize) * 4;
        let a = src[si + 3] as u32;
        if a == 0 {
          continue;
        }
        let r = src[si] as u32;
        let g = src[si + 1] as u32;
        let b = src[si + 2] as u32;
        let didx = dst_row + dx as usize;
        if a == 255 {
          self.data[didx] = (r << 16) | (g << 8) | b;
          continue;
        }
        // Source RGBA is premultiplied. Composite over opaque dst:
        //   dst = src + dst * (1 - a)
        let inv = 255 - a;
        let dst = self.data[didx];
        let dr = (dst >> 16) & 0xFF;
        let dg = (dst >> 8) & 0xFF;
        let db = dst & 0xFF;
        let nr = (r + dr * inv / 255).min(255);
        let ng = (g + dg * inv / 255).min(255);
        let nb = (b + db * inv / 255).min(255);
        self.data[didx] = (nr << 16) | (ng << 8) | nb;
      }
    }
  }
}

#[inline]
fn sk_to_argb(c: SkColor) -> u32 {
  let r = (c.red() * 255.0) as u32;
  let g = (c.green() * 255.0) as u32;
  let b = (c.blue() * 255.0) as u32;
  (r << 16) | (g << 8) | b
}

/// Extract `(rgb24, alpha8)` from a tiny-skia color. Used by translucent
/// axis-aligned rects (inline-code pills, selection bg) so we preserve
/// the theme's intended alpha instead of rendering as opaque.
#[inline]
fn sk_to_rgba(c: SkColor) -> (u32, u8) {
  let r = (c.red() * 255.0) as u32;
  let g = (c.green() * 255.0) as u32;
  let b = (c.blue() * 255.0) as u32;
  let a = (c.alpha() * 255.0) as u8;
  ((r << 16) | (g << 8) | b, a)
}

#[inline]
fn ct_to_argb(c: Color) -> u32 {
  ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | (c.b() as u32)
}

/// Render an opaque, anti-aliased rounded-rect fill via a per-call
/// scratch `Pixmap` and composite into the frame. The path's bounding
/// box is the scratch size, which keeps the per-pixel blend cost
/// bounded to the path's footprint instead of the whole frame.
fn fill_rounded_rect_aa(
  frame: &mut Frame,
  x: i32,
  y: i32,
  w: u32,
  h: u32,
  r: f32,
  color: SkColor,
) {
  if w == 0 || h == 0 {
    return;
  }
  let Some(mut pm) = Pixmap::new(w, h) else {
    return;
  };
  let Some(path) = rounded_rect(0.0, 0.0, w as f32, h as f32, r) else {
    return;
  };
  let mut paint = tiny_skia::Paint::default();
  paint.set_color(color);
  paint.anti_alias = true;
  pm.fill_path(
    &path,
    &paint,
    FillRule::Winding,
    Transform::identity(),
    None,
  );
  frame.composite_pixmap(x, y, &pm);
}

/// Anti-aliased rounded-rect stroke (outline). Same approach as
/// `fill_rounded_rect_aa`: scratch pixmap sized to the path bbox.
fn stroke_rounded_rect_aa(
  frame: &mut Frame,
  x: i32,
  y: i32,
  w: u32,
  h: u32,
  r: f32,
  color: SkColor,
  stroke_w: f32,
) {
  if w == 0 || h == 0 {
    return;
  }
  let Some(mut pm) = Pixmap::new(w, h) else {
    return;
  };
  let Some(path) = rounded_rect(0.0, 0.0, w as f32, h as f32, r) else {
    return;
  };
  let mut paint = tiny_skia::Paint::default();
  paint.set_color(color);
  paint.anti_alias = true;
  let stroke = tiny_skia::Stroke {
    width: stroke_w,
    ..Default::default()
  };
  pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
  frame.composite_pixmap(x, y, &pm);
}

pub struct Painter {
  pub fs: FontSystem,
  pub swash: SwashCache,
}

impl Painter {
  pub fn new(fs: FontSystem) -> Self {
    Self::with_cache(fs, SwashCache::new())
  }

  /// Construct a painter with a pre-built (typically pre-warmed)
  /// SwashCache. Used by the speculative layout thread to deliver an
  /// already-rasterized cache so the first paint's glyph lookups all
  /// hit warm.
  pub fn with_cache(fs: FontSystem, swash: SwashCache) -> Self {
    Self { fs, swash }
  }

  pub fn paint_doc(&mut self, frame: &mut Frame, doc: &LaidDoc, theme: &Theme, scroll_y: f32) {
    frame.fill_solid(sk_to_argb(theme.bg));
    let h = frame.height as f32;
    for block in &doc.blocks {
      let by = block.y - scroll_y;
      if by + block.h < 0.0 || by > h {
        continue;
      }
      paint_block(frame, block, by, theme, &mut self.fs, &mut self.swash);
    }
  }

  pub fn paint_blank(&mut self, frame: &mut Frame, theme: &Theme) {
    frame.fill_solid(sk_to_argb(theme.bg));
  }

  pub fn paint_selection(
    &mut self,
    frame: &mut Frame,
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
        frame,
        &doc.blocks[start.block_idx],
        scroll_y,
        Some(&start.cursor),
        Some(&end.cursor),
        bg,
      );
      return;
    }
    paint_block_selection(
      frame,
      &doc.blocks[start.block_idx],
      scroll_y,
      Some(&start.cursor),
      None,
      bg,
    );
    for i in (start.block_idx + 1)..end.block_idx {
      paint_block_selection(frame, &doc.blocks[i], scroll_y, None, None, bg);
    }
    paint_block_selection(
      frame,
      &doc.blocks[end.block_idx],
      scroll_y,
      None,
      Some(&end.cursor),
      bg,
    );
  }

  pub fn paint_help_overlay(&mut self, frame: &mut Frame, theme: &Theme) {
    // Dim the doc behind. Translucent black via alpha-composite — too
    // visible to skip the alpha math, so go through the scratch-pixmap
    // path the same way other AA fills do.
    let scrim = SkColor::from_rgba8(0, 0, 0, 0x8c);
    let scrim_pm = {
      let mut pm = Pixmap::new(frame.width.max(1), frame.height.max(1)).expect("scrim pm");
      pm.fill(scrim);
      pm
    };
    frame.composite_pixmap(0, 0, &scrim_pm);

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

    let cx = (frame.width as f32 - card_w) / 2.0;
    let cy = (frame.height as f32 - card_h) / 2.0;

    let card_bg = if theme.is_dark {
      SkColor::from_rgba8(0x16, 0x1b, 0x22, 0xff)
    } else {
      SkColor::from_rgba8(0xff, 0xff, 0xff, 0xff)
    };
    fill_rounded_rect_aa(
      frame,
      cx as i32,
      cy as i32,
      card_w as u32,
      card_h as u32,
      10.0,
      card_bg,
    );
    stroke_rounded_rect_aa(
      frame,
      cx as i32,
      cy as i32,
      card_w as u32,
      card_h as u32,
      10.0,
      theme.border,
      1.0,
    );

    let title = crate::layout::make_plain_buffer(
      &mut self.fs,
      "KEYBINDS",
      12.0,
      14.0,
      card_w - pad * 2.0,
      crate::text::FONT_SANS,
    );
    draw_buffer(
      frame,
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
        frame,
        &key_buf,
        &mut self.fs,
        &mut self.swash,
        cx + pad,
        row_y,
        theme.link,
      );
      draw_buffer(
        frame,
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
  frame: &mut Frame,
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
        draw_run_pills(frame, buffer, block.x, y, c, theme.inline_code_bg);
      }
      draw_buffer(frame, buffer, fs, swash, block.x, y, *color);
      for u in underlines {
        draw_run_lines(frame, buffer, block.x, y, u, *color, LinePos::Underline);
      }
      for s in strikes {
        draw_run_lines(frame, buffer, block.x, y, s, *color, LinePos::Strike);
      }
    }
    LaidKind::Rule => {
      let w = frame.width as f32 - block.x * 2.0;
      frame.fill_rect(
        block.x as i32,
        y as i32,
        w as i32,
        1,
        sk_to_argb(theme.rule),
      );
    }
    LaidKind::Bar { color, width } => {
      frame.fill_rect(
        block.x as i32,
        y as i32,
        *width as i32,
        block.h as i32,
        sk_to_argb(*color),
      );
    }
    LaidKind::TaskBox { checked } => paint_task_box(frame, block.x, y, block.h, *checked, theme),
    LaidKind::CodeBlock {
      buffer,
      bg,
      width,
      pad_x,
      pad_y,
      lang_label,
      lang_label_color,
      ..
    } => {
      fill_rounded_rect_aa(
        frame,
        block.x as i32,
        y as i32,
        *width as u32,
        block.h as u32,
        6.0,
        *bg,
      );
      if let Some(label) = lang_label {
        let label_w = label
          .layout_runs()
          .map(|r| r.line_w)
          .fold(0.0_f32, f32::max);
        let lx = block.x + *width - label_w - *pad_x;
        let ly = y + 6.0;
        draw_buffer(frame, label, fs, swash, lx, ly, *lang_label_color);
      }
      draw_buffer(
        frame,
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
      frame, fs, swash, block.x, y, *block_w, rows, *border, *header_bg, theme,
    ),
  }
}

fn paint_table(
  frame: &mut Frame,
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
  let border_argb = sk_to_argb(border);

  // Header background
  if let Some(first) = rows.first() {
    if first.is_header {
      frame.fill_rect(
        x0 as i32,
        y0 as i32,
        block_w as i32,
        first.h as i32,
        sk_to_argb(header_bg),
      );
    }
  }

  // Outer border + horizontal lines (1px each)
  frame.fill_rect(x0 as i32, y0 as i32, block_w as i32, 1, border_argb);
  frame.fill_rect(
    x0 as i32,
    (y0 + total_h - 1.0) as i32,
    block_w as i32,
    1,
    border_argb,
  );
  for r in rows.iter().skip(1) {
    frame.fill_rect(
      x0 as i32,
      (y0 + r.y_top) as i32,
      block_w as i32,
      1,
      border_argb,
    );
  }
  // left + right
  frame.fill_rect(x0 as i32, y0 as i32, 1, total_h as i32, border_argb);
  frame.fill_rect(
    (x0 + block_w - 1.0) as i32,
    y0 as i32,
    1,
    total_h as i32,
    border_argb,
  );
  // vertical column lines
  if let Some(first) = rows.first() {
    for cell in first.cells.iter().skip(1) {
      frame.fill_rect(
        (x0 + cell.x) as i32,
        y0 as i32,
        1,
        total_h as i32,
        border_argb,
      );
    }
  }

  // Cell content
  for r in rows.iter() {
    for c in r.cells.iter() {
      paint_table_cell(frame, fs, swash, x0, y0 + r.y_top, c, theme);
    }
  }
}

fn paint_table_cell(
  frame: &mut Frame,
  fs: &mut FontSystem,
  swash: &mut SwashCache,
  table_x0: f32,
  row_y0: f32,
  cell: &TableCellLayout,
  theme: &Theme,
) {
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
    draw_run_pills(frame, &cell.buffer, cx, cy, c, theme.inline_code_bg);
  }
  draw_buffer(frame, &cell.buffer, fs, swash, cx, cy, cell.color);
  for u in &cell.underlines {
    draw_run_lines(
      frame,
      &cell.buffer,
      cx,
      cy,
      u,
      cell.color,
      LinePos::Underline,
    );
  }
  for s in &cell.strikes {
    draw_run_lines(frame, &cell.buffer, cx, cy, s, cell.color, LinePos::Strike);
  }
}

#[derive(Clone, Copy)]
enum LinePos {
  Underline,
  Strike,
}

fn draw_run_lines(
  frame: &mut Frame,
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
      frame.fill_rect(
        (ox + xs) as i32,
        (oy + underline_y) as i32,
        (xe - xs).max(1.0) as i32,
        thickness as i32,
        ct_to_argb(color),
      );
    }
  }
}

fn draw_run_pills(
  frame: &mut Frame,
  buf: &Buffer,
  ox: f32,
  oy: f32,
  range: &UnderlineRun,
  bg: SkColor,
) {
  let line_height = buf.metrics().line_height;
  let (rgb, alpha) = sk_to_rgba(bg);
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
      // theme.inline_code_bg carries a translucent alpha (0x33 light /
      // 0x40 dark). fill_rect_alpha blends it correctly onto the doc
      // bg — preserves the muted-pill look from before the u32 paint
      // refactor.
      frame.fill_rect_alpha(
        (ox + xs - pad_x) as i32,
        (oy + pill_top + pad_y) as i32,
        ((xe - xs) + pad_x * 2.0) as i32,
        (pill_h - pad_y * 2.0) as i32,
        rgb,
        alpha,
      );
    }
  }
}

fn paint_task_box(frame: &mut Frame, x: f32, y: f32, size: f32, checked: bool, theme: &Theme) {
  let outline = ct_to_sk(theme.muted);
  let stroke_w = (size * 0.12).max(1.5).round();

  stroke_rounded_rect_aa(
    frame,
    x as i32,
    y as i32,
    size as u32,
    size as u32,
    size * 0.18,
    outline,
    stroke_w,
  );

  if checked {
    let pad = size * 0.28;
    frame.fill_rect(
      (x + pad) as i32,
      (y + pad) as i32,
      (size - pad * 2.0) as i32,
      (size - pad * 2.0) as i32,
      ct_to_argb(theme.link),
    );
  }
}

fn ct_to_sk(c: Color) -> SkColor {
  SkColor::from_rgba8(c.r(), c.g(), c.b(), c.a())
}

fn paint_block_selection(
  frame: &mut Frame,
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
  // Selection bg is translucent (alpha 0x70 / 0x80). fill_rect_alpha
  // does the per-pixel blend without a scratch pixmap — cheaper than
  // composite_pixmap for axis-aligned rects.
  let (rgb, alpha) = sk_to_rgba(bg);
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

    let rect_x = (ox + xs) as i32;
    let rect_y = (oy + run.line_top) as i32;
    let rect_w = (xe - xs).max(1.0) as i32;
    let rect_h = lh.max(1.0) as i32;
    frame.fill_rect_alpha(rect_x, rect_y, rect_w, rect_h, rgb, alpha);
  }
}

fn cursor_x_in_run(run: &cosmic_text::LayoutRun, byte_idx: usize) -> Option<f32> {
  if run.glyphs.is_empty() {
    return Some(0.0);
  }
  for g in run.glyphs.iter() {
    if byte_idx <= g.start {
      return Some(g.x);
    }
    if byte_idx <= g.end {
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
  frame: &mut Frame,
  buf: &Buffer,
  fs: &mut FontSystem,
  swash: &mut SwashCache,
  ox: f32,
  oy: f32,
  color: Color,
) {
  // Iterate the cached glyph bitmap directly via `swash.get_image(...)`
  // and write blended u32 pixels straight into the frame buffer — no
  // intermediate BGRA pixmap pass. cosmic-text's own docs note "use
  // `with_image` for better performance"; we keep the same blending
  // semantics but skip per-row and per-pixel callback overhead, and
  // skip the BGRA→u32 conversion that the old Pixmap-based pipeline
  // required at the end of each frame.
  let stride = frame.width as usize;
  let fw = frame.width as i32;
  let fh = frame.height as i32;
  let ox = ox as i32;
  let oy = oy as i32;
  for run in buf.layout_runs() {
    for glyph in run.glyphs.iter() {
      let physical = glyph.physical((0., run.line_y), 1.0);
      let glyph_color = glyph.color_opt.unwrap_or(color);
      let Some(image) = swash.get_image(fs, physical.cache_key).as_ref() else {
        continue;
      };
      let img_w = image.placement.width as i32;
      let img_h = image.placement.height as i32;
      if img_w == 0 || img_h == 0 {
        continue;
      }
      let base_x = physical.x + image.placement.left + ox;
      let base_y = physical.y - image.placement.top + oy;
      match image.content {
        SwashContent::Mask => {
          paint_mask_glyph(
            frame.data,
            stride,
            fw,
            fh,
            base_x,
            base_y,
            img_w,
            img_h,
            &image.data,
            glyph_color,
          );
        }
        SwashContent::Color => {
          paint_color_glyph(
            frame.data,
            stride,
            fw,
            fh,
            base_x,
            base_y,
            img_w,
            img_h,
            &image.data,
          );
        }
        SwashContent::SubpixelMask => {}
      }
    }
  }
}

/// Composite a per-pixel alpha mask (single byte/pixel) onto the u32
/// frame using `glyph_color` for RGB. This is the path 99%+ of glyphs
/// take — regular text is single-channel coverage.
fn paint_mask_glyph(
  data: &mut [u32],
  stride: usize,
  fw: i32,
  fh: i32,
  base_x: i32,
  base_y: i32,
  img_w: i32,
  img_h: i32,
  alpha: &[u8],
  color: Color,
) {
  let sr = color.r() as u32;
  let sg = color.g() as u32;
  let sb = color.b() as u32;
  let mut row_off = 0_usize;
  for off_y in 0..img_h {
    let fy = base_y + off_y;
    if fy < 0 || fy >= fh {
      row_off += img_w as usize;
      continue;
    }
    let row_base = (fy as usize) * stride;
    for off_x in 0..img_w {
      let sa = alpha[row_off + off_x as usize] as u32;
      if sa == 0 {
        continue;
      }
      let fx = base_x + off_x;
      if fx < 0 || fx >= fw {
        continue;
      }
      let idx = row_base + fx as usize;
      blend_mask_argb(data, idx, sr, sg, sb, sa);
    }
    row_off += img_w as usize;
  }
}

/// Color-emoji or other content delivered as RGBA8 per pixel.
fn paint_color_glyph(
  data: &mut [u32],
  stride: usize,
  fw: i32,
  fh: i32,
  base_x: i32,
  base_y: i32,
  img_w: i32,
  img_h: i32,
  rgba: &[u8],
) {
  let mut row_off = 0_usize;
  let src_stride = img_w as usize * 4;
  for off_y in 0..img_h {
    let fy = base_y + off_y;
    if fy < 0 || fy >= fh {
      row_off += src_stride;
      continue;
    }
    let row_base = (fy as usize) * stride;
    for off_x in 0..img_w {
      let i = row_off + off_x as usize * 4;
      let sa = rgba[i + 3] as u32;
      if sa == 0 {
        continue;
      }
      let fx = base_x + off_x;
      if fx < 0 || fx >= fw {
        continue;
      }
      let sr = rgba[i] as u32;
      let sg = rgba[i + 1] as u32;
      let sb = rgba[i + 2] as u32;
      let idx = row_base + fx as usize;
      // Source RGBA is premultiplied. Composite onto opaque dst:
      //   dst = src + dst * (1 - a)
      let inv = 255 - sa;
      let dst = data[idx];
      let dr = (dst >> 16) & 0xFF;
      let dg = (dst >> 8) & 0xFF;
      let db = dst & 0xFF;
      let r = (sr + dr * inv / 255).min(255);
      let g = (sg + dg * inv / 255).min(255);
      let b = (sb + db * inv / 255).min(255);
      data[idx] = (r << 16) | (g << 8) | b;
    }
    row_off += src_stride;
  }
}

/// Alpha-blend `(sr, sg, sb)` with coverage `sa` (0..=255) onto the
/// u32 ARGB pixel at `data[idx]`. Frame pixels are always opaque (we
/// fill bg first), so we don't track destination alpha.
#[inline(always)]
fn blend_mask_argb(data: &mut [u32], idx: usize, sr: u32, sg: u32, sb: u32, sa: u32) {
  let inv = 255 - sa;
  let dst = data[idx];
  let dr = (dst >> 16) & 0xFF;
  let dg = (dst >> 8) & 0xFF;
  let db = dst & 0xFF;
  let r = (sr * sa + dr * inv) / 255;
  let g = (sg * sa + dg * inv) / 255;
  let b = (sb * sa + db * inv) / 255;
  data[idx] = (r << 16) | (g << 8) | b;
}

/// Walk the laid doc and call `swash.get_image(...)` on every glyph
/// whose block falls within the first `viewport_h` physical pixels.
/// Each `get_image` call rasterizes the glyph through swash and stores
/// the result in the cache; subsequent paint-time lookups become
/// O(1) hashmap hits. Designed to run on the speculative-layout
/// background thread so the rasterization cost is paid in parallel with
/// main-thread window/event-loop setup instead of inline during the
/// first paint.
pub fn warm_glyph_cache(
  swash: &mut SwashCache,
  fs: &mut FontSystem,
  laid: &LaidDoc,
  viewport_h: f32,
) {
  for block in &laid.blocks {
    if block.y >= viewport_h {
      // Blocks are layered top-down; once we pass the viewport bottom
      // there's nothing visible left to warm.
      break;
    }
    warm_block(swash, fs, block);
  }
}

/// Parallel warm: round-robins blocks across `1 + worker_fs.len()` lanes.
/// Each lane uses its own (FontSystem, SwashCache); after warming, the
/// worker SwashCaches are drained into `main_swash`. Keys are portable
/// across our FontSystems because all of them load identical fonts in
/// identical order — same property item-2 (parallel layout) relies on.
pub fn warm_glyph_cache_parallel(
  main_swash: &mut SwashCache,
  main_fs: &mut FontSystem,
  worker_fs: &mut [FontSystem],
  worker_swashes: &mut [SwashCache],
  laid: &LaidDoc,
  viewport_h: f32,
) {
  assert_eq!(worker_fs.len(), worker_swashes.len());
  let n_lanes = 1 + worker_fs.len();
  let blocks = &laid.blocks;
  std::thread::scope(|s| {
    let handles: Vec<_> = worker_fs
      .iter_mut()
      .zip(worker_swashes.iter_mut())
      .enumerate()
      .map(|(lane, (wfs, wsw))| {
        let lane_idx = lane + 1;
        s.spawn(move || {
          for (i, block) in blocks.iter().enumerate() {
            if i % n_lanes != lane_idx {
              continue;
            }
            if block.y >= viewport_h {
              // Per-lane monotone y: once past viewport, all remaining
              // on this lane are also past.
              break;
            }
            warm_block(wsw, wfs, block);
          }
        })
      })
      .collect();
    // Caller thread runs lane 0 in parallel with workers.
    for (i, block) in blocks.iter().enumerate() {
      if i % n_lanes != 0 {
        continue;
      }
      if block.y >= viewport_h {
        break;
      }
      warm_block(main_swash, main_fs, block);
    }
    for h in handles {
      h.join().expect("warm worker panicked");
    }
  });
  // Merge worker caches into main. `or_insert` keeps main's entry when
  // duplicates exist (rare — only happens if two lanes warmed the same
  // glyph_id × size, e.g. headings on different lanes that share a font
  // size and chars).
  for w_swash in worker_swashes.iter_mut() {
    let img = std::mem::take(&mut w_swash.image_cache);
    for (k, v) in img {
      main_swash.image_cache.entry(k).or_insert(v);
    }
    let oc = std::mem::take(&mut w_swash.outline_command_cache);
    for (k, v) in oc {
      main_swash.outline_command_cache.entry(k).or_insert(v);
    }
  }
}

fn warm_block(swash: &mut SwashCache, fs: &mut FontSystem, block: &LaidBlock) {
  match &block.kind {
    LaidKind::Text { buffer, .. } => warm_buffer(swash, fs, buffer),
    LaidKind::CodeBlock {
      buffer, lang_label, ..
    } => {
      warm_buffer(swash, fs, buffer);
      if let Some(label) = lang_label.as_ref() {
        warm_buffer(swash, fs, label);
      }
    }
    LaidKind::Table { rows, .. } => {
      for row in rows {
        for cell in &row.cells {
          warm_buffer(swash, fs, &cell.buffer);
        }
      }
    }
    _ => {}
  }
}

fn warm_buffer(swash: &mut SwashCache, fs: &mut FontSystem, buf: &Buffer) {
  for run in buf.layout_runs() {
    for glyph in run.glyphs.iter() {
      let physical = glyph.physical((0., run.line_y), 1.0);
      let _ = swash.get_image(fs, physical.cache_key);
    }
  }
}
