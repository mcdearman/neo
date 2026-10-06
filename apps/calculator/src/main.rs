//! NeoCal: type an expression or use the keypad.
//!
//!     cargo run -p neo-calculator
//!     cargo run -p neo-calculator -- --snapshot target/snapshots
//!
//! Understands `+ − × ÷ ^ % !`, brackets, `sqrt sin cos tan ln log abs`,
//! `π e ans` and implicit multiplication such as `2π`. The result updates
//! as you type; Enter keeps it in the history.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod expr;

use neo::prelude::*;
use neo::{Key, KeyEvent, Size};
use neo_desktop::Desktop;

const HISTORY_LIMIT: usize = 50;

struct Calculator {
    desktop: Desktop,
    input: String,
    ans: f64,
    history: Vec<(String, f64)>,
    scientific: bool,
    error: Option<String>,
}

#[derive(Clone, Debug)]
enum Msg {
    Input(String),
    Insert(&'static str),
    Backspace,
    Clear,
    Equals,
    Recall(usize),
    ClearHistory,
    Scientific(bool),
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
}

impl Calculator {
    fn new() -> Self {
        Self { desktop: Desktop::load(), input: String::new(), ans: 0.0, history: vec![], scientific: false, error: None }
    }

    fn preview(&self) -> Option<Result<f64, String>> {
        if self.input.trim().is_empty() {
            return None;
        }
        Some(expr::eval(&self.input, self.ans))
    }

    fn equals(&mut self) {
        match expr::eval(&self.input, self.ans) {
            Ok(v) => {
                self.history.insert(0, (self.input.trim().to_string(), v));
                self.history.truncate(HISTORY_LIMIT);
                self.ans = v;
                self.input = expr::format(v).replace(',', "").replace('−', "-");
                self.error = None;
            }
            Err(e) if !self.input.trim().is_empty() => self.error = Some(e),
            Err(_) => {}
        }
    }
}

impl App for Calculator {
    type Message = Msg;

    fn title(&self) -> String {
        "NeoCal".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(760.0, 560.0), min_size: Some(Size::new(380.0, 480.0)), app_id: Some("org.neo.Calculator".into()), ..Default::default() }
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

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        match &k.key {
            Key::Enter => Some(Msg::Equals),
            Key::Escape => Some(Msg::Clear),
            Key::Character(c) if c == "=" && !k.modifiers.command() => Some(Msg::Equals),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Input(s) => {
                self.input = s;
                self.error = None;
            }
            Msg::Insert(s) => {
                // Starting a fresh number after a result replaces it; an operator continues from it.
                let continues = s.starts_with(|c: char| "+−×÷^%!)".contains(c));
                if self.history.first().is_some_and(|(_, v)| self.input == expr::format(*v).replace(',', "").replace('−', "-")) && !continues {
                    self.input.clear();
                }
                self.input.push_str(s);
                self.error = None;
            }
            Msg::Backspace => {
                self.input.pop();
                self.error = None;
            }
            Msg::Clear => {
                self.input.clear();
                self.error = None;
            }
            Msg::Equals => self.equals(),
            Msg::Recall(i) => {
                if let Some((e, _)) = self.history.get(i) {
                    self.input = e.clone();
                    self.error = None;
                }
            }
            Msg::ClearHistory => self.history.clear(),
            Msg::Scientific(s) => self.scientific = s,
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "NeoCal Settings", Msg::Desktop, vec![])
    }
}

impl Calculator {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        row().spacing(12.0).width(Length::Fill).height(Length::Fill).padding([0.0, 12.0, 12.0, 12.0]).push(self.main()).push(self.history_panel()).into()
    }
}

