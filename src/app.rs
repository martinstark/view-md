use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cosmic_text::{Cursor, FontSystem, SwashCache};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
  ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::AppEvent;
use crate::doc::Doc;
use crate::layout::{LaidDoc, LaidKind, layout_parallel};
use crate::paint::{Frame, Painter};
use crate::state::{self, Prefs};
use crate::theme::Theme;

/// Tuple returned by the speculative-layout background thread. Stored in
/// `App.spec_handle` until `resumed()` joins it — moves the join from
/// before `event_loop.run_app` to *after* `create_window`, so window/surface
/// init runs in parallel with the bg layout+warm work (item T1.5).
pub type SpecResult = (Doc, FontSystem, Vec<FontSystem>, LaidDoc, bool, SwashCache);

// Used by the resize-anchor heuristic to decide whether a heading is
// "near enough" to viewport top to be the snap target. Approximates the
// body line-height; doesn't need to match exactly — it's a heuristic
// distance threshold.
const BODY_FS_APPROX: f32 = 16.0;
const BODY_LH_APPROX: f32 = 1.55;

pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 3.0;
pub const ZOOM_STEP: f32 = 0.1;
pub const SCROLL_LINE_PX: f32 = 40.0;
pub const HALF_PAGE_FRAC: f32 = 0.5;
pub const FULL_PAGE_FRAC: f32 = 0.9;
pub const HEADING_OFFSET_PX: f32 = 24.0;
pub const WHEEL_PIXEL_SCALE: f32 = 1.0;
pub const WHEEL_LINE_SCALE: f32 = 40.0;

pub struct App {
  /// Wayland clipboard tied to our window's wl_display. Declared before
  /// `window` so it drops first, while the underlying display is still
  /// alive. Populated in `resumed()` once we have the window handle.
  /// `None` on non-Wayland platforms.
  pub wayland_clipboard: Option<smithay_clipboard::Clipboard>,
  pub title: String,
  /// Set when launched with `--watch <file>`. The watcher thread in
  /// `lib::spawn_watcher` posts `AppEvent::Reload` on disk changes,
  /// and `user_event` re-reads this path. `None` for stdin or
  /// non-watched runs.
  pub watch_path: Option<PathBuf>,
  /// Parent directory of the loaded markdown file, used to resolve
  /// relative image paths. `None` for stdin (relative srcs unresolvable).
  pub base_dir: Option<PathBuf>,
  /// Heading anchor parsed from `vmd file.md#section`. Consumed once,
  /// after the first relayout, by `apply_pending_anchor()`. `None` if
  /// the user didn't supply one or after it's been applied.
  pub pending_anchor: Option<String>,
  /// Cache of image dimensions + decoded pixels, shared with the bg
  /// decoder thread. Layout reads dims; paint reads pixels and falls
  /// back to a placeholder rect when not yet present.
  pub images: Arc<crate::images::ImageStore>,
  pub doc: Doc,
  pub painter: Painter,
  /// Extra FontSystems used for parallel block shaping in
  /// `layout_parallel`. Each is a private clone of the painter's font
  /// data — see `text::build_font_system` for the canonical setup. The
  /// painter still owns the FontSystem used at paint time.
  pub layout_workers: Vec<FontSystem>,
  pub dark: bool,
  pub zoom: f32,
  pub scroll_y: f32,
  pub window: Option<Rc<Window>>,
  pub surface: Option<Surface<Rc<Window>, Rc<Window>>>,
  /// Current physical surface dimensions, kept in sync with the
  /// softbuffer surface and the window. Replaces the old `pixmap` field
  /// (paint now writes directly into the softbuffer u32 slice via
  /// `Frame`, so we no longer need a BGRA scratch pixmap on App).
  pub surface_size: (u32, u32),
  pub laid: Option<LaidDoc>,
  /// Speculative-layout join handle, held by the App until `resumed()`
  /// has finished window+surface creation. `take()` + `join()` there
  /// instead of in `lib.rs::run` so the bg layout+warm work overlaps
  /// with main-thread Wayland init (item T1.5). `None` once consumed.
  pub spec_handle: Option<std::thread::JoinHandle<SpecResult>>,
  pub painted_once: bool,
  pub full_highlight: bool,
  pub upgrade_pending: bool,
  /// Surface width and combined scale (zoom × dpi_scale) the speculative
  /// pre-resumed layout was shaped against. `resumed()` compares the
  /// actual surface against these and only re-runs layout on mismatch.
  pub speculative_w: f32,
  pub speculative_scale: f32,
  /// Set true by the precompute background thread once the syntect cache is
  /// warm. `App::relayout` checks this and lays out with full highlighting
  /// from the start when set, avoiding the second-pass upgrade entirely on
  /// no-code or fast-precompute documents.
  pub highlight_ready: Arc<AtomicBool>,
  pub help_visible: bool,
  pub cursor: PhysicalPosition<f64>,
  pub selection: Option<Selection>,
  pub dragging: bool,
  pub modifiers: Modifiers,
  pub dpi_scale: f32,
  /// X11/macOS/Windows fallback. On Wayland `wayland_clipboard` is used.
  pub clipboard: Option<arboard::Clipboard>,
  /// Wall-clock baseline for animation timing. All animated images
  /// share one clock so frame switches are synchronized.
  pub anim_start: Instant,
  /// Earliest "next animation frame" deadline among visible images,
  /// computed at the end of `redraw()`. Drives `ControlFlow::WaitUntil`
  /// in `about_to_wait`. `None` when nothing on screen is animating.
  pub anim_next_deadline: Option<Instant>,
  /// Set when a `--watch` reload observed an empty file while the
  /// current doc has content. We defer applying that state by a short
  /// window so transient truncate-then-write saves don't blank the
  /// view; a follow-up event with content cancels the deadline. If
  /// the timer fires with the file still empty, the erase is genuine
  /// and we apply it.
  pub empty_reload_deadline: Option<Instant>,
  /// Active in-doc search. `Some` while the search overlay is open;
  /// `None` after Esc dismisses it. Driven by `/` to open and Enter
  /// to advance to the next match.
  pub search: Option<SearchState>,
}

/// In-doc search state. Maintains the current query, all match
/// occurrences in document order (one entry per occurrence, even if
/// multiple match within the same block), and the cursor into that
/// list. Jumping cycles forward through the list.
pub struct SearchState {
  pub query: String,
  pub matches: Vec<SearchMatch>,
  pub current: Option<usize>,
}

