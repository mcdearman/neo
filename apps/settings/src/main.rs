//! Settings: appearance, accessibility and information about this computer.
//!
//! Changes are saved to the shared appearance file straight away, and every
//! running Neo app restyles itself to match.
//!
//!     cargo run -p neo-settings
//!     cargo run -p neo-settings -- --snapshot target/snapshots

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use neo::prelude::*;
use neo::Size;
use neo_desktop::fs::human_bytes_binary;
use neo_desktop::ui::{nav_item, notice, section, setting, split};
use neo_desktop::{Appearance, Desktop, SchemePref};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Appearance,
    Accessibility,
    About,
}

impl Page {
    const ALL: [Page; 3] = [Page::Appearance, Page::Accessibility, Page::About];

    fn name(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Accessibility => "Accessibility",
            Page::About => "About",
        }
    }

    fn icon(self) -> neo::theme::Icon {
        match self {
            Page::Appearance => icons::PALETTE,
            Page::Accessibility => icons::ACCESSIBILITY,
            Page::About => icons::LAPTOP,
        }
    }
}

/// Facts about this computer, gathered once at startup.
struct About {
    host: String,
    os: String,
    kernel: String,
    cpu: String,
    cores: usize,
    memory: u64,
    uptime: u64,
}

impl About {
    fn gather() -> Self {
        use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
        let sys = System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing()).with_memory(MemoryRefreshKind::nothing().with_ram()));
        let unknown = || "Unknown".to_string();
        Self {
            host: System::host_name().unwrap_or_else(unknown),
            os: System::long_os_version().unwrap_or_else(unknown),
            kernel: System::kernel_version().unwrap_or_else(unknown),
            cpu: sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty()).unwrap_or_else(unknown),
            cores: sys.cpus().len(),
            memory: sys.total_memory(),
            uptime: System::uptime(),
        }
    }
}

struct Settings {
    desktop: Desktop,
    page: Page,
    about: About,
    error: Option<String>,
}

#[derive(Clone, Debug)]
enum Msg {
    Page(Page),
    Scheme(SchemePref),
    Accent(Accent),
    Radius(f32),
    Glass(bool),
    GlassOpacity(f32),
    GlassBlur(f32),
    TextScale(f32),
    ReduceMotion(bool),
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
    /// The preview controls do nothing.
    Preview,
}

const TEXT_SCALES: [(f32, &str); 3] = [(1.0, "Default"), (1.15, "Large"), (1.3, "Larger")];

impl Settings {
    fn new() -> Self {
        Self { desktop: Desktop::load(), page: Page::Appearance, about: About::gather(), error: None }
    }

    fn change(&mut self, f: impl FnOnce(&mut Appearance)) {
        let mut a = self.desktop.appearance;
        f(&mut a);
        self.error = self.desktop.save(a).err().map(|e| format!("Could not save {}: {e}", Appearance::path().display()));
    }
}

impl App for Settings {
    type Message = Msg;

    fn title(&self) -> String {
        "Settings".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(920.0, 640.0), min_size: Some(Size::new(720.0, 480.0)), app_id: Some("org.neo.Settings".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll)]
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Page(p) => self.page = p,
            Msg::Scheme(s) => self.change(|a| a.scheme = s),
            Msg::Accent(c) => self.change(|a| a.accent = c),
            Msg::Radius(r) => self.change(|a| a.radius = r),
            Msg::Glass(g) => self.change(|a| a.glass.enabled = g),
            Msg::GlassOpacity(o) => self.change(|a| a.glass.opacity = o),
            Msg::GlassBlur(b) => self.change(|a| a.glass.blur = b),
            Msg::TextScale(s) => self.change(|a| a.text_scale = s),
            Msg::ReduceMotion(r) => self.change(|a| a.reduce_motion = r),
            Msg::Preview => {}
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "This Window", Msg::Desktop, vec![])
    }
}

impl Settings {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        let mut side = column().spacing(2.0).width(Length::Fill).push(section("Settings"));
        for p in Page::ALL {
            side = side.push(nav_item(p.icon(), p.name(), p == self.page, Msg::Page(p)));
        }
        let body = match self.page {
            Page::Appearance => self.appearance(),
            Page::Accessibility => self.accessibility(),
            Page::About => self.about(),
        };
        let mut main = column().spacing(20.0).width(Length::Fill).push(text(self.page.name()).role(TextRole::Heading)).push(body);
        if let Some(e) = &self.error {
            main = main.push(notice(Tone::Bad, e.clone()));
        }
        split(side, scrollable(container(main).padding([30.0, 26.0]).max_width(720.0)))
    }
}

fn group<M: 'static>(rows: Vec<Element<M>>) -> Element<M> {
    let mut col = column().spacing(16.0).width(Length::Fill);
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        col = col.push(r);
        if i + 1 < n {
            col = col.push(Divider::horizontal());
        }
    }
    container(col).surface(Surface::Well).padding([20.0, 18.0]).width(Length::Fill).into()
}

