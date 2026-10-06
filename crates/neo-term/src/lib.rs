//! A terminal to put in a Neo app: a shell running in a pty, its screen,
//! and the widget that shows it and takes its keys.
//!
//! An app keeps a [`Shell`], gives it somewhere to send word of what
//! happens ([`Shell::start`]), passes those messages back
//! ([`Shell::update`]) and shows it ([`Shell::view`]).

mod colors;
mod keys;
mod view;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg as PtyMsg};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use neo::prelude::*;

pub use alacritty_terminal::event::WindowSize;
pub use alacritty_terminal::tty::Shell as Program;
pub use view::{Font, TermView};

/// Something that happened in a terminal, for the app to pass back to it.
#[derive(Clone)]
pub enum TermMsg {
    /// The screen changed.
    Wakeup,
    Title(Option<String>),
    /// Something the terminal answers the program with.
    Reply(String),
    SizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
    Resize(WindowSize),
    Exited(Option<i32>),
    /// Start a new shell in place of one that has ended.
    Restart,
}

impl std::fmt::Debug for TermMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TermMsg::Wakeup => "Wakeup",
            TermMsg::Title(_) => "Title",
            TermMsg::Reply(_) => "Reply",
            TermMsg::SizeRequest(_) => "SizeRequest",
            TermMsg::Resize(_) => "Resize",
            TermMsg::Exited(_) => "Exited",
            TermMsg::Restart => "Restart",
        })
    }
}

/// Where word of what happens in a terminal is sent, from its own thread.
type Sender = Arc<dyn Fn(TermMsg) + Send + Sync>;

/// Forwards terminal events to the app from the pty thread.
#[derive(Clone)]
pub struct Listener(Sender);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let m = match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => TermMsg::Wakeup,
            TermEvent::Title(t) => TermMsg::Title(Some(t)),
            TermEvent::ResetTitle => TermMsg::Title(None),
            TermEvent::PtyWrite(s) => TermMsg::Reply(s),
            TermEvent::TextAreaSizeRequest(f) => TermMsg::SizeRequest(f),
            TermEvent::ChildExit(code) => TermMsg::Exited(Some(code)),
            TermEvent::Exit => TermMsg::Exited(None),
            TermEvent::ClipboardStore(..) | TermEvent::ClipboardLoad(..) | TermEvent::ColorRequest(..) | TermEvent::Bell => return,
        };
        (self.0)(m);
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
    fn spawn(send: Sender, size: WindowSize, cwd: PathBuf, shell: Option<Program>) -> std::io::Result<Self> {
        tty::setup_env();
        let listener = Listener(send);
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

/// A terminal an app holds: the shell if one is running, and what to say
/// if it is not.
pub struct Shell {
    send: Option<Sender>,
    session: Option<Session>,
    pub cwd: PathBuf,
    program: Option<Program>,
    size: WindowSize,
    /// The title the program in it has set, if it has.
    pub title: Option<String>,
    /// How the shell ended, if it has.
    pub exited: Option<Option<i32>>,
    pub error: Option<String>,
    pub font: Font,
    /// Changed to hand the terminal the keyboard.
    focus: u64,
}

impl Shell {
    /// A terminal that will start in `cwd`, running `program` or else the
    /// user's own shell.
    pub fn new(cwd: PathBuf, program: Option<Program>) -> Self {
        Self { send: None, session: None, cwd, program, size: WindowSize { num_cols: 80, num_lines: 24, cell_width: 8, cell_height: 18 }, title: None, exited: None, error: None, font: Font { size: 13.5, line_height: 1.3 }, focus: 0 }
    }

    /// Starts the shell. `send` is called from the terminal's own thread
    /// with what happens; pass each message back to [`Shell::update`].
    pub fn start(&mut self, send: impl Fn(TermMsg) + Send + Sync + 'static) {
        self.send = Some(Arc::new(send));
        self.spawn();
    }

    pub fn running(&self) -> bool {
        self.session.is_some() && self.exited.is_none()
    }

    /// Gives the terminal the keyboard the next time it is shown.
    pub fn focus(&mut self) {
        self.focus += 1;
    }

    fn spawn(&mut self) {
        let Some(send) = self.send.clone() else { return };
        self.session = None;
        self.exited = None;
        self.title = None;
        match Session::spawn(send, self.size, self.cwd.clone(), self.program.clone()) {
            Ok(s) => {
                self.session = Some(s);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Could not start a shell: {e}")),
        }
    }

    /// Types into the shell, as if at the keyboard.
    pub fn write(&self, s: String) {
        if let Some(session) = &self.session {
            let _ = session.pty.send(PtyMsg::Input(Cow::Owned(s.into_bytes())));
        }
    }

    pub fn update(&mut self, m: TermMsg) {
        match m {
            TermMsg::Wakeup => {}
            TermMsg::Title(t) => self.title = t,
            TermMsg::Reply(s) => self.write(s),
            TermMsg::SizeRequest(f) => self.write(f(self.size)),
            TermMsg::Resize(size) => {
                self.size = size;
                if let Some(s) = &self.session {
                    s.term.lock().resize(GridSize::of(size));
                    let _ = s.pty.send(PtyMsg::Resize(size));
                }
            }
            TermMsg::Exited(code) => self.exited = Some(code),
            TermMsg::Restart => self.spawn(),
        }
    }

    /// The terminal, with a note under it if the shell has ended or
    /// could not be started.
    pub fn view<M: Clone + 'static>(&self, wrap: impl Fn(TermMsg) -> M + Clone + 'static) -> Element<M> {
        let mut col = column().width(Length::Fill).height(Length::Fill);
        if let Some(s) = &self.session {
            let resize = wrap.clone();
            col = col.push(Element::new(TermView::new(s.term.clone(), s.pty.clone(), self.font, move |size| resize(TermMsg::Resize(size))).focus(self.focus)));
        } else {
            col = col.push(Space::fill_y());
        }
        let banner = |tone: Tone, msg: String| -> Element<M> {
            container(
                row()
                    .spacing(12.0)
                    .align(Align::Center)
                    .width(Length::Fill)
                    .push(icon(icons::SQUARE_TERMINAL).size(16.0).tone(tone))
                    .push(text(msg).tone(tone).width(Length::Fill))
                    .push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::ROTATE_CW).size(15.0)).push(text("New session"))).on_press(wrap(TermMsg::Restart))),
            )
            .surface(Surface::Card)
            .padding([16.0, 10.0])
            .width(Length::Fill)
            .into()
        };
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
