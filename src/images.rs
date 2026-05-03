use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use image::{AnimationDecoder, ImageReader, RgbaImage};

use crate::doc::{Block, Doc, ListItem};

/// Default placeholder dimensions when we can't read an image's headers
/// (missing file, unsupported format, etc.). Picked to roughly match a
/// typical README screenshot so the layout doesn't shift dramatically
/// if the file shows up later.
const PLACEHOLDER_W: u32 = 480;
const PLACEHOLDER_H: u32 = 270;

/// One frame of an animated image (or the only frame of a static one).
#[derive(Clone)]
pub struct AnimFrame {
  /// Display duration before advancing to the next frame, in
  /// milliseconds. Floored at 20 ms so pathologically-fast GIFs don't
  /// burn CPU; matches what most browsers do.
  pub delay_ms: u32,
  pub buffer: Arc<RgbaImage>,
}

/// Per-image cache entry. `dims` is read synchronously during parse from
/// the file's header (cheap — a few µs even for multi-MB PNG/JPEG/GIF).
/// `frames` is filled by the bg decoder thread; until then, paint
/// renders a placeholder rect at the correct dimensions so first paint
/// is fast and there's no layout shift when pixels arrive. Static
/// images store a one-element frames vec with `delay_ms = 0`.
pub struct ImageEntry {
  pub dims: (u32, u32),
  pub frames: Option<Arc<Vec<AnimFrame>>>,
  pub total_duration_ms: u32,
  pub failed: bool,
}

#[derive(Default)]
pub struct ImageStore {
  pub map: RwLock<HashMap<PathBuf, ImageEntry>>,
}

impl ImageStore {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn get_dims(&self, key: &Path) -> Option<(u32, u32)> {
    self.map.read().ok()?.get(key).map(|e| e.dims)
  }

  /// Returns the frames + total duration for an image. Animated images
  /// have multiple frames; static images have one. `None` until the bg
  /// decoder has populated this entry (or if it failed).
  pub fn get_frames(&self, key: &Path) -> Option<(Arc<Vec<AnimFrame>>, u32)> {
    let m = self.map.read().ok()?;
    let e = m.get(key)?;
    Some((e.frames.clone()?, e.total_duration_ms))
  }

  pub fn insert_dims(&self, key: PathBuf, dims: (u32, u32)) {
    if let Ok(mut m) = self.map.write() {
      m.entry(key).or_insert(ImageEntry {
        dims,
        frames: None,
        total_duration_ms: 0,
        failed: false,
      });
    }
  }

  pub fn set_frames(&self, key: &Path, frames: Vec<AnimFrame>) {
    let total: u32 = frames.iter().map(|f| f.delay_ms).sum();
    if let Ok(mut m) = self.map.write() {
      if let Some(e) = m.get_mut(key) {
        e.frames = Some(Arc::new(frames));
        e.total_duration_ms = total;
      }
    }
  }

  /// Append a single frame to an entry, used by the streaming decoder.
  /// Each call clones the existing `Vec<AnimFrame>` so paint sees an
  /// immutable snapshot via its `Arc`. Cloning a Vec of AnimFrames just
  /// bumps each frame's inner `Arc<RgbaImage>` refcount — cheap.
  pub fn append_frame(&self, key: &Path, frame: AnimFrame) {
    let Ok(mut m) = self.map.write() else { return };
    let Some(e) = m.get_mut(key) else { return };
    let new_total = e.total_duration_ms.saturating_add(frame.delay_ms);
    let new_frames: Arc<Vec<AnimFrame>> = match &e.frames {
      Some(existing) => {
        let mut v: Vec<AnimFrame> = (**existing).clone();
        v.push(frame);
        Arc::new(v)
      }
      None => Arc::new(vec![frame]),
    };
    e.frames = Some(new_frames);
    e.total_duration_ms = new_total;
  }

  pub fn set_failed(&self, key: &Path) {
    if let Ok(mut m) = self.map.write() {
      if let Some(e) = m.get_mut(key) {
        e.failed = true;
      }
    }
  }

  /// True if any image in the store has more than one frame. Used to
  /// short-circuit the per-paint visibility-walk when no animation is
  /// possible.
  pub fn has_animations(&self) -> bool {
    let Ok(m) = self.map.read() else { return false };
    m.values()
      .any(|e| e.frames.as_ref().map_or(false, |f| f.len() > 1))
  }
}

/// Resolve an image src against the doc's base directory. Absolute paths
/// pass through; relative paths are joined onto `base_dir`. Returns
/// `None` for non-local sources (http://, https://, data:, etc.) which
/// v1 doesn't support.
pub fn resolve_src(src: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
  if src.starts_with("http://")
    || src.starts_with("https://")
    || src.starts_with("data:")
    || src.starts_with("//")
  {
    return None;
  }
  let p = PathBuf::from(src);
  if p.is_absolute() {
    return Some(p);
  }
  let base = base_dir?;
  Some(base.join(p))
}

/// Walk a doc and collect every block-image's resolved path, in the
/// order they'd be encountered visually. Used by the bg decoder thread
/// so it can decode top-down (the above-the-fold image gets pixels
/// first, which is what the user sees).
pub fn collect_image_paths(doc: &Doc, base_dir: Option<&Path>) -> Vec<PathBuf> {
  let mut out = Vec::new();
  walk_blocks(&doc.blocks, base_dir, &mut out);
  out
}

