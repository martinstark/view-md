use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, Weight};
use tiny_skia::Color as SkColor;

use crate::doc::{Block, CellAlign, Doc, FootnoteDef, Inline, ListItem};
use crate::highlight::{HlSpan, highlight};
use crate::inline::{StyledRuns, build_buffer, build_runs};
use crate::text::{FONT_MONO, FONT_SANS};
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
pub const LIST_INDENT: f32 = 28.0;
pub const LIST_ITEM_GAP: f32 = 4.0;
pub const QUOTE_INDENT: f32 = 16.0;
pub const QUOTE_BAR_W: f32 = 3.0;
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
    Bar { color: SkColor, width: f32 },
    TaskBox { checked: bool },
    CodeBlock {
        buffer: Buffer,
        bg: SkColor,
        width: f32,
        pad_x: f32,
        pad_y: f32,
        lang_label: Option<Buffer>,
        lang_label_color: Color,
        source: String,
    },
    Table {
        block_w: f32,
        rows: Vec<TableRowLayout>,
        border: SkColor,
        header_bg: SkColor,
        alt_bg: SkColor,
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
    pub href: String,
}

pub fn layout(
    doc: &Doc,
    surface_w: f32,
    fs: &mut FontSystem,
    theme: &Theme,
    full_highlight: bool,
    scale: f32,
) -> LaidDoc {
    let pad_x = PAD_X_MIN * scale;
    let pad_y = PAD_Y * scale;
    let content_w = (surface_w - pad_x * 2.0).min(MAX_CONTENT_W * scale).max(120.0);
    let content_x = ((surface_w - content_w) / 2.0).max(pad_x);

    let ctx = Ctx { full_highlight, scale };

    let mut heading_ys = Vec::new();
    let mut block_ys = Vec::new();

    let mut y = 0.0_f32;
    let mut blocks: Vec<LaidBlock> = Vec::new();
    for (i, block) in doc.blocks.iter().enumerate() {
        if i > 0 {
            let gap = if matches!(block, Block::Heading { .. }) {
                HEADING_GAP_TOP * scale
            } else {
                BLOCK_GAP * scale
            };
            y += gap;
        }
        block_ys.push(y + pad_y);
        if matches!(block, Block::Heading { .. }) {
            heading_ys.push(y + pad_y);
        }
        let (mut laid, dy) = layout_block(block, content_w, content_x, fs, theme, &ctx);
        for lb in laid.iter_mut() {
            lb.y += y;
        }
        blocks.extend(laid);
        y += dy;
    }
    for b in blocks.iter_mut() {
        b.y += pad_y;
    }

    LaidDoc {
        blocks,
        total_height: y + pad_y * 2.0,
        width: surface_w,
        content_x,
        content_w,
        heading_ys,
        block_ys,
    }
}

struct Ctx {
    full_highlight: bool,
    scale: f32,
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
            text_block(inlines, theme.heading, size, size * 1.25, w, x, fs, theme, true)
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
            vec![LaidBlock { y: 0.0, h: 1.0, x, kind: LaidKind::Rule }],
            1.0,
        ),
        Block::CodeBlock { lang, code } => layout_code_block(lang, code, w, x, fs, theme, ctx),
        Block::List { ordered, start, items } => {
            layout_list(*ordered, *start, items, w, x, fs, theme, ctx)
        }
        Block::Quote(inner) => layout_quote(inner, w, x, fs, theme, ctx),
        Block::Table { aligns, head, rows } => {
            layout_table(aligns, head, rows, w, x, fs, theme, ctx)
        }
        Block::Footnotes(defs) => layout_footnotes(defs, w, x, fs, theme, ctx),
    }
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
    let cols = head.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0)).max(1);
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
            let (underlines, strikes, code_runs, _links) = compute_runs(&runs);
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
    let mut all: Vec<LaidBlock> = Vec::new();
    let mut total = 0.0_f32;

    let header = make_plain_buffer(
        fs,
        "Footnotes",
        heading_size(3),
        heading_size(3) * 1.25,
        w,
        FONT_SANS,
    );
    let hh = buffer_height(&header);
    all.push(LaidBlock {
        y: 0.0,
        h: hh,
        x,
        kind: LaidKind::Text {
            buffer: header,
            color: theme.heading,
            underlines: Vec::new(),
            strikes: Vec::new(),
            code_runs: Vec::new(),
            links: Vec::new(),
        },
    });
    total += hh + BLOCK_GAP;
    all.push(LaidBlock {
        y: total - BLOCK_GAP * 0.5,
        h: 1.0,
        x,
        kind: LaidKind::Rule,
    });

    for (i, def) in defs.iter().enumerate() {
        if i > 0 {
            total += LIST_ITEM_GAP * 2.0;
        }
        let label_buf = make_plain_buffer(
            fs,
            &format!("{}.", def.label),
            BODY_FS - 1.0,
            (BODY_FS - 1.0) * BODY_LH_RATIO,
            32.0,
            FONT_SANS,
        );
        let lh = buffer_height(&label_buf);
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
                links: Vec::new(),
            },
        });
        let body_x = x + 32.0;
        let body_w = (w - 32.0).max(80.0);
        let (mut laid, dy) =
            layout_blocks(&def.blocks, body_w, body_x, fs, theme, ctx, BLOCK_GAP * ctx.scale);
        for lb in laid.iter_mut() {
            lb.y += total;
        }
        all.extend(laid);
        total += dy;
    }

    (all, total)
}


