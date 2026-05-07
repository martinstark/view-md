use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::json::JsonRange;

/// Position of a JSON chunk within its chunked group. `First` rounds
/// only the top corners and pads only at the top; `Last` is the mirror;
/// `Middle` has no rounded corners and no top/bottom padding so it
/// stitches seamlessly between siblings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkRole {
  First,
  Middle,
  Last,
}

#[derive(Debug)]
pub struct Doc {
  pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy)]
pub enum CellAlign {
  Left,
  Center,
  Right,
}

impl From<Alignment> for CellAlign {
  fn from(a: Alignment) -> Self {
    match a {
      Alignment::Right => CellAlign::Right,
      Alignment::Center => CellAlign::Center,
      _ => CellAlign::Left,
    }
  }
}

#[derive(Debug)]
pub enum Block {
  Heading {
    level: u8,
    inlines: Vec<Inline>,
  },
  Paragraph(Vec<Inline>),
  List {
    ordered: bool,
    start: u64,
    items: Vec<ListItem>,
  },
  Quote(Vec<Block>),
  CodeBlock {
    lang: String,
    code: String,
    /// Per-token byte ranges (keys + values) into `code`. Only populated
    /// when this block was synthesized from a JSON / JSONC / JSON5 input
    /// in `lib::run`; markdown-fenced blocks leave this `None` and the
    /// hint mode falls back to the existing whole-block copy target.
    targets: Option<Vec<JsonRange>>,
    /// `Some(_)` when this block is one slice of a chunked JSON
    /// document (see `lib::chunk_json`). Layout / paint use the role to
    /// suppress the inter-block gap, drop top/bottom padding on
    /// non-edge chunks, and round only the outward-facing corners so
    /// adjacent chunks render as one continuous block. `None` for
    /// fenced markdown code blocks and single-chunk JSON.
    chunk: Option<ChunkRole>,
  },
  Rule,
  Table {
    aligns: Vec<CellAlign>,
    head: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
  },
  Footnotes(Vec<FootnoteDef>),
  Image {
    src: String,
    alt: String,
  },
  Alert {
    kind: AlertKind,
    blocks: Vec<Block>,
  },
  /// YAML/TOML frontmatter block extracted from the top of the file.
  /// `entries` is a flat list of (key, value) pairs in source order;
  /// lines that don't have a `key: value` shape are stored with an
  /// empty key so the painter can render them verbatim.
  Frontmatter {
    entries: Vec<(String, String)>,
  },
}

/// GitHub-flavored markdown alert kinds. Recognized in a blockquote
/// whose first line is exactly `[!KIND]`. Spelled in capital letters in
/// the source (`[!NOTE]`, `[!TIP]`, etc.), matching GFM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
  Note,
  Tip,
  Important,
  Warning,
  Caution,
}

impl AlertKind {
  fn from_marker(s: &str) -> Option<Self> {
    match s {
      "[!NOTE]" => Some(Self::Note),
      "[!TIP]" => Some(Self::Tip),
      "[!IMPORTANT]" => Some(Self::Important),
      "[!WARNING]" => Some(Self::Warning),
      "[!CAUTION]" => Some(Self::Caution),
      _ => None,
    }
  }

  /// Title-case label rendered above the alert body.
  pub fn label(self) -> &'static str {
    match self {
      Self::Note => "Note",
      Self::Tip => "Tip",
      Self::Important => "Important",
      Self::Warning => "Warning",
      Self::Caution => "Caution",
    }
  }
}