/// One match occurrence: the laid block it lives in, which buffer
/// line within that block (always 0 for paragraph/heading buffers,
/// per-source-line for code blocks), and the byte range relative to
/// that line's text. Stored at the granularity the painter needs
/// for `Buffer::layout_runs`.
#[derive(Clone, Copy, Debug)]
pub struct SearchMatch {
  pub laid_block_idx: usize,
  pub line_i: usize,
  pub byte_start: usize,
  pub byte_end: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct HitPoint {
  pub block_idx: usize,
  pub cursor: Cursor,
}

/// Logical pointer at the viewport top, captured before a relayout so
/// scroll position can be restored to the same content reference after
/// reflow (item: resize / zoom / dpi anchor preservation).
///
/// `block_idx` is into `LaidDoc.blocks`; stable across relayouts because
/// the source `doc.blocks` doesn't change. `block_y_offset` is a fallback
/// pixel offset used when the block has no Buffer (Rule, Bar, TaskBox).
///
/// For Text/CodeBlock blocks we capture a `cosmic_text::Cursor` at the
/// viewport-top line and a `residual` so we can restore at sub-line
/// precision after the buffer re-shapes against a new width.
#[derive(Clone, Copy, Debug)]
pub struct ScrollAnchor {
  pub block_idx: usize,
  pub block_y_offset: f32,
  pub cursor: Option<Cursor>,
  /// Pixel offset from the cursor's run top to the viewport top at
  /// capture time. Carries the precise within-line scroll position.
  pub residual: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Selection {
  pub anchor: HitPoint,
  pub head: HitPoint,
}

impl Selection {
  pub fn ordered(&self) -> (HitPoint, HitPoint) {
    let a = self.anchor;
    let b = self.head;
    if a.block_idx < b.block_idx || (a.block_idx == b.block_idx && cursor_le(&a.cursor, &b.cursor))
    {
      (a, b)
    } else {
      (b, a)
    }
  }
  pub fn is_empty(&self) -> bool {
    let (a, b) = self.ordered();
    a.block_idx == b.block_idx && a.cursor.line == b.cursor.line && a.cursor.index == b.cursor.index
  }
}

/// Find the y-coordinate (within `buf`'s coordinate space — i.e., 0 at
/// the top of the buffer's content area) of the run that contains the
/// given cursor's `(line, index)`. Used by the resize anchor restore
/// path to put the same line of text back at the same screen y after
/// the buffer re-shapes against a new width.
fn cursor_y_in_buffer(buf: &cosmic_text::Buffer, cursor: &Cursor) -> Option<f32> {
  // First pass: exact match — same source line AND cursor.index falls in
  // this visual run's glyph byte range. Handles re-wrapped lines where
  // a single source line spans multiple visual rows.
  for run in buf.layout_runs() {
    if run.line_i != cursor.line {
      continue;
    }
    let first = run.glyphs.first().map_or(0, |g| g.start);
    let last = run.glyphs.last().map_or(0, |g| g.end);
    if cursor.index >= first && cursor.index <= last {
      return Some(run.line_top);
    }
  }
  // Fallback: any run on the same source line. Picks the first such
  // run, which is the start of the source line in visual order.
  for run in buf.layout_runs() {
    if run.line_i == cursor.line {
      return Some(run.line_top);
    }
  }
  None
}

fn cursor_le(a: &Cursor, b: &Cursor) -> bool {
  if a.line != b.line {
    a.line < b.line
  } else {
    a.index <= b.index
  }
}

impl ApplicationHandler<AppEvent> for App {
  fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
    // `WaitUntil` woke us up: an animation deadline, an empty-reload
    // recheck, or both. Run the empty-recheck (which may apply a
    // deferred reload) and request a redraw — the redraw is harmless
    // when only the empty-recheck fired with no actual change.
    if matches!(cause, StartCause::ResumeTimeReached { .. }) {
      self.check_empty_deadline();
      self.request_redraw();
    }
  }

  fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    let earliest = match (self.anim_next_deadline, self.empty_reload_deadline) {
      (Some(a), Some(e)) => Some(a.min(e)),
      (Some(a), None) => Some(a),
      (None, Some(e)) => Some(e),
      (None, None) => None,
    };
    match earliest {
      Some(d) => event_loop.set_control_flow(ControlFlow::WaitUntil(d)),
      None => event_loop.set_control_flow(ControlFlow::Wait),
    }
  }

  fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
    match event {
      AppEvent::Reload => self.reload_from_disk(),
      AppEvent::ImageReady => self.request_redraw(),
    }
  }

  fn resumed(&mut self, event_loop: &ActiveEventLoop) {
    if self.window.is_some() {
      return;
    }
    crate::trace!("resumed");

    let attrs = Window::default_attributes()
      .with_title(format!("{} — vmd", self.title))
      .with_inner_size(LogicalSize::new(920.0, 1100.0));
    let window = Rc::new(event_loop.create_window(attrs).expect("window create"));
    crate::trace!("window_created");

    // Tie the clipboard to *this* window's wl_display so the compositor
    // grants ownership to the connection that has surface focus. On
    // non-Wayland platforms we fall through to arboard later.
    if let Ok(dh) = window.display_handle() {
      if let RawDisplayHandle::Wayland(wd) = dh.as_raw() {
        // SAFETY: `wd.display` is a valid wl_display pointer for the
        // lifetime of the winit Connection. We drop the Clipboard before
        // the window (declaration order in App) so the display is still
        // alive at drop time.
        let clip = unsafe { smithay_clipboard::Clipboard::new(wd.display.as_ptr()) };
        self.wayland_clipboard = Some(clip);
        crate::trace!("clipboard: bound to window's wl_display");
      }
    }

    self.dpi_scale = window.scale_factor() as f32;
    let context = Context::new(window.clone()).expect("softbuffer context");
    let mut surface = Surface::new(&context, window.clone()).expect("softbuffer surface");
    let size = window.inner_size();
    let (w, h) = (size.width.max(1), size.height.max(1));
    surface
      .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
      .expect("resize");
    crate::trace!("surface_ready");

    // Join the speculative-layout thread NOW (after window+surface init)
    // instead of before `event_loop.run_app` (item T1.5). The bg thread's
    // layout+warm work overlaps with create_window/clipboard/Context::new/
    // Surface::new above (~1.5–2ms on this Wayland setup); main no longer
    // pays the bg duration as a serial wait.
    if let Some(handle) = self.spec_handle.take() {
      let (doc, fs, layout_workers, laid, full_highlight, swash) =
        handle.join().expect("speculative layout panicked");
      crate::trace!("speculative_layout_joined");
      self.doc = doc;
      self.painter = Painter::with_cache(fs, swash);
      self.layout_workers = layout_workers;
      self.laid = Some(laid);
      self.full_highlight = full_highlight;
    }

    self.surface_size = (w, h);
    let actual_scale = self.zoom * self.dpi_scale.max(1.0);
    let dims_match = (w as f32 - self.speculative_w).abs() < 0.5
      && (actual_scale - self.speculative_scale).abs() < f32::EPSILON;
    if dims_match {
      crate::trace!("speculative_layout_used");
    } else {
      crate::trace!(
        "speculative_layout_mismatch (w {}vs{}, scale {}vs{}); relaying out",
        w as f32,
        self.speculative_w,
        actual_scale,
        self.speculative_scale
      );
      self.relayout(w as f32);
    }
    crate::trace!("layout_ready");

    // Anchor scroll happens here, after layout is in place but before
    // first paint, so the user lands directly on the section without
    // a visible jump.
    self.apply_pending_anchor();

    // The syntect_wait exists to avoid the placeholder→highlighted flash
    // on first paint. That flash is only user-visible if a placeholder
    // code block is in the initial viewport. If no code block is visible
    // (or all visible code blocks are already fully highlighted from a
    // fast-path spec), skip the wait — async upgrade still fires after
    // first paint to fix code blocks below the fold for when the user
    // scrolls.
    let viewport_h = h as f32;
    let placeholder_code_visible = !self.full_highlight
      && self.laid.as_ref().map_or(false, |l| {
        l.blocks
          .iter()
          .any(|b| b.y < viewport_h && matches!(b.kind, LaidKind::CodeBlock { .. }))
      });
    if placeholder_code_visible && !self.highlight_ready.load(Ordering::Acquire) {
      let deadline = Instant::now() + Duration::from_millis(5);
      while !self.highlight_ready.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::yield_now();
      }
      crate::trace!(
        "syntect_wait_done ready={}",
        self.highlight_ready.load(Ordering::Acquire)
      );
    } else if !placeholder_code_visible {
      crate::trace!("syntect_wait_skipped (no placeholder code in viewport)");
    }

    self.window = Some(window.clone());
    self.surface = Some(surface);
    // Paint synchronously instead of going through
    // window.request_redraw() → next event loop tick → RedrawRequested.
    // Saves the ~2ms scheduling round-trip on the critical path.
    // `redraw()` itself will request_redraw for the syntax-highlight
    // upgrade if applicable, so the upgrade pass still flows through
    // the normal event loop.
    self.redraw();
  }

  fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
    match event {
      WindowEvent::CloseRequested => event_loop.exit(),
      WindowEvent::Resized(size) => {
        if let Some(surface) = self.surface.as_mut() {
          let (w, h) = (size.width.max(1), size.height.max(1));
          let _ = surface.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());
          self.surface_size = (w, h);
          let anchor = self.capture_scroll_anchor();
          self.relayout(w as f32);
          if let Some(a) = anchor {
            self.restore_scroll_anchor(a);
          }
          self.request_redraw();
        }
      }
      WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
        let anchor = self.capture_scroll_anchor();
        self.dpi_scale = scale_factor as f32;
        self.relayout(self.current_surface_width());
        if let Some(a) = anchor {
          self.restore_scroll_anchor(a);
        }
        self.request_redraw();
      }
      WindowEvent::MouseWheel { delta, .. } => {
        let dy = match delta {
          MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINE_SCALE,
          MouseScrollDelta::PixelDelta(p) => -p.y as f32 * WHEEL_PIXEL_SCALE,
        };
        self.scroll_by(dy);
        self.request_redraw();
      }
      WindowEvent::CursorMoved { position, .. } => {
        self.cursor = position;
        if self.dragging {
          if let Some(hit) = self.hit_test(position.x as f32, position.y as f32) {
            if let Some(sel) = self.selection.as_mut() {
              sel.head = hit;
            }
            self.request_redraw();
          }
        }
      }
      WindowEvent::ModifiersChanged(modifiers) => {
        self.modifiers = modifiers;
      }
      WindowEvent::MouseInput {
        state,
        button: MouseButton::Left,
        ..
      } => match state {
        ElementState::Pressed => {
          if let Some(hit) = self.hit_test(self.cursor.x as f32, self.cursor.y as f32) {
            self.selection = Some(Selection {
              anchor: hit,
              head: hit,
            });
            self.dragging = true;
            self.request_redraw();
          } else {
            self.selection = None;
            self.dragging = false;
            self.request_redraw();
          }
        }
        ElementState::Released => {
          let was_dragging = self.dragging;
          self.dragging = false;
          if let Some(sel) = self.selection {
            if sel.is_empty() {
              self.selection = None;
              if was_dragging {
                if let Some(target) = self.link_at_cursor() {
                  self.follow_link(target);
                }
              }
              self.request_redraw();
            }
          }
        }
      },
      WindowEvent::KeyboardInput {
        event:
          KeyEvent {
            state: ElementState::Pressed,
            logical_key,
            ..
          },
        ..
      } => self.handle_key(event_loop, logical_key),
      WindowEvent::RedrawRequested => self.redraw(),
      _ => {}
    }
  }
}

impl App {
  fn request_redraw(&self) {
    if let Some(w) = self.window.as_ref() {
      w.request_redraw();
    }
  }

  fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: Key) {
    crate::trace!(
      "key: {:?} ctrl={} shift={} alt={}",
      key.as_ref(),
      self.modifiers.state().control_key(),
      self.modifiers.state().shift_key(),
      self.modifiers.state().alt_key()
    );
    if self.modifiers.state().control_key() {
      if matches!(key.as_ref(), Key::Character("c") | Key::Character("C")) {
        self.copy_selection();
        return;
      }
    }
    // Search captures all input while open: Esc closes, Enter advances
    // to the next match (cycling), Backspace edits the query, and any
    // unmodified printable character appends to the query (live
    // re-search). All other keys are swallowed so vmd's normal
    // shortcuts don't fire mid-search.
    if self.search.is_some() {
      self.handle_search_key(&key);
      return;
    }
    if self.help_visible {
      match key.as_ref() {
        Key::Character("?") | Key::Named(NamedKey::Escape) | Key::Character("q") => {
          self.help_visible = false;
          self.request_redraw();
        }
        _ => {}
      }
      return;
    }
    match key.as_ref() {
      Key::Character("/") => {
        self.search = Some(SearchState {
          query: String::new(),
          matches: Vec::new(),
          current: None,
        });
        self.request_redraw();
      }
      Key::Character("?") => {
        self.help_visible = true;
        self.request_redraw();
      }
      Key::Character("y") => self.yank_visible_code(),
      Key::Character("q") | Key::Named(NamedKey::Escape) => event_loop.exit(),
      Key::Character("t") => {
        self.dark = !self.dark;
        self.relayout(self.current_surface_width());
        self.persist();
        self.request_redraw();
      }
      Key::Character("+") | Key::Character("=") => self.set_zoom(self.zoom + ZOOM_STEP),
      Key::Character("-") => self.set_zoom(self.zoom - ZOOM_STEP),
      Key::Character("0") => self.set_zoom(1.0),
      Key::Character("j") => {
        self.scroll_by(SCROLL_LINE_PX);
        self.request_redraw();
      }
      Key::Character("k") => {
        self.scroll_by(-SCROLL_LINE_PX);
        self.request_redraw();
      }
      Key::Character("d") => {
        self.scroll_by(self.viewport_h() * HALF_PAGE_FRAC);
        self.request_redraw();
      }
      Key::Character("u") => {
        self.scroll_by(-self.viewport_h() * HALF_PAGE_FRAC);
        self.request_redraw();
      }
      Key::Character("f") => {
        self.scroll_by(self.viewport_h() * FULL_PAGE_FRAC);
        self.request_redraw();
      }
      Key::Character("b") => {
        self.scroll_by(-self.viewport_h() * FULL_PAGE_FRAC);
        self.request_redraw();
      }
      Key::Character("g") => {
        self.scroll_y = 0.0;
        self.request_redraw();
      }
      Key::Character("G") => {
        self.scroll_y = self.max_scroll();
        self.request_redraw();
      }
      Key::Character("]") => self.jump_in(JumpKind::Heading, 1),
      Key::Character("[") => self.jump_in(JumpKind::Heading, -1),
      Key::Character("}") => self.jump_in(JumpKind::Block, 1),
      Key::Character("{") => self.jump_in(JumpKind::Block, -1),
      Key::Named(NamedKey::ArrowDown) => {
        self.scroll_by(SCROLL_LINE_PX);
        self.request_redraw();
      }
      Key::Named(NamedKey::ArrowUp) => {
        self.scroll_by(-SCROLL_LINE_PX);
        self.request_redraw();
      }
      Key::Named(NamedKey::PageDown) | Key::Named(NamedKey::Space) => {
        self.scroll_by(self.viewport_h() * FULL_PAGE_FRAC);
        self.request_redraw();
      }
      Key::Named(NamedKey::PageUp) => {
        self.scroll_by(-self.viewport_h() * FULL_PAGE_FRAC);
        self.request_redraw();
      }
      Key::Named(NamedKey::Home) => {
        self.scroll_y = 0.0;
        self.request_redraw();
      }
      Key::Named(NamedKey::End) => {
        self.scroll_y = self.max_scroll();
        self.request_redraw();
      }
      _ => {}
    }
  }

  fn set_zoom(&mut self, z: f32) {
    let new_zoom = z.clamp(ZOOM_MIN, ZOOM_MAX);
    if (new_zoom - self.zoom).abs() < f32::EPSILON {
      return;
    }
    let anchor = self.capture_scroll_anchor();
    self.zoom = new_zoom;
    self.relayout(self.current_surface_width());
    if let Some(a) = anchor {
      self.restore_scroll_anchor(a);
    }
    self.persist();
    self.request_redraw();
  }

  fn jump_in(&mut self, kind: JumpKind, dir: i32) {
    let Some(laid) = self.laid.as_ref() else {
      return;
    };
    let ys: &[f32] = match kind {
      JumpKind::Heading => &laid.heading_ys,
      JumpKind::Block => &laid.block_ys,
    };
    if ys.is_empty() {
      return;
    }
    let cur = self.scroll_y + HEADING_OFFSET_PX;
    let target = if dir > 0 {
      ys.iter().copied().find(|&y| y > cur + 5.0)
    } else {
      ys.iter().rev().copied().find(|&y| y < cur - 5.0)
    };
    if let Some(target) = target {
      let t = (target - HEADING_OFFSET_PX).max(0.0);
      self.scroll_y = t.min(self.max_scroll());
      self.request_redraw();
    }
  }

  fn scroll_by(&mut self, dy: f32) {
    let max = self.max_scroll();
    self.scroll_y = (self.scroll_y + dy).clamp(0.0, max);
  }

  fn max_scroll(&self) -> f32 {
    let total = self.laid.as_ref().map(|l| l.total_height).unwrap_or(0.0);
    (total - self.viewport_h()).max(0.0)
  }

  fn viewport_h(&self) -> f32 {
    self.surface_size.1 as f32
  }

  /// Capture the content reference at the viewport top so it can be
  /// restored after a reflow (resize, dpi, zoom). Prefers a heading
  /// within `2 × line-height` of the viewport top — gives a "snap to
  /// heading" feel during resize. For Text / CodeBlock anchors also
  /// captures a Cursor + sub-line residual so the same line of text
  /// stays at the same screen y, even when re-wrapping shifts where
  /// that line lands inside its block.
  fn capture_scroll_anchor(&self) -> Option<ScrollAnchor> {
    let laid = self.laid.as_ref()?;
    if laid.blocks.is_empty() {
      return None;
    }
    let scroll_y = self.scroll_y;

    // First block whose footprint reaches viewport top.
    let topmost_idx = laid
      .blocks
      .iter()
      .position(|b| b.y + b.h > scroll_y)
      .unwrap_or(laid.blocks.len() - 1);

    // Heading-snap: prefer a heading within `snap_distance` (above OR
    // below) of viewport top so resize feels like the heading is
    // pinned in place.
    let snap_distance = (BODY_FS_APPROX * BODY_LH_APPROX) * self.zoom * self.dpi_scale.max(1.0);
    let viewport_top = scroll_y;
    let mut anchor_idx = topmost_idx;
    for &h_idx in &laid.heading_block_idxs {
      let block = &laid.blocks[h_idx];
      let dist = block.y - viewport_top;
      if dist > snap_distance {
        // headings are y-sorted via the iteration order — past the snap zone
        break;
      }
      if dist.abs() <= snap_distance {
        anchor_idx = h_idx;
        break;
      }
    }

    let block = &laid.blocks[anchor_idx];
    let block_y_offset = scroll_y - block.y;

    // Sub-block precision: for Text/CodeBlock, hit-test the buffer
    // at viewport-top to get a cursor, and capture the residual pixel
    // offset from that cursor's run top to viewport top.
    let (buffer, inner_offset) = match &block.kind {
      LaidKind::Text { buffer, .. } => (Some(buffer), 0.0),
      LaidKind::CodeBlock { buffer, pad_y, .. } => (Some(buffer), *pad_y),
      _ => (None, 0.0),
    };
    let (cursor, residual) = if let Some(buf) = buffer {
      let y_in_block = scroll_y - block.y - inner_offset;
      let probe_y = y_in_block.max(0.0);
      if let Some(cur) = buf.hit(0.0, probe_y) {
        let cur_y = cursor_y_in_buffer(buf, &cur).unwrap_or(probe_y);
        (Some(cur), y_in_block - cur_y)
      } else {
        (None, 0.0)
      }
    } else {
      (None, 0.0)
    };

    Some(ScrollAnchor {
      block_idx: anchor_idx,
      block_y_offset,
      cursor,
      residual,
    })
  }

  /// Restore scroll position from a previously-captured anchor. Uses
  /// the cursor + residual when available (sub-line precision); falls
  /// back to the raw block_y_offset for blocks without buffers.
  fn restore_scroll_anchor(&mut self, a: ScrollAnchor) {
    let Some(laid) = self.laid.as_ref() else {
      return;
    };
    let Some(block) = laid.blocks.get(a.block_idx) else {
      return;
    };
    let target = if let Some(cursor) = a.cursor {
      let (buffer, inner_offset) = match &block.kind {
        LaidKind::Text { buffer, .. } => (Some(buffer), 0.0),
        LaidKind::CodeBlock { buffer, pad_y, .. } => (Some(buffer), *pad_y),
        _ => (None, 0.0),
      };
      buffer
        .and_then(|buf| cursor_y_in_buffer(buf, &cursor))
        .map(|cy| block.y + inner_offset + cy + a.residual)
        .unwrap_or_else(|| block.y + a.block_y_offset)
    } else {
      block.y + a.block_y_offset
    };
    let max = (laid.total_height - self.viewport_h()).max(0.0);
    self.scroll_y = target.clamp(0.0, max);
  }

  /// Re-read the watched file, reparse, relayout, and request a redraw.
  /// On a missing file (e.g., editor mid-rename) we silently keep the
  /// current contents — the next event after the rename completes
  /// will trigger another reload.
  /// Window before a deferred "empty file" reload is treated as a
  /// genuine erase. Editors that truncate-then-write produce a brief
  /// empty moment (a few ms typically); 150ms covers that without
  /// making a real erase feel laggy.
  const EMPTY_RECHECK_MS: u64 = 150;

  fn reload_from_disk(&mut self) {
    let Some(path) = self.watch_path.as_ref() else {
      return;
    };
    let source = match std::fs::read_to_string(path) {
      Ok(s) => s,
      Err(_) => return,
    };
    // Transient-empty handling: editors that truncate-then-write fire
    // a notify event while the file is briefly empty, before the real
    // content lands. If we apply that state immediately the doc
    // collapses and scroll position is lost. Defer applying empty
    // for a short window — a follow-up event with content cancels
    // the deadline (the non-empty branch below); if the timer
    // expires the erase is genuine and `check_empty_deadline` will
    // re-read and apply.
    if source.trim().is_empty() && !self.doc.blocks.is_empty() {
      let deadline =
        Instant::now() + Duration::from_millis(Self::EMPTY_RECHECK_MS);
      self.empty_reload_deadline = Some(deadline);
      crate::trace!(
        "reload_deferred (empty, recheck in {}ms)",
        Self::EMPTY_RECHECK_MS
      );
      return;
    }
    self.empty_reload_deadline = None;
    self.apply_reload(source);
  }

  /// Called from `new_events` when a `WaitUntil` deadline fires; if
  /// the empty-recheck deadline elapsed, re-read the file once more
  /// and apply whatever's there (still empty → genuine erase, now
  /// has content → late write that we apply normally).
  fn check_empty_deadline(&mut self) {
    let Some(deadline) = self.empty_reload_deadline else {
      return;
    };
    if Instant::now() < deadline {
      return;
    }
    self.empty_reload_deadline = None;
    let Some(path) = self.watch_path.as_ref() else {
      return;
    };
    let source = std::fs::read_to_string(path).unwrap_or_default();
    crate::trace!("reload_after_recheck bytes={}", source.len());
    self.apply_reload(source);
  }

  fn apply_reload(&mut self, source: String) {
    crate::trace!("reload_from_disk bytes={}", source.len());
    self.doc = crate::doc::parse(&source);
    self.selection = None;
    // Refresh image dims for any new srcs and synchronously decode the
    // new ones. Existing entries are left in place so already-loaded
    // pixels stay cached. Reload happens in response to a user save,
    // not the cold-launch path, so blocking briefly here is acceptable.
    let new_paths = crate::images::collect_image_paths(&self.doc, self.base_dir.as_deref());
    for p in &new_paths {
      if self.images.get_dims(p).is_none() {
        let dims = crate::images::read_dims(p);
        self.images.insert_dims(p.clone(), dims);
      }
      if self.images.get_frames(p).is_none() {
        if let Some(frames) = crate::images::decode_frames(p) {
          self.images.set_frames(p, frames);
        } else {
          self.images.set_failed(p);
        }
      }
    }
    // Pin the currently-topmost block to the same screen position
    // across the relayout, the same way resize handles it. Without
    // this, any height change above the viewport (a new paragraph,
    // an image's real dims arriving, a heading inserted) just clamps
    // scroll_y and the user visually jumps.
    let anchor = self.capture_scroll_anchor();
    self.relayout(self.current_surface_width());
    if let Some(a) = anchor {
      self.restore_scroll_anchor(a);
    }
    self.request_redraw();
  }

  fn relayout(&mut self, surface_w: f32) {
    // If the precompute thread has already populated the cache, lay out with
    // full highlighting from the start — avoids the placeholder pass and a
    // later in-place upgrade.
    if !self.full_highlight && self.highlight_ready.load(Ordering::Acquire) {
      self.full_highlight = true;
    }
    let theme = Theme::select(self.dark);
    let scale = self.zoom * self.dpi_scale.max(1.0);
    let laid = layout_parallel(
      &self.doc,
      surface_w,
      &mut self.painter.fs,
      &mut self.layout_workers,
      &theme,
      self.full_highlight,
      scale,
      self.images.clone(),
      self.base_dir.clone(),
    );
    let max = (laid.total_height - self.viewport_h()).max(0.0);
    self.scroll_y = self.scroll_y.clamp(0.0, max);
    self.laid = Some(laid);
  }

  /// Walk visible blocks; for each animated image, ask the store when
  /// its next frame transition is, and return the soonest among them.
  /// `None` means nothing visible is animating, so the loop can sleep
  /// indefinitely (`ControlFlow::Wait`).
  fn compute_next_anim_deadline(&self, now: Instant, elapsed_ms: u128) -> Option<Instant> {
    let laid = self.laid.as_ref()?;
    let view_h = self.viewport_h();
    let mut earliest_ms: Option<u32> = None;
    for block in &laid.blocks {
      let by = block.y - self.scroll_y;
      if by + block.h < 0.0 || by > view_h {
        continue;
      }
      let LaidKind::Image { path, .. } = &block.kind else {
        continue;
      };
      let Some(p) = path.as_ref() else { continue };
      let Some((frames, total_ms)) = self.images.get_frames(p) else {
        continue;
      };
      if let Some(ms) = crate::images::ms_until_next_frame(&frames, total_ms, elapsed_ms) {
        earliest_ms = Some(earliest_ms.map_or(ms, |e| e.min(ms)));
      }
    }
    earliest_ms.map(|ms| now + Duration::from_millis(ms as u64))
  }

  /// Resolve the URL fragment passed as `vmd file.md#section`: walk
  /// headings in document order, slugify each, scroll to the first
  /// match. Run once after the first layout; clears `pending_anchor`
  /// either way so resizes don't keep snapping back. Silent on miss.
  fn apply_pending_anchor(&mut self) {
    let Some(anchor) = self.pending_anchor.take() else {
      return;
    };
    let target = slugify(&anchor);
    let Some(laid) = self.laid.as_ref() else {
      return;
    };
    let mut hi = 0usize;
    for block in &self.doc.blocks {
      if let crate::doc::Block::Heading { inlines, .. } = block {
        let text = crate::doc::flatten_text(inlines);
        if slugify(&text) == target {
          if let Some(y) = laid.heading_ys.get(hi) {
            let max = (laid.total_height - self.viewport_h()).max(0.0);
            self.scroll_y =
              (*y - HEADING_OFFSET_PX * self.dpi_scale.max(1.0)).clamp(0.0, max);
            crate::trace!("anchor_jump '{}' -> y={}", anchor, *y);
          }
          return;
        }
        hi += 1;
      }
    }
    crate::trace!("anchor_miss '{}'", anchor);
  }

  fn handle_search_key(&mut self, key: &Key) {
    match key.as_ref() {
      Key::Named(NamedKey::Escape) => {
        self.search = None;
        self.request_redraw();
      }
      Key::Named(NamedKey::Enter) => {
        self.advance_search_match();
      }
      Key::Named(NamedKey::Backspace) => {
        if let Some(s) = self.search.as_mut() {
          if s.query.pop().is_some() {
            self.recompute_search();
          }
        }
      }
      Key::Character(s) => {
        // Skip Ctrl/Alt-modified character events so e.g. Ctrl+C copy
        // (handled above) doesn't double-fire and so other shortcuts
        // don't end up in the query string. Plain Shift is fine —
        // it's already baked into the logical key character.
        if self.modifiers.state().control_key() || self.modifiers.state().alt_key() {
          return;
        }
        if let Some(state) = self.search.as_mut() {
          state.query.push_str(s);
        }
        self.recompute_search();
      }
      _ => {}
    }
  }

  /// Re-walks the laid doc against the current query, populates the
  /// matches list, and jumps to the first occurrence. Called on every
  /// query mutation (char append, backspace).
  fn recompute_search(&mut self) {
    let Some(state) = self.search.as_mut() else {
      return;
    };
    state.matches.clear();
    state.current = None;
    let q_lower = state.query.to_lowercase();
    if q_lower.is_empty() {
      self.request_redraw();
      return;
    }
    let Some(laid) = self.laid.as_ref() else {
      self.request_redraw();
      return;
    };
    for (idx, lb) in laid.blocks.iter().enumerate() {
      match &lb.kind {
        LaidKind::Text { buffer, .. } | LaidKind::CodeBlock { buffer, .. } => {
          for (line_i, line) in buffer.lines.iter().enumerate() {
            push_line_matches(line.text(), &q_lower, idx, line_i, &mut state.matches);
          }
        }
        _ => {}
      }
    }
    if !state.matches.is_empty() {
      state.current = Some(0);
      let m = state.matches[0];
      self.scroll_to_match(m);
    }
    self.request_redraw();
  }

  fn advance_search_match(&mut self) {
    let Some(state) = self.search.as_mut() else {
      return;
    };
    if state.matches.is_empty() {
      return;
    }
    let next = state.current.map(|c| (c + 1) % state.matches.len()).unwrap_or(0);
    state.current = Some(next);
    let m = state.matches[next];
    self.scroll_to_match(m);
    self.request_redraw();
  }

  /// Build a per-laid-block table of search hits for the current
  /// query, marking the active occurrence so paint can color it
  /// distinctly. Returns `None` when search is closed or has no
  /// matches; otherwise a Vec sized to `laid.blocks.len()` with
  /// each slot's hits in document order.
  fn search_hits_by_block(&self) -> Option<Vec<Vec<crate::paint::SearchHit>>> {
    let state = self.search.as_ref()?;
    if state.matches.is_empty() {
      return None;
    }
    let laid = self.laid.as_ref()?;
    let mut out: Vec<Vec<crate::paint::SearchHit>> =
      (0..laid.blocks.len()).map(|_| Vec::new()).collect();
    for (i, m) in state.matches.iter().enumerate() {
      if m.laid_block_idx >= out.len() {
        continue;
      }
      out[m.laid_block_idx].push(crate::paint::SearchHit {
        line_i: m.line_i,
        byte_start: m.byte_start,
        byte_end: m.byte_end,
        active: state.current == Some(i),
      });
    }
    Some(out)
  }

  /// Scrolls so the laid block containing the match sits near
  /// viewport top, with heading-style breathing room.
  fn scroll_to_match(&mut self, m: SearchMatch) {
    let Some(laid) = self.laid.as_ref() else {
      return;
    };
    let Some(block) = laid.blocks.get(m.laid_block_idx) else {
      return;
    };
    let max = (laid.total_height - self.viewport_h()).max(0.0);
    self.scroll_y =
      (block.y - HEADING_OFFSET_PX * self.dpi_scale.max(1.0)).clamp(0.0, max);
  }

  fn current_surface_width(&self) -> f32 {
    let w = self.surface_size.0;
    if w == 0 { 920.0 } else { w as f32 }
  }

  fn persist(&self) {
    state::save(&Prefs {
      theme: Some(self.dark),
      zoom: Some(self.zoom),
    });
  }

  fn yank_visible_code(&mut self) {
    let viewport_top = self.scroll_y;
    let viewport_bottom = self.scroll_y + self.viewport_h();
    let viewport_center = (viewport_top + viewport_bottom) / 2.0;
    let mut best: Option<(f32, String)> = None;
    if let Some(laid) = self.laid.as_ref() {
      for block in &laid.blocks {
        if let LaidKind::CodeBlock { source, .. } = &block.kind {
          if block.y + block.h < viewport_top || block.y > viewport_bottom {
            continue;
          }
          let center = block.y + block.h / 2.0;
          let dist = (center - viewport_center).abs();
          if best.as_ref().map_or(true, |(d, _)| dist < *d) {
            best = Some((dist, source.clone()));
          }
        }
      }
    }
    if let Some((_, src)) = best {
      crate::trace!("yank_visible_code: {} chars", src.len());
      self.set_clipboard(src);
    } else {
      crate::trace!("yank_visible_code: no code block in viewport");
    }
  }

  fn hit_test(&self, win_x: f32, win_y: f32) -> Option<HitPoint> {
    let laid = self.laid.as_ref()?;
    let dy = win_y + self.scroll_y;
    let dx = win_x;
    for (i, block) in laid.blocks.iter().enumerate() {
      if dy < block.y || dy > block.y + block.h {
        continue;
      }
      match &block.kind {
        LaidKind::Text { buffer, .. } => {
          let lx = dx - block.x;
          let ly = dy - block.y;
          if let Some(c) = buffer.hit(lx, ly) {
            return Some(HitPoint {
              block_idx: i,
              cursor: c,
            });
          }
        }
        LaidKind::CodeBlock {
          buffer,
          pad_x,
          pad_y,
          ..
        } => {
          let lx = dx - block.x - *pad_x;
          let ly = dy - block.y - *pad_y;
          if let Some(c) = buffer.hit(lx, ly) {
            return Some(HitPoint {
              block_idx: i,
              cursor: c,
            });
          }
        }
        _ => {}
      }
    }
    None
  }

  fn copy_selection(&mut self) {
    let Some(sel) = self.selection else {
      crate::trace!("copy_selection: no selection");
      return;
    };
    if sel.is_empty() {
      crate::trace!("copy_selection: empty selection");
      return;
    }
    let Some(laid) = self.laid.as_ref() else {
      return;
    };
    let (start, end) = sel.ordered();
    let text = collect_selection_text(laid, &start, &end);
    crate::trace!("copy_selection: {} chars", text.len());
    if text.is_empty() {
      return;
    }
    self.set_clipboard(text);
  }

  fn set_clipboard(&mut self, text: String) {
    let n = text.len();
    // Wayland: store on the smithay-clipboard tied to our window's
    // wl_display. The connection has a focused surface, so the
    // compositor accepts our wl_data_source.
    if let Some(clip) = self.wayland_clipboard.as_ref() {
      clip.store(text);
      crate::trace!("clipboard: smithay {n} chars");
      return;
    }
    // Non-Wayland fallback (X11 / macOS / Windows).
    if self.clipboard.is_none() {
      self.clipboard = arboard::Clipboard::new().ok();
    }
    match self.clipboard.as_mut() {
      Some(clip) => match clip.set_text(text) {
        Ok(()) => crate::trace!("clipboard: arboard {n} chars"),
        Err(e) => crate::trace!("clipboard: arboard set_text failed: {e}"),
      },
      None => crate::trace!("clipboard: init failed"),
    }
  }

  fn link_at_cursor(&self) -> Option<crate::layout::LinkTarget> {
    let laid = self.laid.as_ref()?;
    let cx = self.cursor.x as f32;
    let cy = self.cursor.y as f32 + self.scroll_y;
    for block in &laid.blocks {
      if cy < block.y || cy > block.y + block.h {
        continue;
      }
      match &block.kind {
        LaidKind::Text { buffer, links, .. } => {
          if let Some(t) = hit_test_links_in_buffer(buffer, links, cx - block.x, cy - block.y) {
            return Some(t);
          }
        }
        LaidKind::Table { rows, .. } => {
          for row in rows.iter() {
            let row_y_abs = block.y + row.y_top;
            if cy < row_y_abs || cy > row_y_abs + row.h {
              continue;
            }
            for cell in &row.cells {
              if cell.links.is_empty() {
                continue;
              }
              // Mirror paint_table_cell's positioning: pad scales with
              // cell.w, then alignment offset shifts left/right within
              // the cell's text column.
              let pad_x = (cell.w * 0.04).clamp(6.0, 24.0);
              let pad_y = (pad_x * 0.7).max(6.0);
              let cell_text_w = (cell.w - pad_x * 2.0).max(0.0);
              let actual_text_w = cell
                .buffer
                .layout_runs()
                .map(|r| r.line_w)
                .fold(0.0_f32, f32::max);
              let extra = (cell_text_w - actual_text_w).max(0.0);
              let dx = match cell.align {
                crate::doc::CellAlign::Left => 0.0,
                crate::doc::CellAlign::Center => extra / 2.0,
                crate::doc::CellAlign::Right => extra,
              };
              let text_origin_x = block.x + cell.x + pad_x + dx;
              let text_origin_y = row_y_abs + pad_y;
              if let Some(t) = hit_test_links_in_buffer(
                &cell.buffer,
                &cell.links,
                cx - text_origin_x,
                cy - text_origin_y,
              ) {
                return Some(t);
              }
            }
          }
        }
        _ => {}
      }
    }
    None
  }

  fn follow_link(&mut self, target: crate::layout::LinkTarget) {
    use crate::layout::LinkTarget;
    match target {
      LinkTarget::Url(href) => {
        let _ = opener::open(&href);
      }
      LinkTarget::Footnote(label) => self.scroll_to_footnote(&label, /* forward */ true),
      LinkTarget::FootnoteBack(label) => self.scroll_to_footnote(&label, /* forward */ false),
    }
  }

  fn scroll_to_footnote(&mut self, label: &str, forward: bool) {
    let Some(laid) = self.laid.as_ref() else {
      return;
    };
    let Some(jump) = laid.footnote_jumps.get(label) else {
      crate::trace!("footnote_miss '{}'", label);
      return;
    };
    let Some(target_y) = (if forward {
      Some(jump.def_y)
    } else {
      jump.first_ref_y
    }) else {
      return;
    };
    let max = (laid.total_height - self.viewport_h()).max(0.0);
    self.scroll_y =
      (target_y - HEADING_OFFSET_PX * self.dpi_scale.max(1.0)).clamp(0.0, max);
    crate::trace!(
      "footnote_jump '{}' {} -> y={}",
      label,
      if forward { "fwd" } else { "back" },
      target_y
    );
    self.request_redraw();
  }

  fn redraw(&mut self) {
    // Run the in-place code-block upgrade if it was scheduled OR if the
    // syntect cache has just become ready. The latter check folds an
    // upgrade INTO the current redraw whenever possible, avoiding the
    // 2-frame placeholder→highlighted flash and the request_redraw
    // round-trip for the upgrade pass.
    let do_upgrade = self.upgrade_pending
      || (!self.full_highlight && self.highlight_ready.load(Ordering::Acquire));
    if do_upgrade {
      self.upgrade_pending = false;
      self.full_highlight = true;
      crate::trace!("relayout_full_highlight");
      if let Some(laid) = self.laid.as_mut() {
        let theme = Theme::select(self.dark);
        let scale = self.zoom * self.dpi_scale.max(1.0);
        crate::layout::upgrade_code_block_highlights(laid, &mut self.painter.fs, &theme, scale);
        let max = (laid.total_height - self.viewport_h()).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
      }
      crate::trace!("relayout_full_highlight_done");
    }

    // Compute search hits before grabbing the surface mutably below —
    // builder borrows `self` immutably and would conflict with the
    // surface's mutable borrow, which has to live until present().
    let hits_by_block = self.search_hits_by_block();

    let Some(surface) = self.surface.as_mut() else {
      return;
    };

    if !self.painted_once {
      crate::trace!("redraw_first");
    }

    let theme = Theme::select(self.dark);
    let now = Instant::now();
    let anim_elapsed_ms = now.saturating_duration_since(self.anim_start).as_millis();
    let mut buffer = surface.buffer_mut().expect("buffer_mut");
    let (fw, fh) = self.surface_size;
    let mut frame = Frame::new(&mut buffer, fw, fh);
    if let Some(laid) = self.laid.as_ref() {
      let hits_view = hits_by_block.as_ref().map(|v| crate::paint::SearchHits {
        by_block: v.as_slice(),
      });
      self.painter.paint_doc(
        &mut frame,
        laid,
        &theme,
        self.scroll_y,
        &self.images,
        anim_elapsed_ms,
        hits_view.as_ref(),
      );
    } else {
      self.painter.paint_blank(&mut frame, &theme);
    }

    if let Some(sel) = self.selection {
      if !sel.is_empty() {
        if let Some(laid) = self.laid.as_ref() {
          self
            .painter
            .paint_selection(&mut frame, laid, &sel, &theme, self.scroll_y);
        }
      }
    }

    let overlay_scale = self.zoom * self.dpi_scale.max(1.0);
    if let Some(s) = self.search.as_ref() {
      self.painter.paint_search_overlay(
        &mut frame,
        &theme,
        &s.query,
        s.current,
        s.matches.len(),
        overlay_scale,
      );
    }
    if self.help_visible {
      self
        .painter
        .paint_help_overlay(&mut frame, &theme, overlay_scale);
    }

    drop(frame);
    buffer.present().expect("present");

    // Recompute the earliest animation deadline among visible images.
    // `about_to_wait` reads `anim_next_deadline` to set the control
    // flow. Skip the walk entirely if no image in the doc is animated.
    self.anim_next_deadline = if self.images.has_animations() {
      self.compute_next_anim_deadline(now, anim_elapsed_ms)
    } else {
      None
    };

    if !self.painted_once {
      crate::trace!("first_present");
      self.painted_once = true;
      if !self.full_highlight {
        // Trigger the in-place code-block upgrade now. `highlight()` cache
        // hits return immediately; cache misses fall through to synchronous
        // compute. Doing this immediately (vs. waiting for precompute to
        // signal) lets the cache-hit code blocks be re-shaped in parallel
        // with any remaining precompute work, which empirically wins on
        // code-heavy docs.
        self.upgrade_pending = true;
        self.request_redraw();
      }
    }
  }
}

