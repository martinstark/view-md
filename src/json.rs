//! JSON / JSONC / JSON5 reader. Validates input and re-emits it as
//! canonical 2-space-indented JSON, alongside per-token byte ranges
//! used by hint-mode to anchor `f` badges and pick the copy payload.
//!
//! Out of scope (v1):
//! - Comments are accepted by the parser but dropped from the formatted
//!   output. Preserving them with stable placement is fiddly enough to
//!   defer until someone needs it.
//! - Unicode identifier characters in unquoted keys (we accept the ASCII
//!   subset: `[A-Za-z_$][A-Za-z0-9_$]*`).

use std::fmt;

use cosmic_text::Color;

use crate::highlight::HlSpan;

/// A copyable token in the formatted output. `byte_start..byte_end` is
/// the range in the formatted string (not the input); hint-mode uses it
/// to find the visual run for the badge anchor. `copy` is the payload
/// `f` writes to the clipboard — for keys and string values, the
/// unquoted text; for numbers/booleans/null, the literal; for objects
/// and arrays, the verbatim formatted subtree.
#[derive(Debug, Clone)]
pub struct JsonRange {
  pub byte_start: usize,
  pub byte_end: usize,
  pub copy: String,
}

#[derive(Debug)]
pub struct JsonError {
  pub line: usize,
  pub col: usize,
  pub msg: String,
}

impl fmt::Display for JsonError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}:{}: {}", self.line, self.col, self.msg)
  }
}

pub fn format(src: &str) -> Result<(String, Vec<JsonRange>), JsonError> {
  let mut p = Parser::new(src.as_bytes());
  let mut out = String::new();
  let mut ranges = Vec::new();
  p.skip_trivia()?;
  if p.at_eof() {
    return Err(p.err("empty input"));
  }
  p.emit_value(&mut out, &mut ranges, 0)?;
  p.skip_trivia()?;
  if !p.at_eof() {
    return Err(p.err("trailing content after top-level value"));
  }
  Ok((out, ranges))
}

const IND: &str = "  ";

fn push_indent(out: &mut String, depth: usize) {
  for _ in 0..depth {
    out.push_str(IND);
  }
}

struct Parser<'a> {
  src: &'a [u8],
  pos: usize,
  line: usize,
  col: usize,
}

impl<'a> Parser<'a> {
  fn new(src: &'a [u8]) -> Self {
    let mut p = Self {
      src,
      pos: 0,
      line: 1,
      col: 1,
    };
    if src.starts_with(b"\xEF\xBB\xBF") {
      p.pos = 3;
    }
    p
  }

  fn err(&self, msg: &str) -> JsonError {
    JsonError {
      line: self.line,
      col: self.col,
      msg: msg.into(),
    }
  }

  fn err_owned(&self, msg: String) -> JsonError {
    JsonError {
      line: self.line,
      col: self.col,
      msg,
    }
  }

  fn peek(&self) -> Option<u8> {
    self.src.get(self.pos).copied()
  }

  fn peek_at(&self, off: usize) -> Option<u8> {
    self.src.get(self.pos + off).copied()
  }

  fn at_eof(&self) -> bool {
    self.pos >= self.src.len()
  }

  /// Advance one byte. Multi-byte UTF-8 callers should use `bump_char`.
  fn bump(&mut self) -> Option<u8> {
    let b = self.peek()?;
    self.pos += 1;
    if b == b'\n' {
      self.line += 1;
      self.col = 1;
    } else {
      self.col += 1;
    }
    Some(b)
  }

  /// Skip whitespace and JSON5 line/block comments. Comments are
  /// dropped (see module docs).
  fn skip_trivia(&mut self) -> Result<(), JsonError> {
    loop {
      match self.peek() {
        Some(b) if is_ws(b) => {
          self.bump();
        }
        Some(b'/') if self.peek_at(1) == Some(b'/') => {
          self.bump();
          self.bump();
          while let Some(b) = self.peek() {
            if b == b'\n' {
              break;
            }
            self.bump();
          }
        }
        Some(b'/') if self.peek_at(1) == Some(b'*') => {
          self.bump();
          self.bump();
          loop {
            match (self.peek(), self.peek_at(1)) {
              (Some(b'*'), Some(b'/')) => {
                self.bump();
                self.bump();
                break;
              }
              (None, _) => return Err(self.err("unterminated block comment")),
              _ => {
                self.bump();
              }
            }
          }
        }
        _ => break,
      }
    }
    Ok(())
  }

