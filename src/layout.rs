use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping};
use tiny_skia::Color as SkColor;

use crate::doc::{Block, Doc, Inline, ListItem};
use crate::inline::{StyledRuns, build_buffer, build_runs};
use crate::text::{FONT_MONO, FONT_SANS};
use crate::theme::Theme;

pub const MAX_CONTENT_W: f32 = 824.0;
pub const PAD_X_MIN: f32 = 48.0;
pub const PAD_Y: f32 = 40.0;
pub const BODY_FS: f32 = 16.0;
pub const BODY_LH_RATIO: f32 = 1.55;
pub const BLOCK_GAP: f32 = 16.0;
pub const HEADING_GAP_TOP: f32 = 24.0;
pub const LIST_INDENT: f32 = 28.0;
pub const LIST_ITEM_GAP: f32 = 4.0;
pub const QUOTE_INDENT: f32 = 16.0;
pub const QUOTE_BAR_W: f32 = 3.0;
pub const TASK_BOX: f32 = 14.0;

pub struct LaidDoc {
    pub blocks: Vec<LaidBlock>,
    pub total_height: f32,
    pub width: f32,
    pub content_x: f32,
    pub content_w: f32,
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
        links: Vec<LinkRange>,
    },
    Rule,
    Bar { color: SkColor, width: f32 },
    TaskBox { checked: bool, color: SkColor, fg: SkColor },
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

pub fn layout(doc: &Doc, surface_w: f32, fs: &mut FontSystem, theme: &Theme) -> LaidDoc {
    let content_w = (surface_w - PAD_X_MIN * 2.0).min(MAX_CONTENT_W).max(120.0);
    let content_x = ((surface_w - content_w) / 2.0).max(PAD_X_MIN);

    let (mut blocks, h) = layout_blocks(&doc.blocks, content_w, content_x, fs, theme);
    for b in blocks.iter_mut() {
        b.y += PAD_Y;
    }

    LaidDoc {
        blocks,
        total_height: h + PAD_Y * 2.0,
        width: surface_w,
        content_x,
        content_w,
    }
}

fn layout_blocks(
    blocks: &[Block],
    w: f32,
    x: f32,
    fs: &mut FontSystem,
    theme: &Theme,
) -> (Vec<LaidBlock>, f32) {
    let mut y = 0.0_f32;
    let mut out: Vec<LaidBlock> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            let gap = if matches!(block, Block::Heading { .. }) {
                HEADING_GAP_TOP
            } else {
                BLOCK_GAP
            };
            y += gap;
        }
        let (mut laid, dy) = layout_block(block, w, x, fs, theme);
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
) -> (Vec<LaidBlock>, f32) {
    match block {
        Block::Heading { level, inlines } => {
            let size = heading_size(*level);
            text_block(inlines, theme.heading, size, size * 1.25, w, x, fs, theme, true)
        }
        Block::Paragraph(inlines) => text_block(
            inlines,
            theme.fg,
            BODY_FS,
            BODY_FS * BODY_LH_RATIO,
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
        Block::CodeBlock { lang, code } => {
            let label = if lang.is_empty() {
                code.trim_end_matches('\n').to_string()
            } else {
                format!("[{}]\n{}", lang, code.trim_end_matches('\n'))
            };
            let buf = make_plain_buffer(fs, &label, BODY_FS - 1.0, (BODY_FS - 1.0) * 1.45, w, FONT_MONO);
            let h = buffer_height(&buf);
            (
                vec![LaidBlock {
                    y: 0.0,
                    h,
                    x,
                    kind: LaidKind::Text {
                        buffer: buf,
                        color: theme.code_fg,
                        underlines: Vec::new(),
                        strikes: Vec::new(),
                        links: Vec::new(),
                    },
                }],
                h,
            )
        }
        Block::List { ordered, start, items } => {
            layout_list(*ordered, *start, items, w, x, fs, theme)
        }
        Block::Quote(inner) => layout_quote(inner, w, x, fs, theme),
    }
}

fn layout_list(
    ordered: bool,
    start: u64,
    items: &[ListItem],
    w: f32,
    x: f32,
    fs: &mut FontSystem,
    theme: &Theme,
) -> (Vec<LaidBlock>, f32) {
    let item_x = x + LIST_INDENT;
    let item_w = (w - LIST_INDENT).max(80.0);
    let mut all: Vec<LaidBlock> = Vec::new();
    let mut total = 0.0_f32;
    let mut idx = start;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            total += LIST_ITEM_GAP;
        }

        if let Some(checked) = item.task {
            let baseline_offset = ((BODY_FS * BODY_LH_RATIO) - TASK_BOX) / 2.0;
            all.push(LaidBlock {
                y: total + baseline_offset,
                h: TASK_BOX,
                x: x + LIST_INDENT - TASK_BOX - 6.0,
                kind: LaidKind::TaskBox {
                    checked,
                    color: theme.border,
                    fg: theme.rule,
                },
            });
        } else {
            let marker = if ordered { format!("{}.", idx) } else { "•".into() };
            let buf = make_plain_buffer(
                fs,
                &marker,
                BODY_FS,
                BODY_FS * BODY_LH_RATIO,
                LIST_INDENT,
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
                    links: Vec::new(),
                },
            });
        }
        idx += 1;

        let (mut item_laid, item_h) = layout_blocks(&item.blocks, item_w, item_x, fs, theme);
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
) -> (Vec<LaidBlock>, f32) {
    let inner_x = x + QUOTE_INDENT;
    let inner_w = (w - QUOTE_INDENT).max(80.0);
    let (inner_laid, inner_h) = layout_blocks(inner, inner_w, inner_x, fs, theme);
    let mut all: Vec<LaidBlock> = Vec::new();
    all.push(LaidBlock {
        y: 0.0,
        h: inner_h,
        x,
        kind: LaidKind::Bar {
            color: theme.quote_bar,
            width: QUOTE_BAR_W,
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
    let (underlines, strikes, links) = compute_runs(&runs);
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
                links,
            },
        }],
        h,
    )
}

fn compute_runs(runs: &StyledRuns) -> (Vec<UnderlineRun>, Vec<UnderlineRun>, Vec<LinkRange>) {
    let mut byte = 0usize;
    let mut underlines = Vec::new();
    let mut strikes = Vec::new();
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
        if let Some(idx) = s.link {
            links.push(LinkRange {
                byte_start: start,
                byte_end: end,
                href: runs.links[idx].clone(),
            });
        }
        byte = end;
    }
    (underlines, strikes, links)
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