fn layout_code_block(
    lang: &str,
    code: &str,
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
    let spans = highlight(code.trim_end_matches('\n'), lang, theme.is_dark, ctx.full_highlight);
    let buf = build_highlighted_buffer(fs, spans.as_ref(), CODE_FS * s, CODE_FS * s * CODE_LH_RATIO, inner_w);
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
                source: code.trim_end_matches('\n').to_string(),
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
    buf.set_size(fs, Some(width), None);
    let default_attrs = Attrs::new().family(Family::Name(FONT_MONO));

    let rich: Vec<(&str, Attrs)> = spans
        .iter()
        .map(|s| {
            let mut a = Attrs::new().family(Family::Name(FONT_MONO)).color(s.fg);
            if s.bold {
                a = a.weight(Weight::BOLD);
            }
            if s.italic {
                a = a.style(Style::Italic);
            }
            (s.text.as_str(), a)
        })
        .collect();

    if rich.is_empty() {
        buf.set_text(fs, "", default_attrs, Shaping::Advanced);
    } else {
        buf.set_rich_text(fs, rich.into_iter(), default_attrs, Shaping::Advanced);
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
    let indent = LIST_INDENT * s;
    let task_box = TASK_BOX * s;
    let item_x = x + indent;
    let item_w = (w - indent).max(80.0);
    let mut all: Vec<LaidBlock> = Vec::new();
    let mut total = 0.0_f32;
    let mut idx = start;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            total += LIST_ITEM_GAP * s;
        }

        if let Some(checked) = item.task {
            let baseline_offset = ((BODY_FS * s * BODY_LH_RATIO) - task_box) / 2.0;
            all.push(LaidBlock {
                y: total + baseline_offset,
                h: task_box,
                x: x + indent - task_box - 6.0 * s,
                kind: LaidKind::TaskBox { checked },
            });
        } else {
            let marker = if ordered { format!("{}.", idx) } else { "•".into() };
            let buf = make_plain_buffer(
                fs,
                &marker,
                BODY_FS * s,
                BODY_FS * s * BODY_LH_RATIO,
                indent,
                FONT_SANS,
            );
            let mh = buffer_height(&buf);
            all.push(LaidBlock {
                y: total,
                h: mh,
                x,
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
        idx += 1;

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

fn layout_quote(
    inner: &[Block],
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
    let (inner_laid, inner_h) =
        layout_blocks(inner, inner_w, inner_x, fs, theme, ctx, BLOCK_GAP * s);
    let mut all: Vec<LaidBlock> = Vec::new();
    all.push(LaidBlock {
        y: 0.0,
        h: inner_h,
        x,
        kind: LaidKind::Bar {
            color: theme.quote_bar,
            width: QUOTE_BAR_W * s,
        },
    });
    all.extend(inner_laid);
    (all, inner_h)
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
            underlines.push(UnderlineRun { byte_start: start, byte_end: end });
        }
        if s.strike {
            strikes.push(UnderlineRun { byte_start: start, byte_end: end });
        }
        if s.mono {
            code_runs.push(UnderlineRun { byte_start: start, byte_end: end });
        }
        if let Some(idx) = s.link {
            links.push(LinkRange {
                byte_start: start,
                byte_end: end,
                href: runs.links[idx].clone(),
            });
        }
        byte = end;
    }
    (underlines, strikes, code_runs, links)
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
    buf.set_size(fs, Some(width), None);
    let attrs = Attrs::new().family(Family::Name(family));
    buf.set_text(fs, text, attrs, Shaping::Advanced);
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