  fn emit_value(
    &mut self,
    out: &mut String,
    ranges: &mut Vec<JsonRange>,
    depth: usize,
  ) -> Result<(), JsonError> {
    self.skip_trivia()?;
    let value_start = out.len();
    match self.peek() {
      Some(b'{') => self.emit_object(out, ranges, depth)?,
      Some(b'[') => self.emit_array(out, ranges, depth)?,
      Some(b'"') | Some(b'\'') => {
        let text = self.read_string()?;
        out.push('"');
        push_json_escaped(out, &text);
        out.push('"');
        ranges.push(JsonRange {
          byte_start: value_start,
          byte_end: out.len(),
          copy: text,
        });
      }
      Some(b'-') | Some(b'+') => {
        let lit = self.read_number()?;
        out.push_str(&lit);
        ranges.push(JsonRange {
          byte_start: value_start,
          byte_end: out.len(),
          copy: lit,
        });
      }
      Some(b) if b.is_ascii_digit() || b == b'.' => {
        let lit = self.read_number()?;
        out.push_str(&lit);
        ranges.push(JsonRange {
          byte_start: value_start,
          byte_end: out.len(),
          copy: lit,
        });
      }
      Some(b) if is_ident_start(b) => {
        let id = self.read_ident();
        match id.as_str() {
          "true" | "false" | "null" | "Infinity" | "NaN" => {
            out.push_str(&id);
            ranges.push(JsonRange {
              byte_start: value_start,
              byte_end: out.len(),
              copy: id,
            });
          }
          _ => return Err(self.err_owned(format!("unexpected identifier '{}'", id))),
        }
      }
      Some(c) => {
        return Err(self.err_owned(format!(
          "unexpected '{}' while parsing value",
          char_repr(c)
        )));
      }
      None => return Err(self.err("unexpected end of input")),
    }
    Ok(())
  }

  fn emit_object(
    &mut self,
    out: &mut String,
    ranges: &mut Vec<JsonRange>,
    depth: usize,
  ) -> Result<(), JsonError> {
    let value_start = out.len();
    debug_assert_eq!(self.peek(), Some(b'{'));
    self.bump();
    out.push('{');
    self.skip_trivia()?;
    if self.peek() == Some(b'}') {
      self.bump();
      out.push('}');
      ranges.push(JsonRange {
        byte_start: value_start,
        byte_end: out.len(),
        copy: out[value_start..].to_string(),
      });
      return Ok(());
    }
    out.push('\n');
    let inner_depth = depth + 1;
    loop {
      self.skip_trivia()?;
      push_indent(out, inner_depth);
      let key_start = out.len();
      let key_text = match self.peek() {
        Some(b'"') | Some(b'\'') => self.read_string()?,
        Some(b) if is_ident_start(b) => self.read_ident(),
        Some(c) => {
          return Err(self.err_owned(format!(
            "expected key, found '{}'",
            char_repr(c)
          )));
        }
        None => return Err(self.err("expected key, found EOF")),
      };
      out.push('"');
      push_json_escaped(out, &key_text);
      out.push('"');
      ranges.push(JsonRange {
        byte_start: key_start,
        byte_end: out.len(),
        copy: key_text,
      });
      self.skip_trivia()?;
      match self.peek() {
        Some(b':') => {
          self.bump();
        }
        Some(c) => {
          return Err(self.err_owned(format!(
            "expected ':' after key, found '{}'",
            char_repr(c)
          )));
        }
        None => return Err(self.err("expected ':' after key, found EOF")),
      }
      out.push_str(": ");
      self.emit_value(out, ranges, inner_depth)?;
      self.skip_trivia()?;
      match self.peek() {
        Some(b',') => {
          self.bump();
          self.skip_trivia()?;
          if self.peek() == Some(b'}') {
            self.bump();
            out.push('\n');
            push_indent(out, depth);
            out.push('}');
            break;
          }
          out.push(',');
          out.push('\n');
        }
        Some(b'}') => {
          self.bump();
          out.push('\n');
          push_indent(out, depth);
          out.push('}');
          break;
        }
        Some(c) => {
          return Err(self.err_owned(format!(
            "expected ',' or '}}' in object, found '{}'",
            char_repr(c)
          )));
        }
        None => return Err(self.err("expected ',' or '}' in object, found EOF")),
      }
    }
    ranges.push(JsonRange {
      byte_start: value_start,
      byte_end: out.len(),
      copy: out[value_start..].to_string(),
    });
    Ok(())
  }

