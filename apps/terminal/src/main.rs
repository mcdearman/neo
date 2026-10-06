//! NeoTerm: a GPU-drawn terminal emulator.
//!
//! Terminal emulation and the pty come from `alacritty_terminal`; Neo draws
//! the grid and handles input.
//!
//!     cargo run -p neo-terminal [folder]
//!     cargo run -p neo-terminal -- --snapshot target/snapshots
//!
//! Copy and paste with Command+C and Command+V on macOS, Ctrl+Shift+C and
//! Ctrl+Shift+V elsewhere. Shift+Page Up and Page Down scroll the history.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod colors;
mod keys;
mod view;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg as PtyMsg};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use neo::prelude::*;
use neo::{Proxy, Size};
use neo_desktop::fs::home_dir;
use neo_desktop::Desktop;

use view::{Font, TermView};

const FONT: Font = Font { size: 13.5, line_height: 1.3 };

/// Forwards terminal events to the app from the pty thread.
#[derive(Clone)]
struct Listener(Proxy<Msg>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let m = match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => Msg::Wakeup,
            TermEvent::Title(t) => Msg::Title(Some(t)),
            TermEvent::ResetTitle => Msg::Title(None),
            TermEvent::PtyWrite(s) => Msg::Reply(s),
            TermEvent::TextAreaSizeRequest(f) => Msg::SizeRequest(f),
            TermEvent::ChildExit(code) => Msg::Exited(Some(code)),
            TermEvent::Exit => Msg::Exited(None),
            TermEvent::ClipboardStore(..) | TermEvent::ClipboardLoad(..) | TermEvent::ColorRequest(..) | TermEvent::Bell => return,
        };
        self.0.send(m);
    }
}

/// The terminal's size in cells.
struct GridSize {
    columns: usize,
    lines: usize,
}

impl GridSize {
    fn of(size: WindowSize) -> Self {
        Self { columns: size.num_cols as usize, lines: size.num_lines as usize }
    }
}

impl alacritty_terminal::grid::Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// A running shell and its screen.
struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
}

impl Session {
    fn spawn(proxy: Proxy<Msg>, size: WindowSize, cwd: PathBuf, shell: Option<tty::Shell>) -> std::io::Result<Self> {
        tty::setup_env();
        let listener = Listener(proxy);
        let term = Arc::new(FairMutex::new(Term::new(Config::default(), &GridSize::of(size), listener.clone())));
        // The update fills Windows-only fields.
        #[allow(clippy::needless_update)]
        let options = tty::Options { shell, working_directory: Some(cwd), drain_on_exit: true, env: Default::default(), ..Default::default() };
        let pty = tty::new(&options, size, 0)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        event_loop.spawn();
        Ok(Self { term, pty: sender })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pty.send(PtyMsg::Shutdown);
    }
}

struct Terminal {
    desktop: Desktop,
    proxy: Option<Proxy<Msg>>,
    session: Option<Session>,
    cwd: PathBuf,
    shell: Option<tty::Shell>,
    size: WindowSize,
    title: Option<String>,
    exited: Option<Option<i32>>,
    error: Option<String>,
}

#[derive(Clone)]
enum Msg {
    Wakeup,
    Title(Option<String>),
    Reply(String),
    SizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
    Resize(WindowSize),
    Exited(Option<i32>),
    Restart,
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
}

impl Terminal {
    fn new(cwd: PathBuf, shell: Option<tty::Shell>) -> Self {
        Self {
            desktop: Desktop::load(),
            proxy: None,
            session: None,
            cwd,
            shell,
            size: WindowSize { num_cols: 80, num_lines: 24, cell_width: 8, cell_height: 18 },
            title: None,
            exited: None,
            error: None,
        }
    }

    fn spawn(&mut self) {
        let Some(proxy) = self.proxy.clone() else { return };
        self.session = None;
        self.exited = None;
        self.title = None;
        match Session::spawn(proxy, self.size, self.cwd.clone(), self.shell.clone()) {
            Ok(s) => {
                self.session = Some(s);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Could not start a shell: {e}")),
        }
    }

    fn write(&self, s: String) {
        if let Some(session) = &self.session {
            let _ = session.pty.send(PtyMsg::Input(Cow::Owned(s.into_bytes())));
        }
    }
}

impl App for Terminal {
    type Message = Msg;

