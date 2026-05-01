use cosmic_text::{FontSystem, SwashCache, fontdb};

const INTER_REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../assets/Inter-Bold.ttf");
const INTER_ITALIC: &[u8] = include_bytes!("../assets/Inter-Italic.ttf");
const INTER_BOLD_ITALIC: &[u8] = include_bytes!("../assets/Inter-BoldItalic.ttf");
const JBM_REGULAR: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");
const JBM_BOLD: &[u8] = include_bytes!("../assets/JetBrainsMono-Bold.ttf");
const JBM_ITALIC: &[u8] = include_bytes!("../assets/JetBrainsMono-Italic.ttf");

pub const FONT_SANS: &str = "Inter";
pub const FONT_MONO: &str = "JetBrains Mono";

pub fn build_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(INTER_REGULAR.to_vec());
    db.load_font_data(INTER_BOLD.to_vec());
    db.load_font_data(INTER_ITALIC.to_vec());
    db.load_font_data(INTER_BOLD_ITALIC.to_vec());
    db.load_font_data(JBM_REGULAR.to_vec());
    db.load_font_data(JBM_BOLD.to_vec());
    db.load_font_data(JBM_ITALIC.to_vec());
    db.set_sans_serif_family(FONT_SANS);
    db.set_monospace_family(FONT_MONO);
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

pub fn new_swash_cache() -> SwashCache {
    SwashCache::new()
}
