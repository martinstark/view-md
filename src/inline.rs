use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, Weight};

use crate::doc::Inline;
use crate::layout::LinkTarget;
use crate::text::{FONT_MONO, FONT_SANS, mono_features, sans_features};
use crate::theme::Theme;

#[derive(Clone, Debug)]
pub struct StyleSpan {
  pub text: String,
  pub mono: bool,
  pub bold: bool,
  pub italic: bool,
  pub strike: bool,
  pub underline: bool,
  pub color: Option<Color>,
  pub link: Option<usize>,
}

#[derive(Clone, Copy)]
struct StyleState {
  mono: bool,
  bold: bool,
  italic: bool,
  strike: bool,
  underline: bool,
  color: Option<Color>,
  link: Option<usize>,
}

impl StyleState {
  fn root() -> Self {
    Self {
      mono: false,
      bold: false,
      italic: false,
      strike: false,
      underline: false,
      color: None,
      link: None,
    }
  }
}

pub struct StyledRuns {
  pub spans: Vec<StyleSpan>,
  pub links: Vec<LinkTarget>,
}

pub fn build_runs(inlines: &[Inline], theme: &Theme) -> StyledRuns {
  let mut runs = StyledRuns {
    spans: Vec::new(),
    links: Vec::new(),
  };
  walk(inlines, StyleState::root(), theme, &mut runs);
  runs
}

fn walk(inlines: &[Inline], state: StyleState, theme: &Theme, out: &mut StyledRuns) {
  for inline in inlines {
    match inline {
      Inline::Text(s) => push(out, s.clone(), state),
      Inline::Code(s) => {
        let mut s2 = state;
        s2.mono = true;
        s2.color = Some(theme.code_fg);
        push(out, s.clone(), s2);
      }
      Inline::Strong(k) => {
        let mut s2 = state;
        s2.bold = true;
        walk(k, s2, theme, out);
      }
      Inline::Em(k) => {
        let mut s2 = state;
        s2.italic = true;
        walk(k, s2, theme, out);
      }
      Inline::Strike(k) => {
        let mut s2 = state;
        s2.strike = true;
        walk(k, s2, theme, out);
      }
      Inline::Link { href, kids } => {
        let idx = out.links.len();
        out.links.push(LinkTarget::Url(href.clone()));
        let mut s2 = state;
        s2.color = Some(theme.link);
        s2.underline = true;
        s2.link = Some(idx);
        walk(kids, s2, theme, out);
      }
      Inline::Image { alt, .. } => {
        let mut s2 = state;
        s2.italic = true;
        s2.color = Some(theme.muted);
        push(out, format!("[{}]", alt), s2);
      }
      Inline::FootnoteRef(label) => {
        let idx = out.links.len();
        out.links.push(LinkTarget::Footnote(label.clone()));
        let mut s2 = state;
        s2.color = Some(theme.link);
        s2.link = Some(idx);
        push(out, format!("[{}]", label), s2);
      }
      Inline::SoftBreak => push(out, " ".into(), state),
      Inline::HardBreak => push(out, "\n".into(), state),
    }
  }
}

fn push(out: &mut StyledRuns, text: String, s: StyleState) {
  if text.is_empty() {
    return;
  }
  out.spans.push(StyleSpan {
    text,
    mono: s.mono,
    bold: s.bold,
    italic: s.italic,
    strike: s.strike,
    underline: s.underline,
    color: s.color,
    link: s.link,
  });
}

pub fn build_buffer(
  fs: &mut FontSystem,
  runs: &StyledRuns,
  base_color: Color,
  font_size: f32,
  line_height: f32,
  width: f32,
  bold_default: bool,
) -> Buffer {
  let metrics = Metrics::new(font_size, line_height);
  let mut buf = Buffer::new(fs, metrics);
  buf.set_size(Some(width), None);

  let default_attrs = base_attrs(base_color, bold_default);
  let spans: Vec<(&str, Attrs)> = runs
    .spans
    .iter()
    .map(|s| (s.text.as_str(), span_attrs(s, base_color, bold_default)))
    .collect();

  if spans.is_empty() {
    buf.set_text("", &default_attrs, Shaping::Advanced, None);
  } else {
    buf.set_rich_text(spans.into_iter(), &default_attrs, Shaping::Advanced, None);
  }
  buf.shape_until_scroll(fs, false);
  buf
}

fn base_attrs<'a>(color: Color, bold: bool) -> Attrs<'a> {
  let mut a = Attrs::new()
    .family(Family::Name(FONT_SANS))
    .color(color)
    .font_features(sans_features());
  if bold {
    a = a.weight(Weight::BOLD);
  }
  a
}

fn span_attrs<'a>(s: &'a StyleSpan, base_color: Color, bold_default: bool) -> Attrs<'a> {
  let mono = s.mono;
  let family = if mono { FONT_MONO } else { FONT_SANS };
  let features = if mono {
    mono_features()
  } else {
    sans_features()
  };
  let mut a = Attrs::new()
    .family(Family::Name(family))
    .font_features(features);
  a = a.color(s.color.unwrap_or(base_color));
  if s.bold || bold_default {
    a = a.weight(Weight::BOLD);
  }
  if s.italic {
    a = a.style(Style::Italic);
  }
  a
}