fn walk_blocks(blocks: &[Block], base_dir: Option<&Path>, out: &mut Vec<PathBuf>) {
  for b in blocks {
    match b {
      Block::Image { src, .. } => {
        if let Some(p) = resolve_src(src, base_dir) {
          out.push(p);
        }
      }
      Block::Quote(inner) => walk_blocks(inner, base_dir, out),
      Block::List { items, .. } => walk_items(items, base_dir, out),
      Block::Footnotes(defs) => {
        for def in defs {
          walk_blocks(&def.blocks, base_dir, out);
        }
      }
      _ => {}
    }
  }
}

fn walk_items(items: &[ListItem], base_dir: Option<&Path>, out: &mut Vec<PathBuf>) {
  for item in items {
    walk_blocks(&item.blocks, base_dir, out);
  }
}

/// Read just enough header bytes to learn the image dimensions —
/// essentially µs per file, even for multi-MB sources. Falls back to
/// PLACEHOLDER dims on read/parse errors so layout still produces a
/// usable box.
pub fn read_dims(path: &Path) -> (u32, u32) {
  match ImageReader::open(path).and_then(|r| r.with_guessed_format()) {
    Ok(r) => match r.into_dimensions() {
      Ok(d) => d,
      Err(_) => (PLACEHOLDER_W, PLACEHOLDER_H),
    },
    Err(_) => (PLACEHOLDER_W, PLACEHOLDER_H),
  }
}

/// Streaming decode used by the cold-launch bg thread. Invokes
/// `on_frame(AnimFrame)` for each successfully decoded frame as it
/// becomes available, so paint can show frame 0 within ms of decode
/// start instead of waiting the full gif duration. Returns true if at
/// least one frame was produced.
pub fn decode_streaming<F: FnMut(AnimFrame)>(path: &Path, mut on_frame: F) -> bool {
  let ext = path
    .extension()
    .and_then(|e| e.to_str())
    .map(|s| s.to_ascii_lowercase());
  let is_gif = matches!(ext.as_deref(), Some("gif"));
  let is_webp = matches!(ext.as_deref(), Some("webp"));

  if is_gif || is_webp {
    if decode_animated_streaming(path, is_gif, &mut on_frame) {
      return true;
    }
  }

  // Static path (PNG/JPEG/static WebP/non-animated GIF).
  let Some(reader) = ImageReader::open(path)
    .ok()
    .and_then(|r| r.with_guessed_format().ok())
  else {
    return false;
  };
  let Ok(img) = reader.decode() else {
    return false;
  };
  on_frame(AnimFrame {
    delay_ms: 0,
    buffer: Arc::new(img.to_rgba8()),
  });
  true
}

/// Synchronous all-at-once decode. Used by the reload-from-disk path
/// where streaming would require shuffling a proxy through `App`.
/// Reload happens in response to a user save (not the cold-launch
/// critical path), so blocking briefly is acceptable.
pub fn decode_frames(path: &Path) -> Option<Vec<AnimFrame>> {
  let mut frames = Vec::new();
  let ok = decode_streaming(path, |f| frames.push(f));
  if !ok || frames.is_empty() {
    None
  } else {
    Some(frames)
  }
}

fn decode_animated_streaming<F: FnMut(AnimFrame)>(
  path: &Path,
  is_gif: bool,
  on_frame: &mut F,
) -> bool {
  use image::codecs::gif::GifDecoder;
  use image::codecs::webp::WebPDecoder;
  let Ok(f) = std::fs::File::open(path) else {
    return false;
  };
  let buf = BufReader::new(f);
  if is_gif {
    let Ok(dec) = GifDecoder::new(buf) else {
      return false;
    };
    drive_frames(dec.into_frames(), on_frame)
  } else {
    let Ok(dec) = WebPDecoder::new(buf) else {
      return false;
    };
    drive_frames(dec.into_frames(), on_frame)
  }
}

fn drive_frames<I, F>(iter: I, on_frame: &mut F) -> bool
where
  I: Iterator<Item = image::ImageResult<image::Frame>>,
  F: FnMut(AnimFrame),
{
  let mut produced = false;
  for frame_result in iter {
    let Ok(frame) = frame_result else { break };
    let (n, d) = frame.delay().numer_denom_ms();
    let raw_ms = if d == 0 { 100 } else { (n / d).max(0) };
    // Match browser behavior: clamp very-fast GIFs to 20ms (50 fps) so
    // the redraw loop doesn't burn CPU on adversarial encodings.
    let delay_ms = (raw_ms as u32).max(20);
    on_frame(AnimFrame {
      delay_ms,
      buffer: Arc::new(frame.into_buffer()),
    });
    produced = true;
  }
  produced
}

/// For an image of total animation duration `total_ms` made of `frames`,
/// pick the active frame index given `elapsed_ms` since the app started.
/// Static images (single frame) always return 0.
pub fn pick_frame_index(frames: &[AnimFrame], total_ms: u32, elapsed_ms: u128) -> usize {
  if frames.len() <= 1 || total_ms == 0 {
    return 0;
  }
  let t = (elapsed_ms % total_ms as u128) as u32;
  let mut acc = 0u32;
  for (i, f) in frames.iter().enumerate() {
    acc = acc.saturating_add(f.delay_ms);
    if t < acc {
      return i;
    }
  }
  frames.len() - 1
}

/// Milliseconds from `elapsed_ms` to the next frame transition for this
/// animation. Static images return `None` (no upcoming deadline).
pub fn ms_until_next_frame(
  frames: &[AnimFrame],
  total_ms: u32,
  elapsed_ms: u128,
) -> Option<u32> {
  if frames.len() <= 1 || total_ms == 0 {
    return None;
  }
  let t = (elapsed_ms % total_ms as u128) as u32;
  let mut acc = 0u32;
  for f in frames {
    acc = acc.saturating_add(f.delay_ms);
    if t < acc {
      return Some(acc - t);
    }
  }
  Some(1)
}

