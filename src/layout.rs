use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, Weight};
use tiny_skia::Color as SkColor;

use crate::doc::{AlertKind, Block, CellAlign, Doc, FootnoteDef, Inline, ListItem};
use crate::highlight::{HlSpan, highlight};
use crate::images::{self, ImageStore};
use crate::inline::{StyledRuns, build_buffer, build_runs};
use crate::json::JsonRange;
use crate::text::{FONT_MONO, FONT_SANS, marker_features, mono_features, sans_features};
use crate::theme::Theme;

pub const MAX_CONTENT_W: f32 = 824.0;
pub const PAD_X_MIN: f32 = 48.0;
pub const PAD_Y: f32 = 40.0;
pub const BODY_FS: f32 = 16.0;
pub const BODY_LH_RATIO: f32 = 1.55;
pub const CODE_FS: f32 = 14.0;
pub const CODE_LH_RATIO: f32 = 1.5;
pub const BLOCK_GAP: f32 = 16.0;
pub const HEADING_GAP_TOP: f32 = 24.0;
pub const LIST_MARKER_W: f32 = 18.0;
pub const LIST_MARKER_GAP: f32 = 8.0;
pub const LIST_INDENT: f32 = LIST_MARKER_W + LIST_MARKER_GAP;
pub const LIST_ITEM_GAP: f32 = 4.0;
pub const QUOTE_INDENT: f32 = 16.0;
pub const QUOTE_BAR_W: f32 = 3.0;
pub const CALLOUT_TITLE_GAP: f32 = BLOCK_GAP * 0.5;
pub const CALLOUT_VPAD: f32 = 4.0;
/// Footnotes render at ~87.5% of body size (14 px at 1x zoom) and pull
/// every internal gap/indent through the same `ctx.scale` path, so the
/// section stays proportional at any zoom or DPI.
pub const FOOTNOTE_SCALE: f32 = 0.875;
pub const TASK_BOX: f32 = 14.0;
pub const CODE_PAD_X: f32 = 14.0;
pub const CODE_PAD_Y: f32 = 12.0;
pub const CODE_RADIUS: f32 = 6.0;
pub const LANG_LABEL_FS: f32 = 11.0;
pub const TABLE_CELL_PAD_X: f32 = 12.0;
pub const TABLE_CELL_PAD_Y: f32 = 8.0;

pub struct LaidDoc {
  pub blocks: Vec<LaidBlock>,
  pub total_height: f32,
  pub width: f32,
  pub content_x: f32,
  pub content_w: f32,
  pub heading_ys: Vec<f32>,
  pub block_ys: Vec<f32>,
  /// Indices into `blocks` for each heading. Each `Block::Heading` source
  /// block produces exactly one `LaidKind::Text` LaidBlock, so this is a
  /// 1:1 map onto `heading_ys` (same length, same order). Used by the
  /// resize anchor logic to snap to a nearby heading instead of mid-text.
  pub heading_block_idxs: Vec<usize>,
  /// Resolved scroll targets for clickable footnote refs and back-links.
  /// Built in a single pass at the end of `layout()` from the per-block
  /// `LinkRange`s. `def_y` is always populated (one definition per
  /// label); `first_ref_y` is `None` if the doc has a definition with
  /// no body reference (rare but legal).
  pub footnote_jumps: HashMap<String, FootnoteJump>,
}

#[derive(Clone, Copy)]
pub struct FootnoteJump {
  pub def_y: f32,
  pub first_ref_y: Option<f32>,
}

pub struct LaidBlock {
  pub y: f32,
  pub h: f32,
  pub x: f32,
  pub kind: LaidKind,
}

pub enum LaidKind {
  Text {
    buffer: Buffer,
    color: Color,
    underlines: Vec<UnderlineRun>,
    strikes: Vec<UnderlineRun>,
    code_runs: Vec<UnderlineRun>,
    links: Vec<LinkRange>,
  },
  Rule,
  Bar {
    color: SkColor,
    width: f32,
  },
  TaskBox {
    checked: bool,
  },
  CodeBlock {
    buffer: Buffer,
    bg: SkColor,
    width: f32,
    pad_x: f32,
    pad_y: f32,
    lang_label: Option<Buffer>,
    lang_label_color: Color,
    lang: String,
    source: String,
    /// Per-key / per-value byte ranges into `source`. `Some` for the
    /// synthetic JSON block produced by `lib::run`; `None` for fenced
    /// markdown code blocks. When `Some`, hint-mode emits one badge per
    /// range instead of a single whole-block copy hint.
    targets: Option<Vec<JsonRange>>,
  },
  Table {
    block_w: f32,
    rows: Vec<TableRowLayout>,
    border: SkColor,
    header_bg: SkColor,
    alt_bg: SkColor,
  },
  Image {
    /// Absolute path the painter looks up in the ImageStore. `None`
    /// when the source couldn't be resolved (remote URL in v1, or
    /// stdin with a relative src) — paint just draws the alt-text
    /// placeholder.
    path: Option<std::path::PathBuf>,
    alt: String,
    width: f32,
    height: f32,
  },
  /// Frontmatter rendered as a muted key/value box at the top of the
  /// doc. One pre-formatted monospace buffer with column-aligned rows.
  Frontmatter {
    buffer: Buffer,
    bg: SkColor,
    width: f32,
    pad_x: f32,
    pad_y: f32,
  },
}

