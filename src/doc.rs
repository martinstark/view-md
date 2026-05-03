use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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
  let parser = Parser::new_ext(md, opts);
  let mut b = Builder::default();
  for ev in parser {
    b.feed(ev);
  }
  let mut blocks = b.finish();
  promote_image_paragraphs(&mut blocks);
  Doc { blocks }
}

/// Promote a paragraph whose only inline content is a single image to a
/// `Block::Image`. This is the canonical "image-as-figure" pattern in
/// markdown — `![alt](url)` on its own line — and the only image
/// presentation we render at block size in v1. Inline images mixed
/// with text continue to render as alt-text in their paragraph.
fn promote_image_paragraphs(blocks: &mut Vec<Block>) {
  for block in blocks.iter_mut() {
    if let Block::Quote(inner) = block {
      promote_image_paragraphs(inner);
    } else if let Block::List { items, .. } = block {
      for item in items.iter_mut() {
        promote_image_paragraphs(&mut item.blocks);
      }
    } else if let Block::Footnotes(defs) = block {
      for def in defs.iter_mut() {
        promote_image_paragraphs(&mut def.blocks);
      }
    }
  }
  for block in blocks.iter_mut() {
    if let Block::Paragraph(inlines) = block {
      if let [Inline::Image { src, alt }] = inlines.as_slice() {
        let (src, alt) = (src.clone(), alt.clone());
        *block = Block::Image { src, alt };
      }
    }
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
          self.push_block(Block::CodeBlock { lang, code });
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
        if let Some(Frame::Item { task, blocks, .. }) = self.stack.pop() {
          if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
            items.push(ListItem { task, blocks });
          }
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
        if let Some(t) = self.table.as_mut() {
          if let Some(row) = t.current_row.take() {
            if t.in_head {
              t.head = row;
            } else {
              t.rows.push(row);
            }
          }
        }
      }
      TagEnd::TableCell => {
        if let Some(Frame::TableCell(inlines)) = self.stack.pop() {
          if let Some(t) = self.table.as_mut() {
            if let Some(row) = t.current_row.as_mut() {
              row.push(inlines);
            } else if t.in_head {
              t.head.push(inlines);
            }
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
    {
      if !pending.is_empty() {
        let inlines = std::mem::take(pending);
        blocks.push(Block::Paragraph(inlines));
      }
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
