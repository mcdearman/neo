//! Connects a [`Ui`] to a real window with winit and wgpu.

use std::sync::Arc;
use std::time::{Duration, Instant};

use neo_render::{wgpu, Point, Renderer, Size, SurfaceTarget};
use neo_theme::Scheme;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::window::{ResizeDirection, Window, WindowId};

use crate::app::{App, Decorations};
use crate::core::{CursorIcon, ResizeEdge, WindowRequest};
use crate::event::{Event, Key, KeyEvent, Modifiers, PointerButton};
use crate::runtime::Ui;

/// Errors that stop an application from starting.
#[derive(Debug)]
pub enum Error {
    EventLoop(winit::error::EventLoopError),
    Graphics(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::EventLoop(e) => write!(f, "event loop: {e}"),
            Error::Graphics(e) => write!(f, "graphics: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Opens a window and runs `app` until it closes.
pub fn run<A: App>(app: A) -> Result<(), Error> {
    let event_loop = EventLoop::new().map_err(Error::EventLoop)?;
    let settings = app.window();
    let mut ui = Ui::new(app, settings.size, Scheme::Light);
    let waker = event_loop.create_proxy();
    ui.start(Arc::new(move || {
        let _ = waker.send_event(());
    }));
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        ui.set_clipboard_reader(Box::new(move || clipboard.get_text().ok()));
    }
    let mut shell = Shell {
        ui,
        settings,
        gpu: None,
        error: None,
        modifiers: ModifiersState::empty(),
        pointer: None,
        clipboard: arboard::Clipboard::new().ok(),
        glass: None,
        close: false,
        blur_strength: None,
        blur_attempts: 0,
        hidden: false,
        retry_at: None,
    };
    event_loop.run_app(&mut shell).map_err(Error::EventLoop)?;
    match shell.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct Gpu {
    // Field order matters: the surface must drop before the window.
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    target: SurfaceTarget,
    instance: wgpu::Instance,
    window: Arc<Window>,
}

struct Shell<A: App> {
    ui: Ui<A>,
    settings: crate::app::WindowSettings,
    gpu: Option<Gpu>,
    error: Option<Error>,
    modifiers: ModifiersState,
    pointer: Option<Point>,
    clipboard: Option<arboard::Clipboard>,
    /// Glass state last applied to the window: enabled and corner radius.
    glass: Option<(bool, f32)>,
    close: bool,
    /// Blur strength last applied, and how many frames we have waited for
    /// the platform's blur layers to appear.
    blur_strength: Option<f32>,
    blur_attempts: u32,
    /// The window is fully covered, minimised or on a sleeping display, so
    /// nothing is drawn until it is visible again.
    hidden: bool,
    /// The surface had no frame to give; try again at this time rather than
    /// straight away, which would spin a processor core.
    retry_at: Option<Instant>,
}

fn scheme_of(t: winit::window::Theme) -> Scheme {
    match t {
        winit::window::Theme::Dark => Scheme::Dark,
        winit::window::Theme::Light => Scheme::Light,
    }
}

impl<A: App> Shell<A> {
    fn create(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Error> {
        let s = &self.settings;
        let mut attrs = Window::default_attributes()
            .with_title(self.ui.title())
            .with_inner_size(LogicalSize::new(s.size.w as f64, s.size.h as f64))
            .with_transparent(true)
            .with_decorations(s.decorations == Decorations::System)
            .with_resizable(s.resizable);
        if let Some(m) = s.min_size {
            attrs = attrs.with_min_inner_size(LogicalSize::new(m.w as f64, m.h as f64));
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android"), not(target_os = "ios")))]
        if let Some(id) = &s.app_id {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attrs = WindowAttributesExtWayland::with_name(attrs, id.clone(), id.clone());
            use winit::platform::x11::WindowAttributesExtX11;
            attrs = WindowAttributesExtX11::with_name(attrs, id.clone(), id.clone());
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            if s.decorations == Decorations::Neo {
                // Keep the native shadow and resize edges while Neo draws the chrome.
                attrs = attrs.with_decorations(true).with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true).with_titlebar_buttons_hidden(true);
            }
        }
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| Error::Graphics(e.to_string()))?);
        if let Some(t) = window.theme() {
            self.ui.set_system_scheme(scheme_of(t));
        }

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(event_loop.owned_display_handle())));
        let surface = instance.create_surface(window.clone()).map_err(|e| Error::Graphics(e.to_string()))?;
        let (adapter, device, queue) = pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() })
                .await
                .map_err(|e| Error::Graphics(format!("no GPU adapter: {e}")))?;
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.map_err(|e| Error::Graphics(e.to_string()))?;
            Ok::<_, Error>((adapter, device, queue))
        })?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).or_else(|| caps.formats.first().copied()).ok_or_else(|| Error::Graphics("surface has no formats".into()))?;
        let (alpha_mode, unpremultiply) = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            (wgpu::CompositeAlphaMode::PreMultiplied, false)
        } else if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PostMultiplied) {
            (wgpu::CompositeAlphaMode::PostMultiplied, true)
        } else {
            (caps.alpha_modes[0], false)
        };
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        let scale = window.scale_factor() as f32;
        self.ui.resize(Size::new(size.width as f32 / scale, size.height as f32 / scale));
        self.gpu = Some(Gpu {
            renderer: Renderer::new(device, queue),
            surface,
            config,
            target: SurfaceTarget { format, unpremultiply },
            instance,
            window,
        });
        self.sync_window();
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
        Ok(())
    }

    /// Applies theme-driven window state such as glass blur.
    fn sync_window(&mut self) {
        let Some(gpu) = &self.gpu else { return };
        let glass = (self.ui.theme().glass.enabled, self.ui.window_radius());
        if self.glass != Some(glass) {
            crate::platform::set_blur(&gpu.window, glass.0, glass.1);
            self.glass = Some(glass);
            self.blur_strength = None;
            self.blur_attempts = 0;
        }
        let title = self.ui.title();
        if gpu.window.title() != title {
            gpu.window.set_title(&title);
        }
    }

    /// Applies the theme's blur strength once the platform blur exists.
    fn sync_blur_strength(&mut self) {
        let Some(gpu) = &self.gpu else { return };
        let theme = self.ui.theme();
        if !theme.glass.enabled {
            return;
        }
        let want = theme.glass.blur;
        if self.blur_strength == Some(want) {
            return;
        }
        if crate::platform::set_blur_strength(&gpu.window, want) {
            self.blur_strength = Some(want);
        } else if self.blur_attempts < 60 {
            // The system builds its glass layers after a display pass.
            self.blur_attempts += 1;
            gpu.window.request_redraw();
        }
    }

    /// Whether a redraw request would lead to a frame.
    fn can_draw(&self) -> bool {
        !self.hidden && self.retry_at.is_none()
    }

    fn render(&mut self) {
        if self.hidden {
            return;
        }
        let Some(gpu) = &mut self.gpu else { return };
        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            status @ (wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded) => {
                // Asking again at once would loop at full speed while the window
                // is covered. Wait; `Occluded(false)` also restarts drawing.
                let wait = if matches!(status, wgpu::CurrentSurfaceTexture::Occluded) { Duration::from_secs(1) } else { Duration::from_millis(100) };
                self.retry_at = Some(Instant::now() + wait);
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                gpu.surface.configure(gpu.renderer.device(), &gpu.config);
                gpu.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                match gpu.instance.create_surface(gpu.window.clone()) {
                    Ok(s) => gpu.surface = s,
                    Err(e) => eprintln!("neo: could not recreate surface: {e}"),
                }
                gpu.surface.configure(gpu.renderer.device(), &gpu.config);
                gpu.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("neo: surface validation error");
                return;
            }
        };
        self.retry_at = None;
        let scale = gpu.window.scale_factor() as f32;
        let scene = self.ui.draw(gpu.renderer.text(), Instant::now());
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        gpu.window.pre_present_notify();
        gpu.renderer.render(&scene, &view, gpu.target, gpu.config.width, gpu.config.height, scale);
        gpu.renderer.queue().present(frame);
        // Widgets may copy while laying out (for example Vim yanks).
        if let Some(t) = self.ui.take_clipboard()
            && let Some(c) = &mut self.clipboard {
                let _ = c.set_text(t);
            }
        self.sync_blur_strength();
    }

    fn dispatch(&mut self, event: Event) {
        let Some(gpu) = &mut self.gpu else { return };
        self.ui.event(gpu.renderer.text(), event);
        self.after_input();
    }

    fn after_input(&mut self) {
        let Some(gpu) = &self.gpu else { return };
        for r in self.ui.take_window_requests() {
            match r {
                WindowRequest::Drag => {
                    let _ = gpu.window.drag_window();
                }
                WindowRequest::Resize(edge) => {
                    let _ = gpu.window.drag_resize_window(resize_dir(edge));
                }
                WindowRequest::Minimize => gpu.window.set_minimized(true),
                WindowRequest::ToggleMaximize => gpu.window.set_maximized(!gpu.window.is_maximized()),
                WindowRequest::Close => self.close = true,
            }
        }
        if let Some(t) = self.ui.take_clipboard()
            && let Some(c) = &mut self.clipboard {
                let _ = c.set_text(t);
            }
        gpu.window.set_cursor(cursor_icon(self.ui.cursor()));
        self.sync_window();
        if let Some(gpu) = &self.gpu
            && self.ui.needs_redraw()
            && self.can_draw() {
                gpu.window.request_redraw();
            }
    }

    fn modifiers(&self) -> Modifiers {
        let m = self.modifiers;
        Modifiers { shift: m.shift_key(), ctrl: m.control_key(), alt: m.alt_key(), logo: m.super_key() }
    }

    /// With Neo decorations on Linux and Windows, the window edges resize.
    fn resize_edge(&self, p: Point) -> Option<ResizeEdge> {
        if cfg!(target_os = "macos") || self.settings.decorations != Decorations::Neo || !self.settings.resizable {
            return None;
        }
        let gpu = self.gpu.as_ref()?;
        if gpu.window.is_maximized() {
            return None;
        }
        let scale = gpu.window.scale_factor() as f32;
        let (w, h) = (gpu.config.width as f32 / scale, gpu.config.height as f32 / scale);
        let m = 6.0;
        let (l, r, t, b) = (p.x < m, p.x > w - m, p.y < m, p.y > h - m);
        Some(match (l, r, t, b) {
            (true, _, true, _) => ResizeEdge::NorthWest,
            (_, true, true, _) => ResizeEdge::NorthEast,
            (true, _, _, true) => ResizeEdge::SouthWest,
            (_, true, _, true) => ResizeEdge::SouthEast,
            (true, ..) => ResizeEdge::West,
            (_, true, ..) => ResizeEdge::East,
            (_, _, true, _) => ResizeEdge::North,
            (_, _, _, true) => ResizeEdge::South,
            _ => return None,
        })
    }
}

