use std::sync::Arc;

use cosmic_text::{FeatureTag, FontFeatures, FontSystem, SwashCache, fontdb};

const INTER_REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../assets/Inter-Bold.ttf");
const INTER_ITALIC: &[u8] = include_bytes!("../assets/Inter-Italic.ttf");
const INTER_BOLD_ITALIC: &[u8] = include_bytes!("../assets/Inter-BoldItalic.ttf");
const JBM_REGULAR: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");
const JBM_BOLD: &[u8] = include_bytes!("../assets/JetBrainsMono-Bold.ttf");
const JBM_ITALIC: &[u8] = include_bytes!("../assets/JetBrainsMono-Italic.ttf");

pub const FONT_SANS: &str = "Inter";
pub const FONT_MONO: &str = "JetBrains Mono";

/// Wraps a `&'static [u8]` so we can hand it to fontdb as a
/// `Source::Binary(Arc<dyn AsRef<[u8]> + Send + Sync>)` without copying
/// the font into the heap. `load_font_data(Vec<u8>)` would otherwise
/// allocate ~400KB per face (× 7 faces × N FontSystems).
struct StaticFont(&'static [u8]);
impl AsRef<[u8]> for StaticFont {
  fn as_ref(&self) -> &[u8] {
    self.0
  }
}

fn load_static(db: &mut fontdb::Database, bytes: &'static [u8]) {
  db.load_font_source(fontdb::Source::Binary(Arc::new(StaticFont(bytes))));
}

pub fn build_font_system() -> FontSystem {
  let mut db = fontdb::Database::new();
  load_static(&mut db, INTER_REGULAR);
  load_static(&mut db, INTER_BOLD);
  load_static(&mut db, INTER_ITALIC);
  load_static(&mut db, INTER_BOLD_ITALIC);
  load_static(&mut db, JBM_REGULAR);
  load_static(&mut db, JBM_BOLD);
  load_static(&mut db, JBM_ITALIC);
  db.set_sans_serif_family(FONT_SANS);
  db.set_monospace_family(FONT_MONO);
  FontSystem::new_with_locale_and_db("en-US".into(), db)
}

pub fn new_swash_cache() -> SwashCache {
  SwashCache::new()
}

/// Inter ss02: disambiguation set. Clarifies confusable glyphs (capital I,
/// lowercase l, digit 1) without otherwise altering letter shapes.
pub fn sans_features() -> FontFeatures {
  let mut f = FontFeatures::new();
  f.set(FeatureTag::new(b"ss02"), 1);
  f
}

/// JetBrains Mono with contextual alternates and standard ligatures so
/// programming digraphs (->, =>, !=, >=, ...) render as ligatures.
pub fn mono_features() -> FontFeatures {
  let mut f = FontFeatures::new();
  f.set(FeatureTag::CONTEXTUAL_ALTERNATES, 1);
  f.set(FeatureTag::STANDARD_LIGATURES, 1);
  f
}