impl Calculator {
    fn main(&self) -> Element<Msg> {
        let (result, tone) = match (self.preview(), &self.error) {
            (_, Some(e)) => (e.clone(), Tone::Bad),
            (None, _) => ("0".to_string(), Tone::Faint),
            (Some(Ok(v)), _) => (format!("= {}", expr::format(v)), Tone::Muted),
            (Some(Err(_)), _) => ("…".to_string(), Tone::Faint),
        };
        let display = container(
            column()
                .spacing(6.0)
                .width(Length::Fill)
                .push(text_input("Type an expression", self.input.clone()).on_input(Msg::Input).on_submit(Msg::Equals).autofocus(true))
                .push(text(result).mono().role(TextRole::Heading).tone(tone).align(Align::End).width(Length::Fill).no_wrap()),
        )
        .surface(Surface::Well)
        .padding(14.0)
        .width(Length::Fill);

        let basic: [[&'static str; 4]; 5] = [["C", "( )", "%", "÷"], ["7", "8", "9", "×"], ["4", "5", "6", "−"], ["1", "2", "3", "+"], ["0", ".", "⌫", "="]];
        let sci: [[&'static str; 3]; 5] = [["sin", "cos", "tan"], ["ln", "log", "√"], ["x²", "^", "!"], ["π", "e", "ans"], ["(", ")", "1/x"]];
        let key = |k: &'static str| -> Element<Msg> {
            let (msg, tone, accent) = match k {
                "C" => (Msg::Clear, Tone::Bad, false),
                "⌫" => (Msg::Backspace, Tone::Muted, false),
                "=" => (Msg::Equals, Tone::Inherit, true),
                "( )" => (Msg::Insert(if self.input.matches('(').count() > self.input.matches(')').count() { ")" } else { "(" }), Tone::Muted, false),
                "÷" | "×" | "−" | "+" | "%" | "^" | "!" => (Msg::Insert(k), Tone::Accent, false),
                "sin" | "cos" | "tan" | "ln" | "log" => (Msg::Insert(match k { "sin" => "sin(", "cos" => "cos(", "tan" => "tan(", "ln" => "ln(", _ => "log(" }), Tone::Muted, false),
                "√" => (Msg::Insert("√("), Tone::Muted, false),
                "x²" => (Msg::Insert("^2"), Tone::Muted, false),
                "1/x" => (Msg::Insert("^-1"), Tone::Muted, false),
                "π" | "e" | "ans" | "(" | ")" => (Msg::Insert(k), Tone::Muted, false),
                d => (Msg::Insert(d), Tone::Inherit, false),
            };
            let label = text(k).role(TextRole::Title).tone(tone);
            let mut b = Button::new(label).on_press(msg).width(Length::Fill).height(Length::Fill).padding(0.0);
            if accent {
                b = b.kind(ButtonKind::Accent);
            }
            b.into()
        };
        let mut pad = row().spacing(10.0).width(Length::Fill).height(Length::Fill);
        if self.scientific {
            let mut col = column().spacing(10.0).width(Length::Portion(3)).height(Length::Fill);
            for r in sci {
                col = col.push(r.iter().fold(row().spacing(10.0).width(Length::Fill).height(Length::Fill), |line, k| line.push(key(k))));
            }
            pad = pad.push(col);
        }
        let mut col = column().spacing(10.0).width(Length::Portion(4)).height(Length::Fill);
        for r in basic {
            col = col.push(r.iter().fold(row().spacing(10.0).width(Length::Fill).height(Length::Fill), |line, k| line.push(key(k))));
        }
        pad = pad.push(col);

        let mode = row().width(Length::Fill).align(Align::Center).push(segmented(["Basic", "Scientific"], Some(self.scientific as usize), |i| Msg::Scientific(i == 1))).push(Space::fill_x());
        container(column().spacing(14.0).width(Length::Fill).height(Length::Fill).push(mode).push(display).push(pad)).surface(Surface::Card).padding(16.0).width(Length::Fill).height(Length::Fill).into()
    }

    fn history_panel(&self) -> Element<Msg> {
        let mut list = column().spacing(2.0).width(Length::Fill);
        if self.history.is_empty() {
            list = list.push(container(text("Results you keep with Enter or = appear here. Click one to reuse it.").role(TextRole::Caption).tone(Tone::Muted)).padding([8.0, 6.0]));
        }
        for (i, (e, v)) in self.history.iter().enumerate() {
            let item = column().spacing(2.0).width(Length::Fill).push(text(e.clone()).mono().role(TextRole::Caption).tone(Tone::Muted).no_wrap()).push(text(expr::format(*v)).mono().role(TextRole::Strong).no_wrap());
            list = list.push(Button::new(item).kind(ButtonKind::Ghost).padding([10.0, 8.0]).width(Length::Fill).align_x(Align::Start).on_press(Msg::Recall(i)));
        }
        let header = row()
            .width(Length::Fill)
            .align(Align::Center)
            .padding([6.0, 4.0])
            .push(text("History").role(TextRole::Label).tone(Tone::Muted))
            .push(Space::fill_x())
            .push(icon_button(icons::TRASH_2, 30.0).kind(ButtonKind::Ghost).on_press_maybe((!self.history.is_empty()).then_some(Msg::ClearHistory)));
        column().spacing(4.0).width(220.0).height(Length::Fill).push(header).push(scrollable(list).height(Length::Fill)).into()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    if let Err(e) = neo::run(Calculator::new()) {
        eprintln!("neo-calculator: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: std::path::PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, scheme, sci) in [("calculator-light", neo_desktop::SchemePref::Light, false), ("calculator-dark", neo_desktop::SchemePref::Dark, true)] {
        let mut app = Calculator::new();
        app.desktop.appearance.scheme = scheme;
        app.scientific = sci;
        for e in ["1280 × 0.15", "2π × 3^2", "sqrt(2)"] {
            app.input = e.into();
            app.equals();
        }
        app.input = "ans × 100 + 12%".into();
        let mut h = Harness::new(app, Size::new(760.0, 560.0)).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo::testing::Harness;
    use neo::Modifiers;

    #[test]
    fn typing_and_enter_keep_a_result() {
        let mut h = Harness::new(Calculator::new(), Size::new(760.0, 560.0)).expect("GPU");
        // The expression field has focus when the window opens.
        h.type_text("2+3×4");
        assert_eq!(h.app().input, "2+3×4");
        h.key(Key::Enter, Modifiers::default());
        assert_eq!(h.app().history[0].1, 14.0);
        assert_eq!(h.app().input, "14");
        // An operator continues from the result; a digit starts afresh.
        h.app_mut().update(Msg::Insert("×"));
        h.app_mut().update(Msg::Insert("2"));
        h.app_mut().update(Msg::Equals);
        assert_eq!(h.app().ans, 28.0);
        h.app_mut().update(Msg::Insert("5"));
        assert_eq!(h.app().input, "5");
    }
}