#[derive(Clone, Copy)]
enum JumpKind {
  Heading,
  Block,
}

fn collect_selection_text(laid: &LaidDoc, start: &HitPoint, end: &HitPoint) -> String {
  if start.block_idx == end.block_idx {
    let block = &laid.blocks[start.block_idx];
    return block_substring(block, &start.cursor, &end.cursor);
  }
  let mut out = String::new();
  let start_block = &laid.blocks[start.block_idx];
  out.push_str(&block_substring_from(start_block, &start.cursor));
  out.push('\n');
  for i in (start.block_idx + 1)..end.block_idx {
    let b = &laid.blocks[i];
    let text = block_full_text(b);
    if !text.is_empty() {
      out.push_str(&text);
      out.push('\n');
    }
  }
  let end_block = &laid.blocks[end.block_idx];
  out.push_str(&block_substring_to(end_block, &end.cursor));
  out
}

fn buffer_text_lines(buf: &cosmic_text::Buffer) -> Vec<&str> {
  buf.lines.iter().map(|l| l.text()).collect()
}

fn substring_between(lines: &[&str], from: &Cursor, to: &Cursor) -> String {
  if from.line == to.line {
    let line = lines.get(from.line).copied().unwrap_or("");
    let s = from.index.min(line.len());
    let e = to.index.min(line.len());
    return line[s..e].to_string();
  }
  let mut out = String::new();
  if let Some(first) = lines.get(from.line) {
    let s = from.index.min(first.len());
    out.push_str(&first[s..]);
  }
  out.push('\n');
  for i in (from.line + 1)..to.line {
    if let Some(line) = lines.get(i) {
      out.push_str(line);
      out.push('\n');
    }
  }
  if let Some(last) = lines.get(to.line) {
    let e = to.index.min(last.len());
    out.push_str(&last[..e]);
  }
  out
}