pub struct TableRowLayout {
  pub y_top: f32,
  pub h: f32,
  pub is_header: bool,
  pub cells: Vec<TableCellLayout>,
}

pub struct TableCellLayout {
  pub x: f32,
  pub w: f32,
  pub buffer: Buffer,
  pub align: CellAlign,
  pub color: Color,
  pub underlines: Vec<UnderlineRun>,
  pub strikes: Vec<UnderlineRun>,
  pub code_runs: Vec<UnderlineRun>,
  pub links: Vec<LinkRange>,
}

#[derive(Clone)]
pub struct UnderlineRun {
  pub byte_start: usize,
  pub byte_end: usize,
}

#[derive(Clone)]
pub struct LinkRange {
  pub byte_start: usize,
  pub byte_end: usize,
  pub target: LinkTarget,
}

#[derive(Clone, Debug)]
pub enum LinkTarget {
  /// External URL (or `#fragment` etc) — opened via `opener`.
  Url(String),
  /// Footnote reference `[^label]` in body text. Click scrolls to the
  /// matching definition row.
  Footnote(String),
  /// Back-arrow on a footnote definition. Click scrolls to the first
  /// reference in body text with the same label.
  FootnoteBack(String),
}

pub fn layout(
  doc: &Doc,
  surface_w: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  full_highlight: bool,
  scale: f32,
  images: Arc<ImageStore>,
  base_dir: Option<PathBuf>,
) -> LaidDoc {
  layout_parallel(
    doc,
    surface_w,
    fs,
    &mut [],
    theme,
    full_highlight,
    scale,
    images,
    base_dir,
  )
}

/// Parallel variant: top-level blocks are partitioned round-robin across
/// `1 + worker_fs.len()` lanes (lane 0 = caller's thread using `main_fs`,
/// lanes 1..N = scoped threads each using one of `worker_fs`). cosmic-text
/// shaping is independent per block and the per-lane font_id assignments
/// match the painter's because all FontSystems load identical fonts in
/// identical order (fontdb's slotmap is deterministic for fresh maps).
/// Sub-block y-offsets are assembled sequentially after join.
pub fn layout_parallel(
  doc: &Doc,
  surface_w: f32,
  main_fs: &mut FontSystem,
  worker_fs: &mut [FontSystem],
  theme: &Theme,
  full_highlight: bool,
  scale: f32,
  images: Arc<ImageStore>,
  base_dir: Option<PathBuf>,
) -> LaidDoc {
  let pad_x = PAD_X_MIN * scale;
  let pad_y = PAD_Y * scale;
  let content_w = (surface_w - pad_x * 2.0)
    .min(MAX_CONTENT_W * scale)
    .max(120.0);
  let content_x = ((surface_w - content_w) / 2.0).max(pad_x);

  let ctx = Ctx {
    full_highlight,
    scale,
    images,
    base_dir,
  };

  let n_lanes = 1 + worker_fs.len();
  let n_blocks = doc.blocks.len();
  let mut by_idx: Vec<Option<(Vec<LaidBlock>, f32)>> = (0..n_blocks).map(|_| None).collect();

  // Round-robin partition: block i goes to lane (i % n_lanes). Lane 0 is
  // the caller; lanes 1.. are workers. Round-robin keeps cost balanced
  // when block costs vary (a code block is ~10x a heading).
  let mut lane_indices: Vec<Vec<usize>> = vec![Vec::new(); n_lanes];
  for i in 0..n_blocks {
    lane_indices[i % n_lanes].push(i);
  }
  let main_indices = std::mem::take(&mut lane_indices[0]);
  let worker_indices: Vec<Vec<usize>> = lane_indices.drain(1..).collect();

  std::thread::scope(|s| {
    let handles: Vec<_> = worker_fs
      .iter_mut()
      .zip(worker_indices)
      .map(|(fs, indices)| {
        let blocks = &doc.blocks;
        let ctx = &ctx;
        s.spawn(move || -> Vec<(usize, Vec<LaidBlock>, f32)> {
          indices
            .into_iter()
            .map(|i| {
              let (laid, h) = layout_block(&blocks[i], content_w, content_x, fs, theme, ctx);
              (i, laid, h)
            })
            .collect()
        })
      })
      .collect();

    // Caller thread does its lane while workers run.
    for i in main_indices {
      let (laid, h) = layout_block(&doc.blocks[i], content_w, content_x, main_fs, theme, &ctx);
      by_idx[i] = Some((laid, h));
    }

    for handle in handles {
      let lane_results = handle.join().expect("layout worker panicked");
      for (i, laid, h) in lane_results {
        by_idx[i] = Some((laid, h));
      }
    }
  });

  let mut heading_ys = Vec::new();
  let mut heading_block_idxs = Vec::new();
  let mut block_ys = Vec::new();
  let mut y = 0.0_f32;
  let mut blocks: Vec<LaidBlock> = Vec::new();
  for (i, slot) in by_idx.into_iter().enumerate() {
    let (mut sub_blocks, sub_h) = slot.expect("missing layout result");
    if i > 0 {
      let gap = if matches!(doc.blocks[i], Block::Heading { .. }) {
        HEADING_GAP_TOP * scale
      } else {
        BLOCK_GAP * scale
      };
      y += gap;
    }
    block_ys.push(y + pad_y);
    if matches!(doc.blocks[i], Block::Heading { .. }) {
      heading_ys.push(y + pad_y);
      // Heading source blocks produce exactly one LaidBlock (a Text);
      // the next push extends `blocks` from this index.
      heading_block_idxs.push(blocks.len());
    }
    for lb in sub_blocks.iter_mut() {
      lb.y += y;
    }
    blocks.extend(sub_blocks);
    y += sub_h;
  }
  for b in blocks.iter_mut() {
    b.y += pad_y;
  }

  let footnote_jumps = build_footnote_jumps(&blocks);

  LaidDoc {
    blocks,
    total_height: y + pad_y * 2.0,
    width: surface_w,
    content_x,
    content_w,
    heading_ys,
    block_ys,
    heading_block_idxs,
    footnote_jumps,
  }
}

