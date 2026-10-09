//! A game in a Neo window: an app that draws its own picture on the
//! window's graphics device and shows it in a viewport among Neo's widgets.
//!
//! Needs a GPU adapter.

use std::time::{Duration, Instant};

use neo::prelude::*;
use neo::testing::Harness;
use neo::{wgpu, Graphics, Image, Point, PointerButton, Rect, Size};

#[derive(Clone, Debug)]
enum Msg {
    Resized(Rect, f32),
    Input(ViewportEvent),
    Play,
}

#[derive(Default)]
struct Editor {
    graphics: Option<Graphics>,
    target: Option<(wgpu::Texture, Image)>,
    size: (u32, u32),
    playing: bool,
    heard: Vec<ViewportEvent>,
    steps: u32,
}

impl App for Editor {
    type Message = Msg;

    fn wanted_limits(&self, available: &wgpu::Limits) -> wgpu::Limits {
        // As much room for one buffer as this computer allows.
        wgpu::Limits { max_buffer_size: available.max_buffer_size, ..wgpu::Limits::default() }
    }

    fn graphics(&mut self, graphics: &Graphics) {
        self.graphics = Some(graphics.clone());
    }

    fn step(&mut self, _now: Instant, _dt: Duration) -> bool {
        self.steps += 1;
        let (Some(g), (w, h)) = (&self.graphics, self.size) else { return false };
        if w == 0 || h == 0 {
            return false;
        }
        if self.target.as_ref().is_none_or(|(t, _)| (t.width(), t.height()) != (w, h)) {
            let texture = g.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("the game's frame"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
            });
            let shown = texture.create_view(&wgpu::TextureViewDescriptor { format: Some(wgpu::TextureFormat::Rgba8Unorm), ..Default::default() });
            self.target = Some((texture, Image::from_texture(shown, w, h)));
        }
        let (texture, _) = self.target.as_ref().expect("made above");
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = g.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("the game draws"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 }), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        g.queue.submit(Some(encoder.finish()));
        true
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Resized(bounds, scale) => self.size = ((bounds.w * scale).round() as u32, (bounds.h * scale).round() as u32),
            Msg::Input(e) => self.heard.push(e),
            Msg::Play => self.playing = !self.playing,
        }
    }

    fn view(&self) -> Element<Msg> {
        // A bar of Neo's own controls above, and the game below it.
        column()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(container(button(if self.playing { "Stop" } else { "Play" }).on_press(Msg::Play)).padding(8.0))
            .push(Element::new(viewport(self.target.as_ref().map(|(_, image)| image)).on_resize(Msg::Resized).on_input(Msg::Input).playing(self.playing)))
            .into()
    }
}

#[test]
fn a_game_is_shown_among_neos_own_controls_and_hears_what_happens_in_it() {
    let mut h = Harness::new(Editor::default(), Size::new(400.0, 300.0)).expect("a GPU adapter is required for these tests");
    let g = h.app().graphics.clone().expect("the window's device, before the first view");
    assert!(g.limits.max_buffer_size >= wgpu::Limits::default().max_buffer_size, "made with the limits asked for");
    let tick = Duration::from_millis(16);
    h.frame(Duration::ZERO, 1.0);
    let px = h.frame(tick, 1.0);
    let at = |px: &[u8], x: usize, y: usize| [px[(y * 400 + x) * 4], px[(y * 400 + x) * 4 + 1], px[(y * 400 + x) * 4 + 2]];
    assert_eq!(at(&px, 200, 250), [255, 0, 255], "the game's picture, under the bar");
    assert_ne!(at(&px, 380, 8), [255, 0, 255], "and the bar is Neo's");
    let (w, tall) = h.app().size;
    assert!(w == 400 && (200..290).contains(&tall), "the viewport has what the bar leaves: {w} by {tall}");

    // A click in the game is the game's, where it fell in the game; one on the bar is the button's.
    h.click(Point::new(200.0, 250.0));
    let below_bar = 300.0 - tall as f32;
    assert_eq!(h.app().heard[..2], [ViewportEvent::Focused(true), ViewportEvent::Pressed(Point::new(200.0, 250.0 - below_bar), PointerButton::Primary)]);
    // A click on the bar above is not the game's, and the keyboard goes with it.
    h.click(Point::new(390.0, below_bar - 4.0));
    assert_eq!((h.app().heard.len(), h.app().heard.last()), (5, Some(&ViewportEvent::Focused(false))), "{:?}", h.app().heard);
    // Not playing, it is drawn when something changes and then left.
    h.frame(tick, 1.0);
    h.frame(tick, 1.0);
    assert!(!h.wants_frame());
    h.app_mut().update(Msg::Play);
    // Playing, it is stepped and drawn frame after frame.
    let before = h.app().steps;
    h.frame(tick, 1.0);
    h.frame(tick, 1.0);
    assert!(h.wants_frame() && h.app().steps == before + 2);
}
