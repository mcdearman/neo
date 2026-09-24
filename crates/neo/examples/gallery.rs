//! The Neo widget gallery: a music player, calculator, system monitor,
//! settings and a form, all live.
//!
//!     cargo run -p neo --example gallery
//!     cargo run -p neo --example gallery -- --glass --blur 24
//!     cargo run -p neo --example gallery -- --snapshot target/snapshots

use std::time::Duration;

use neo::prelude::*;
use neo::testing::Harness;
use neo::Size;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SchemePref {
    Auto,
    Light,
    Dark,
}

struct Gallery {
    // Appearance
    style: Style,
    scheme: SchemePref,
    accent: Accent,
    depth: f32,
    radius: f32,
    large_text: bool,
    reduce_motion: bool,
    glass: bool,
    glass_opacity: f32,
    glass_blur: f32,
    // Player
    playing: bool,
    liked: bool,
    track: usize,
    pos: f32,
    volume: f32,
    // Calculator
    calc: Calc,
    // Monitor
    cpu: Vec<f32>,
    mem: f32,
    temp: f32,
    seed: u32,
    // Form
    name: String,
    email: String,
    notify: bool,
    updates: bool,
    submitted: Option<String>,
}

const TRACKS: [(&str, &str, f32); 4] = [
    ("Low Tide Letters", "Marin Holt · Harbour Sessions", 243.0),
    ("Paper Lanterns", "The Quiet Engines · Night Shift", 198.0),
    ("Glass Orchard", "Ines Varga · Greenhouse", 276.0),
    ("Slow Ferry", "Marin Holt · Harbour Sessions", 221.0),
];

impl Default for Gallery {
    fn default() -> Self {
        let cpu = (0..40).map(|i| 34.0 + 14.0 * (i as f32 / 4.0).sin() + ((i * 7) % 10) as f32).collect();
        Self {
            style: Style::Flat,
            scheme: SchemePref::Auto,
            accent: Accent::Royal,
            depth: 6.0,
            radius: 18.0,
            large_text: false,
            reduce_motion: false,
            glass: false,
            glass_opacity: 0.72,
            glass_blur: 10.0,
            playing: false,
            liked: false,
            track: 0,
            pos: 82.0,
            volume: 64.0,
            calc: Calc::default(),
            cpu,
            mem: 54.0,
            temp: 52.0,
            seed: 7,
            name: "Robin Ellis".into(),
            email: String::new(),
            notify: true,
            updates: false,
            submitted: None,
        }
    }
}

#[derive(Clone, Debug)]
enum Msg {
    Style(Style),
    Scheme(SchemePref),
    Accent(Accent),
    Depth(f32),
    Radius(f32),
    LargeText(bool),
    ReduceMotion(bool),
    Glass(bool),
    GlassOpacity(f32),
    GlassBlur(f32),
    Play,
    Like,
    Prev,
    Next,
    Seek(f32),
    Volume(f32),
    Tick,
    Key(&'static str),
    Sample,
    Name(String),
    Email(String),
    Notify(bool),
    Updates(bool),
    Submit,
}

impl Gallery {
    fn rand(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed % 10_000) as f32 / 10_000.0
    }
}

impl App for Gallery {
    type Message = Msg;