fn block_buffer(block: &crate::layout::LaidBlock) -> Option<&cosmic_text::Buffer> {
  match &block.kind {
    LaidKind::Text { buffer, .. } => Some(buffer),
    LaidKind::CodeBlock { buffer, .. } => Some(buffer),
    _ => None,
  }
}

fn block_substring(block: &crate::layout::LaidBlock, from: &Cursor, to: &Cursor) -> String {
  let Some(buf) = block_buffer(block) else {
    return String::new();
  };
  let lines = buffer_text_lines(buf);
  substring_between(&lines, from, to)
}

fn block_substring_from(block: &crate::layout::LaidBlock, from: &Cursor) -> String {
  let Some(buf) = block_buffer(block) else {
    return String::new();
  };
  let lines = buffer_text_lines(buf);
  let last_line = lines.len().saturating_sub(1);
  let to = Cursor {
    line: last_line,
    index: lines.last().map(|l| l.len()).unwrap_or(0),
    affinity: cosmic_text::Affinity::After,
  };
  substring_between(&lines, from, &to)
}

fn block_substring_to(block: &crate::layout::LaidBlock, to: &Cursor) -> String {
  let Some(buf) = block_buffer(block) else {
    return String::new();
  };
  let lines = buffer_text_lines(buf);
  let from = Cursor {
    line: 0,
    index: 0,
    affinity: cosmic_text::Affinity::Before,
  };
  substring_between(&lines, &from, to)
}

