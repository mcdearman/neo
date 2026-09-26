use std::time::{Duration, Instant};

use neo_render::{Point, Rect, Scene, Size, TextSystem};
use neo_theme::{Color, Scheme, Theme};

use crate::app::{App, Decorations};
use crate::core::{CursorIcon, Cx, DrawCx, Element, EventCx, IdPass, Limits, Node, RuntimeState, Shared, StateStore, WidgetId, WindowRequest};
use crate::event::{Event, Key, Status};
use crate::widgets::frame::Frame;

/// Runs an [`App`] without a window: builds the view, lays it out, routes
/// events and draws scenes. The windowing shell and the test harness both
/// drive this.
pub struct Ui<A: App> {
    app: A,
    root: Option<Element<A::Message>>,
    states: StateStore,
    rt: RuntimeState,
    size: Size,
    system_scheme: Scheme,
    theme: Theme,
    decorations: Decorations,
    maximized: bool,
    needs_view: bool,
    needs_layout: bool,
    timers: Vec<(Duration, Instant)>,
}

impl<A: App> Ui<A> {
    pub fn new(app: A, size: Size, system_scheme: Scheme) -> Self {
        let decorations = app.window().decorations;
        let theme = app.theme(system_scheme);
        Self {
            app,
            root: None,
            states: StateStore::default(),
            rt: RuntimeState { window_focused: true, ..Default::default() },
            size,
            system_scheme,
            theme,
            decorations,
            maximized: false,
            needs_view: true,
            needs_layout: true,
            timers: vec![],
        }
    }

    pub fn app(&self) -> &A {
        &self.app
    }