    fn title(&self) -> String {
        "Neo Gallery".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1240.0, 860.0), ..Default::default() }
    }

    fn theme(&self, system: Scheme) -> Theme {
        Theme {
            style: self.style,
            scheme: match self.scheme {
                SchemePref::Auto => system,
                SchemePref::Light => Scheme::Light,
                SchemePref::Dark => Scheme::Dark,
            },
            accent: self.accent,
            depth: self.depth,
            radius: self.radius,
            text_scale: if self.large_text { 1.18 } else { 1.0 },
            reduce_motion: self.reduce_motion,
            glass: Glass { enabled: self.glass, opacity: self.glass_opacity, blur: self.glass_blur },
        }
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut s = vec![Subscription::every(Duration::from_millis(1600), Msg::Sample)];
        if self.playing {
            s.push(Subscription::every(Duration::from_secs(1), Msg::Tick));
        }
        s
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Style(s) => self.style = s,
            Msg::Scheme(s) => self.scheme = s,
            Msg::Accent(a) => self.accent = a,
            Msg::Depth(d) => self.depth = d,
            Msg::Radius(r) => self.radius = r,
            Msg::LargeText(b) => self.large_text = b,
            Msg::ReduceMotion(b) => self.reduce_motion = b,
            Msg::Glass(b) => self.glass = b,
            Msg::GlassOpacity(o) => self.glass_opacity = o,
            Msg::GlassBlur(b) => self.glass_blur = b,
            Msg::Play => self.playing = !self.playing,
            Msg::Like => self.liked = !self.liked,
            Msg::Prev => {
                if self.pos > 3.0 {
                    self.pos = 0.0
                } else {
                    self.track = (self.track + TRACKS.len() - 1) % TRACKS.len();
                    self.pos = 0.0;
                }
            }
            Msg::Next => {
                self.track = (self.track + 1) % TRACKS.len();
                self.pos = 0.0;
            }
            Msg::Seek(p) => self.pos = p,
            Msg::Volume(v) => self.volume = v,
            Msg::Tick => {
                self.pos += 1.0;
                if self.pos >= TRACKS[self.track].2 {
                    self.update(Msg::Next);
                }
            }
            Msg::Key(k) => self.calc.press(k),
            Msg::Sample => {
                let last = *self.cpu.last().unwrap();
                let next = (last + (self.rand() - 0.5) * 22.0).clamp(4.0, 96.0);
                self.cpu.remove(0);
                self.cpu.push(next);
                self.mem = (self.mem + (self.rand() - 0.5) * 4.0).clamp(30.0, 88.0);
                self.temp = (42.0 + next * 0.35 + self.rand() * 3.0).round();
            }
            Msg::Name(s) => self.name = s,
            Msg::Email(s) => self.email = s,
            Msg::Notify(b) => self.notify = b,
            Msg::Updates(b) => self.updates = b,
            Msg::Submit => {
                self.submitted = Some(if self.email.contains('@') {
                    format!("Saved. We'll write to {}.", self.email)
                } else {
                    "Enter an email address with an @ sign.".into()
                })
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        let top = row()
            .spacing(24.0)
            .width(Length::Fill)
            .push(self.player())
            .push(self.calculator())
            .push(self.monitor());
        let bottom = row().spacing(24.0).width(Length::Fill).push(self.settings()).push(self.form());
        scrollable(column().spacing(24.0).padding([24.0, 26.0, 28.0, 24.0]).width(Length::Fill).push(top).push(bottom)).into()
    }
}

fn card<M: Clone + 'static>(title: &str, glyph: neo::theme::Icon, portion: u16, body: impl Into<Element<M>>) -> Element<M> {
    let header = row()
        .spacing(10.0)
        .align(Align::Center)
        .push(icon(glyph).size(17.0).tone(Tone::Accent))
        .push(text(title).role(TextRole::Title));
    container(column().spacing(18.0).width(Length::Fill).push(header).push(body))
        .surface(Surface::Card)
        .padding(22.0)
        .width(Length::Portion(portion))
        .into()
}

fn setting<M: Clone + 'static>(title: &str, help: &str, control: impl Into<Element<M>>) -> Element<M> {
    row()
        .spacing(16.0)
        .align(Align::Center)
        .width(Length::Fill)
        .push(column().spacing(2.0).width(Length::Fill).push(text(title).role(TextRole::Strong)).push(text(help).role(TextRole::Caption).tone(Tone::Muted)))
        .push(control)
        .into()
}

