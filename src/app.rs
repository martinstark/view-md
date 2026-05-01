use std::num::NonZeroU32;
use std::rc::Rc;

use softbuffer::{Context, Surface};
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::doc::Doc;
use crate::layout::{LaidDoc, layout};
use crate::paint::{Painter, pixmap_to_softbuffer};
use crate::state::{self, Prefs};
use crate::theme::Theme;

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
    pub title: String,
    pub doc: Doc,
    pub painter: Painter,
    pub dark: bool,
    pub zoom: f32,
    pub scroll_y: f32,
    pub window: Option<Rc<Window>>,
    pub surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    pub pixmap: Option<Pixmap>,
    pub laid: Option<LaidDoc>,
    pub painted_once: bool,
    pub full_highlight: bool,
    pub upgrade_pending: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        crate::trace!("resumed");

        let attrs = Window::default_attributes()
            .with_title(format!("{} — mdv", self.title))
            .with_inner_size(LogicalSize::new(920.0, 1100.0));
        let window = Rc::new(event_loop.create_window(attrs).expect("window create"));
        crate::trace!("window_created");

        let context = Context::new(window.clone()).expect("softbuffer context");
        let mut surface = Surface::new(&context, window.clone()).expect("softbuffer surface");
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .expect("resize");
        crate::trace!("surface_ready");

        self.pixmap = Some(Pixmap::new(w, h).expect("pixmap"));
        self.relayout(w as f32);
        crate::trace!("layout_ready");
        self.window = Some(window.clone());
        self.surface = Some(surface);
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface.as_mut() {
                    let (w, h) = (size.width.max(1), size.height.max(1));
                    let _ = surface.resize(
                        NonZeroU32::new(w).unwrap(),
                        NonZeroU32::new(h).unwrap(),
                    );
                    self.pixmap = Some(Pixmap::new(w, h).expect("pixmap"));
                    self.relayout(w as f32);
                    self.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINE_SCALE,
                    MouseScrollDelta::PixelDelta(p) => -p.y as f32 * WHEEL_PIXEL_SCALE,
                };
                self.scroll_by(dy);
                self.request_redraw();
            }
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
        match key.as_ref() {
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
        let Some(laid) = self.laid.as_ref() else { return };
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
        self.pixmap.as_ref().map(|p| p.height() as f32).unwrap_or(0.0)
    }

    fn relayout(&mut self, surface_w: f32) {
        let theme = Theme::select(self.dark);
        let laid = layout(
            &self.doc,
            surface_w,
            &mut self.painter.fs,
            &theme,
            self.full_highlight,
            self.zoom,
        );
        let max = (laid.total_height - self.viewport_h()).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
        self.laid = Some(laid);
    }

    fn current_surface_width(&self) -> f32 {
        self.pixmap.as_ref().map(|p| p.width() as f32).unwrap_or(920.0)
    }

    fn persist(&self) {
        state::save(&Prefs {
            theme: Some(self.dark),
            zoom: Some(self.zoom),
        });
    }

    fn redraw(&mut self) {
        if self.upgrade_pending {
            self.upgrade_pending = false;
            self.full_highlight = true;
            crate::trace!("relayout_full_highlight");
            self.relayout(self.current_surface_width());
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

        let mut buffer = surface.buffer_mut().expect("buffer_mut");
        pixmap_to_softbuffer(pixmap, &mut buffer);
        buffer.present().expect("present");

        if !self.painted_once {
            crate::trace!("first_present");
            self.painted_once = true;
            if !self.full_highlight {
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