fn block_full_text(block: &crate::layout::LaidBlock) -> String {
  let Some(buf) = block_buffer(block) else {
    return String::new();
  };
  let lines = buffer_text_lines(buf);
  lines.join("\n")
}

/// Find every case-insensitive occurrence of `q_lower` in `line_text`
/// and push a `SearchMatch` for each, byte-anchored to the original
/// (un-lowercased) line. We lowercase only as a comparison key —
/// some Unicode cases change byte length under to_lowercase, but
/// case-folding in ASCII text (the common case) preserves length, so
/// the offsets line up with the original text.
fn push_line_matches(
  line_text: &str,
  q_lower: &str,
  laid_block_idx: usize,
  line_i: usize,
  out: &mut Vec<SearchMatch>,
) {
  if q_lower.is_empty() {
    return;
  }
  let lower = line_text.to_lowercase();
  // Bail if lowercasing changed length (would skew byte offsets) —
  // matches in non-ASCII text won't highlight rather than highlight
  // the wrong glyphs. Search-in-block-list still finds them.
  if lower.len() != line_text.len() {
    return;
  }
  let mut start = 0;
  while let Some(pos) = lower[start..].find(q_lower) {
    let abs = start + pos;
    let end = abs + q_lower.len();
    out.push(SearchMatch {
      laid_block_idx,
      line_i,
      byte_start: abs,
      byte_end: end,
    });
    if end == abs {
      break;
    }
    start = end;
  }
}

