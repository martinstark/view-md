use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cosmic_text::{Cursor, FontSystem, SwashCache};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use softbuffer::{Context, Surface};
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::doc::Doc;
use crate::layout::{LaidDoc, LaidKind, layout_parallel};
use crate::paint::{Painter, pixmap_to_softbuffer};
use crate::state::{self, Prefs};
use crate::theme::Theme;

/// Tuple returned by the speculative-layout background thread. Stored in
/// `App.spec_handle` until `resumed()` joins it — moves the join from
/// before `event_loop.run_app` to *after* `create_window`, so window/surface
/// init runs in parallel with the bg layout+warm work (item T1.5).
pub type SpecResult = (Doc, FontSystem, Vec<FontSystem>, LaidDoc, bool, SwashCache);

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
  pub pixmap: Option<Pixmap>,
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
}

#[derive(Clone, Copy, Debug)]
pub struct HitPoint {
  pub block_idx: usize,
  pub cursor: Cursor,
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

fn cursor_le(a: &Cursor, b: &Cursor) -> bool {
  if a.line != b.line {
    a.line < b.line
  } else {
    a.index <= b.index
  }
}

impl ApplicationHandler for App {
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

    self.pixmap = Some(Pixmap::new(w, h).expect("pixmap"));
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
          self.pixmap = Some(Pixmap::new(w, h).expect("pixmap"));
          self.relayout(w as f32);
          self.request_redraw();
        }
      }
      WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
        self.dpi_scale = scale_factor as f32;
        self.relayout(self.current_surface_width());
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
                if let Some(href) = self.link_at_cursor() {
                  let _ = opener::open(&href);
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
    self.zoom = new_zoom;
    self.relayout(self.current_surface_width());
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
    self
      .pixmap
      .as_ref()
      .map(|p| p.height() as f32)
      .unwrap_or(0.0)
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
    );
    let max = (laid.total_height - self.viewport_h()).max(0.0);
    self.scroll_y = self.scroll_y.clamp(0.0, max);
    self.laid = Some(laid);
  }

  fn current_surface_width(&self) -> f32 {
    self
      .pixmap
      .as_ref()
      .map(|p| p.width() as f32)
      .unwrap_or(920.0)
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

  fn link_at_cursor(&self) -> Option<String> {
    let laid = self.laid.as_ref()?;
    let cx = self.cursor.x as f32;
    let cy = self.cursor.y as f32 + self.scroll_y;
    for block in &laid.blocks {
      if cy < block.y || cy > block.y + block.h {
        continue;
      }
      if let LaidKind::Text { buffer, links, .. } = &block.kind {
        if links.is_empty() {
          continue;
        }
        let local_x = cx - block.x;
        let local_y = cy - block.y;
        for run in buffer.layout_runs() {
          let run_top = run.line_top;
          let run_bot = run_top + buffer.metrics().line_height;
          if local_y < run_top || local_y > run_bot {
            continue;
          }
          for g in run.glyphs.iter() {
            if local_x < g.x || local_x > g.x + g.w {
              continue;
            }
            for link in links {
              if g.start >= link.byte_start && g.end <= link.byte_end {
                return Some(link.href.clone());
              }
            }
          }
        }
      }
    }
    None
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

    let (Some(surface), Some(pixmap)) = (self.surface.as_mut(), self.pixmap.as_mut()) else {
      return;
    };

    if !self.painted_once {
      crate::trace!("redraw_first");
    }

    let theme = Theme::select(self.dark);
    if let Some(laid) = self.laid.as_ref() {
      self.painter.paint_doc(pixmap, laid, &theme, self.scroll_y);
    } else {
      self.painter.paint_blank(pixmap, &theme);
    }

    if let Some(sel) = self.selection {
      if !sel.is_empty() {
        if let Some(laid) = self.laid.as_ref() {
          self
            .painter
            .paint_selection(pixmap, laid, &sel, &theme, self.scroll_y);
        }
      }
    }

    if self.help_visible {
      self.painter.paint_help_overlay(pixmap, &theme);
    }

    let mut buffer = surface.buffer_mut().expect("buffer_mut");
    pixmap_to_softbuffer(pixmap, &mut buffer);
    buffer.present().expect("present");

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