/// Walk the laid blocks once, collecting per-label scroll targets:
/// `def_y` from FootnoteBack ranges (one per definition), `first_ref_y`
/// from the first Footnote range encountered in document order.
fn build_footnote_jumps(blocks: &[LaidBlock]) -> HashMap<String, FootnoteJump> {
  let mut jumps: HashMap<String, FootnoteJump> = HashMap::new();
  for block in blocks {
    let LaidKind::Text { links, .. } = &block.kind else {
      continue;
    };
    for link in links {
      match &link.target {
        LinkTarget::Footnote(label) => {
          jumps
            .entry(label.clone())
            .and_modify(|j| {
              if j.first_ref_y.is_none() {
                j.first_ref_y = Some(block.y);
              }
            })
            .or_insert(FootnoteJump {
              def_y: 0.0,
              first_ref_y: Some(block.y),
            });
        }
        LinkTarget::FootnoteBack(label) => {
          jumps
            .entry(label.clone())
            .and_modify(|j| j.def_y = block.y)
            .or_insert(FootnoteJump {
              def_y: block.y,
              first_ref_y: None,
            });
        }
        LinkTarget::Url(_) => {}
      }
    }
  }
  jumps
}

#[derive(Clone)]
struct Ctx {
  full_highlight: bool,
  scale: f32,
  images: Arc<ImageStore>,
  base_dir: Option<PathBuf>,
}

fn layout_blocks(
  blocks: &[Block],
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
  block_gap: f32,
) -> (Vec<LaidBlock>, f32) {
  let mut y = 0.0_f32;
  let mut out: Vec<LaidBlock> = Vec::new();
  for (i, block) in blocks.iter().enumerate() {
    if i > 0 {
      let gap = if matches!(block, Block::Heading { .. }) {
        HEADING_GAP_TOP * ctx.scale
      } else {
        block_gap
      };
      y += gap;
    }
    let (mut laid, dy) = layout_block(block, w, x, fs, theme, ctx);
    for lb in laid.iter_mut() {
      lb.y += y;
    }
    out.extend(laid);
    y += dy;
  }
  (out, y)
}

fn layout_block(
  block: &Block,
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  let z = ctx.scale;
  match block {
    Block::Heading { level, inlines } => {
      let size = heading_size(*level) * z;
      text_block(
        inlines,
        theme.heading,
        size,
        size * 1.25,
        w,
        x,
        fs,
        theme,
        true,
      )
    }
    Block::Paragraph(inlines) => text_block(
      inlines,
      theme.fg,
      BODY_FS * z,
      BODY_FS * z * BODY_LH_RATIO,
      w,
      x,
      fs,
      theme,
      false,
    ),
    Block::Rule => (
      vec![LaidBlock {
        y: 0.0,
        h: 1.0,
        x,
        kind: LaidKind::Rule,
      }],
      1.0,
    ),
    Block::CodeBlock {
      lang,
      code,
      targets,
    } => layout_code_block(lang, code, targets.clone(), w, x, fs, theme, ctx),
    Block::List {
      ordered,
      start,
      items,
    } => layout_list(*ordered, *start, items, w, x, fs, theme, ctx),
    Block::Quote(inner) => layout_quote(inner, w, x, fs, theme, ctx),
    Block::Table { aligns, head, rows } => layout_table(aligns, head, rows, w, x, fs, theme, ctx),
    Block::Footnotes(defs) => layout_footnotes(defs, w, x, fs, theme, ctx),
    Block::Image { src, alt } => layout_image(src, alt, w, x, ctx),
    Block::Alert { kind, blocks } => layout_alert(*kind, blocks, w, x, fs, theme, ctx),
    Block::Frontmatter { entries } => layout_frontmatter(entries, w, x, fs, theme, ctx),
  }
}