  fn emit_array(
    &mut self,
    out: &mut String,
    ranges: &mut Vec<JsonRange>,
    depth: usize,
  ) -> Result<(), JsonError> {
    let value_start = out.len();
    debug_assert_eq!(self.peek(), Some(b'['));
    self.bump();
    out.push('[');
    self.skip_trivia()?;
    if self.peek() == Some(b']') {
      self.bump();
      out.push(']');
      ranges.push(JsonRange {
        byte_start: value_start,
        byte_end: out.len(),
        copy: out[value_start..].to_string(),
      });
      return Ok(());
    }
    out.push('\n');
    let inner_depth = depth + 1;
    loop {
      self.skip_trivia()?;
      push_indent(out, inner_depth);
      self.emit_value(out, ranges, inner_depth)?;
      self.skip_trivia()?;
      match self.peek() {
        Some(b',') => {
          self.bump();
          self.skip_trivia()?;
          if self.peek() == Some(b']') {
            self.bump();
            out.push('\n');
            push_indent(out, depth);
            out.push(']');
            break;
          }
          out.push(',');
          out.push('\n');
        }
        Some(b']') => {
          self.bump();
          out.push('\n');
          push_indent(out, depth);
          out.push(']');
          break;
        }
        Some(c) => {
          return Err(self.err_owned(format!(
            "expected ',' or ']' in array, found '{}'",
            char_repr(c)
          )));
        }
        None => return Err(self.err("expected ',' or ']' in array, found EOF")),
      }
    }
    ranges.push(JsonRange {
      byte_start: value_start,
      byte_end: out.len(),
      copy: out[value_start..].to_string(),
    });
    Ok(())
  }

  fn read_string(&mut self) -> Result<String, JsonError> {
    let quote = self.peek().expect("read_string called at non-quote");
    debug_assert!(quote == b'"' || quote == b'\'');
    self.bump();
    let mut out = String::new();
    loop {
      let b = match self.peek() {
        Some(b) => b,
        None => return Err(self.err("unterminated string")),
      };
      if b == quote {
        self.bump();
        return Ok(out);
      }
      if b == b'\n' || b == b'\r' {
        return Err(self.err("unescaped line break in string"));
      }
      if b == b'\\' {
        self.bump();
        self.read_escape(&mut out)?;
        continue;
      }
      // Append one full UTF-8 char.
      let len = utf8_len(b);
      if self.pos + len > self.src.len() {
        return Err(self.err("truncated UTF-8 in string"));
      }
      let chunk = &self.src[self.pos..self.pos + len];
      let s = std::str::from_utf8(chunk).map_err(|_| self.err("invalid UTF-8 in string"))?;
      out.push_str(s);
      self.pos += len;
      self.col += 1;
    }
  }