    /// Mutable access to the application. The view is rebuilt afterwards.
    pub fn app_mut(&mut self) -> &mut A {
        self.needs_view = true;
        &mut self.app
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Corner radius of the window as Neo draws it: zero with system
    /// decorations or when maximized.
    pub fn window_radius(&self) -> f32 {
        if self.decorations == Decorations::Neo && !self.maximized { self.theme.window_radius() } else { 0.0 }
    }

    pub fn title(&self) -> String {
        self.app.title()
    }

    pub fn resize(&mut self, size: Size) {
        if size != self.size {
            self.size = size;
            self.needs_layout = true;
        }
    }

    pub fn set_system_scheme(&mut self, s: Scheme) {
        self.system_scheme = s;
        self.refresh_theme();
    }

    pub fn set_maximized(&mut self, m: bool) {
        if m != self.maximized {
            self.maximized = m;
            self.needs_view = true;
        }
    }

    pub fn set_window_focused(&mut self, f: bool) {
        self.rt.window_focused = f;
        self.rt.redraw = true;
    }

    /// Modifier keys currently held, as reported by the platform.
    pub fn set_modifiers(&mut self, m: crate::event::Modifiers) {
        self.rt.modifiers = m;
    }

    /// Lets widgets read the system clipboard on demand.
    pub fn set_clipboard_reader(&mut self, reader: Box<dyn FnMut() -> Option<String>>) {
        self.rt.clipboard_reader = Some(reader);
    }

    /// Text to hand to widgets on the next paste shortcut.
    pub fn set_clipboard(&mut self, text: Option<String>) {
        self.rt.clipboard_in = text;
    }

    pub fn take_clipboard(&mut self) -> Option<String> {
        self.rt.clipboard_out.take()
    }

    pub fn take_window_requests(&mut self) -> Vec<WindowRequest> {
        std::mem::take(&mut self.rt.window_requests)
    }

    pub fn cursor(&self) -> CursorIcon {
        self.rt.cursor
    }

    /// Whether anything changed that needs a new frame.
    pub fn needs_redraw(&self) -> bool {
        self.rt.redraw || self.rt.animating || self.needs_view || self.needs_layout
    }

    fn refresh_theme(&mut self) {
        let theme = self.app.theme(self.system_scheme);
        if theme != self.theme {
            self.theme = theme;
            self.needs_layout = true;
            self.rt.redraw = true;
        }
    }

    fn ensure(&mut self, text: &mut TextSystem) {
        self.refresh_theme();
        if self.needs_view {
            let chrome = self.decorations == Decorations::Neo;
            let rounded = chrome && !self.maximized;
            let mut root = Element::new(Frame::new(self.app.view(), self.app.title(), chrome, rounded));
            let mut pass = IdPass::default();
            root.assign_ids(WidgetId(1), 0, &mut pass);
            self.states.sweep(&pass.live);
            self.rt.focus_chain = pass.focus_chain;
            if self.rt.focus.is_some_and(|f| !pass.live.contains(&f)) {
                self.rt.focus = None;
            }
            self.root = Some(root);
            self.needs_view = false;
            self.needs_layout = true;
        }
        if self.needs_layout || self.rt.relayout {
            self.rt.relayout = false;
            let root = self.root.as_mut().expect("view built above");
            let mut shared = Shared { text, theme: self.theme, states: &mut self.states, runtime: &mut self.rt };
            let mut cx = Cx { shared: &mut shared, id: WidgetId(0), bounds: Rect::ZERO };
            root.layout(&mut cx, Limits::tight(self.size));
            root.set_position(Point::ZERO);
            root.place(Point::ZERO);
            self.needs_layout = false;
        }
    }

    /// Rebuilds and lays out the view if anything changed.
    pub fn refresh(&mut self, text: &mut TextSystem) {
        self.ensure(text);
    }

    /// Draws a frame.
    pub fn draw(&mut self, text: &mut TextSystem, now: Instant) -> Scene {
        self.ensure(text);
        self.rt.now = Some(now);
        self.rt.animating = false;
        self.rt.redraw = false;
        self.rt.wake_at = None;
        let mut scene = Scene::new(Color::TRANSPARENT);
        let root = self.root.as_ref().expect("view built in ensure");
        let mut shared = Shared { text, theme: self.theme, states: &mut self.states, runtime: &mut self.rt };
        let mut cx = DrawCx { cx: Cx { shared: &mut shared, id: WidgetId(0), bounds: root.bounds() }, scene: &mut scene };
        root.draw(&mut cx);
        scene
    }

    /// Routes an input event, then applies any resulting messages.
    pub fn event(&mut self, text: &mut TextSystem, event: Event) {
        self.ensure(text);
        if let Event::Key(k) = &event {
            self.rt.modifiers = k.modifiers;
        }
        match &event {
            Event::PointerLeft => self.rt.pointer = None,
            Event::PointerPressed { .. } => self.rt.focus_visible = false,
            Event::WindowFocus(f) => self.set_window_focused(*f),
            e => {
                if let Some(p) = e.pos() {
                    self.rt.pointer = Some(p);
                }
            }
        }
        if matches!(event, Event::PointerMoved { .. }) {
            self.rt.cursor = CursorIcon::Default;
        }
        let mut messages = Vec::new();
        let status = {
            let root = self.root.as_mut().expect("view built in ensure");
            let mut shared = Shared { text, theme: self.theme, states: &mut self.states, runtime: &mut self.rt };
            let mut cx = EventCx { cx: Cx { shared: &mut shared, id: WidgetId(0), bounds: root.bounds() }, messages: &mut messages };
            root.event(&mut cx, &event)
        };
        if let Event::Key(k) = &event
            && status == Status::Ignored
            && k.pressed
        {
            if let Some(m) = self.app.on_key(k) {
                messages.push(m);
            } else if k.key == Key::Tab {
                self.move_focus(!k.modifiers.shift);
            }
        }
        for m in messages {
            self.app.update(m);
            self.needs_view = true;
        }
        if self.rt.relayout {
            self.needs_layout = true;
        }
        self.rt.clipboard_in = None;
    }

    fn move_focus(&mut self, forward: bool) {
        let chain = &self.rt.focus_chain;
        if chain.is_empty() {
            return;
        }
        let pos = self.rt.focus.and_then(|f| chain.iter().position(|&c| c == f));
        let next = match (pos, forward) {
            (None, true) => 0,
            (None, false) => chain.len() - 1,
            (Some(i), true) => (i + 1) % chain.len(),
            (Some(i), false) => (i + chain.len() - 1) % chain.len(),
        };
        self.rt.focus = Some(chain[next]);
        self.rt.focus_visible = true;
        self.rt.redraw = true;
    }

    /// Fires due timers. Returns when the runtime next needs to wake up.
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        let subs = self.app.subscriptions();
        if subs.len() != self.timers.len() || subs.iter().zip(&self.timers).any(|(s, t)| s.period != t.0) {
            self.timers = subs.iter().map(|s| (s.period, now + s.period)).collect();
        }
        for (sub, timer) in subs.into_iter().zip(self.timers.iter_mut()) {
            if now >= timer.1 {
                self.app.update(sub.message);
                self.needs_view = true;
                timer.1 += sub.period;
                if timer.1 < now {
                    timer.1 = now + sub.period;
                }
            }
        }
        if self.rt.wake_at.is_some_and(|w| w <= now) {
            self.rt.wake_at = None;
            self.rt.redraw = true;
        }
        let next_timer = self.timers.iter().map(|t| t.1).min();
        match (next_timer, self.rt.wake_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}