/// Lay out a block-level image. Dimensions come from the ImageStore (set
/// at parse time by `read_dims`). Width clamps to content width;
/// aspect ratio is preserved. Image is centered horizontally if smaller
/// than content width, so it doesn't look pinned-left in a wide window.
fn layout_image(src: &str, alt: &str, w: f32, x: f32, ctx: &Ctx) -> (Vec<LaidBlock>, f32) {
  let path = images::resolve_src(src, ctx.base_dir.as_deref());
  let (nat_w, nat_h) = path
    .as_ref()
    .and_then(|p| ctx.images.get_dims(p))
    .unwrap_or((480, 270));
  let nat_wf = nat_w as f32;
  let nat_hf = nat_h as f32;
  let display_w = nat_wf.min(w);
  let aspect = if nat_wf > 0.0 {
    nat_hf / nat_wf
  } else {
    0.5625
  };
  let display_h = display_w * aspect;
  let img_x = x + ((w - display_w) / 2.0).max(0.0);
  (
    vec![LaidBlock {
      y: 0.0,
      h: display_h,
      x: img_x,
      kind: LaidKind::Image {
        path,
        alt: alt.to_string(),
        width: display_w,
        height: display_h,
      },
    }],
    display_h,
  )
}

/// Lay out a frontmatter block as a muted rounded box. Renders each
/// entry as a single line of monospace text with the key padded to
/// the longest key in the block plus two spaces, then the value.
/// Lines without a key (verbatim continuation lines) print indented
/// to align with the value column. Single buffer, single color —
/// keeps the renderer simple and avoids inventing inline-styling for
/// what's intentionally low-emphasis content.
fn layout_frontmatter(
  entries: &[(String, String)],
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  _ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  let s = _ctx.scale;
  let pad_x = CODE_PAD_X * s;
  let pad_y = CODE_PAD_Y * s;
  let inner_w = (w - pad_x * 2.0).max(40.0);

  let key_col: usize = entries
    .iter()
    .filter(|(k, _)| !k.is_empty())
    .map(|(k, _)| k.chars().count())
    .max()
    .unwrap_or(0);
  let mut lines = String::new();
  for (i, (k, v)) in entries.iter().enumerate() {
    if i > 0 {
      lines.push('\n');
    }
    if k.is_empty() {
      // Continuation / non-kv line: indent to value column.
      for _ in 0..(key_col + 2) {
        lines.push(' ');
      }
      lines.push_str(v);
    } else {
      lines.push_str(k);
      let pad = (key_col - k.chars().count()) + 2;
      for _ in 0..pad {
        lines.push(' ');
      }
      lines.push_str(v);
    }
  }

  let buf = make_plain_buffer(
    fs,
    &lines,
    CODE_FS * s,
    CODE_FS * s * CODE_LH_RATIO,
    inner_w,
    FONT_MONO,
  );
  let buf_h = buffer_height(&buf);
  let total_h = buf_h + pad_y * 2.0;

  (
    vec![LaidBlock {
      y: 0.0,
      h: total_h,
      x,
      kind: LaidKind::Frontmatter {
        buffer: buf,
        bg: theme.code_bg,
        width: w,
        pad_x,
        pad_y,
      },
    }],
    total_h,
  )
}

fn layout_table(
  aligns: &[CellAlign],
  head: &[Vec<Inline>],
  rows: &[Vec<Vec<Inline>>],
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  let s = ctx.scale;
  let pad_x = TABLE_CELL_PAD_X * s;
  let pad_y = TABLE_CELL_PAD_Y * s;
  let cols = head
    .len()
    .max(rows.iter().map(|r| r.len()).max().unwrap_or(0))
    .max(1);
  let col_w = w / cols as f32;
  let cell_text_w = (col_w - pad_x * 2.0).max(40.0);
  let cell_fs = (BODY_FS - 1.0) * s;

  let build_row = |cells: &[Vec<Inline>],
                   is_header: bool,
                   fs: &mut FontSystem,
                   theme: &Theme|
   -> TableRowLayout {
    let color = if is_header { theme.heading } else { theme.fg };
    let mut row_h = 0.0_f32;
    let mut laid_cells: Vec<TableCellLayout> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
      let runs = build_runs(cell, theme);
      let (underlines, strikes, code_runs, links) = compute_runs(&runs);
      let buf = build_buffer(
        fs,
        &runs,
        color,
        cell_fs,
        cell_fs * BODY_LH_RATIO,
        cell_text_w,
        is_header,
      );
      let h = buffer_height(&buf);
      row_h = row_h.max(h);
      let align = aligns.get(i).copied().unwrap_or(CellAlign::Left);
      laid_cells.push(TableCellLayout {
        x: i as f32 * col_w,
        w: col_w,
        buffer: buf,
        align,
        color,
        underlines,
        strikes,
        code_runs,
        links,
      });
    }
    TableRowLayout {
      y_top: 0.0,
      h: row_h + pad_y * 2.0,
      is_header,
      cells: laid_cells,
    }
  };

  let mut row_layouts: Vec<TableRowLayout> = Vec::new();
  if !head.is_empty() {
    row_layouts.push(build_row(head, true, fs, theme));
  }
  for r in rows {
    row_layouts.push(build_row(r, false, fs, theme));
  }

  let mut y = 0.0_f32;
  for r in row_layouts.iter_mut() {
    r.y_top = y;
    y += r.h;
  }

  (
    vec![LaidBlock {
      y: 0.0,
      h: y,
      x,
      kind: LaidKind::Table {
        block_w: w,
        rows: row_layouts,
        border: theme.border,
        header_bg: theme.code_bg,
        alt_bg: theme.code_bg,
      },
    }],
    y,
  )
}