  fn read_escape(&mut self, out: &mut String) -> Result<(), JsonError> {
    let b = match self.peek() {
      Some(b) => b,
      None => return Err(self.err("unterminated escape")),
    };
    match b {
      b'"' => {
        out.push('"');
        self.bump();
      }
      b'\'' => {
        out.push('\'');
        self.bump();
      }
      b'\\' => {
        out.push('\\');
        self.bump();
      }
      b'/' => {
        out.push('/');
        self.bump();
      }
      b'b' => {
        out.push('\u{08}');
        self.bump();
      }
      b'f' => {
        out.push('\u{0c}');
        self.bump();
      }
      b'n' => {
        out.push('\n');
        self.bump();
      }
      b'r' => {
        out.push('\r');
        self.bump();
      }
      b't' => {
        out.push('\t');
        self.bump();
      }
      b'0' => {
        out.push('\0');
        self.bump();
      }
      b'\n' => {
        self.bump();
      }
      b'\r' => {
        self.bump();
        if self.peek() == Some(b'\n') {
          self.bump();
        }
      }
      b'x' => {
        self.bump();
        let mut v = 0u32;
        for _ in 0..2 {
          let d = self
            .peek()
            .and_then(hex_digit)
            .ok_or_else(|| self.err("invalid \\x escape"))?;
          v = v * 16 + d;
          self.bump();
        }
        out.push(char::from_u32(v).ok_or_else(|| self.err("invalid \\x codepoint"))?);
      }
      b'u' => {
        self.bump();
        let v = self.read_hex4()?;
        if (0xD800..=0xDBFF).contains(&v) {
          if self.peek() == Some(b'\\') && self.peek_at(1) == Some(b'u') {
            self.bump();
            self.bump();
            let lo = self.read_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&lo) {
              return Err(self.err("invalid surrogate pair"));
            }
            let cp = 0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00);
            out.push(char::from_u32(cp).ok_or_else(|| self.err("invalid codepoint"))?);
          } else {
            return Err(self.err("lone high surrogate"));
          }
        } else if (0xDC00..=0xDFFF).contains(&v) {
          return Err(self.err("lone low surrogate"));
        } else {
          out.push(char::from_u32(v).ok_or_else(|| self.err("invalid codepoint"))?);
        }
      }
      // JSON5 leniency: any other char escapes to itself.
      _ => {
        let len = utf8_len(b);
        if self.pos + len > self.src.len() {
          return Err(self.err("invalid UTF-8 in escape"));
        }
        let chunk = &self.src[self.pos..self.pos + len];
        let s = std::str::from_utf8(chunk).map_err(|_| self.err("invalid UTF-8 in escape"))?;
        out.push_str(s);
        self.pos += len;
        self.col += 1;
      }
    }
    Ok(())
  }

  fn read_hex4(&mut self) -> Result<u32, JsonError> {
    let mut v = 0u32;
    for _ in 0..4 {
      let d = self
        .peek()
        .and_then(hex_digit)
        .ok_or_else(|| self.err("invalid \\u escape"))?;
      v = v * 16 + d;
      self.bump();
    }
    Ok(v)
  }

  fn read_number(&mut self) -> Result<String, JsonError> {
    let start = self.pos;
    if matches!(self.peek(), Some(b'+') | Some(b'-')) {
      self.bump();
    }
    if self.peek() == Some(b'I') {
      let id = self.read_ident();
      if id != "Infinity" {
        return Err(self.err_owned(format!("expected Infinity, got '{}'", id)));
      }
      return Ok(slice_str(self.src, start, self.pos));
    }
    if self.peek() == Some(b'N') {
      let id = self.read_ident();
      if id != "NaN" {
        return Err(self.err_owned(format!("expected NaN, got '{}'", id)));
      }
      return Ok(slice_str(self.src, start, self.pos));
    }
    // Hex literal
    if self.peek() == Some(b'0') && matches!(self.peek_at(1), Some(b'x') | Some(b'X')) {
      self.bump();
      self.bump();
      let mut any = false;
      while let Some(b) = self.peek() {
        if b.is_ascii_hexdigit() {
          self.bump();
          any = true;
        } else {
          break;
        }
      }
      if !any {
        return Err(self.err("expected hex digits"));
      }
      return Ok(slice_str(self.src, start, self.pos));
    }
    let mut int_digits = false;
    while let Some(b) = self.peek() {
      if b.is_ascii_digit() {
        self.bump();
        int_digits = true;
      } else {
        break;
      }
    }
    let mut frac_digits = false;
    if self.peek() == Some(b'.') {
      self.bump();
      while let Some(b) = self.peek() {
        if b.is_ascii_digit() {
          self.bump();
          frac_digits = true;
        } else {
          break;
        }
      }
    }
    if !int_digits && !frac_digits {
      return Err(self.err("invalid number"));
    }
    if matches!(self.peek(), Some(b'e') | Some(b'E')) {
      self.bump();
      if matches!(self.peek(), Some(b'+') | Some(b'-')) {
        self.bump();
      }
      let mut any = false;
      while let Some(b) = self.peek() {
        if b.is_ascii_digit() {
          self.bump();
          any = true;
        } else {
          break;
        }
      }
      if !any {
        return Err(self.err("expected exponent digits"));
      }
    }
    Ok(slice_str(self.src, start, self.pos))
  }

  fn read_ident(&mut self) -> String {
    let start = self.pos;
    while let Some(b) = self.peek() {
      if is_ident_cont(b) {
        self.bump();
      } else {
        break;
      }
    }
    slice_str(self.src, start, self.pos)
  }
}