fn resize_dir(e: ResizeEdge) -> ResizeDirection {
    match e {
        ResizeEdge::North => ResizeDirection::North,
        ResizeEdge::South => ResizeDirection::South,
        ResizeEdge::East => ResizeDirection::East,
        ResizeEdge::West => ResizeDirection::West,
        ResizeEdge::NorthEast => ResizeDirection::NorthEast,
        ResizeEdge::NorthWest => ResizeDirection::NorthWest,
        ResizeEdge::SouthEast => ResizeDirection::SouthEast,
        ResizeEdge::SouthWest => ResizeDirection::SouthWest,
    }
}

fn cursor_icon(c: CursorIcon) -> winit::window::CursorIcon {
    use winit::window::CursorIcon as W;
    match c {
        CursorIcon::Default => W::Default,
        CursorIcon::Pointer => W::Pointer,
        CursorIcon::Text => W::Text,
        CursorIcon::Grab => W::Grab,
        CursorIcon::Grabbing => W::Grabbing,
        CursorIcon::Resize(e) => match e {
            ResizeEdge::North | ResizeEdge::South => W::NsResize,
            ResizeEdge::East | ResizeEdge::West => W::EwResize,
            ResizeEdge::NorthEast | ResizeEdge::SouthWest => W::NeswResize,
            ResizeEdge::NorthWest | ResizeEdge::SouthEast => W::NwseResize,
        },
    }
}

