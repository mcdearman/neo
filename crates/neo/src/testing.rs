//! Drive an app without a window: send input, inspect state, save screenshots.

use std::path::Path;
use std::time::{Duration, Instant};

use neo_render::{Point, Renderer, Size};
use neo_theme::Scheme;

use crate::app::App;
use crate::event::{Event, Key, KeyEvent, Modifiers, PointerButton};
use crate::runtime::Ui;

/// Runs an [`App`] headlessly on the GPU.
pub struct Harness<A: App> {
    ui: Ui<A>,
    renderer: Renderer,
    size: Size,
    clock: Instant,
}

impl<A: App> Harness<A> {
    /// Creates a harness with a window of `size` logical pixels.
    pub fn new(app: A, size: Size) -> Result<Self, String> {
        let renderer = Renderer::headless()?;
        Ok(Self { ui: Ui::new(app, size, Scheme::Light), renderer, size, clock: Instant::now() })
    }

    pub fn app(&self) -> &A {
        self.ui.app()
    }

    pub fn app_mut(&mut self) -> &mut A {
        self.ui.app_mut()
    }

    pub fn set_system_scheme(&mut self, s: Scheme) {
        self.ui.set_system_scheme(s);
    }

    pub fn resize(&mut self, size: Size) {
        self.size = size;
        self.ui.resize(size);
    }

    pub fn event(&mut self, e: Event) {
        self.ui.event(self.renderer.text(), e);
    }

    pub fn move_to(&mut self, p: Point) {
        self.event(Event::PointerMoved { pos: p });
    }

    /// Moves to `p`, then presses and releases the primary button.
    pub fn click(&mut self, p: Point) {
        self.move_to(p);
        self.event(Event::PointerPressed { pos: p, button: PointerButton::Primary });
        self.event(Event::PointerReleased { pos: p, button: PointerButton::Primary });
    }

    pub fn key(&mut self, key: Key, modifiers: Modifiers) {
        let text = match &key {
            Key::Character(c) if !modifiers.command() => Some(c.clone()),
            Key::Space => Some(" ".into()),
            _ => None,
        };
        for pressed in [true, false] {
            self.event(Event::Key(KeyEvent { key: key.clone(), pressed, repeat: false, modifiers, text: text.clone() }));
        }
    }

    /// Types `text` one character at a time.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let key = if c == ' ' { Key::Space } else { Key::Character(c.to_string()) };
            self.event(Event::Key(KeyEvent { key, pressed: true, repeat: false, modifiers: Modifiers::default(), text: Some(c.to_string()) }));
        }
    }

    /// Advances the clock, firing timers and finishing animations.
    pub fn advance(&mut self, d: Duration) {
        self.clock += d;
        self.ui.tick(self.clock);
        let _ = self.ui.draw(self.renderer.text(), self.clock);
    }

    /// Renders the current state and returns straight-alpha RGBA pixels.
    pub fn render(&mut self, scale: f32) -> Vec<u8> {
        // Two frames so that state transitions settle at their resting values.
        let _ = self.ui.draw(self.renderer.text(), self.clock);
        self.clock += Duration::from_secs(1);
        let scene = self.ui.draw(self.renderer.text(), self.clock);
        let (w, h) = ((self.size.w * scale) as u32, (self.size.h * scale) as u32);
        self.renderer.render_to_rgba(&scene, w, h, scale)
    }

    /// Renders the current state to a PNG file.
    pub fn save_png(&mut self, path: impl AsRef<Path>, scale: f32) -> std::io::Result<()> {
        let _ = self.ui.draw(self.renderer.text(), self.clock);
        self.clock += Duration::from_secs(1);
        let scene = self.ui.draw(self.renderer.text(), self.clock);
        let (w, h) = ((self.size.w * scale) as u32, (self.size.h * scale) as u32);
        self.renderer.save_png(&scene, w, h, scale, path)
    }
}