#[derive(Debug)]
pub struct FootnoteDef {
  pub label: String,
  pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub struct ListItem {
  pub task: Option<bool>,
  pub blocks: Vec<Block>,
}

#[derive(Debug, Clone)]
pub enum Inline {
  Text(String),
  Code(String),
  Strong(Vec<Inline>),
  Em(Vec<Inline>),
  Strike(Vec<Inline>),
  Link { href: String, kids: Vec<Inline> },
  Image { src: String, alt: String },
  FootnoteRef(String),
  SoftBreak,
  HardBreak,
}

pub fn parse(md: &str) -> Doc {
  let opts = Options::ENABLE_TABLES
    | Options::ENABLE_STRIKETHROUGH
    | Options::ENABLE_TASKLISTS
    | Options::ENABLE_FOOTNOTES
    | Options::ENABLE_HEADING_ATTRIBUTES
    | Options::ENABLE_SMART_PUNCTUATION;
  let (frontmatter, body) = extract_frontmatter(md);
  let parser = Parser::new_ext(body, opts);
  let mut b = Builder::default();
  for ev in parser {
    b.feed(ev);
  }
  let mut blocks = b.finish();
  promote_image_paragraphs(&mut blocks);
  promote_alerts(&mut blocks);
  if let Some(entries) = frontmatter
    && !entries.is_empty()
  {
    blocks.insert(0, Block::Frontmatter { entries });
  }
  Doc { blocks }
}

/// If `source` opens with a YAML/TOML-style frontmatter block delimited
/// by `---` lines, peel it off and parse it into (key, value) pairs.
/// The body returned is the source from the line after the closing
/// `---` onward — the markdown parser sees only that. Closing
/// delimiter must be on its own line for the block to count; if it's
/// missing, the whole source falls through as plain markdown.
fn extract_frontmatter(source: &str) -> (Option<Vec<(String, String)>>, &str) {
  let after_open = if let Some(rest) = source.strip_prefix("---\n") {
    rest
  } else if let Some(rest) = source.strip_prefix("---\r\n") {
    rest
  } else {
    return (None, source);
  };
  // Search for a line that is exactly `---` (with optional CR), at the
  // start of a line, terminated by `\n` or end-of-input.
  let mut search = 0usize;
  let close = loop {
    let Some(rel) = after_open[search..].find("\n---") else {
      return (None, source);
    };
    let line_start = search + rel + 1;
    let after_marker = line_start + 3;
    let tail = &after_open[after_marker..];
    if tail.is_empty() || tail.starts_with('\n') {
      break (line_start, after_marker + tail.starts_with('\n') as usize);
    }
    if let Some(rest) = tail.strip_prefix("\r\n") {
      let _ = rest;
      break (line_start, after_marker + 2);
    }
    if tail.starts_with('\r') && tail.get(1..2) == Some("\n") {
      break (line_start, after_marker + 2);
    }
    search = after_marker;
  };
  let (content_end, body_start_in_after) = close;
  let content = &after_open[..content_end];
  let body = &after_open[body_start_in_after..];
  let entries: Vec<(String, String)> = content
    .lines()
    .filter(|l| !l.trim().is_empty())
    .map(|l| match l.split_once(':') {
      Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
      None => (String::new(), l.trim().to_string()),
    })
    .collect();
  (Some(entries), body)
}

/// Promote a paragraph whose only inline content is a single image to a
/// `Block::Image`. This is the canonical "image-as-figure" pattern in
/// markdown — `![alt](url)` on its own line — and the only image
/// presentation we render at block size in v1. Inline images mixed
/// with text continue to render as alt-text in their paragraph.
fn promote_image_paragraphs(blocks: &mut [Block]) {
  for block in blocks.iter_mut() {
    walk_children_mut(block, promote_image_paragraphs);
  }
  for block in blocks.iter_mut() {
    if let Block::Paragraph(inlines) = block
      && let [Inline::Image { src, alt }] = inlines.as_slice()
    {
      let (src, alt) = (src.clone(), alt.clone());
      *block = Block::Image { src, alt };
    }
  }
}

/// Promote `Block::Quote` to `Block::Alert` when its first line is
/// `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, or `[!CAUTION]`.
/// The marker line is stripped from the body; the rest renders inside
/// the alert. Per GFM, the marker must be on its own line at the top
/// of the quote — anything else is left as a regular quote.
fn promote_alerts(blocks: &mut [Block]) {
  for block in blocks.iter_mut() {
    walk_children_mut(block, promote_alerts);
  }
  for block in blocks.iter_mut() {
    let Block::Quote(inner) = block else { continue };
    let Some(kind) = detect_alert_marker(inner) else {
      continue;
    };
    let mut body = std::mem::take(inner);
    strip_alert_marker(&mut body);
    *block = Block::Alert { kind, blocks: body };
  }
}

fn walk_children_mut(block: &mut Block, f: fn(&mut [Block])) {
  match block {
    Block::Quote(inner) => f(inner),
    Block::List { items, .. } => {
      for item in items.iter_mut() {
        f(&mut item.blocks);
      }
    }
    Block::Footnotes(defs) => {
      for def in defs.iter_mut() {
        f(&mut def.blocks);
      }
    }
    Block::Alert { blocks, .. } => f(blocks),
    _ => {}
  }
}

fn detect_alert_marker(blocks: &[Block]) -> Option<AlertKind> {
  let first = blocks.first()?;
  let Block::Paragraph(inlines) = first else {
    return None;
  };
  // pulldown-cmark splits `[!NOTE]` into three Text inlines (`[`, `!NOTE`,
  // `]`) because the brackets parse as link-reference syntax that fails to
  // resolve. Concatenate every leading Text up to the first break and
  // check the result. `Text("[!NOTE]")` (single-inline) also works.
  let mut concat = String::new();
  for inl in inlines {
    match inl {
      Inline::Text(t) => concat.push_str(t),
      Inline::SoftBreak | Inline::HardBreak => break,
      _ => return None,
    }
  }
  AlertKind::from_marker(concat.trim())
}

fn strip_alert_marker(blocks: &mut Vec<Block>) {
  let Some(Block::Paragraph(inlines)) = blocks.first_mut() else {
    return;
  };
  // Drop leading Text inlines (the `[`, `!KIND`, `]` triple, or a single
  // `Text("[!KIND]")` if pulldown-cmark ever changes its bracket parsing)
  // and the soft/hard break that separated marker from body.
  while matches!(inlines.first(), Some(Inline::Text(_))) {
    inlines.remove(0);
  }
  if matches!(inlines.first(), Some(Inline::SoftBreak | Inline::HardBreak)) {
    inlines.remove(0);
  }
  if inlines.is_empty() {
    blocks.remove(0);
  }
}

#[derive(Default)]
struct PendingTable {
  aligns: Vec<CellAlign>,
  head: Vec<Vec<Inline>>,
  rows: Vec<Vec<Vec<Inline>>>,
  in_head: bool,
  current_row: Option<Vec<Vec<Inline>>>,
}

#[derive(Default)]
struct Builder {
  stack: Vec<Frame>,
  blocks: Vec<Block>,
  footnotes: Vec<FootnoteDef>,
  table: Option<PendingTable>,
}

enum Frame {
  Paragraph(Vec<Inline>),
  Heading {
    level: u8,
    inlines: Vec<Inline>,
  },
  Strong(Vec<Inline>),
  Em(Vec<Inline>),
  Strike(Vec<Inline>),
  Link {
    href: String,
    kids: Vec<Inline>,
  },
  Image {
    src: String,
    alt: String,
  },
  Quote(Vec<Block>),
  List {
    ordered: bool,
    start: u64,
    items: Vec<ListItem>,
  },
  Item {
    task: Option<bool>,
    blocks: Vec<Block>,
    pending: Vec<Inline>,
  },
  CodeBlock {
    lang: String,
    code: String,
  },
  TableCell(Vec<Inline>),
  FootnoteDef {
    label: String,
    blocks: Vec<Block>,
  },
}

impl Builder {
  fn feed(&mut self, ev: Event<'_>) {
    match ev {
      Event::Start(tag) => self.start(tag),
      Event::End(tag) => self.end(tag),
      Event::Text(t) => self.push_text(t.into_string()),
      Event::Code(t) => self.push_inline(Inline::Code(t.into_string())),
      Event::SoftBreak => self.push_inline(Inline::SoftBreak),
      Event::HardBreak => self.push_inline(Inline::HardBreak),
      Event::Rule => self.push_block(Block::Rule),
      Event::TaskListMarker(checked) => {
        if let Some(Frame::Item { task, .. }) = self.stack.last_mut() {
          *task = Some(checked);
        }
      }
      Event::Html(_) | Event::InlineHtml(_) => {}
      Event::FootnoteReference(label) => {
        self.push_inline(Inline::FootnoteRef(label.into_string()));
      }
      _ => {}
    }
  }

