use std::sync::OnceLock;

use cosmic_text::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

pub fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub fn themes() -> &'static ThemeSet {
    THEMES.get_or_init(ThemeSet::load_defaults)
}

pub struct HlSpan {
    pub text: String,
    pub fg: Color,
    pub bold: bool,
    pub italic: bool,
}

pub fn highlight(code: &str, lang: &str, dark: bool, enabled: bool) -> Vec<HlSpan> {
    if !enabled {
        return plain(code, dark);
    }
    let ss = syntaxes();
    let ts = themes();
    let theme_name = if dark { "base16-ocean.dark" } else { "InspiredGitHub" };
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
    vec![HlSpan { text: code.to_string(), fg, bold: false, italic: false }]
}