fn slice_str(src: &[u8], a: usize, b: usize) -> String {
  std::str::from_utf8(&src[a..b]).unwrap_or("").to_string()
}

fn is_ws(b: u8) -> bool {
  matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn is_ident_start(b: u8) -> bool {
  b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident_cont(b: u8) -> bool {
  b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn hex_digit(b: u8) -> Option<u32> {
  match b {
    b'0'..=b'9' => Some(u32::from(b - b'0')),
    b'a'..=b'f' => Some(u32::from(b - b'a' + 10)),
    b'A'..=b'F' => Some(u32::from(b - b'A' + 10)),
    _ => None,
  }
}

fn utf8_len(b: u8) -> usize {
  // Continuation bytes (0x80..0xBF) shouldn't appear here as a leading
  // byte; treat them as 1 to make forward progress on malformed input.
  if b < 0xC0 {
    1
  } else if b < 0xE0 {
    2
  } else if b < 0xF0 {
    3
  } else {
    4
  }
}

fn char_repr(b: u8) -> String {
  if b.is_ascii_graphic() || b == b' ' {
    (b as char).to_string()
  } else {
    format!("\\x{:02x}", b)
  }
}

fn push_json_escaped(out: &mut String, s: &str) {
  for c in s.chars() {
    match c {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      '\u{08}' => out.push_str("\\b"),
      '\u{0c}' => out.push_str("\\f"),
      c if (c as u32) < 0x20 => {
        let _ = std::fmt::Write::write_fmt(out, format_args!("\\u{:04x}", c as u32));
      }
      c => out.push(c),
    }
  }
}

/// JSON-aware syntax-highlight pass. Walks the canonical formatted
/// output (or any JSON5-shaped text) once, emitting one `HlSpan` per
/// token + one per whitespace run. Bypasses syntect on the JSON path
/// because syntect's bundled JSON grammar scopes JSON5 extras
/// (`NaN`, `±Infinity`, hex literals) as "invalid" — which the dark
/// theme renders in a near-invisible muted grey. Hand-classifying gets
/// us deterministic, theme-aware colors and avoids the syntect regex
/// pass entirely for JSON.
pub fn highlight_canonical(text: &str, dark: bool) -> Vec<HlSpan> {
  let pal = Palette::for_dark(dark);
  let bytes = text.as_bytes();
  let mut out: Vec<HlSpan> = Vec::new();
  let mut i = 0usize;
  // Containers: `true` = object (next string is a key), `false` = array.
  let mut stack: Vec<bool> = Vec::new();
  // True right after `{` or after `,` inside an object — the next string
  // we see is the key, not a value. Cleared by `:`, refreshed by `,`.
  let mut expecting_key = false;

  while i < bytes.len() {
    let b = bytes[i];
    if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
      let start = i;
      while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
      }
      push_span(&mut out, &text[start..i], pal.fg);
      continue;
    }
    match b {
      b'{' => {
        push_span(&mut out, "{", pal.punct);
        stack.push(true);
        expecting_key = true;
        i += 1;
      }
      b'[' => {
        push_span(&mut out, "[", pal.punct);
        stack.push(false);
        expecting_key = false;
        i += 1;
      }
      b'}' | b']' => {
        let s = if b == b'}' { "}" } else { "]" };
        push_span(&mut out, s, pal.punct);
        stack.pop();
        // Closing a container doesn't change the parent's expecting_key
        // — that's locked in by the preceding `,`/`:` already.
        i += 1;
      }
      b',' => {
        push_span(&mut out, ",", pal.punct);
        expecting_key = matches!(stack.last(), Some(true));
        i += 1;
      }
      b':' => {
        push_span(&mut out, ":", pal.punct);
        expecting_key = false;
        i += 1;
      }
      b'"' | b'\'' => {
        let q = b;
        let start = i;
        i += 1;
        while i < bytes.len() {
          let c = bytes[i];
          if c == b'\\' {
            i += 1;
            if i < bytes.len() {
              i += 1;
            }
            continue;
          }
          if c == q {
            i += 1;
            break;
          }
          i += 1;
        }
        let color = if expecting_key { pal.key } else { pal.string };
        push_span(&mut out, &text[start..i], color);
      }
      b'-' | b'+' | b'.' | b'0'..=b'9' => {
        let start = i;
        i += 1;
        while i < bytes.len() && is_number_cont(bytes[i]) {
          i += 1;
        }
        push_span(&mut out, &text[start..i], pal.number);
      }
      b if is_ident_start(b) => {
        let start = i;
        i += 1;
        while i < bytes.len() && is_ident_cont(bytes[i]) {
          i += 1;
        }
        let id = &text[start..i];
        let color = match id {
          "true" | "false" => pal.literal,
          "null" => pal.literal,
          "Infinity" | "NaN" => pal.number,
          _ => pal.fg,
        };
        push_span(&mut out, id, color);
      }
      _ => {
        let start = i;
        i += 1;
        push_span(&mut out, &text[start..i], pal.fg);
      }
    }
  }
  out
}

fn push_span(out: &mut Vec<HlSpan>, s: &str, fg: Color) {
  if s.is_empty() {
    return;
  }
  out.push(HlSpan {
    text: s.to_string(),
    fg,
    bold: false,
    italic: false,
  });
}

fn is_number_cont(b: u8) -> bool {
  b.is_ascii_alphanumeric() || b == b'.' || b == b'+' || b == b'-'
}

/// Hand-tuned for legibility against `Theme::code_bg` on each side. We
/// don't read the existing Theme palette because it has no syntax
/// colors to draw from; alerts hardcode their own palette the same way.
struct Palette {
  fg: Color,
  punct: Color,
  key: Color,
  string: Color,
  number: Color,
  literal: Color,
}

impl Palette {
  fn for_dark(dark: bool) -> Self {
    if dark {
      Self {
        fg: Color::rgb(0xe6, 0xed, 0xf3),
        punct: Color::rgb(0x9b, 0xa3, 0xae),
        key: Color::rgb(0x7e, 0xc7, 0xcf),
        string: Color::rgb(0xa3, 0xc8, 0x86),
        number: Color::rgb(0xe5, 0xa3, 0x76),
        literal: Color::rgb(0xc5, 0x8a, 0xe5),
      }
    } else {
      Self {
        fg: Color::rgb(0x19, 0x12, 0x0c),
        punct: Color::rgb(0x4d, 0x55, 0x5f),
        key: Color::rgb(0x0a, 0x5a, 0x73),
        string: Color::rgb(0x32, 0x6c, 0x2d),
        number: Color::rgb(0xae, 0x4f, 0x10),
        literal: Color::rgb(0x6c, 0x2d, 0x9c),
      }
    }
  }
}

/// Content sniff: skip BOM + leading whitespace; return `true` if the
/// first non-whitespace byte is `{` or `[`.
pub fn looks_like_json(src: &str) -> bool {
  let bytes = src.as_bytes();
  let mut i = 0;
  if bytes.starts_with(b"\xEF\xBB\xBF") {
    i = 3;
  }
  while let Some(&b) = bytes.get(i) {
    if is_ws(b) {
      i += 1;
      continue;
    }
    return b == b'{' || b == b'[';
  }
  false
}

#[cfg(test)]
mod tests {
  use super::*;

  fn fmt(src: &str) -> String {
    format(src).unwrap().0
  }

  #[test]
  fn formats_object() {
    assert_eq!(fmt(r#"{"a":1,"b":2}"#), "{\n  \"a\": 1,\n  \"b\": 2\n}");
  }

  #[test]
  fn formats_array() {
    assert_eq!(fmt(r#"[1, 2, 3]"#), "[\n  1,\n  2,\n  3\n]");
  }

  #[test]
  fn formats_empty() {
    assert_eq!(fmt("{}"), "{}");
    assert_eq!(fmt("[]"), "[]");
  }

  #[test]
  fn formats_nested() {
    assert_eq!(
      fmt(r#"{"a":{"b":[1,2]}}"#),
      "{\n  \"a\": {\n    \"b\": [\n      1,\n      2\n    ]\n  }\n}"
    );
  }

  #[test]
  fn json5_unquoted_keys() {
    assert_eq!(fmt("{a: 1}"), "{\n  \"a\": 1\n}");
  }

  #[test]
  fn json5_single_quotes() {
    assert_eq!(fmt(r#"{'a': 'b'}"#), "{\n  \"a\": \"b\"\n}");
  }

  #[test]
  fn json5_trailing_comma() {
    assert_eq!(fmt("[1, 2,]"), "[\n  1,\n  2\n]");
    assert_eq!(fmt("{a: 1,}"), "{\n  \"a\": 1\n}");
  }

  #[test]
  fn jsonc_comments() {
    let src = r#"{
      // line comment
      "a": 1, /* block */
      "b": 2
    }"#;
    assert_eq!(fmt(src), "{\n  \"a\": 1,\n  \"b\": 2\n}");
  }

  #[test]
  fn json5_hex_and_inf() {
    assert_eq!(fmt("[0xFF, Infinity, -Infinity, NaN]"), "[\n  0xFF,\n  Infinity,\n  -Infinity,\n  NaN\n]");
  }

  #[test]
  fn ranges_cover_keys_and_values() {
    let (out, ranges) = format(r#"{"name": "John"}"#).unwrap();
    // Expect ranges for: key "name", value "John", outer object.
    assert_eq!(ranges.len(), 3);
    let key = &ranges[0];
    assert_eq!(key.copy, "name");
    assert_eq!(&out[key.byte_start..key.byte_end], "\"name\"");
    let val = &ranges[1];
    assert_eq!(val.copy, "John");
    assert_eq!(&out[val.byte_start..val.byte_end], "\"John\"");
    let obj = &ranges[2];
    assert_eq!(obj.copy, out);
    assert_eq!(obj.byte_start, 0);
    assert_eq!(obj.byte_end, out.len());
  }

  #[test]
  fn ranges_for_array_elements() {
    let (out, ranges) = format("[1, 2]").unwrap();
    // Expect: number 1, number 2, outer array.
    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[0].copy, "1");
    assert_eq!(ranges[1].copy, "2");
    assert_eq!(ranges[2].copy, out);
  }

  #[test]
  fn errors_on_invalid() {
    assert!(format("").is_err());
    assert!(format("{").is_err());
    assert!(format("{a 1}").is_err());
    assert!(format("[1 2]").is_err());
    assert!(format(r#"{"a": 1} junk"#).is_err());
  }

  #[test]
  fn looks_like_json_works() {
    assert!(looks_like_json("{}"));
    assert!(looks_like_json("  \n[1,2]"));
    assert!(looks_like_json("\u{FEFF}{}"));
    assert!(!looks_like_json("# heading"));
    assert!(!looks_like_json(""));
  }
}
