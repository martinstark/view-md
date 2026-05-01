use std::num::NonZeroU32;
use std::rc::Rc;

use cosmic_text::Cursor;
use softbuffer::{Context, Surface};
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::doc::Doc;
use crate::layout::{LaidDoc, LaidKind, layout};
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
    pub help_visible: bool,
    pub cursor: PhysicalPosition<f64>,
    pub selection: Option<Selection>,
    pub dragging: bool,
    pub modifiers: Modifiers,
    pub dpi_scale: f32,
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
        if a.block_idx < b.block_idx
            || (a.block_idx == b.block_idx && cursor_le(&a.cursor, &b.cursor))
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
            .with_title(format!("{} — mdv", self.title))
            .with_inner_size(LogicalSize::new(920.0, 1100.0));
        let window = Rc::new(event_loop.create_window(attrs).expect("window create"));
        crate::trace!("window_created");

        self.dpi_scale = window.scale_factor() as f32;
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
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                match state {
                    ElementState::Pressed => {
                        if let Some(hit) =
                            self.hit_test(self.cursor.x as f32, self.cursor.y as f32)
                        {
                            self.selection = Some(Selection { anchor: hit, head: hit });
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
    fn request_redraw(&self) {
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: Key) {
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
        let scale = self.zoom * self.dpi_scale.max(1.0);
        let laid = layout(
            &self.doc,
            surface_w,
            &mut self.painter.fs,
            &theme,
            self.full_highlight,
            scale,
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

    fn yank_visible_code(&mut self) {
        let Some(laid) = self.laid.as_ref() else { return };
        let viewport_top = self.scroll_y;
        let viewport_bottom = self.scroll_y + self.viewport_h();
        let viewport_center = (viewport_top + viewport_bottom) / 2.0;
        let mut best: Option<(f32, &str)> = None;
        for block in &laid.blocks {
            if let LaidKind::CodeBlock { source, .. } = &block.kind {
                if block.y + block.h < viewport_top || block.y > viewport_bottom {
                    continue;
                }
                let center = block.y + block.h / 2.0;
                let dist = (center - viewport_center).abs();
                if best.map_or(true, |(d, _)| dist < d) {
                    best = Some((dist, source.as_str()));
                }
            }
        }
        if let Some((_, src)) = best {
            if let Ok(mut clip) = arboard::Clipboard::new() {
                let _ = clip.set_text(src.to_string());
            }
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
                        return Some(HitPoint { block_idx: i, cursor: c });
                    }
                }
                LaidKind::CodeBlock { buffer, pad_x, pad_y, .. } => {
                    let lx = dx - block.x - *pad_x;
                    let ly = dy - block.y - *pad_y;
                    if let Some(c) = buffer.hit(lx, ly) {
                        return Some(HitPoint { block_idx: i, cursor: c });
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn copy_selection(&self) {
        let Some(sel) = self.selection else { return };
        if sel.is_empty() {
            return;
        }
        let Some(laid) = self.laid.as_ref() else { return };
        let (start, end) = sel.ordered();
        let text = collect_selection_text(laid, &start, &end);
        if !text.is_empty() {
            if let Ok(mut clip) = arboard::Clipboard::new() {
                let _ = clip.set_text(text);
            }
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

        if let Some(sel) = self.selection {
            if !sel.is_empty() {
                if let Some(laid) = self.laid.as_ref() {
                    self.painter.paint_selection(pixmap, laid, &sel, &theme, self.scroll_y);
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
    let Some(buf) = block_buffer(block) else { return String::new() };
    let lines = buffer_text_lines(buf);
    substring_between(&lines, from, to)
}

fn block_substring_from(block: &crate::layout::LaidBlock, from: &Cursor) -> String {
    let Some(buf) = block_buffer(block) else { return String::new() };
    let lines = buffer_text_lines(buf);
    let last_line = lines.len().saturating_sub(1);
    let to = Cursor { line: last_line, index: lines.last().map(|l| l.len()).unwrap_or(0), affinity: cosmic_text::Affinity::After };
    substring_between(&lines, from, &to)
}

fn block_substring_to(block: &crate::layout::LaidBlock, to: &Cursor) -> String {
    let Some(buf) = block_buffer(block) else { return String::new() };
    let lines = buffer_text_lines(buf);
    let from = Cursor { line: 0, index: 0, affinity: cosmic_text::Affinity::Before };
    substring_between(&lines, &from, to)
}

fn block_full_text(block: &crate::layout::LaidBlock) -> String {
    let Some(buf) = block_buffer(block) else { return String::new() };
    let lines = buffer_text_lines(buf);
    lines.join("\n")
}
