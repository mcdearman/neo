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

mod shells;

use std::path::PathBuf;

use neo::prelude::*;
use neo::{Image, Proxy, Size};
use neo_desktop::fs::home_dir;
use neo_desktop::Desktop;
use neo_term::{Program, Shell, TermMsg};

struct Terminal {
    desktop: Desktop,
    shell: Shell,
    /// The shells this computer has, each with its icon where it has one,
    /// once they have been looked for.
    shells: Vec<(shells::Kind, Option<Image>)>,
    /// What the one running is called.
    running: String,
    /// Whether a choice of shell is written down: not when one is given to run.
    keeps: bool,
}

#[derive(Clone)]
enum Msg {
    /// Something that happened in the terminal.
    Term(TermMsg),
    /// The shells found on this computer.
    Shells(Vec<(shells::Kind, Option<Image>)>),
    /// Run this one of them in place of what is running.
    Run(usize),
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
}

impl Terminal {
    /// A terminal in `cwd`, running `program` or else the shell chosen
    /// last, or else the user's own.
    fn new(cwd: PathBuf, program: Option<Program>) -> Self {
        let keeps = program.is_none() && !cfg!(test);
        let chosen = if keeps { shells::chosen(&neo_desktop::config::File::app("neo-terminal")) } else { None };
        let running = chosen.as_ref().map_or_else(shells::default_name, |k| k.name.clone());
        let program = program.or_else(|| chosen.map(|k| Program::new(k.program, k.args)));
        Self { desktop: Desktop::load(), shell: Shell::new(cwd, program), shells: vec![], running, keeps }
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
        // Looking for shells can take a moment; the terminal does not wait for it.
        let found = proxy.clone();
        std::thread::spawn(move || {
            found.send(Msg::Shells(shells::found().into_iter().map(|kind| (shells::icon(&kind), kind)).map(|(icon, kind)| (kind, icon)).collect()));
        });
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
            Msg::Shells(found) => self.shells = found,
            Msg::Run(i) => {
                if let Some((kind, _)) = self.shells.get(i).cloned() {
                    self.shell.run(Some(Program::new(kind.program.clone(), kind.args.clone())));
                    if self.keeps {
                        let _ = shells::keep(&kind);
                    }
                    self.running = kind.name;
                }
                self.shell.focus();
            }
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    /// Beside File, the shell that is running: its menu lists the others.
    /// The system's menu bar on macOS calls it Shell, as every app's menu
    /// there keeps its name.
    fn menus(&self) -> Vec<Menu<Msg>> {
        let mut shells = Menu::new(if cfg!(target_os = "macos") { "Shell".to_owned() } else { self.running.clone() });
        if self.shells.is_empty() {
            shells = shells.push(MenuEntry::disabled("Looking for shells…"));
        }
        for (i, (kind, picture)) in self.shells.iter().enumerate() {
            let entry = MenuEntry::new(kind.name.clone(), Msg::Run(i));
            shells = shells.push(match picture {
                Some(picture) => entry.icon(picture.clone()),
                None => entry,
            });
        }
        vec![Menu::new("File").push(MenuEntry::new("New Session", Msg::Term(TermMsg::Restart))), shells]
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

#[cfg(test)]
mod tests {
    use super::*;
    use neo::testing::Harness;

    #[test]
    fn the_shells_are_in_a_menu_beside_file_and_choosing_one_runs_it() {
        let shell = if cfg!(windows) { Program::new("cmd.exe".into(), vec![]) } else { Program::new("/bin/sh".into(), vec![]) };
        let mut h = Harness::new(Terminal::new(home_dir(), Some(shell)), Size::new(900.0, 580.0)).expect("GPU");
        let found: Vec<(shells::Kind, Option<Image>)> = shells::found().into_iter().map(|kind| (shells::icon(&kind), kind)).map(|(icon, kind)| (kind, icon)).collect();
        assert!(!found.is_empty());
        if cfg!(windows) {
            assert!(found.iter().all(|(_, icon)| icon.is_some()), "every shell on Windows has an icon of its own");
        }
        h.app_mut().update(Msg::Shells(found.clone()));
        let menus = h.app().menus();
        assert_eq!((menus[0].title.as_str(), menus[1].entries.len()), ("File", found.len()), "one entry to a shell, after File");
        assert_eq!(menus[1].entries.iter().map(|e| e.icon.is_some()).collect::<Vec<_>>(), found.iter().map(|(_, icon)| icon.is_some()).collect::<Vec<_>>());
        // Choosing one names the menu after it. Nothing is written down: this terminal was given what to run.
        let last = found.len() - 1;
        h.app_mut().update(Msg::Run(last));
        assert_eq!(h.app().running, found[last].0.name);
    }
}