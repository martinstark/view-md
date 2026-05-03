use cosmic_text::Color;
use tiny_skia::Color as SkColor;

use crate::doc::AlertKind;

pub struct Theme {
  pub is_dark: bool,
  pub bg: SkColor,
  pub fg: Color,
  pub muted: Color,
  pub link: Color,
  pub heading: Color,
  pub code_bg: SkColor,
  pub code_fg: Color,
  pub inline_code_bg: SkColor,
  pub quote_bar: SkColor,
  pub rule: SkColor,
  pub border: SkColor,
  /// Per-alert-kind accent colors used for the left bar and title
  /// label. Same RGB across light/dark for the title; the bar uses
  /// the same value (tiny-skia accepts an RGBA8 directly).
  pub alert_note: (u8, u8, u8),
  pub alert_tip: (u8, u8, u8),
  pub alert_important: (u8, u8, u8),
  pub alert_warning: (u8, u8, u8),
  pub alert_caution: (u8, u8, u8),
}

impl Theme {
  pub fn dark() -> Self {
    Self {
      is_dark: true,
      bg: SkColor::from_rgba8(0x0d, 0x11, 0x17, 0xff),
      fg: Color::rgb(0xe6, 0xed, 0xf3),
      muted: Color::rgb(0x8b, 0x94, 0x9e),
      link: Color::rgb(0x58, 0xa6, 0xff),
      heading: Color::rgb(0xf0, 0xf6, 0xfc),
      code_bg: SkColor::from_rgba8(0x15, 0x1b, 0x23, 0xff),
      code_fg: Color::rgb(0xe6, 0xed, 0xf3),
      inline_code_bg: SkColor::from_rgba8(0x6e, 0x76, 0x81, 0x40),
      quote_bar: SkColor::from_rgba8(0x30, 0x36, 0x3d, 0xff),
      rule: SkColor::from_rgba8(0x21, 0x26, 0x2d, 0xff),
      border: SkColor::from_rgba8(0x30, 0x36, 0x3d, 0xff),
      alert_note: (0x4e, 0x97, 0xff),
      alert_tip: (0x3f, 0xb9, 0x50),
      alert_important: (0xa3, 0x71, 0xf7),
      alert_warning: (0xd2, 0x99, 0x22),
      alert_caution: (0xf8, 0x51, 0x49),
    }
  }

  pub fn light() -> Self {
    Self {
      is_dark: false,
      bg: SkColor::from_rgba8(0xff, 0xff, 0xff, 0xff),
      fg: Color::rgb(0x1f, 0x23, 0x28),
      muted: Color::rgb(0x59, 0x63, 0x6e),
      link: Color::rgb(0x09, 0x69, 0xda),
      heading: Color::rgb(0x1f, 0x23, 0x28),
      code_bg: SkColor::from_rgba8(0xf6, 0xf8, 0xfa, 0xff),
      code_fg: Color::rgb(0x1f, 0x23, 0x28),
      inline_code_bg: SkColor::from_rgba8(0xaf, 0xb8, 0xc1, 0x33),
      quote_bar: SkColor::from_rgba8(0xd0, 0xd7, 0xde, 0xff),
      rule: SkColor::from_rgba8(0xd8, 0xde, 0xe4, 0xff),
      border: SkColor::from_rgba8(0xd0, 0xd7, 0xde, 0xff),
      alert_note: (0x1f, 0x6f, 0xeb),
      alert_tip: (0x1a, 0x7f, 0x37),
      alert_important: (0x82, 0x50, 0xdf),
      alert_warning: (0x9a, 0x67, 0x00),
      alert_caution: (0xcf, 0x22, 0x2e),
    }
  }

  pub fn select(dark: bool) -> Self {
    if dark { Self::dark() } else { Self::light() }
  }

  /// (bar SkColor, title cosmic-text Color) for a given alert kind.
  pub fn alert_colors(&self, kind: AlertKind) -> (SkColor, Color) {
    let (r, g, b) = match kind {
      AlertKind::Note => self.alert_note,
      AlertKind::Tip => self.alert_tip,
      AlertKind::Important => self.alert_important,
      AlertKind::Warning => self.alert_warning,
      AlertKind::Caution => self.alert_caution,
    };
    (SkColor::from_rgba8(r, g, b, 0xff), Color::rgb(r, g, b))
  }
}