  fn start(&mut self, tag: Tag<'_>) {
    match tag {
      Tag::Paragraph => self.stack.push(Frame::Paragraph(Vec::new())),
      Tag::Heading { level, .. } => self.stack.push(Frame::Heading {
        level: heading_level(level),
        inlines: Vec::new(),
      }),
      Tag::BlockQuote(_) => self.stack.push(Frame::Quote(Vec::new())),
      Tag::CodeBlock(kind) => {
        let lang = match kind {
          pulldown_cmark::CodeBlockKind::Fenced(s) => s.into_string(),
          pulldown_cmark::CodeBlockKind::Indented => String::new(),
        };
        self.stack.push(Frame::CodeBlock {
          lang,
          code: String::new(),
        });
      }
      Tag::List(start) => self.stack.push(Frame::List {
        ordered: start.is_some(),
        start: start.unwrap_or(1),
        items: Vec::new(),
      }),
      Tag::Item => self.stack.push(Frame::Item {
        task: None,
        blocks: Vec::new(),
        pending: Vec::new(),
      }),
      Tag::Emphasis => self.stack.push(Frame::Em(Vec::new())),
      Tag::Strong => self.stack.push(Frame::Strong(Vec::new())),
      Tag::Strikethrough => self.stack.push(Frame::Strike(Vec::new())),
      Tag::Link { dest_url, .. } => self.stack.push(Frame::Link {
        href: dest_url.into_string(),
        kids: Vec::new(),
      }),
      Tag::Image { dest_url, .. } => self.stack.push(Frame::Image {
        src: dest_url.into_string(),
        alt: String::new(),
      }),
      Tag::Table(aligns) => {
        self.table = Some(PendingTable {
          aligns: aligns.into_iter().map(CellAlign::from).collect(),
          ..Default::default()
        });
      }
      Tag::TableHead => {
        if let Some(t) = self.table.as_mut() {
          t.in_head = true;
        }
      }
      Tag::TableRow => {
        if let Some(t) = self.table.as_mut() {
          t.current_row = Some(Vec::new());
        }
      }
      Tag::TableCell => {
        self.stack.push(Frame::TableCell(Vec::new()));
      }
      Tag::FootnoteDefinition(label) => {
        self.stack.push(Frame::FootnoteDef {
          label: label.into_string(),
          blocks: Vec::new(),
        });
      }
      _ => {}
    }
  }

