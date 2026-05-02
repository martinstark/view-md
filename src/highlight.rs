use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use cosmic_text::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

type CacheKey = (String, String, bool);
type CachedSpans = Arc<Vec<HlSpan>>;
static CACHE: OnceLock<Mutex<HashMap<CacheKey, CachedSpans>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<CacheKey, CachedSpans>> {
  CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn syntaxes() -> &'static SyntaxSet {
  SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub fn themes() -> &'static ThemeSet {
  THEMES.get_or_init(ThemeSet::load_defaults)
}

#[derive(Clone)]
pub struct HlSpan {
  pub text: String,
  pub fg: Color,
  pub bold: bool,
  pub italic: bool,
}

/// Highlight a code block. Cached by (lang, code, dark) so repeated
/// relayouts (resize / zoom / theme toggle) don't re-run the syntect
/// state machine, which costs 10–25ms per language.
pub fn highlight(code: &str, lang: &str, dark: bool, enabled: bool) -> CachedSpans {
  if !enabled {
    return Arc::new(plain(code, dark));
  }
  let key: CacheKey = (lang.to_string(), code.to_string(), dark);
  if let Some(cached) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
    return cached;
  }
  let spans = Arc::new(compute_highlight(code, lang, dark));
  if let Ok(mut c) = cache().lock() {
    c.insert(key, spans.clone());
  }
  spans
}

fn compute_highlight(code: &str, lang: &str, dark: bool) -> Vec<HlSpan> {
  let ss = syntaxes();
  let ts = themes();
  let theme_name = if dark {
    "base16-ocean.dark"
  } else {
    "InspiredGitHub"
  };
  let theme = match ts.themes.get(theme_name) {
    Some(t) => t,
    None => return plain(code, dark),
  };

  let resolved = alias_lang(lang);
  let syntax = if resolved.is_empty() {
    ss.find_syntax_plain_text()
  } else {
    ss.find_syntax_by_token(resolved)
      .or_else(|| ss.find_syntax_by_name(resolved))
      .unwrap_or_else(|| ss.find_syntax_plain_text())
  };

  let mut hl = HighlightLines::new(syntax, theme);
  let mut out: Vec<HlSpan> = Vec::new();
  for line in LinesWithEndings::from(code) {
    let regions = match hl.highlight_line(line, ss) {
      Ok(v) => v,
      Err(_) => return plain(code, dark),
    };
    for (style, text) in regions {
      if text.is_empty() {
        continue;
      }
      let fg = Color::rgb(style.foreground.r, style.foreground.g, style.foreground.b);
      let bold = style.font_style.contains(FontStyle::BOLD);
      let italic = style.font_style.contains(FontStyle::ITALIC);
      out.push(HlSpan {
        text: text.to_string(),
        fg,
        bold,
        italic,
      });
    }
  }
  out
}

/// Pre-compile syntect's lazy per-language regexes for the given
/// fenced-block tokens. First call to `highlight_line` for a language
/// triggers regex compilation (10–25ms each); doing it on a background
/// thread parallel to window creation removes that cost from the
/// frame-2 relayout. Light-theme codepath is also touched because
/// regex state lives on the SyntaxSet, shared across themes.
pub fn warm_languages(langs: &[String]) {
  let ss = syntaxes();
  let ts = themes();
  let theme = match ts
    .themes
    .get("base16-ocean.dark")
    .or_else(|| ts.themes.values().next())
  {
    Some(t) => t,
    None => return,
  };
  for lang in langs {
    let resolved = alias_lang(lang);
    let syntax = match ss
      .find_syntax_by_token(resolved)
      .or_else(|| ss.find_syntax_by_name(resolved))
    {
      Some(s) => s,
      None => continue,
    };
    let mut hl = HighlightLines::new(syntax, theme);
    let _ = hl.highlight_line("\n", ss);
  }
}

/// Eagerly populate the highlight cache for the given (lang, code) blocks
/// in the active theme only. Spawns one worker per block — different
/// languages compile their regexes independently, so we get near-linear
/// speedup on multi-core machines. Inactive theme is computed lazily.
pub fn precompute(blocks: Vec<(String, String)>, dark: bool) {
  let handles: Vec<_> = blocks
    .into_iter()
    .map(|(lang, code)| {
      std::thread::spawn(move || {
        let key: CacheKey = (lang.clone(), code.clone(), dark);
        if cache().lock().ok().map_or(false, |c| c.contains_key(&key)) {
          return;
        }
        let spans = Arc::new(compute_highlight(&code, &lang, dark));
        if let Ok(mut c) = cache().lock() {
          c.insert(key, spans);
        }
      })
    })
    .collect();
  for h in handles {
    let _ = h.join();
  }
}

// syntect's bundled defaults don't ship TypeScript or some common
// shortnames. Map them to the closest available syntax.
fn alias_lang(lang: &str) -> &str {
  match lang.to_ascii_lowercase().as_str() {
    "ts" | "tsx" | "typescript" | "jsx" => "javascript",
    "sh" | "zsh" | "fish" => "bash",
    "yml" => "yaml",
    "md" => "markdown",
    "rs" => "rust",
    "py" => "python",
    "rb" => "ruby",
    _ => return lang,
  }
}

fn plain(code: &str, dark: bool) -> Vec<HlSpan> {
  let fg = if dark {
    Color::rgb(0xe6, 0xed, 0xf3)
  } else {
    Color::rgb(0x1f, 0x23, 0x28)
  };
  vec![HlSpan {
    text: code.to_string(),
    fg,
    bold: false,
    italic: false,
  }]
}