/// Hit-test a (local_x, local_y) point against a buffer's link ranges.
/// `local_*` are coordinates relative to the buffer's draw origin —
/// the same origin `draw_buffer` uses. Returns the link target whose
/// glyph the point falls inside, if any.
fn hit_test_links_in_buffer(
  buffer: &cosmic_text::Buffer,
  links: &[crate::layout::LinkRange],
  local_x: f32,
  local_y: f32,
) -> Option<crate::layout::LinkTarget> {
  if links.is_empty() {
    return None;
  }
  let lh = buffer.metrics().line_height;
  for run in buffer.layout_runs() {
    let run_top = run.line_top;
    let run_bot = run_top + lh;
    if local_y < run_top || local_y > run_bot {
      continue;
    }
    for g in run.glyphs.iter() {
      if local_x < g.x || local_x > g.x + g.w {
        continue;
      }
      for link in links {
        if g.start >= link.byte_start && g.end <= link.byte_end {
          return Some(link.target.clone());
        }
      }
    }
  }
  None
}

/// Convert heading text to a GitHub-style anchor slug:
///   "Foo Bar"      → "foo-bar"
///   "C++ in 5min"  → "c-in-5min"
///   "  spaces  "   → "spaces"
/// Lowercase, ASCII-alphanumeric and `-` survive, everything else
/// becomes a hyphen, then collapse runs of hyphens and trim leading
/// and trailing hyphens. Matches the slugs `vmd file.md#section`
/// users will type from a TOC or another viewer.
pub fn slugify(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let mut last_hyphen = true; // suppress leading hyphens
  for c in s.chars() {
    if c.is_ascii_alphanumeric() {
      out.push(c.to_ascii_lowercase());
      last_hyphen = false;
    } else if !last_hyphen {
      out.push('-');
      last_hyphen = true;
    }
  }
  while out.ends_with('-') {
    out.pop();
  }
  out
}