  fn end(&mut self, tag: TagEnd) {
    match tag {
      TagEnd::Paragraph => {
        if let Some(Frame::Paragraph(inlines)) = self.stack.pop() {
          self.push_block(Block::Paragraph(inlines));
        }
      }
      TagEnd::Heading(_) => {
        if let Some(Frame::Heading { level, inlines }) = self.stack.pop() {
          self.push_block(Block::Heading { level, inlines });
        }
      }
      TagEnd::BlockQuote(_) => {
        if let Some(Frame::Quote(blocks)) = self.stack.pop() {
          self.push_block(Block::Quote(blocks));
        }
      }
      TagEnd::CodeBlock => {
        if let Some(Frame::CodeBlock { lang, code }) = self.stack.pop() {
          self.push_block(Block::CodeBlock {
            lang,
            code,
            targets: None,
            chunk: None,
          });
        }
      }
      TagEnd::List(_) => {
        if let Some(Frame::List {
          ordered,
          start,
          items,
        }) = self.stack.pop()
        {
          self.push_block(Block::List {
            ordered,
            start,
            items,
          });
        }
      }
      TagEnd::Item => {
        self.flush_item_pending();
        if let Some(Frame::Item { task, blocks, .. }) = self.stack.pop()
          && let Some(Frame::List { items, .. }) = self.stack.last_mut()
        {
          items.push(ListItem { task, blocks });
        }
      }
      TagEnd::Emphasis => {
        if let Some(Frame::Em(kids)) = self.stack.pop() {
          self.push_inline(Inline::Em(kids));
        }
      }
      TagEnd::Strong => {
        if let Some(Frame::Strong(kids)) = self.stack.pop() {
          self.push_inline(Inline::Strong(kids));
        }
      }
      TagEnd::Strikethrough => {
        if let Some(Frame::Strike(kids)) = self.stack.pop() {
          self.push_inline(Inline::Strike(kids));
        }
      }
      TagEnd::Link => {
        if let Some(Frame::Link { href, kids }) = self.stack.pop() {
          self.push_inline(Inline::Link { href, kids });
        }
      }
      TagEnd::Image => {
        if let Some(Frame::Image { src, alt }) = self.stack.pop() {
          self.push_inline(Inline::Image { src, alt });
        }
      }
      TagEnd::Table => {
        if let Some(t) = self.table.take() {
          self.push_block(Block::Table {
            aligns: t.aligns,
            head: t.head,
            rows: t.rows,
          });
        }
      }
      TagEnd::TableHead => {
        if let Some(t) = self.table.as_mut() {
          t.in_head = false;
        }
      }
      TagEnd::TableRow => {
        if let Some(t) = self.table.as_mut()
          && let Some(row) = t.current_row.take()
        {
          if t.in_head {
            t.head = row;
          } else {
            t.rows.push(row);
          }
        }
      }
      TagEnd::TableCell => {
        if let Some(Frame::TableCell(inlines)) = self.stack.pop()
          && let Some(t) = self.table.as_mut()
        {
          if let Some(row) = t.current_row.as_mut() {
            row.push(inlines);
          } else if t.in_head {
            t.head.push(inlines);
          }
        }
      }
      TagEnd::FootnoteDefinition => {
        if let Some(Frame::FootnoteDef { label, blocks }) = self.stack.pop() {
          self.footnotes.push(FootnoteDef { label, blocks });
        }
      }
      _ => {}
    }
  }

