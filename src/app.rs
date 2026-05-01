use std::num::NonZeroU32;
use std::rc::Rc;

use softbuffer::{Context, Surface};
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::doc::Doc;
use crate::layout::{LaidDoc, layout};
use crate::paint::{Painter, pixmap_to_softbuffer};
use crate::theme::Theme;

pub struct App {
    pub title: String,
    pub doc: Doc,
    pub painter: Painter,
    pub dark: bool,
    pub scroll_y: f32,
    pub window: Option<Rc<Window>>,
    pub surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    pub pixmap: Option<Pixmap>,
    pub laid: Option<LaidDoc>,
    pub painted_once: bool,
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
                    if let Some(win) = self.window.as_ref() {
                        win.request_redraw();
                    }
                }
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
    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: Key) {
        let request_redraw = |this: &Self| {
            if let Some(w) = this.window.as_ref() {
                w.request_redraw();
            }
        };
        match key.as_ref() {
            Key::Character("q") | Key::Named(NamedKey::Escape) => event_loop.exit(),
            Key::Character("t") => {
                self.dark = !self.dark;
                request_redraw(self);
            }
            Key::Character("j") => {
                self.scroll_by(40.0);
                request_redraw(self);
            }
            Key::Character("k") => {
                self.scroll_by(-40.0);
                request_redraw(self);
            }
            Key::Character("g") => {
                self.scroll_y = 0.0;
                request_redraw(self);
            }
            Key::Character("G") => {
                self.scroll_to_bottom();
                request_redraw(self);
            }
            _ => {}
        }
    }

    fn scroll_by(&mut self, dy: f32) {
        let max = self.max_scroll();
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, max);
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_y = self.max_scroll();
    }

    fn max_scroll(&self) -> f32 {
        let viewport_h = self.pixmap.as_ref().map(|p| p.height() as f32).unwrap_or(0.0);
        let total = self.laid.as_ref().map(|l| l.total_height).unwrap_or(0.0);
        (total - viewport_h).max(0.0)
    }

    fn relayout(&mut self, surface_w: f32) {
        let theme = Theme::select(self.dark);
        let laid = layout(&self.doc, surface_w, &mut self.painter.fs, &theme);
        let max = (laid.total_height - self.pixmap.as_ref().map(|p| p.height() as f32).unwrap_or(0.0)).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
        self.laid = Some(laid);
    }

    fn redraw(&mut self) {
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
        }
    }
}