fn layout_footnotes(
  defs: &[FootnoteDef],
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  // The whole section runs through a sub-context at FOOTNOTE_SCALE, so
  // every interior font size, gap, indent, and code padding shrinks
  // together — and still tracks the outer `ctx.scale` for zoom/DPI.
  let s = ctx.scale * FOOTNOTE_SCALE;
  let foot_ctx = Ctx {
    full_highlight: ctx.full_highlight,
    scale: s,
    images: ctx.images.clone(),
    base_dir: ctx.base_dir.clone(),
  };

  let mut all: Vec<LaidBlock> = Vec::new();
  let mut total = 0.0_f32;

  // Section header: smaller than a regular h3 so the section reads as a
  // footer rather than a peer of body sections.
  let header_fs = BODY_FS * s;
  let header_lh = header_fs * BODY_LH_RATIO;
  let header = make_plain_buffer(fs, "Footnotes", header_fs, header_lh, w, FONT_SANS);
  let hh = buffer_height(&header);
  all.push(LaidBlock {
    y: 0.0,
    h: hh,
    x,
    kind: LaidKind::Text {
      buffer: header,
      color: theme.muted,
      underlines: Vec::new(),
      strikes: Vec::new(),
      code_runs: Vec::new(),
      links: Vec::new(),
    },
  });
  total += hh + BLOCK_GAP * s * 0.5;
  all.push(LaidBlock {
    y: total,
    h: 1.0,
    x,
    kind: LaidKind::Rule,
  });
  total += BLOCK_GAP * s * 0.75;

  // Footnote labels can be numeric ("1") or word-form ("edge"). Word
  // labels would wrap mid-word inside a fixed column. Compute a shared
  // column width sized to the longest label, capped so very long labels
  // don't eat the body's width.
  const FOOTNOTE_LABEL_PAD: f32 = 8.0;
  const FOOTNOTE_LABEL_CAP: f32 = 50.0;
  let label_fs = BODY_FS * s;
  let label_lh = label_fs * BODY_LH_RATIO;
  let label_pad = FOOTNOTE_LABEL_PAD * s;
  let label_cap = FOOTNOTE_LABEL_CAP * s;
  let label_max_w = (label_cap - label_pad).max(8.0);

  // Each label is two styled runs: muted "<n>. " + link-colored "↩".
  // The whole buffer is one click target so a forgiving hit area takes
  // you back to the first reference; the colored arrow makes it visible.
  let label_bufs: Vec<(Buffer, usize)> = defs
    .iter()
    .map(|def| build_footnote_label(fs, &def.label, label_fs, label_lh, label_max_w, theme))
    .collect();
  let measured = label_bufs
    .iter()
    .flat_map(|(b, _)| b.layout_runs())
    .map(|r| r.line_w)
    .fold(0.0_f32, f32::max);
  let col_w = (measured + label_pad).min(label_cap);

  for (i, (def, (label_buf, total_bytes))) in defs.iter().zip(label_bufs).enumerate() {
    if i > 0 {
      total += LIST_ITEM_GAP * 2.0 * s;
    }
    let lh = buffer_height(&label_buf);
    let label_links = vec![LinkRange {
      byte_start: 0,
      byte_end: total_bytes,
      target: LinkTarget::FootnoteBack(def.label.clone()),
    }];
    all.push(LaidBlock {
      y: total,
      h: lh,
      x,
      kind: LaidKind::Text {
        buffer: label_buf,
        color: theme.muted,
        underlines: Vec::new(),
        strikes: Vec::new(),
        code_runs: Vec::new(),
        links: label_links,
      },
    });
    let body_x = x + col_w;
    let body_w = (w - col_w).max(80.0);
    let (mut laid, dy) = layout_blocks(
      &def.blocks,
      body_w,
      body_x,
      fs,
      theme,
      &foot_ctx,
      BLOCK_GAP * s,
    );
    for lb in laid.iter_mut() {
      lb.y += total;
    }
    all.extend(laid);
    // Row height = max(label, body) so wrapped content can't overlap
    // the next row.
    total += dy.max(lh);
  }

  (all, total)
}