  fn push_text(&mut self, t: String) {
    match self.stack.last_mut() {
      Some(Frame::CodeBlock { code, .. }) => code.push_str(&t),
      Some(Frame::Image { alt, .. }) => alt.push_str(&t),
      _ => self.push_inline(Inline::Text(t)),
    }
  }

  fn push_inline(&mut self, inline: Inline) {
    match self.stack.last_mut() {
      Some(Frame::Paragraph(v))
      | Some(Frame::Heading { inlines: v, .. })
      | Some(Frame::Strong(v))
      | Some(Frame::Em(v))
      | Some(Frame::Strike(v))
      | Some(Frame::Link { kids: v, .. })
      | Some(Frame::TableCell(v)) => v.push(inline),
      // Tight list items emit text directly without a Paragraph
      // wrapper; collect into pending and flush as an implicit
      // Paragraph at item end (or before any nested block).
      Some(Frame::Item { pending, .. }) => pending.push(inline),
      _ => {}
    }
  }

  fn push_block(&mut self, block: Block) {
    self.flush_item_pending();
    match self.stack.last_mut() {
      Some(Frame::Quote(blocks))
      | Some(Frame::Item { blocks, .. })
      | Some(Frame::FootnoteDef { blocks, .. }) => blocks.push(block),
      _ => self.blocks.push(block),
    }
  }

  fn flush_item_pending(&mut self) {
    if let Some(Frame::Item {
      pending, blocks, ..
    }) = self.stack.last_mut()
      && !pending.is_empty()
    {
      let inlines = std::mem::take(pending);
      blocks.push(Block::Paragraph(inlines));
    }
  }

  fn finish(mut self) -> Vec<Block> {
    if !self.footnotes.is_empty() {
      self.blocks.push(Block::Footnotes(self.footnotes));
    }
    self.blocks
  }
}

fn heading_level(level: HeadingLevel) -> u8 {
  match level {
    HeadingLevel::H1 => 1,
    HeadingLevel::H2 => 2,
    HeadingLevel::H3 => 3,
    HeadingLevel::H4 => 4,
    HeadingLevel::H5 => 5,
    HeadingLevel::H6 => 6,
  }
}

pub fn flatten_text(inlines: &[Inline]) -> String {
  let mut out = String::new();
  for i in inlines {
    flatten_into(i, &mut out);
  }
  out
}

fn flatten_into(i: &Inline, out: &mut String) {
  match i {
    Inline::Text(s) | Inline::Code(s) => out.push_str(s),
    Inline::Strong(k) | Inline::Em(k) | Inline::Strike(k) => {
      for x in k {
        flatten_into(x, out);
      }
    }
    Inline::Link { kids, .. } => {
      for x in kids {
        flatten_into(x, out);
      }
    }
    Inline::Image { alt, .. } => out.push_str(alt),
    Inline::FootnoteRef(label) => {
      out.push('[');
      out.push_str(label);
      out.push(']');
    }
    Inline::SoftBreak => out.push(' '),
    Inline::HardBreak => out.push('\n'),
  }
}