fn map_key(k: &WKey) -> Key {
    match k {
        WKey::Named(n) => match n {
            NamedKey::Enter => Key::Enter,
            NamedKey::Space => Key::Space,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            _ => Key::Other,
        },
        WKey::Character(c) => {
            if c.as_str() == " " { Key::Space } else { Key::Character(c.to_lowercase()) }
        }
        _ => Key::Other,
    }
}

impl<A: App> ApplicationHandler for Shell<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        if let Err(e) = self.create(event_loop) {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gpu) = &mut self.gpu else { return };
        let scale = gpu.window.scale_factor() as f32;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                gpu.config.width = size.width.max(1);
                gpu.config.height = size.height.max(1);
                gpu.surface.configure(gpu.renderer.device(), &gpu.config);
                self.ui.resize(Size::new(size.width as f32 / scale, size.height as f32 / scale));
                self.ui.set_maximized(gpu.window.is_maximized());
                gpu.window.request_redraw();
                self.blur_strength = None;
                self.blur_attempts = 0;
                self.sync_window();
            }
            WindowEvent::ScaleFactorChanged { .. } => gpu.window.request_redraw(),
            WindowEvent::Occluded(hidden) => {
                self.hidden = hidden;
                if !hidden {
                    self.retry_at = None;
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::ThemeChanged(t) => {
                self.blur_strength = None;
                self.blur_attempts = 0;
                self.ui.set_system_scheme(scheme_of(t));
                self.after_input();
            }
            WindowEvent::Focused(f) => {
                self.blur_strength = None;
                self.blur_attempts = 0;
                self.dispatch(Event::WindowFocus(f));
            }
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                let mods = self.modifiers();
                self.ui.set_modifiers(mods);
            }
            WindowEvent::CursorMoved { position: PhysicalPosition { x, y }, .. } => {
                let p = Point::new(x as f32 / scale, y as f32 / scale);
                self.pointer = Some(p);
                self.dispatch(Event::PointerMoved { pos: p });
                if let (Some(edge), Some(gpu)) = (self.resize_edge(p), &self.gpu) {
                    gpu.window.set_cursor(cursor_icon(CursorIcon::Resize(edge)));
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer = None;
                self.dispatch(Event::PointerLeft);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(pos) = self.pointer else { return };
                let button = match button {
                    MouseButton::Left => PointerButton::Primary,
                    MouseButton::Right => PointerButton::Secondary,
                    MouseButton::Middle => PointerButton::Middle,
                    MouseButton::Back => PointerButton::Other(3),
                    MouseButton::Forward => PointerButton::Other(4),
                    MouseButton::Other(n) => PointerButton::Other(n),
                };
                if state == ElementState::Pressed && button == PointerButton::Primary
                    && let (Some(edge), Some(gpu)) = (self.resize_edge(pos), &self.gpu) {
                        let _ = gpu.window.drag_resize_window(resize_dir(edge));
                        return;
                    }
                self.dispatch(match state {
                    ElementState::Pressed => Event::PointerPressed { pos, button },
                    ElementState::Released => Event::PointerReleased { pos, button },
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(pos) = self.pointer else { return };
                let d = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Point::new(-x * 48.0, -y * 48.0),
                    MouseScrollDelta::PixelDelta(p) => Point::new(-p.x as f32 / scale, -p.y as f32 / scale),
                };
                self.dispatch(Event::Wheel { pos, delta: d });
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let modifiers = self.modifiers();
                let key = map_key(&event.logical_key);
                if modifiers.command() && key == Key::Character("v".into()) {
                    let text = self.clipboard.as_mut().and_then(|c| c.get_text().ok());
                    self.ui.set_clipboard(text);
                }
                self.dispatch(Event::Key(KeyEvent {
                    key,
                    pressed: event.state == ElementState::Pressed,
                    repeat: event.repeat,
                    modifiers,
                    text: event.text.map(|t| t.to_string()),
                }));
            }
            WindowEvent::Ime(winit::event::Ime::Commit(t)) => self.dispatch(Event::Ime(t)),
            _ => {}
        }
        if self.close {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut wake = self.ui.tick(now);
        if self.retry_at.is_some_and(|t| t <= now) {
            self.retry_at = None;
        }
        if let Some(gpu) = &self.gpu
            && self.ui.needs_redraw()
            && self.can_draw() {
                gpu.window.request_redraw();
            }
        if let Some(t) = self.retry_at.filter(|_| !self.hidden) {
            wake = Some(wake.map_or(t, |w| w.min(t)));
        }
        self.sync_window();
        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}