impl Settings {
    fn appearance(&self) -> Element<Msg> {
        let a = &self.desktop.appearance;
        let scheme_idx = SchemePref::ALL.iter().position(|s| *s == a.scheme);
        let swatches = Accent::PRESETS.iter().fold(row().spacing(8.0), |r, c| {
            r.push(
                Button::new(container(Space::new(18.0, 18.0)).background(Background::Color(c.swatch())).radius(9.0))
                    .round()
                    .padding(7.0)
                    .selected(*c == a.accent)
                    .on_press(Msg::Accent(*c)),
            )
        });
        let preview = row()
            .spacing(12.0)
            .align(Align::Center)
            .push(button("Cancel").on_press(Msg::Preview))
            .push(Button::new(text("Save changes").role(TextRole::Strong)).kind(ButtonKind::Accent).on_press(Msg::Preview))
            .push(toggle(true, |_| Msg::Preview))
            .push(container(progress_bar(0.62)).width(120.0));
        let mut glass = vec![setting("Glass windows", "Makes windows translucent and blurs what is behind them.", toggle(a.glass.enabled, Msg::Glass))];
        if a.glass.enabled {
            glass.push(setting("Opacity", &format!("{:.0}%. Lower shows more of the desktop.", a.glass.opacity * 100.0), container(slider(0.3..=0.95, a.glass.opacity, Msg::GlassOpacity).step(0.05)).width(220.0)));
            glass.push(setting("Blur", &format!("{:.0} px.", a.glass.blur), container(slider(0.0..=40.0, a.glass.blur, Msg::GlassBlur).step(1.0)).width(220.0)));
        }
        column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(group(vec![
                setting("Colour scheme", "Auto follows your system's light or dark setting.", segmented(["Auto", "Light", "Dark"], scheme_idx, |i| Msg::Scheme(SchemePref::ALL[i]))),
                setting("Accent", a.accent.name(), swatches),
                setting("Corner radius", &format!("{} px. Applies to windows and controls.", a.radius), container(slider(4.0..=28.0, a.radius, Msg::Radius).step(1.0)).width(220.0)),
            ]))
            .push(group(glass))
            .push(section("Preview"))
            .push(container(preview).surface(Surface::Well).padding(18.0).width(Length::Fill))
            .into()
    }

    fn accessibility(&self) -> Element<Msg> {
        let a = &self.desktop.appearance;
        let scale_idx = TEXT_SCALES.iter().position(|(s, _)| (s - a.text_scale).abs() < 0.01);
        column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(group(vec![
                setting("Text size", "Scales text in every Neo app.", segmented(TEXT_SCALES.map(|(_, n)| n), scale_idx, |i| Msg::TextScale(TEXT_SCALES[i].0))),
                setting("Reduce motion", "Turns off transitions and animation.", toggle(a.reduce_motion, Msg::ReduceMotion)),
            ]))
            .push(text("Text and controls meet WCAG AA contrast in both colour schemes with the royal blue accent.").role(TextRole::Caption).tone(Tone::Muted))
            .into()
    }

    fn about(&self) -> Element<Msg> {
        let a = &self.about;
        let fact = |k: &str, v: String| -> Element<Msg> {
            row().spacing(16.0).width(Length::Fill).push(text(k).tone(Tone::Muted).width(140.0)).push(text(v).role(TextRole::Strong).width(Length::Fill)).into()
        };
        let up = a.uptime;
        let uptime = match (up / 86_400, up / 3600 % 24, up / 60 % 60) {
            (0, 0, m) => format!("{m} min"),
            (0, h, m) => format!("{h} h {m} min"),
            (d, h, _) => format!("{d} d {h} h"),
        };
        let badge = row()
            .spacing(16.0)
            .align(Align::Center)
            .push(container(icon(icons::SPARKLES).size(30.0).tone(Tone::Accent)).surface(Surface::Pressed).padding(16.0).radius(18.0))
            .push(column().spacing(2.0).push(text("Neo").role(TextRole::Heading)).push(text(format!("Desktop {}", env!("CARGO_PKG_VERSION"))).tone(Tone::Muted)));
        column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(badge)
            .push(group(vec![
                fact("Device name", a.host.clone()),
                fact("Operating system", a.os.clone()),
                fact("Kernel", a.kernel.clone()),
                fact("Processor", format!("{} × {}", a.cores, a.cpu)),
                fact("Memory", human_bytes_binary(a.memory)),
                fact("Uptime", uptime),
            ]))
            .push(text(format!("Settings file: {}", Appearance::path().display())).role(TextRole::Caption).tone(Tone::Muted))
            .into()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    if let Err(e) = neo::run(Settings::new()) {
        eprintln!("neo-settings: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: std::path::PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, page, scheme) in [("settings-appearance", Page::Appearance, SchemePref::Light), ("settings-about", Page::About, SchemePref::Dark)] {
        let mut app = Settings::new();
        app.page = page;
        app.desktop.appearance = Appearance { scheme, ..Appearance::default() };
        let mut h = Harness::new(app, Size::new(920.0, 640.0)).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_reach_other_apps() {
        let dir = std::env::temp_dir().join(format!("neo-settings-{}", std::process::id()));
        // SAFETY: this crate's only test sets the variable before reading it.
        unsafe { std::env::set_var("NEO_CONFIG_DIR", &dir) };
        let mut settings = Settings::new();
        let mut other = Desktop::load();
        settings.update(Msg::Accent(Accent::Teal));
        settings.update(Msg::Scheme(SchemePref::Dark));
        assert!(settings.error.is_none(), "{:?}", settings.error);
        assert!(other.poll(), "another app sees the change");
        assert_eq!(other.appearance.accent, Accent::Teal);
        assert_eq!(other.theme(Scheme::Light).scheme, Scheme::Dark);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