    fn title(&self) -> String {
        self.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "NeoTerm".into())
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(900.0, 580.0), min_size: Some(Size::new(360.0, 200.0)), app_id: Some("org.neo.Terminal".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy);
        self.spawn();
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll)]
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Wakeup => {}
            Msg::Title(t) => self.title = t,
            Msg::Reply(s) => self.write(s),
            Msg::SizeRequest(f) => self.write(f(self.size)),
            Msg::Resize(size) => {
                self.size = size;
                if let Some(s) = &self.session {
                    s.term.lock().resize(GridSize::of(size));
                    let _ = s.pty.send(PtyMsg::Resize(size));
                }
            }
            Msg::Exited(code) => self.exited = Some(code),
            Msg::Restart => self.spawn(),
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "NeoTerm Settings", Msg::Desktop, vec![])
    }
}

impl Terminal {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        let mut col = column().width(Length::Fill).height(Length::Fill);
        if let Some(s) = &self.session {
            col = col.push(Element::new(TermView::new(s.term.clone(), s.pty.clone(), FONT, Msg::Resize)));
        } else {
            col = col.push(Space::fill_y());
        }
        if let Some(e) = &self.error {
            col = col.push(banner(Tone::Bad, e.clone()));
        } else if let Some(code) = self.exited {
            let msg = match code {
                Some(0) | None => "The shell has exited.".to_string(),
                Some(c) => format!("The shell exited with status {c}."),
            };
            col = col.push(banner(Tone::Muted, msg));
        }
        col.into()
    }
}

fn banner(tone: Tone, msg: String) -> Element<Msg> {
    container(
        row()
            .spacing(12.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(icons::SQUARE_TERMINAL).size(16.0).tone(tone))
            .push(text(msg).tone(tone).width(Length::Fill))
            .push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::ROTATE_CW).size(15.0)).push(text("New session"))).on_press(Msg::Restart)),
    )
    .surface(Surface::Card)
    .padding([16.0, 10.0])
    .width(Length::Fill)
    .into()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    let cwd = args.first().map(PathBuf::from).unwrap_or_else(home_dir);
    if let Err(e) = neo::run(Terminal::new(cwd, None)) {
        eprintln!("neo-terminal: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    use std::time::Duration;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    // A fixed script, so the snapshot does not depend on the user's shell setup.
    let script = r#"printf '\033]0;neo@aurora: ~/neo\007'
printf '\033[1;32mneo@aurora\033[0m:\033[1;34m~/neo\033[0m$ ls\n'
printf '\033[1;34mapps\033[0m  \033[1;34mcrates\033[0m  Cargo.lock  Cargo.toml  README.md\n'
printf '\033[1;32mneo@aurora\033[0m:\033[1;34m~/neo\033[0m$ cargo test -q\n'
printf 'running 52 tests\n'
printf '\033[32m....................................................\033[0m\n'
printf 'test result: \033[32mok\033[0m. 52 passed; 0 failed\n\n'
for i in 0 1 2 3 4 5 6 7; do printf "\033[4${i}m  \033[0m"; done; printf '\n'
for i in 0 1 2 3 4 5 6 7; do printf "\033[10${i}m  \033[0m"; done; printf '\n\n'
printf '\033[1mbold\033[0m \033[3mitalic\033[0m \033[4munderline\033[0m \033[7minverse\033[0m \033[2mdim\033[0m \033[9mstrike\033[0m\n'
printf '\033[38;2;255;97;136mtrue\033[38;2;169;220;118mcolour\033[0m 你好 ✓ → λ\n\n'
printf '\033[1;32mneo@aurora\033[0m:\033[1;34m~/neo\033[0m$ '
sleep 5"#;
    for (name, scheme) in [("terminal-dark", neo_desktop::SchemePref::Dark), ("terminal-light", neo_desktop::SchemePref::Light)] {
        let shell = tty::Shell::new("/bin/sh".into(), vec!["-c".into(), script.into()]);
        let mut app = Terminal::new(home_dir(), Some(shell));
        app.desktop.appearance.scheme = scheme;
        let mut h = Harness::new(app, Size::new(900.0, 580.0)).expect("GPU");
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(40));
            h.advance(Duration::from_millis(40));
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}
