use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub enum Block {
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Quote(Vec<Block>),
    CodeBlock { lang: String, code: String },
    Rule,
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
    Doc { blocks: b.finish() }
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    blocks: Vec<Block>,
}

enum Frame {
    Paragraph(Vec<Inline>),
    Heading { level: u8, inlines: Vec<Inline> },
    Strong(Vec<Inline>),
    Em(Vec<Inline>),
    Strike(Vec<Inline>),
    Link { href: String, kids: Vec<Inline> },
    Image { src: String, alt: String },
    Quote(Vec<Block>),
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Item { task: Option<bool>, blocks: Vec<Block> },
    CodeBlock { lang: String, code: String },
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
            Event::FootnoteReference(_) => {}
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
                self.stack.push(Frame::CodeBlock { lang, code: String::new() });
            }
            Tag::List(start) => self.stack.push(Frame::List {
                ordered: start.is_some(),
                start: start.unwrap_or(1),
                items: Vec::new(),
            }),
            Tag::Item => self.stack.push(Frame::Item {
                task: None,
                blocks: Vec::new(),
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
                if let Some(Frame::List { ordered, start, items }) = self.stack.pop() {
                    self.push_block(Block::List { ordered, start, items });
                }
            }
            TagEnd::Item => {
                if let Some(Frame::Item { task, blocks }) = self.stack.pop() {
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
            | Some(Frame::Link { kids: v, .. }) => v.push(inline),
            _ => {}
        }
    }

    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Quote(blocks)) | Some(Frame::Item { blocks, .. }) => blocks.push(block),
            _ => self.blocks.push(block),
        }
    }

    fn finish(self) -> Vec<Block> {
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
        Inline::SoftBreak => out.push(' '),
        Inline::HardBreak => out.push('\n'),
    }
}
