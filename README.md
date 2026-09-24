# Neo

A cross-platform GUI toolkit in Rust, and the foundation for the Neo Linux desktop.

Every widget paints through a theme, so one codebase renders in two styles:

- **Flat** (default): borders and fills with higher contrast.
- **Soft**: neumorphic surfaces that are extruded from, or pressed into, the background.

Both styles come in light and dark schemes with four accents: royal blue (default), teal, coral and amber. Users can adjust shadow depth, corner radius, text size and motion. **Glass** windows are translucent, and the compositor blurs what is behind them.

## Try it

```sh
cargo run -p neo --example gallery                  # live widget gallery
cargo run -p neo --example gallery -- --glass --blur 24   # start with glass windows on
cargo run -p neo --example gallery -- --snapshot target/snapshots   # PNGs of every style
cargo run -p neo-render --example primitives        # renderer test card
cargo test --workspace
```

The gallery's Settings card changes the theme live.

## Writing an app

Neo uses the Elm architecture. State lives in your type, `update` applies messages, and `view` describes the interface.

```rust
use neo::prelude::*;

#[derive(Default)]
struct Counter { n: i32 }

#[derive(Clone)]
enum Msg { Inc, Dec }

impl App for Counter {
    type Message = Msg;
    fn update(&mut self, m: Msg) {
        match m { Msg::Inc => self.n += 1, Msg::Dec => self.n -= 1 }
    }
    fn view(&self) -> Element<Msg> {
        row().spacing(12.0)
            .push(button("−").on_press(Msg::Dec))
            .push(text(self.n.to_string()).role(TextRole::Heading))
            .push(button("+").on_press(Msg::Inc))
            .into()
    }
}

fn main() -> Result<(), neo::Error> { neo::run(Counter::default()) }
```

## Crates

| Crate | Role |
|---|---|
| `neo-theme` | Design tokens: palettes, the Flat and Soft materials, type scale, bundled fonts and 1,539 Lucide icons. No dependencies. |
| `neo-render` | wgpu renderer. Every shape is one instanced SDF quad: rounded rects, borders, Gaussian drop and inner shadows, arcs, lines and area fills. Includes dual-Kawase backdrop blur, glyphon text and PNG readback. |
| `neo` | Widgets, layout, events, focus, animation, the winit shell and a headless test harness. |

### Widgets

Text, Icon, Container, Row, Column, Stack, Space, Divider, Scrollable, Button (raised, accent, ghost, round), Toggle, Checkbox, Slider, Segmented, TextInput, ProgressBar, Gauge and Sparkline.

### How it works

- **Surfaces, not colours.** Widgets ask the theme to paint a role such as card, raised, pressed, well, inset or accent. Flat or Soft is decided in one place, in `Theme::paint`.
- **Pressed means active.** A raised surface can be pressed. A sunken or pressed surface is on, selected or being pressed. Transitions cross-fade the outer and inner shadows.
- **Widget state survives rebuilds.** Hover, focus, caret and animation state is keyed by tree position, or by an explicit `.key()`.
- **Browser-style blending.** The canvas is sRGB-encoded and blends in sRGB space, so it matches the HTML mock-ups.
- **Accessibility.** Keyboard focus with Tab and Shift+Tab, visible focus rings, contrast tested against WCAG AA, larger text and reduced motion.

## Platform support

| | Rendering | Glass blur | Title bar |
|---|---|---|---|
| Linux, Wayland | Vulkan / GL | KDE blur protocol over the whole window for now; translucent elsewhere | Neo, with edge resizing |
| Linux, X11 | Vulkan / GL | Translucent, no blur | Neo, with edge resizing |
| macOS | Metal | Clear Liquid Glass on macOS 26+, HUD blur before that; both clipped to Neo's corner radius | Neo, with native shadow and resizing |
| Windows 10/11 | DX12 / Vulkan | Acrylic (Windows 11 rounds it with its own 8px corner) | Neo, with edge resizing |

Blur strength is adjustable on macOS and for in-window glass everywhere. Windows acrylic and KDE's blur have no strength setting. On macOS, Neo sets the radius on the system glass's backdrop filter. That key is undocumented, so if a macOS update renames it, the slider stops affecting the window blur and the system default applies.

Tested so far: macOS, both running and with the test suite. Linux and Windows type-check (`cargo check --target …`) but have not been run yet.

## Roadmap

1. Toolkit gaps: overlays (menus, popovers, tooltips, dialogs), multi-line text editing, IME, AccessKit screen-reader support, lists with virtualised rows, images and async tasks.
2. The Neo app suite built on the toolkit: files, music, calculator, system monitor and settings.
3. The desktop: a Wayland compositor (smithay) that renders with `neo-render`, so glass blur, window shadows and rounded corners work the same for every app.
