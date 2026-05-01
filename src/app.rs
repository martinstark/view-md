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

use crate::paint::{Painter, pixmap_to_softbuffer};

pub struct App {
    pub title: String,
    pub painter: Painter,
    pub dark: bool,
    pub window: Option<Rc<Window>>,
    pub surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    pub pixmap: Option<Pixmap>,
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
            .resize(
                NonZeroU32::new(w).unwrap(),
                NonZeroU32::new(h).unwrap(),
            )
            .expect("resize");
        crate::trace!("surface_ready");

        self.pixmap = Some(Pixmap::new(w, h).expect("pixmap"));
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
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let (Some(surface), Some(_)) = (self.surface.as_mut(), self.window.as_ref()) {
                    let (w, h) = (size.width.max(1), size.height.max(1));
                    let _ = surface.resize(
                        NonZeroU32::new(w).unwrap(),
                        NonZeroU32::new(h).unwrap(),
                    );
                    self.pixmap = Some(Pixmap::new(w, h).expect("pixmap"));
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
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
            } => {
                self.handle_key(event_loop, logical_key);
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

impl App {
    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: Key) {
        match key.as_ref() {
            Key::Character("q") | Key::Named(NamedKey::Escape) => event_loop.exit(),
            Key::Character("t") => {
                self.dark = !self.dark;
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn redraw(&mut self) {
        let (Some(surface), Some(pixmap), Some(window)) =
            (self.surface.as_mut(), self.pixmap.as_mut(), self.window.as_ref())
        else {
            return;
        };

        if !self.painted_once {
            crate::trace!("redraw_first");
        }

        self.painter.paint_placeholder(pixmap, self.dark);

        let mut buffer = surface.buffer_mut().expect("buffer_mut");
        pixmap_to_softbuffer(pixmap, &mut buffer);
        buffer.present().expect("present");

        if !self.painted_once {
            crate::trace!("first_present");
            self.painted_once = true;
        }
        let _ = window;
    }
}
