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

use std::path::PathBuf;

use neo::prelude::*;
use neo::{Proxy, Size};
use neo_desktop::fs::home_dir;
use neo_desktop::Desktop;
use neo_term::{Program, Shell, TermMsg};

struct Terminal {
    desktop: Desktop,
    shell: Shell,
}

#[derive(Clone)]
enum Msg {
    /// Something that happened in the terminal.
    Term(TermMsg),
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
}

impl Terminal {
    fn new(cwd: PathBuf, program: Option<Program>) -> Self {
        Self { desktop: Desktop::load(), shell: Shell::new(cwd, program) }
    }
}

impl App for Terminal {
    type Message = Msg;

    fn title(&self) -> String {
        self.shell.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "NeoTerm".into())
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
        self.shell.start(move |m| {
            proxy.send(Msg::Term(m));
        });
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll)]
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Term(m) => self.shell.update(m),
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.shell.view(Msg::Term), "NeoTerm Settings", Msg::Desktop, vec![])
    }
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
        let shell = Program::new("/bin/sh".into(), vec!["-c".into(), script.into()]);
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
