use cosmic_text::Color;
use tiny_skia::Color as SkColor;

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
    }
  }

  pub fn select(dark: bool) -> Self {
    if dark { Self::dark() } else { Self::light() }
  }
}