/// Build a footnote-definition label as a 2-run rich-text buffer:
/// muted "<n>. " followed by a link-colored "↩". Returns the buffer
/// and total byte length so callers can size the LinkRange.
fn build_footnote_label(
  fs: &mut FontSystem,
  label: &str,
  font_size: f32,
  line_height: f32,
  width: f32,
  theme: &Theme,
) -> (Buffer, usize) {
  const ARROW: &str = "↩";
  let prefix = format!("{}. ", label);
  let total_bytes = prefix.len() + ARROW.len();

  let mut buf = Buffer::new(fs, Metrics::new(font_size, line_height));
  buf.set_size(Some(width), None);
  let base = Attrs::new()
    .family(Family::Name(FONT_SANS))
    .font_features(sans_features())
    .color(theme.muted);
  let arrow_attrs = base.clone().color(theme.link);
  buf.set_rich_text(
    [(prefix.as_str(), base.clone()), (ARROW, arrow_attrs)],
    &base,
    Shaping::Advanced,
    None,
  );
  buf.shape_until_scroll(fs, false);
  (buf, total_bytes)
}

fn layout_code_block(
  lang: &str,
  code: &str,
  targets: Option<Vec<JsonRange>>,
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  let s = ctx.scale;
  let pad_x = CODE_PAD_X * s;
  let pad_y = CODE_PAD_Y * s;
  let inner_w = (w - pad_x * 2.0).max(80.0);
  // JSON-mode block: hand-classify with our own palette to keep JSON5
  // literals (NaN, Infinity, hex) legible on the dark theme — syntect's
  // grammar scopes them as "invalid" and their dark color sits a few
  // points above the background.
  let spans = if targets.is_some() {
    Arc::new(crate::json::highlight_canonical(
      code.trim_end_matches('\n'),
      theme.is_dark,
    ))
  } else {
    highlight(
      code.trim_end_matches('\n'),
      lang,
      theme.is_dark,
      ctx.full_highlight,
    )
  };
  let buf = build_highlighted_buffer(
    fs,
    spans.as_ref(),
    CODE_FS * s,
    CODE_FS * s * CODE_LH_RATIO,
    inner_w,
  );
  let inner_h = buffer_height(&buf);
  let block_h = inner_h + pad_y * 2.0;
  let lang_label = if !lang.is_empty() {
    Some(make_plain_buffer(
      fs,
      &lang.to_uppercase(),
      LANG_LABEL_FS * s,
      LANG_LABEL_FS * s * 1.2,
      120.0 * s,
      FONT_SANS,
    ))
  } else {
    None
  };
  (
    vec![LaidBlock {
      y: 0.0,
      h: block_h,
      x,
      kind: LaidKind::CodeBlock {
        buffer: buf,
        bg: theme.code_bg,
        width: w,
        pad_x,
        pad_y,
        lang_label,
        lang_label_color: theme.muted,
        lang: lang.to_string(),
        source: code.trim_end_matches('\n').to_string(),
        targets,
      },
    }],
    block_h,
  )
}

fn build_highlighted_buffer(
  fs: &mut FontSystem,
  spans: &[HlSpan],
  font_size: f32,
  line_height: f32,
  width: f32,
) -> Buffer {
  let metrics = Metrics::new(font_size, line_height);
  let mut buf = Buffer::new(fs, metrics);
  buf.set_size(Some(width), None);
  let default_attrs = Attrs::new()
    .family(Family::Name(FONT_MONO))
    .font_features(mono_features());

  let rich: Vec<(&str, Attrs)> = spans
    .iter()
    .map(|s| {
      let mut a = Attrs::new()
        .family(Family::Name(FONT_MONO))
        .font_features(mono_features())
        .color(s.fg);
      if s.bold {
        a = a.weight(Weight::BOLD);
      }
      if s.italic {
        a = a.style(Style::Italic);
      }
      (s.text.as_str(), a)
    })
    .collect();

  // T6 (mono fast-path): code blocks use JBM (fixed-pitch). Shaping::Basic
  // skips BiDi + script segmentation + the OpenType shaper. Loses the
  // `calt`/`liga` substitutions for `->`/`=>`/`!=`/`>=`/`<=`/`==`/`::`/...
  // since those are applied by the shaper. ASCII code is still readable
  // — `-` `>` instead of `→` etc. Trades visual polish for shaping speed.
  if rich.is_empty() {
    buf.set_text("", &default_attrs, Shaping::Basic, None);
  } else {
    buf.set_rich_text(rich, &default_attrs, Shaping::Basic, None);
  }
  buf.shape_until_scroll(fs, false);
  buf
}

