use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};
use syntect::highlighting::ThemeSet;
use syntect::html::{ClassStyle, ClassedHTMLGenerator, css_for_theme_with_class_style};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub fn render(md: &str) -> String {
    let ss = SyntaxSet::load_defaults_newlines();

    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_SMART_PUNCTUATION;

    let parser = Parser::new_ext(md, opts);
    let events = transform(parser, &ss);

    let mut out = String::with_capacity(md.len() * 2);
    html::push_html(&mut out, events.into_iter());
    out
}

pub fn theme_css() -> (String, String) {
    let ts = ThemeSet::load_defaults();
    let light = css_for_theme_with_class_style(&ts.themes["InspiredGitHub"], ClassStyle::Spaced)
        .unwrap_or_default();
    let dark = css_for_theme_with_class_style(&ts.themes["base16-ocean.dark"], ClassStyle::Spaced)
        .unwrap_or_default();
    (light, dark)
}

fn transform<'a>(parser: Parser<'a>, ss: &SyntaxSet) -> Vec<Event<'a>> {
    let mut out: Vec<Event<'a>> = Vec::new();
    let mut in_code: Option<String> = None;
    let mut buf = String::new();

    for ev in parser {
        match ev {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                in_code = Some(lang.into_string());
                buf.clear();
            }
            Event::Start(Tag::CodeBlock(CodeBlockKind::Indented)) => {
                in_code = Some(String::new());
                buf.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(lang) = in_code.take() {
                    let block = highlight(&buf, &lang, ss);
                    out.push(Event::Html(CowStr::Boxed(block.into_boxed_str())));
                    buf.clear();
                }
            }
            Event::Text(t) if in_code.is_some() => buf.push_str(&t),
            Event::Html(_) | Event::InlineHtml(_) => {} // strip raw HTML
            other => out.push(other),
        }
    }

    out
}

fn highlight(code: &str, lang: &str, ss: &SyntaxSet) -> String {
    let syntax = if lang.is_empty() {
        ss.find_syntax_plain_text()
    } else {
        ss.find_syntax_by_token(lang)
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    };

    let mut gen = ClassedHTMLGenerator::new_with_class_style(syntax, ss, ClassStyle::Spaced);
    for line in LinesWithEndings::from(code) {
        let _ = gen.parse_html_for_line_which_includes_newline(line);
    }
    let inner = gen.finalize();

    format!(
        "<pre class=\"code\" data-lang=\"{}\"><code>{}</code></pre>",
        escape(lang),
        inner
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