fn mmss(s: f32) -> String {
    let s = s.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

impl Gallery {
    fn player(&self) -> Element<Msg> {
        let (title, artist, len) = TRACKS[self.track];
        let art = container(gauge(self.pos / len, format!("{}/{}", self.track + 1, TRACKS.len())).diameter(150.0)).width(Length::Fill).align_x(Align::Center);
        let times = row()
            .width(Length::Fill)
            .push(text(mmss(self.pos)).mono().role(TextRole::Caption).tone(Tone::Muted))
            .push(Space::fill_x())
            .push(text(mmss(len)).mono().role(TextRole::Caption).tone(Tone::Muted));
        let controls = row()
            .spacing(14.0)
            .align(Align::Center)
            .push(icon_button(icons::HEART, 44.0).selected(self.liked).on_press(Msg::Like))
            .push(icon_button(icons::SKIP_BACK, 44.0).on_press(Msg::Prev))
            .push(icon_button(if self.playing { icons::PAUSE } else { icons::PLAY }, 64.0).selected(self.playing).on_press(Msg::Play))
            .push(icon_button(icons::SKIP_FORWARD, 44.0).on_press(Msg::Next))
            .push(icon_button(icons::SHUFFLE, 44.0).selected(true).on_press(Msg::Like));
        let body = column()
            .spacing(16.0)
            .width(Length::Fill)
            .push(art)
            .push(column().width(Length::Fill).spacing(2.0).push(text(title).role(TextRole::Title).align(Align::Center).width(Length::Fill)).push(text(artist).tone(Tone::Muted).role(TextRole::Caption).align(Align::Center).width(Length::Fill)))
            .push(slider(0.0..=len, self.pos, Msg::Seek).step(1.0))
            .push(times)
            .push(container(controls).width(Length::Fill).align_x(Align::Center))
            .push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(icon(icons::VOLUME_2).tone(Tone::Muted)).push(slider(0.0..=100.0, self.volume, Msg::Volume)));
        card("Tunes", icons::MUSIC, 4, body)
    }

    fn calculator(&self) -> Element<Msg> {
        let screen = container(
            column()
                .width(Length::Fill)
                .spacing(4.0)
                .push(text(self.calc.expr.clone()).mono().tone(Tone::Muted).role(TextRole::Caption).align(Align::End).width(Length::Fill).no_wrap())
                .push(text(self.calc.display()).mono().role(TextRole::Display).align(Align::End).width(Length::Fill).no_wrap()),
        )
        .surface(Surface::Well)
        .padding([16.0, 14.0])
        .width(Length::Fill);
        let rows: [[&'static str; 4]; 5] = [["C", "±", "%", "÷"], ["7", "8", "9", "×"], ["4", "5", "6", "−"], ["1", "2", "3", "+"], ["0", "", ".", "="]];
        let mut keys = column().spacing(12.0).width(Length::Fill);
        for r in rows {
            let mut line = row().spacing(12.0).width(Length::Fill);
            for k in r {
                if k.is_empty() {
                    continue;
                }
                let tone = match k {
                    "÷" | "×" | "−" | "+" => Tone::Accent,
                    "C" | "±" | "%" => Tone::Muted,
                    _ => Tone::Inherit,
                };
                let mut b = Button::new(text(k).role(TextRole::Title).tone(tone)).on_press(Msg::Key(k)).width(Length::Portion(if k == "0" { 2 } else { 1 })).height(48.0).padding(0.0);
                if k == "=" {
                    b = Button::new(text(k).role(TextRole::Title)).kind(ButtonKind::Accent).on_press(Msg::Key(k)).width(Length::Fill).height(48.0).padding(0.0);
                }
                line = line.push(container(b).width(Length::Portion(if k == "0" { 2 } else { 1 })).align_x(Align::Stretch));
            }
            keys = keys.push(line);
        }
        card("Calculator", icons::CALCULATOR, 3, column().spacing(18.0).width(Length::Fill).push(screen).push(keys))
    }

    fn monitor(&self) -> Element<Msg> {
        let cpu = *self.cpu.last().unwrap();
        let g = |v: f32, label: String, cap: &str| -> Element<Msg> {
            column()
                .spacing(8.0)
                .align(Align::Center)
                .width(Length::Fill)
                .push(gauge(v, label).diameter(112.0))
                .push(text(cap).role(TextRole::Caption).tone(Tone::Muted))
                .into()
        };
        let gauges = row()
            .width(Length::Fill)
            .push(g(cpu / 100.0, format!("{}%", cpu.round()), "CPU · 8 cores"))
            .push(g(self.mem / 100.0, format!("{}%", self.mem.round()), &format!("{:.1} / 16 GiB", self.mem / 100.0 * 16.0)))
            .push(g(self.temp / 100.0, format!("{}°", self.temp), "Package temp"));
        let chart = container(
            column()
                .spacing(8.0)
                .width(Length::Fill)
                .push(row().width(Length::Fill).push(text("CPU history · 60 s").role(TextRole::Label).tone(Tone::Muted)).push(Space::fill_x()).push(text(format!("load {:.2}", cpu / 100.0 * 8.0 * 0.45)).mono().role(TextRole::Caption).tone(Tone::Muted)))
                .push(sparkline(self.cpu.clone(), 0.0, 100.0).height(84.0)),
        )
        .surface(Surface::Well)
        .padding([14.0, 16.0])
        .width(Length::Fill);
        let procs = [("firefox", 14.2, 1.9), ("neo-compositor", 6.8, 0.4), ("rust-analyzer", 9.5, 2.3), ("tunes", 3.1, 0.2)];
        let mut list = column().spacing(10.0).width(Length::Fill);
        for (i, (n, c, m)) in procs.iter().enumerate() {
            let c = (c + (cpu - 40.0) * 0.05 * (i as f32 + 1.0) / 2.0).max(0.1);
            list = list.push(
                row()
                    .spacing(12.0)
                    .align(Align::Center)
                    .width(Length::Fill)
                    .push(text(*n).role(TextRole::Strong).width(Length::Fill))
                    .push(text(format!("{c:.1}% · {m:.1} GiB")).mono().role(TextRole::Caption).tone(Tone::Muted))
                    .push(progress_bar(c / 25.0).width(64.0).height(8.0)),
            );
        }
        card("System Monitor", icons::ACTIVITY, 5, column().spacing(18.0).width(Length::Fill).push(gauges).push(chart).push(list))
    }

    fn settings(&self) -> Element<Msg> {
        let sections = [(icons::PALETTE, "Appearance"), (icons::ACCESSIBILITY, "Accessibility"), (icons::MONITOR, "Displays"), (icons::WIFI, "Network"), (icons::BELL, "Notifications")];
        let mut side = column().spacing(8.0).width(190.0);
        for (i, (g, name)) in sections.iter().enumerate() {
            side = side.push(
                Button::new(row().spacing(10.0).align(Align::Center).push(icon(*g).size(17.0)).push(text(*name).role(TextRole::Strong)))
                    .kind(ButtonKind::Ghost)
                    .selected(i == 0)
                    .on_press(Msg::Style(self.style))
                    .width(Length::Fill)
                    .padding([12.0, 10.0])
                    .align_x(Align::Start),
            );
        }
        let swatches = Accent::PRESETS.iter().fold(row().spacing(10.0), |r, a| {
            r.push(
                Button::new(container(Space::new(16.0, 16.0)).background(Background::Color(a.swatch())).radius(8.0))
                    .round()
                    .padding(8.0)
                    .selected(*a == self.accent)
                    .on_press(Msg::Accent(*a)),
            )
        });
        let style_idx = if self.style == Style::Flat { 0 } else { 1 };
        let scheme_idx = match self.scheme {
            SchemePref::Auto => 0,
            SchemePref::Light => 1,
            SchemePref::Dark => 2,
        };
        let pane = column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(setting("Style", "Flat uses borders and fills, with higher contrast. Soft uses light and shadow for depth.", segmented(["Flat", "Soft"], Some(style_idx), |i| Msg::Style(if i == 0 { Style::Flat } else { Style::Soft }))))
            .push(setting("Colour scheme", "Auto follows your system setting.", segmented(["Auto", "Light", "Dark"], Some(scheme_idx), |i| Msg::Scheme([SchemePref::Auto, SchemePref::Light, SchemePref::Dark][i]))))
            .push(setting("Accent", self.accent.name(), swatches))
            .push(Divider::horizontal())
            .push(setting("Shadow depth", &format!("{} px. How far soft surfaces lift off the background.", self.depth), container(slider(2.0..=12.0, self.depth, Msg::Depth).step(1.0)).width(220.0)))
            .push(setting("Corner radius", &format!("{} px. Applies to windows and controls.", self.radius), container(slider(6.0..=32.0, self.radius, Msg::Radius).step(1.0)).width(220.0)))
            .push(Divider::horizontal())
            .push(setting("Glass windows", "Makes windows translucent and blurs what is behind them.", toggle(self.glass, Msg::Glass)))
            .push_if(self.glass, || setting("Glass opacity", &format!("{}%", (self.glass_opacity * 100.0).round()), container(slider(0.3..=0.95, self.glass_opacity, Msg::GlassOpacity).step(0.05)).width(220.0)))
            .push_if(self.glass, || setting("Blur strength", &format!("{} px. How much the desktop behind is blurred.", self.glass_blur), container(slider(0.0..=40.0, self.glass_blur, Msg::GlassBlur).step(1.0)).width(220.0)))
            .push(setting("Larger text", "Scales interface text up by about 18%.", toggle(self.large_text, Msg::LargeText)))
            .push(setting("Reduce motion", "Turns off animation and transitions.", toggle(self.reduce_motion, Msg::ReduceMotion)));
        card("Settings", icons::SETTINGS, 7, row().spacing(24.0).width(Length::Fill).push(side).push(pane))
    }

    fn form(&self) -> Element<Msg> {
        let field = |label: &str, input: TextInput<Msg>| -> Element<Msg> { column().spacing(8.0).width(Length::Fill).push(text(label).role(TextRole::Label).tone(Tone::Muted)).push(input).into() };
        let complete = [!self.name.is_empty(), self.email.contains('@'), self.notify || self.updates].iter().filter(|b| **b).count() as f32 / 3.0;
        let mut body = column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(field("Name", text_input("Your name", self.name.clone()).on_input(Msg::Name)))
            .push(field("Email", text_input("you@example.org", self.email.clone()).on_input(Msg::Email).on_submit(Msg::Submit)))
            .push(checkbox("Email me when someone mentions me", self.notify, Msg::Notify))
            .push(checkbox("Send me product updates", self.updates, Msg::Updates))
            .push(column().spacing(8.0).width(Length::Fill).push(row().width(Length::Fill).push(text("Profile complete").role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x()).push(text(format!("{}%", (complete * 100.0).round())).mono().role(TextRole::Caption))).push(progress_bar(complete)))
            .push(row().spacing(12.0).push(button("Cancel").on_press(Msg::Name(String::new()))).push(button("Save profile").kind(ButtonKind::Accent).on_press(Msg::Submit)));
        if let Some(s) = &self.submitted {
            let tone = if s.starts_with("Saved") { Tone::Good } else { Tone::Bad };
            body = body.push(text(s.clone()).tone(tone).role(TextRole::Strong));
        }
        card("Account", icons::USER, 5, body)
    }
}

#[derive(Default)]
struct Calc {
    cur: String,
    prev: Option<f64>,
    op: Option<&'static str>,
    fresh: bool,
    expr: String,
}

impl Calc {
    fn display(&self) -> String {
        if self.cur.is_empty() { "0".into() } else { self.cur.clone() }
    }

    fn fmt(n: f64) -> String {
        if !n.is_finite() {
            return "Error".into();
        }
        let s = format!("{}", (n * 1e10).round() / 1e10);
        if s.len() > 14 { format!("{n:.6e}") } else { s }
    }

    fn apply(op: &str, a: f64, b: f64) -> f64 {
        match op {
            "+" => a + b,
            "−" => a - b,
            "×" => a * b,
            "÷" if b != 0.0 => a / b,
            _ => f64::NAN,
        }
    }

    fn press(&mut self, k: &'static str) {
        let val = |s: &str| s.parse::<f64>().unwrap_or(0.0);
        match k {
            "C" => *self = Calc::default(),
            "±" => {
                if let Some(rest) = self.cur.strip_prefix('-') {
                    self.cur = rest.into();
                } else if !self.cur.is_empty() && self.cur != "0" {
                    self.cur = format!("-{}", self.cur);
                }
            }
            "%" => {
                self.cur = Self::fmt(val(&self.cur) / 100.0);
                self.fresh = true;
            }
            "+" | "−" | "×" | "÷" => {
                if let (Some(op), Some(p), false) = (self.op, self.prev, self.fresh) {
                    self.cur = Self::fmt(Self::apply(op, p, val(&self.cur)));
                }
                self.prev = Some(val(&self.cur));
                self.op = Some(k);
                self.fresh = true;
                self.expr = format!("{} {k}", self.display());
            }
            "=" => {
                if let (Some(op), Some(p)) = (self.op, self.prev) {
                    let b = val(&self.cur);
                    self.expr = format!("{} {op} {} =", Self::fmt(p), Self::fmt(b));
                    self.cur = Self::fmt(Self::apply(op, p, b));
                    self.prev = None;
                    self.op = None;
                    self.fresh = true;
                }
            }
            "." => {
                if self.fresh || self.cur.is_empty() {
                    self.cur = "0.".into();
                    self.fresh = false;
                } else if !self.cur.contains('.') {
                    self.cur.push('.');
                }
            }
            d => {
                if self.fresh || self.cur == "0" || self.cur == "Error" {
                    self.cur = d.into();
                    self.fresh = false;
                } else if self.cur.len() < 14 {
                    self.cur.push_str(d);
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        let dir = std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into()));
        std::fs::create_dir_all(&dir).expect("create snapshot dir");
        let variants = [
            ("flat-light", Style::Flat, SchemePref::Light, false),
            ("flat-dark", Style::Flat, SchemePref::Dark, false),
            ("soft-light", Style::Soft, SchemePref::Light, false),
            ("soft-dark", Style::Soft, SchemePref::Dark, false),
            ("soft-light-glass", Style::Soft, SchemePref::Light, true),
        ];
        for (name, style, scheme, glass) in variants {
            let app = Gallery { style, scheme, glass, playing: true, ..Default::default() };
            let mut h = Harness::new(app, Size::new(1240.0, 900.0)).expect("GPU");
            h.click(neo::Point::new(800.0, 300.0));
            for k in ["1", "2", "8", "0", "×", "0", ".", "1", "5", "="] {
                h.app_mut().calc.press(k);
            }
            let path = dir.join(format!("{name}.png"));
            h.save_png(&path, 1.0).expect("write png");
            println!("wrote {}", path.display());
        }
        return;
    }
    let glass = args.iter().any(|a| a == "--glass");
    let glass_blur = args.iter().position(|a| a == "--blur").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(10.0);
    if let Err(e) = neo::run(Gallery { glass, glass_blur, ..Default::default() }) {
        eprintln!("gallery: {e}");
        std::process::exit(1);
    }
}