fn layout_list(
  ordered: bool,
  start: u64,
  items: &[ListItem],
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  let s = ctx.scale;
  let task_box = TASK_BOX * s;
  let gap = LIST_MARKER_GAP * s;
  let min_marker_w = LIST_MARKER_W * s;
  let marker_buf_w = w.max(min_marker_w);

  let mut marker_bufs: Vec<Option<(Buffer, f32)>> = items
    .iter()
    .enumerate()
    .map(|(i, item)| {
      if item.task.is_some() {
        return None;
      }
      let text = if ordered {
        format!("{}.", start + i as u64)
      } else {
        "•".into()
      };
      let metrics = Metrics::new(BODY_FS * s, BODY_FS * s * BODY_LH_RATIO);
      let mut buf = Buffer::new(fs, metrics);
      buf.set_size(Some(marker_buf_w), None);
      let attrs = Attrs::new()
        .family(Family::Name(FONT_SANS))
        .font_features(marker_features());
      buf.set_text(&text, &attrs, Shaping::Advanced, None);
      buf.shape_until_scroll(fs, false);
      let measured = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
      Some((buf, measured))
    })
    .collect();

  let widest = marker_bufs
    .iter()
    .filter_map(|m| m.as_ref().map(|(_, mw)| *mw))
    .fold(0.0_f32, f32::max);
  let marker_w = min_marker_w.max(widest);
  let indent = marker_w + gap;
  let item_x = x + indent;
  let item_w = (w - indent).max(80.0);
  let mut all: Vec<LaidBlock> = Vec::new();
  let mut total = 0.0_f32;
  for (i, item) in items.iter().enumerate() {
    if i > 0 {
      total += LIST_ITEM_GAP * s;
    }

    if let Some(checked) = item.task {
      let baseline_offset = ((BODY_FS * s * BODY_LH_RATIO) - task_box) / 2.0;
      all.push(LaidBlock {
        y: total + baseline_offset,
        h: task_box,
        x: x + marker_w - task_box,
        kind: LaidKind::TaskBox { checked },
      });
    } else if let Some((buf, measured)) = marker_bufs[i].take() {
      let mh = buffer_height(&buf);
      all.push(LaidBlock {
        y: total,
        h: mh,
        x: x + marker_w - measured,
        kind: LaidKind::Text {
          buffer: buf,
          color: theme.muted,
          underlines: Vec::new(),
          strikes: Vec::new(),
          code_runs: Vec::new(),
          links: Vec::new(),
        },
      });
    }

    let inner_gap = LIST_ITEM_GAP * 2.0 * s;
    let (mut item_laid, item_h) =
      layout_blocks(&item.blocks, item_w, item_x, fs, theme, ctx, inner_gap);
    for lb in item_laid.iter_mut() {
      lb.y += total;
    }
    all.extend(item_laid);
    total += item_h;
  }
  (all, total)
}

/// Shared callout primitive used by both `Block::Quote` and `Block::Alert`.
/// One left bar + indented body, with an optional bold title row above the
/// body. Quote = no title + muted bar. Alert = title + kind-colored bar.
fn layout_callout(
  blocks: &[Block],
  title: Option<(&str, Color)>,
  bar_color: SkColor,
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  let s = ctx.scale;
  let indent = QUOTE_INDENT * s;
  let inner_x = x + indent;
  let inner_w = (w - indent).max(80.0);

  let (mut title_laid, title_h, title_to_body) = match title {
    Some((label, color)) => {
      let inlines = [Inline::Text(label.to_string())];
      let (laid, h) = text_block(
        &inlines,
        color,
        BODY_FS * s,
        BODY_FS * s * BODY_LH_RATIO,
        inner_w,
        inner_x,
        fs,
        theme,
        true,
      );
      (laid, h, CALLOUT_TITLE_GAP * s)
    }
    None => (Vec::new(), 0.0, 0.0),
  };

  let (mut body_laid, body_h) =
    layout_blocks(blocks, inner_w, inner_x, fs, theme, ctx, BLOCK_GAP * s);
  let body_y = title_h + title_to_body;
  for lb in body_laid.iter_mut() {
    lb.y += body_y;
  }
  let total_h = body_y + body_h;

  let vpad = CALLOUT_VPAD * s;
  let bar_y = vpad.min(total_h * 0.5);
  let bar_h = (total_h - 2.0 * vpad).max(0.0);

  let mut all: Vec<LaidBlock> = Vec::with_capacity(title_laid.len() + body_laid.len() + 1);
  all.push(LaidBlock {
    y: bar_y,
    h: bar_h,
    x,
    kind: LaidKind::Bar {
      color: bar_color,
      width: QUOTE_BAR_W * s,
    },
  });
  all.append(&mut title_laid);
  all.extend(body_laid);
  (all, total_h)
}

fn layout_quote(
  inner: &[Block],
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  layout_callout(inner, None, theme.quote_bar(), w, x, fs, theme, ctx)
}

fn layout_alert(
  kind: AlertKind,
  inner: &[Block],
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  ctx: &Ctx,
) -> (Vec<LaidBlock>, f32) {
  let (bar, title) = theme.alert_colors(kind);
  layout_callout(
    inner,
    Some((kind.label(), title)),
    bar,
    w,
    x,
    fs,
    theme,
    ctx,
  )
}

fn text_block(
  inlines: &[Inline],
  color: Color,
  font_size: f32,
  line_height: f32,
  w: f32,
  x: f32,
  fs: &mut FontSystem,
  theme: &Theme,
  bold_default: bool,
) -> (Vec<LaidBlock>, f32) {
  let runs = build_runs(inlines, theme);
  let (underlines, strikes, code_runs, links) = compute_runs(&runs);
  let buf = build_buffer(fs, &runs, color, font_size, line_height, w, bold_default);
  let h = buffer_height(&buf);
  (
    vec![LaidBlock {
      y: 0.0,
      h,
      x,
      kind: LaidKind::Text {
        buffer: buf,
        color,
        underlines,
        strikes,
        code_runs,
        links,
      },
    }],
    h,
  )
}

fn compute_runs(
  runs: &StyledRuns,
) -> (
  Vec<UnderlineRun>,
  Vec<UnderlineRun>,
  Vec<UnderlineRun>,
  Vec<LinkRange>,
) {
  let mut byte = 0usize;
  let mut underlines = Vec::new();
  let mut strikes = Vec::new();
  let mut code_runs = Vec::new();
  let mut links: Vec<LinkRange> = Vec::new();
  for s in &runs.spans {
    let start = byte;
    let end = byte + s.text.len();
    if s.underline {
      underlines.push(UnderlineRun {
        byte_start: start,
        byte_end: end,
      });
    }
    if s.strike {
      strikes.push(UnderlineRun {
        byte_start: start,
        byte_end: end,
      });
    }
    if s.mono {
      code_runs.push(UnderlineRun {
        byte_start: start,
        byte_end: end,
      });
    }
    if let Some(idx) = s.link {
      links.push(LinkRange {
        byte_start: start,
        byte_end: end,
        target: runs.links[idx].clone(),
      });
    }
    byte = end;
  }
  (underlines, strikes, code_runs, links)
}

/// Re-shape just the code blocks in `laid` with full syntax highlighting,
/// reusing all other blocks as-is. Cheap second-pass after the syntect
/// precompute thread finishes — avoids re-shaping every paragraph, list,
/// table and heading. Heights of code blocks may shift slightly when
/// highlighted spans force different wrapping, so subsequent block y's
/// and the heading/block jump tables are adjusted by accumulated delta.
pub fn upgrade_code_block_highlights(
  laid: &mut LaidDoc,
  fs: &mut FontSystem,
  theme: &Theme,
  scale: f32,
) {
  let mut delta = 0.0_f32;
  let mut delta_points: Vec<(f32, f32)> = Vec::new();

  for block in laid.blocks.iter_mut() {
    let orig_y = block.y;
    block.y += delta;

    if let LaidKind::CodeBlock {
      buffer,
      pad_x,
      pad_y,
      width,
      lang,
      source,
      targets,
      ..
    } = &mut block.kind
    {
      // JSON path skips the syntect upgrade entirely: its colors come
      // from `json::highlight_canonical`, not the syntect cache, so the
      // first-pass highlight is already final.
      if targets.is_some() {
        continue;
      }
      let inner_w = (*width - *pad_x * 2.0).max(80.0);
      let spans = highlight(source, lang, theme.is_dark, true);
      let new_buf = build_highlighted_buffer(
        fs,
        spans.as_ref(),
        CODE_FS * scale,
        CODE_FS * scale * CODE_LH_RATIO,
        inner_w,
      );
      let new_inner_h = buffer_height(&new_buf);
      let new_block_h = new_inner_h + *pad_y * 2.0;
      let dh = new_block_h - block.h;
      *buffer = new_buf;
      block.h = new_block_h;
      if dh != 0.0 {
        delta_points.push((orig_y, dh));
        delta += dh;
      }
    }
  }

  if delta_points.is_empty() {
    return;
  }

  let shift = |y: f32| -> f32 {
    let mut d = 0.0_f32;
    for &(code_y, code_dh) in &delta_points {
      if code_y < y {
        d += code_dh;
      }
    }
    y + d
  };
  for y in laid.heading_ys.iter_mut() {
    *y = shift(*y);
  }
  for y in laid.block_ys.iter_mut() {
    *y = shift(*y);
  }
  laid.total_height += delta;
}

pub fn make_plain_buffer(
  fs: &mut FontSystem,
  text: &str,
  font_size: f32,
  line_height: f32,
  width: f32,
  family: &str,
) -> Buffer {
  let metrics = Metrics::new(font_size, line_height);
  let mut buf = Buffer::new(fs, metrics);
  buf.set_size(Some(width), None);
  let features = if family == FONT_MONO {
    mono_features()
  } else {
    sans_features()
  };
  let attrs = Attrs::new()
    .family(Family::Name(family))
    .font_features(features);
  buf.set_text(text, &attrs, Shaping::Advanced, None);
  buf.shape_until_scroll(fs, false);
  buf
}

pub fn buffer_height(buf: &Buffer) -> f32 {
  let lh = buf.metrics().line_height;
  let lines = buf.layout_runs().count().max(1);
  lines as f32 * lh
}

fn heading_size(level: u8) -> f32 {
  match level {
    1 => 32.0,
    2 => 24.0,
    3 => 20.0,
    4 => 16.0,
    5 => 14.0,
    _ => 13.0,
  }
}
