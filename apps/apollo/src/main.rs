//! Apollo: Neo's assistant.
//!
//! Apollo reads the folders it is given (pictures, videos, documents)
//! and remembers what each is about, in an encrypted database on this
//! computer. It can be asked things, and answers from what it remembers.
//! The Memory page looks through the database directly: a search by
//! meaning, and a cloud of the words the memories are about. That page
//! asks for the user's login first.
//!
//!     cargo run -p neo-apollo
//!     cargo run -p neo-apollo -- --index        read the folders, without a window
//!     cargo run -p neo-apollo -- --ask "…"      ask it, in the Apollo that is running or a new one
//!     cargo run -p neo-apollo -- --snapshot target/snapshots

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use neo::prelude::*;
use neo::{Image, Point, Rect, Size};
use neo_apollo_core::assistant::{self, Answer};
use neo_apollo_core::index::{self, Progress};
use neo_apollo_core::key;
use neo_apollo_core::model::{Model, Ollama, Role, Turn};
use neo_apollo_core::settings::{Folder, Settings};
use neo_apollo_core::store::{Hit, Kind, Memory, Stats, Store, Word};
use neo_apollo_core::warden::{self, Warning};
use neo_desktop::ui::{nav_item, notice, section, setting, split};
use neo_desktop::{Desktop, DesktopMsg};

mod cloud;
use cloud::{Cloud, Graph, Hub};
mod tray;
use tray::{Tray, TrayAction, TrayState};

/// How many words the cloud shows, and how many memories a list.
const CLOUD_WORDS: usize = 90;
const LISTED: usize = 40;
/// The longest side of a photo's or a video's small picture in the list,
/// in pixels, and the size it is shown at.
const THUMB_SIDE: u32 = 192;
const THUMB: (f32, f32) = (72.0, 48.0);
/// How many small pictures are kept before the oldest listing's are let go.
const THUMBS_KEPT: usize = 400;
/// The pictures under an answer: how large, and how many at most.
const TILE: (f32, f32) = (108.0, 72.0);
const TILES: usize = 6;
/// How many listed-only files a question may look at before answering,
/// and how many a search sends to be looked at behind it.
const LOOK_FOR_ANSWER: usize = 6;
const LOOK_FOR_SEARCH: usize = 12;
/// How many files the Activity page's list goes back.
const LOGGED: usize = 60;
/// The place among the model menus of the one that listens, after the
/// three that answer, see and embed.
const HEARS: usize = 3;
/// How often memory and disk are looked at, how often what stands open
/// is, and how many warnings the Activity page keeps.
const WATCH_EVERY: Duration = Duration::from_secs(60);
const LOOK_OVER_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const WARNINGS_KEPT: usize = 20;
/// The choices for how long the memory stays unlocked unused, in minutes.
const STAY_UNLOCKED: [(u32, &str); 4] = [(5, "5 minutes"), (15, "15 minutes"), (60, "1 hour"), (0, "Until it quits")];
/// How wide a model's menu button is in Sources.
const MODEL_W: f32 = 230.0;
/// How tall the cloud is, and the graph that takes its place.
const CLOUD_H: f32 = 300.0;
/// How many picked words the graph shows at once, and how many words go round each.
const HUBS: usize = 3;
const ROUND_EACH: usize = 12;
/// How often the folders are looked through in full while the app is
/// open. Changes are heard of as they happen; this is for any that were not.
const READ_EVERY: Duration = Duration::from_secs(60 * 60);
/// How long the folders must be quiet before what changed is read, and
/// the longest that changes are gathered for before some are read anyway.
const CHANGES_QUIET: Duration = Duration::from_millis(1500);
const CHANGES_AT_MOST: Duration = Duration::from_secs(20);
/// How soon reading is tried again after being held off for want of memory.
const RETRY_EVERY: Duration = Duration::from_secs(2 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Ask,
    Memory,
    Sources,
    /// What Apollo is doing: how the reading is going, and what it has taken in.
    Activity,
}

/// Whether the model is there to be used.
#[derive(Clone, Debug, PartialEq)]
enum Engine {
    Checking,
    /// The server could not be started or asked.
    Trouble(String),
    /// These models have not been downloaded yet.
    Missing(Vec<String>),
    Pulling {
        model: String,
        status: String,
        done: u64,
        total: u64,
    },
    Ready,
}

/// Something said in the conversation.
#[derive(Clone, Debug, PartialEq)]
struct Said {
    role: Role,
    text: String,
    /// The memories an answer was given to draw on.
    recalled: Vec<Hit>,
    /// The things listed for an answer about a kind or a time.
    listed: Vec<Memory>,
}

/// The memories listed under the cloud, and what they are.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    heading: String,
    /// Each with how near it is to what was asked, if something was.
    items: Vec<(Memory, Option<f32>)>,
}

/// Something done with a file.
#[derive(Clone, Debug, PartialEq)]
struct Logged {
    path: PathBuf,
    /// Looked at, as against listed or read as text.
    looked: bool,
    seconds: f32,
    /// Why it could not be done, if it could not.
    trouble: Option<String>,
}

/// What a reading is of.
enum Reading {
    /// Everything in the folders.
    All,
    /// What the system said changed.
    Changed(Vec<PathBuf>),
    /// Files that are only listed, to be looked at now.
    LookAt(Vec<PathBuf>),
    /// The rest of recordings that go on past their first stretch.
    HearMore,
}

/// Asks the user to prove who they are; an error says why they did not.
type Check = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

struct Apollo {
    desktop: Desktop,
    settings: Settings,
    /// Where settings are kept. Tests and snapshots keep none.
    settings_file: Option<PathBuf>,
    page: Page,
    proxy: Option<Proxy<Msg>>,
    model: Arc<dyn Model>,
    /// The model server, to start and to download models with. Tests have none.
    server: Option<Arc<Ollama>>,
    ready: Engine,
    db: PathBuf,
    key: Option<[u8; 32]>,
    /// The length of the model's embeddings, once it has been asked.
    dims: usize,
    check: Check,

    talk: Vec<Said>,
    draft: String,
    answering: bool,
    stop_answer: Arc<AtomicBool>,
    /// Counts what is said, to keep the newest in view.
    said: u64,

    /// The memory, open for looking through. `None` while it is locked.
    store: Option<Store>,
    unlocking: bool,
    query: String,
    /// The last search, kept so that choosing a kind need not ask the model again.
    asked: Option<(String, Vec<f32>)>,
    searching: bool,
    kind: Option<Kind>,
    /// The words picked, in the order they were: the last is the one whose
    /// memories are listed, and together they are the graph.
    trail: Vec<String>,
    graph: Rc<Vec<Hub>>,
    /// The trails there have been, to step back and forward through, and
    /// which of them is showing. The first is the cloud, with no word picked.
    history: Vec<Vec<String>>,
    at: usize,
    shown: Shown,
    /// Small pictures of the photos and videos listed. `None` for one being
    /// made, or that could not be. Kept only in memory, and only while the
    /// memory is unlocked: on disk they would give away what is encrypted.
    thumbs: HashMap<PathBuf, Option<Image>>,
    cloud: Rc<Vec<Word>>,
    stats: Stats,

    reading: bool,
    progress: Progress,
    stop_reading: Arc<AtomicBool>,
    /// Reading is held off because the computer is short of memory.
    held: bool,
    /// The models as typed in Sources, before they are put to use: the one
    /// that answers, the one that sees, the one that embeds.
    drafts: [String; 3],
    /// The models the servers have, each with what it can do (`completion`,
    /// `vision`, `embedding`; nothing, for a server too old to say) and
    /// whether it is on the other computer.
    installed: Vec<(String, Vec<String>, bool)>,
    /// Whether each of the models as typed is one on the other computer.
    drafts_away: [bool; 3],
    /// The other computer's address as typed, before it is put to use.
    remote_draft: String,
    /// Which model's menu is open, and where it hangs from.
    choosing: Option<(usize, Point)>,
    /// Which model's name is being typed, for one that is not here yet.
    typing: Option<usize>,
    /// The model that writes speech down is being fetched: how far it has got.
    hearing: Option<(u64, u64)>,
    /// What has been done with files lately, newest last, for the Activity page.
    log: std::collections::VecDeque<Logged>,
    /// The file being worked on, since when, and whether it is being looked at.
    working: Option<(PathBuf, std::time::Instant, bool)>,
    /// How many files a question is looking at before it answers: which, of how many.
    looking: Option<(usize, usize)>,
    /// Files a search turned up only listed that were sent to be looked at,
    /// so that none is sent twice.
    asked_after: Vec<PathBuf>,
    /// Apollo's icon in the menu bar, and what its menu last said. Apollo
    /// keeps out of the Dock, so this is where it is found.
    tray: Option<Tray>,
    tray_state: TrayState,
    tray_tried: bool,
    /// The window is showing. Closed with Apollo set to stay ready, it is
    /// only out of sight, and a question brings it back.
    in_sight: bool,
    quit: bool,
    /// When the memory was last used, for locking it again after a while.
    last_used: std::time::Instant,
    /// Unlocking was refused for the question that is waiting, which is
    /// then asked without the memory.
    declined: bool,
    /// What has been warned of lately, newest last, and when each kind of
    /// thing was last said, so that it is not said over and over.
    warnings: Vec<Warning>,
    warned: HashMap<String, std::time::Instant>,
    /// When what stands open was last looked over.
    looked_over: Option<std::time::Instant>,
    /// Tells the user of a warning: through NeoShell, as it runs. False if
    /// there was nobody to tell.
    tell: fn(&Warning) -> bool,
    /// A question from outside the window (the search bar, the command
    /// line) that is waiting for the model to be ready or free.
    waiting_question: Option<String>,
    /// Files the system says have changed, waiting to be read.
    pending: Vec<PathBuf>,
    /// Tells of changes in the folders that are read. Dropped to stop.
    watcher: Option<notify::RecommendedWatcher>,
    /// Why the memory cannot be used with the model now chosen, if it was
    /// made with another.
    stale: Option<String>,
    asking_reset: bool,
    /// Says whether it is. Tests say so themselves.
    short_of_memory: fn() -> bool,
    trouble: Option<String>,
}

#[derive(Clone, Debug)]
enum Msg {
    Page(Page),
    Desktop(DesktopMsg),
    Poll,

    /// What was found of the model, and the length of its embeddings.
    Ready(Engine, usize),
    CheckModel,
    Pull,

    Draft(String),
    Send,
    /// A suggestion was clicked: ask it.
    Suggest(String),
    /// A question from outside the window: the search bar, or another
    /// Apollo started with one.
    Ask(String),
    Piece(String),
    Answered(Result<Answer, String>),
    StopAnswer,
    NewTalk,

    Unlock,
    Unlocked(Result<(), String>),
    Lock,
    Query(String),
    Search,
    Asked(String, Result<Vec<f32>, String>),
    Kind(Option<Kind>),
    Word(String),
    Clear,
    /// One step back through the words picked, or forward again.
    Back,
    Forward,
    Open(PathBuf),
    Reveal(PathBuf),
    Forget(i64),
    /// A small picture of a file has been made, or could not be.
    Thumb(PathBuf, Option<(u32, u32, Vec<u8>)>),

    Read,
    Progress(Progress),
    ReadDone(Progress),
    /// Reading stopped, or did not start, for want of memory.
    Held,
    ModelDraft(usize, String),
    /// Put the models typed to use.
    UseModels,
    Installed(Vec<(String, Vec<String>, bool)>),
    RemoteDraft(String),
    /// Put the other computer's address, as typed, to use.
    UseRemote,
    /// Open the menu of models for one of the three jobs, under its control.
    ChooseModel(usize, Rect),
    CloseChoice,
    /// A model was picked from the menu: put it to use.
    /// A model was picked from the menu: put it to use. The last says
    /// whether it is one on the other computer.
    PickModel(usize, String, bool),
    /// Type the name of one that is not in the menu, for here or for the
    /// other computer.
    OtherModel(usize, bool),
    /// The system says these files or folders have changed.
    Changed(Vec<PathBuf>),
    /// A question is looking at a file before it answers: which, of how many.
    Looking(usize, usize),
    LookAhead(bool),
    /// Fetch the model that writes down what is said.
    GetHearing,
    /// How far that has got: bytes of how many; or how it ended.
    Hearing(Result<Option<(u64, u64)>, String>),
    /// Time to look at how the computer is doing.
    Watch,
    /// What was found worth warning of.
    Warned(Vec<Warning>),
    WarnMe(bool),
    /// Something was chosen from the menu bar icon's menu.
    Tray(TrayAction),
    /// Put the window out of sight, with Apollo still running.
    Hide,
    Show,
    Quit,
    /// Time to see whether the memory has sat unused long enough to lock.
    Tick,
    Background(bool),
    /// How long the memory stays unlocked unused, in minutes; nothing for
    /// until Apollo quits.
    StayUnlocked(u32),
    AskReset,
    CancelReset,
    /// Forget everything and read it all again with the model now chosen.
    Reset,
    StopReading,
    AddFolder,
    FolderOn(usize, bool),
    RemoveFolder(usize),
    RememberTalk(bool),
    ReadAutomatically(bool),
}

/// Has the system tell of changes under `roots`, giving `heard` the paths
/// that changed. Changes come in bursts (a file is written in pieces, a
/// folder is copied in), so they are gathered until it has gone quiet and
/// given together. It goes on until what is returned is dropped, or
/// `heard` returns false. `None` if the system will not tell.
fn listen(roots: &[PathBuf], heard: impl Fn(Vec<PathBuf>) -> bool + Send + 'static) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Being opened or read is not a change.
        if let Ok(event) = event
            && !event.kind.is_access()
        {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    })
    .ok()?;
    for root in roots {
        let _ = watcher.watch(root, notify::RecursiveMode::Recursive);
    }
    // The thread ends when the watcher is dropped, which closes the channel.
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let (mut changed, began) = (vec![first], std::time::Instant::now());
            while began.elapsed() < CHANGES_AT_MOST {
                match rx.recv_timeout(CHANGES_QUIET) {
                    Ok(path) if !changed.contains(&path) => changed.push(path),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            if !heard(changed) {
                break;
            }
        }
    });
    Some(watcher)
}

/// How long before the same thing is warned of again: memory soon, since
/// it is what brings a computer down; what stands open once a day.
fn quiet_for(w: &Warning) -> Duration {
    Duration::from_secs(match w.key.split(':').next().unwrap_or_default() {
        "memory" => 15 * 60,
        "greedy" => 60 * 60,
        "disk" => 6 * 60 * 60,
        _ => 24 * 60 * 60,
    })
}

/// Tells the user of a warning through NeoShell, which shows it at the
/// corner of the screen. A card has room for a line: the first sentence.
fn tell_neoshell(w: &Warning) -> bool {
    let first = w.body.split_inclusive(". ").next().unwrap_or(&w.body).trim();
    let mut note = neo_desktop::notify::Notification::new(format!("Apollo: {}", w.title), first);
    if let Some(file) = &w.file {
        note = note.reveal(file);
    }
    note.send()
}

/// A length of time said roughly: "under a minute", "about 12 minutes", "about 3 hours".
fn how_long(seconds: f32) -> String {
    let minutes = (seconds / 60.0).round() as u32;
    match minutes {
        0 => "under a minute".into(),
        1 => "about a minute".into(),
        2..=89 => format!("about {minutes} minutes"),
        _ => format!("about {} hours", ((minutes as f32) / 60.0).round() as u32),
    }
}

/// A number with its noun: "1 memory", "1,204 memories".
fn count(n: u32, one: &str, many: &str) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped} {}", if n == 1 { one } else { many })
}

/// The start of a text, for a list: one line, no longer than `most`.
fn snippet(text: &str, most: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= most { flat } else { format!("{}…", flat.chars().take(most - 1).collect::<String>().trim_end()) }
}

fn kind_icon(kind: Kind) -> neo::theme::Icon {
    match kind {
        Kind::Photo => icons::IMAGE,
        Kind::Video => icons::FILM,
        Kind::Document => icons::FILE_TEXT,
        Kind::Conversation => icons::MESSAGES_SQUARE,
        Kind::Note => icons::STICKY_NOTE,
        Kind::Folder => icons::FOLDER,
        Kind::Audio => icons::AUDIO_LINES,
    }
}

/// A path with the home folder written as `~`.
fn short_path(path: &Path) -> String {
    match path.strip_prefix(neo_desktop::fs::home_dir()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

impl Apollo {
    /// Apollo as it runs: the model on this computer, the database in
    /// Neo's settings folder, and the system's own check of who is asking.
    fn new() -> Self {
        let settings = Settings::load();
        let server = Arc::new(Ollama::from_settings(&settings));
        let mut app = Self::with(server.clone(), neo_apollo_core::dir().join("memory.db"), None, Arc::new(key::authenticate));
        app.settings = settings;
        app.drafts = app.chosen();
        neo_apollo_core::hear::use_listener(&app.settings.hear_model);
        app.drafts_away = app.away();
        app.remote_draft = app.settings.remote.clone();
        app.settings_file = Some(Settings::file());
        app.server = Some(server);
        app.short_of_memory = neo_apollo_core::pressure::short_of_memory;
        app.tell = tell_neoshell;
        app.ready = Engine::Checking;
        app
    }

    /// Apollo with a model, a database and a check of the caller's choosing.
    fn with(model: Arc<dyn Model>, db: PathBuf, key: Option<[u8; 32]>, check: Check) -> Self {
        let dims = if key.is_some() { model.dims().unwrap_or(0) } else { 0 };
        Self {
            desktop: Desktop::load(),
            settings: Settings { folders: vec![], ..Settings::default() },
            settings_file: None,
            page: Page::Ask,
            proxy: None,
            model,
            server: None,
            ready: Engine::Ready,
            db,
            key,
            dims,
            check,
            talk: vec![],
            draft: String::new(),
            answering: false,
            stop_answer: Arc::default(),
            said: 0,
            store: None,
            unlocking: false,
            query: String::new(),
            asked: None,
            searching: false,
            kind: None,
            trail: vec![],
            graph: Rc::default(),
            history: vec![vec![]],
            at: 0,
            shown: Shown::default(),
            thumbs: HashMap::new(),
            cloud: Rc::default(),
            stats: Stats::default(),
            reading: false,
            progress: Progress::default(),
            stop_reading: Arc::default(),
            held: false,
            drafts: Default::default(),
            installed: vec![],
            drafts_away: [false; 3],
            remote_draft: String::new(),
            choosing: None,
            typing: None,
            tray: None,
            tray_state: TrayState::default(),
            tray_tried: false,
            in_sight: true,
            quit: false,
            last_used: std::time::Instant::now(),
            declined: false,
            warnings: vec![],
            warned: HashMap::new(),
            looked_over: None,
            tell: |_| false,
            waiting_question: None,
            pending: vec![],
            hearing: None,
            log: Default::default(),
            working: None,
            looking: None,
            asked_after: vec![],
            watcher: None,
            stale: None,
            asking_reset: false,
            short_of_memory: || false,
            trouble: None,
        }
    }

    /// Does `job` off the main thread, where there is one to come back
    /// to; in tests, here and now. What it sends comes back as messages.
    fn work(&mut self, job: impl FnOnce(&mut dyn FnMut(Msg)) + Send + 'static) {
        match self.proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    job(&mut |m| {
                        proxy.send(m);
                    })
                });
            }
            None => {
                let mut sent = vec![];
                job(&mut |m| sent.push(m));
                for m in sent {
                    self.update(m);
                }
            }
        }
    }

    /// What a thread needs to open the memory for itself.
    fn memory(&self) -> Option<(PathBuf, [u8; 32])> {
        (self.ready == Engine::Ready && self.dims > 0 && self.stale.is_none()).then_some(())?;
        Some((self.db.clone(), self.key?))
    }

    /// The models in use: the one that answers, the one that sees, the one that embeds.
    fn chosen(&self) -> [String; 3] {
        [self.settings.chat_model.clone(), self.settings.vision_model.clone(), self.settings.embed_model.clone()]
    }

    /// Whether each job is done on the other computer.
    fn away(&self) -> [bool; 3] {
        let there = !self.settings.remote.is_empty();
        [self.settings.chat_remote && there, self.settings.vision_remote && there, self.settings.embed_remote && there]
    }

    /// The other computer by its name alone, as it is said in menus: the
    /// address without its scheme and port.
    fn remote_name(&self) -> String {
        let address = self.settings.remote.split_once("://").map_or(self.settings.remote.as_str(), |(_, rest)| rest);
        address.split([':', '/']).next().unwrap_or(address).to_owned()
    }

    /// Puts the models typed in Sources to use, each where it was said to be.
    fn use_models(&mut self) {
        let typed = self.drafts.clone().map(|m| m.trim().to_owned());
        let there = !self.settings.remote.is_empty();
        let away = self.drafts_away.map(|a| a && there);
        if typed.iter().any(String::is_empty) || (typed == self.chosen() && away == self.away()) {
            return;
        }
        [self.settings.chat_model, self.settings.vision_model, self.settings.embed_model] = typed.clone();
        [self.settings.chat_remote, self.settings.vision_remote, self.settings.embed_remote] = away;
        (self.drafts, self.drafts_away) = (typed, away);
        self.save_settings();
        self.serve_anew();
    }

    /// Puts the other computer's address to use. Taken away, every job
    /// comes back to this computer.
    fn use_remote(&mut self) {
        let address = self.remote_draft.trim().trim_end_matches('/').to_owned();
        if address == self.settings.remote {
            return;
        }
        self.settings.remote = address;
        if self.settings.remote.is_empty() {
            (self.settings.chat_remote, self.settings.vision_remote, self.settings.embed_remote) = (false, false, false);
            self.drafts_away = [false; 3];
        }
        self.remote_draft = self.settings.remote.clone();
        self.save_settings();
        self.serve_anew();
    }

    /// Takes up a change in how long the models are kept, which needs no
    /// checking of them again and no locking of the memory.
    fn serve_anew_quietly(&mut self) {
        if self.server.is_some() {
            let server = Arc::new(Ollama::from_settings(&self.settings));
            (self.server, self.model) = (Some(server.clone()), server.clone());
            if self.settings.background && self.ready == Engine::Ready {
                self.work(move |_| server.warm());
            } else if !self.settings.background {
                // Not kept any longer: they go when they have sat idle a few minutes.
                self.work(move |_| server.rest_all());
            }
        }
    }

    /// Takes up the models and servers as they are now set.
    fn serve_anew(&mut self) {
        // Tests have a model of their own, and no server to ask for another.
        if self.server.is_some() {
            let server = Arc::new(Ollama::from_settings(&self.settings));
            (self.server, self.model) = (Some(server.clone()), server);
            // Another model may make embeddings of another length, or ones that
            // cannot be compared with those kept: found out when it is opened.
            self.dims = 0;
            self.stale = None;
            self.lock();
            self.check_model();
        }
    }

    fn reset(&mut self) {
        self.asking_reset = false;
        self.stop_reading.store(true, Ordering::Relaxed);
        self.lock();
        match Store::delete(&self.db) {
            Ok(()) => {
                self.stale = None;
                self.progress = Progress::default();
                self.read();
            }
            Err(e) => self.trouble = Some(format!("The memory could not be removed: {e}")),
        }
    }

    fn save_settings(&mut self) {
        if let Some(path) = &self.settings_file
            && let Err(e) = self.settings.save_to(path)
        {
            self.trouble = Some(format!("Couldn't save Apollo's settings: {e}"));
        }
    }

    /// Finds out whether the model is there, starting its server if not.
    fn check_model(&mut self) {
        let Some(server) = self.server.clone() else { return };
        self.ready = Engine::Checking;
        self.work(move |send| {
            let found = server.start().and_then(|()| server.missing()).and_then(|missing| {
                if !missing.is_empty() {
                    return Ok((Engine::Missing(missing), 0));
                }
                // There, but can each do what it was chosen for?
                match server.unfit() {
                    Some(why) => Ok((Engine::Trouble(why), 0)),
                    None => server.dims().map(|d| (Engine::Ready, d)),
                }
            });
            // What each server has, for the menus: this computer's, then the other's.
            let mut have = vec![];
            for away in [false, true] {
                if away && server.remote().is_none() {
                    continue;
                }
                // Each once, and without the server's own entries for its runners.
                let mut names: Vec<String> = vec![];
                for model in server.models_at(away).unwrap_or_default() {
                    if !model.starts_with("llamacpp:") && !names.contains(&model) {
                        names.push(model);
                    }
                }
                for model in names {
                    let can = server.capabilities_at(&model, away).unwrap_or_default();
                    have.push((model, can, away));
                }
            }
            send(Msg::Installed(have));
            let (ready, dims) = found.unwrap_or_else(|e| (Engine::Trouble(e), 0));
            send(Msg::Ready(ready, dims));
        });
    }

    fn pull(&mut self) {
        let (Some(server), Engine::Missing(models)) = (self.server.clone(), self.ready.clone()) else { return };
        self.ready = Engine::Pulling { model: models[0].clone(), status: "Starting".into(), done: 0, total: 0 };
        self.work(move |send| {
            for model in &models {
                let mut last = std::time::Instant::now();
                let pulled = server.pull(model, &mut |p| {
                    // Often enough to see it move, not so often as to do nothing else.
                    if last.elapsed() > Duration::from_millis(200) {
                        last = std::time::Instant::now();
                        send(Msg::Ready(Engine::Pulling { model: model.clone(), status: p.status, done: p.done, total: p.total }, 0));
                    }
                    true
                });
                if let Err(e) = pulled {
                    return send(Msg::Ready(Engine::Trouble(format!("{model} could not be downloaded: {e}")), 0));
                }
            }
            send(Msg::CheckModel);
        });
    }

    /// Takes in warnings: each is said, and kept for the Activity page,
    /// unless the same thing was said too lately to say again.
    fn warn(&mut self, found: Vec<Warning>) {
        if !self.settings.warn {
            return;
        }
        let now = std::time::Instant::now();
        for w in found {
            if self.warned.get(&w.key).is_some_and(|last| now.duration_since(*last) < quiet_for(&w)) {
                continue;
            }
            self.warned.insert(w.key.clone(), now);
            // Short of memory, Apollo gives back what it holds; it is a second or two in coming back.
            if w.key == "memory" && self.server.is_some() && !self.reading && !self.answering {
                let model = self.model.clone();
                self.work(move |_| model.rest_all());
            }
            (self.tell)(&w);
            self.warnings.push(w);
            if self.warnings.len() > WARNINGS_KEPT {
                self.warnings.remove(0);
            }
        }
    }

    /// Looks at how the computer is doing, off the main thread: memory
    /// and disk every time, and what stands open now and then.
    fn keep_watch(&mut self) {
        if !self.settings.warn {
            return;
        }
        let over = self.looked_over.is_none_or(|t| t.elapsed() >= LOOK_OVER_EVERY);
        if over {
            self.looked_over = Some(std::time::Instant::now());
        }
        let (remote, there) = (self.settings.remote.clone(), self.away().contains(&true));
        self.work(move |send| {
            let mut found = warden::resources();
            if over {
                found.extend(warden::security(&remote, there));
            }
            if !found.is_empty() {
                send(Msg::Warned(found));
            }
        });
    }

    /// Brings Apollo's window in front of whatever else is open. Keeping
    /// out of the Dock, Apollo is not brought forward by the system on
    /// its own: not when its window is shown again, and not when the
    /// login check has been and gone and left another app in front.
    fn come_forward(&mut self) {
        self.in_sight = true;
        if cfg!(target_os = "macos") && !cfg!(test) {
            let _ = std::process::Command::new("open").args(["-b", "org.neo.Apollo"]).spawn();
        }
    }

    /// Asks the question that came from outside the window, once the
    /// model is ready and not in the middle of another. Until then it
    /// shows in the box, so that it is plain it was heard.
    fn ask_what_waits(&mut self) {
        let Some(question) = self.waiting_question.clone() else { return };
        self.draft = question;
        if self.ready != Engine::Ready || self.answering || self.unlocking {
            return;
        }
        // Locked, the memory is asked for first, so that the answer can be
        // from it: the check comes up once, and not again while it stays
        // unlocked. Refused, the question is asked all the same.
        if self.store.is_none() && self.key.is_some() && !self.declined {
            return self.unlock();
        }
        (self.waiting_question, self.declined) = (None, false);
        self.send();
    }

    fn send(&mut self) {
        let question = self.draft.trim().to_owned();
        if question.is_empty() || self.answering || self.ready != Engine::Ready {
            return;
        }
        let earlier: Vec<Turn> = self.talk.iter().map(|s| Turn::new(s.role, s.text.clone())).collect();
        self.talk.push(Said { role: Role::User, text: question.clone(), recalled: vec![], listed: vec![] });
        self.talk.push(Said { role: Role::Assistant, text: String::new(), recalled: vec![], listed: vec![] });
        self.draft.clear();
        self.said += 1;
        self.answering = true;
        self.last_used = std::time::Instant::now();
        self.stop_answer = Arc::default();
        let (model, memory, stop) = (self.model.clone(), self.memory(), self.stop_answer.clone());
        // Apollo draws on the memory only once it has been unlocked. What
        // it is told and asked is written down either way.
        let (open, keep) = (self.store.is_some(), self.settings.remember_conversations);
        let aware = assistant::Aware { reading: (self.reading && self.progress.total > 0).then_some((self.progress.done, self.progress.total)), look: LOOK_FOR_ANSWER };
        self.work(move |send| {
            let mut store = memory.and_then(|(db, key)| Store::open_for(&db, &key, &*model).ok());
            if let Some(note) = assistant::told_to_remember(&question) {
                let kept = store.as_mut().ok_or_else(|| "The memory could not be opened.".to_owned()).and_then(|s| assistant::keep_note(s, &*model, note));
                return send(Msg::Answered(kept.map(|_| Answer { text: "I'll remember that.".into(), recalled: vec![], listed: vec![] })));
            }
            // Two things are heard from it as it goes, through the one way out.
            let out = std::cell::RefCell::new(&mut *send);
            let answer = assistant::ask(
                store.as_mut().filter(|_| open),
                &*model,
                &earlier,
                &question,
                &aware,
                &mut |i, n| {
                    (out.borrow_mut())(Msg::Looking(i, n));
                    !stop.load(Ordering::Relaxed)
                },
                &mut |piece| {
                    (out.borrow_mut())(Msg::Piece(piece.to_owned()));
                    !stop.load(Ordering::Relaxed)
                },
            );
            if let (Ok(a), true, Some(store)) = (&answer, keep, store.as_mut())
                && !a.text.is_empty()
            {
                let _ = assistant::keep_exchange(store, &*model, &question, &a.text);
            }
            send(Msg::Answered(answer));
        });
    }

    fn unlock(&mut self) {
        if self.store.is_some() || self.unlocking {
            return;
        }
        self.unlocking = true;
        self.trouble = None;
        let check = self.check.clone();
        self.work(move |send| send(Msg::Unlocked(check("look through Apollo's memory"))));
    }

    fn open_memory(&mut self) {
        let Some(key) = self.key else {
            self.trouble = Some("The key to Apollo's memory could not be read.".into());
            return;
        };
        // Before the model has been asked, the database says what it was made for.
        let opened = if self.ready == Engine::Ready && self.dims > 0 { Store::open_for(&self.db, &key, &*self.model) } else { Store::made_for(&self.db, &key).ok_or_else(|| "There is nothing to look through until the model is ready.".to_owned()).and_then(|dims| Store::open(&self.db, &key, dims)) };
        match opened {
            Ok(store) => {
                self.store = Some(store);
                self.show();
            }
            Err(e) if e.starts_with(neo_apollo_core::store::OTHER_MODEL) => self.stale = Some(e),
            Err(e) => self.trouble = Some(e),
        }
    }

    fn lock(&mut self) {
        self.store = None;
        (self.shown, self.cloud, self.graph, self.stats, self.asked) = (Shown::default(), Rc::default(), Rc::default(), Stats::default(), None);
        self.trail.clear();
        (self.history, self.at) = (vec![vec![]], 0);
        self.thumbs.clear();
        self.asked_after.clear();
        self.query.clear();
    }

    /// Moves to a trail of picked words, noting it as a step that can be
    /// gone back to. Whatever was ahead, after stepping back, is dropped.
    fn go(&mut self, trail: Vec<String>) {
        if trail != self.trail {
            self.history.truncate(self.at + 1);
            self.history.push(trail.clone());
            // Far more steps than anyone goes back through.
            if self.history.len() > 200 {
                self.history.remove(0);
            }
            self.at = self.history.len() - 1;
        }
        self.trail = trail;
    }

    /// The words folded into `word` in the cloud.
    fn also(&self, word: &str) -> Vec<String> {
        self.cloud.iter().find(|w| w.word == word).map(|w| w.also.clone()).unwrap_or_default()
    }

    /// The memories to list for whatever is being looked at.
    fn listed(&self, store: &Store) -> Result<Shown, String> {
        if let Some(word) = self.trail.last() {
            // What is about the word, and the words folded into it, and
            // then what is near it in meaning without saying so.
            let mut items: Vec<(Memory, Option<f32>)> = vec![];
            for w in std::iter::once(word).chain(&self.also(word)) {
                for m in store.about(w, LISTED)? {
                    if self.kind.is_none_or(|k| k == m.kind) && !items.iter().any(|(have, _)| have.id == m.id) {
                        items.push((m, None));
                    }
                }
            }
            if let Some(vector) = store.word_vector(word)? {
                for hit in store.search(&vector, LISTED, self.kind)? {
                    if hit.distance < assistant::NEAR && !items.iter().any(|(have, _)| have.id == hit.memory.id) {
                        items.push((hit.memory, Some(hit.distance)));
                    }
                }
            }
            items.truncate(LISTED);
            return Ok(Shown { heading: format!("About “{word}”"), items });
        }
        if let Some((question, vector)) = &self.asked {
            let items = store.search_for(vector, &neo_apollo_core::words::terms(question), LISTED, self.kind)?.into_iter().map(|h| (h.memory, Some(h.distance))).collect();
            return Ok(Shown { heading: format!("Nearest to “{question}”"), items });
        }
        Ok(Shown { heading: "Newest".into(), items: store.recent(LISTED, self.kind)?.into_iter().map(|m| (m, None)).collect() })
    }

    /// The last few picked words, each with the words that go with it.
    fn hubs(&self, store: &Store) -> Result<Vec<Hub>, String> {
        let shown = &self.trail[self.trail.len().saturating_sub(HUBS)..];
        // Fewer round each when there are more of them to fit.
        let each = ROUND_EACH - (shown.len().saturating_sub(1)) * 2;
        shown
            .iter()
            .map(|word| {
                let related = store.related(word, &self.also(word), each, self.kind)?;
                let most = related.iter().map(|r| r.shared).max().unwrap_or(0).max(1) as f32;
                // One that only means something near counts for less than any that shares a memory.
                Ok(Hub { word: word.clone(), related: related.into_iter().map(|r| (r.word, if r.shared > 0 { 0.25 + 0.75 * r.shared as f32 / most } else { 0.1 })).collect() })
            })
            .collect()
    }

    /// Fills the Memory page from the database: the counts, the cloud,
    /// the graph of picked words, and the list.
    fn show(&mut self) {
        let Some(store) = self.store.take() else { return };
        let filled = (|| -> Result<(), String> {
            self.stats = store.stats()?;
            self.cloud = Rc::new(store.cloud(CLOUD_WORDS, self.kind)?);
            self.graph = Rc::new(self.hubs(&store)?);
            self.shown = self.listed(&store)?;
            Ok(())
        })();
        if let Err(e) = filled {
            self.trouble = Some(e);
        }
        self.store = Some(store);
        self.picture_the_list();
        self.look_into_the_list();
    }

    /// Has small pictures made of the photos and videos listed that have
    /// none yet, off the main thread.
    fn picture_the_list(&mut self) {
        if self.thumbs.len() > THUMBS_KEPT {
            let listed: Vec<&PathBuf> = self.shown.items.iter().filter_map(|(m, _)| m.source.as_ref()).collect();
            self.thumbs.retain(|path, _| listed.contains(&path));
        }
        let wanted = self.shown.items.iter().filter(|(m, _)| matches!(m.kind, Kind::Photo | Kind::Video)).filter_map(|(m, _)| m.source.clone()).collect();
        self.picture(wanted);
    }

    /// Has small pictures made of those of `files` that have none yet, off
    /// the main thread.
    fn picture(&mut self, files: Vec<PathBuf>) {
        let mut wanted: Vec<PathBuf> = vec![];
        for path in files {
            if !self.thumbs.contains_key(&path) && !wanted.contains(&path) {
                wanted.push(path);
            }
        }
        if wanted.is_empty() {
            return;
        }
        for path in &wanted {
            self.thumbs.insert(path.clone(), None);
        }
        self.work(move |send| {
            for path in wanted {
                let pixels = index::thumbnail(&path, THUMB_SIDE);
                send(Msg::Thumb(path, pixels));
            }
        });
    }

    fn search(&mut self) {
        let question = self.query.trim().to_owned();
        if question.is_empty() {
            return self.update(Msg::Clear);
        }
        if self.store.is_none() || self.searching {
            return;
        }
        self.searching = true;
        self.last_used = std::time::Instant::now();
        let model = self.model.clone();
        self.work(move |send| {
            let vector = model.embed(std::slice::from_ref(&question), true).and_then(|mut v| v.pop().ok_or_else(|| "no embedding".to_owned()));
            send(Msg::Asked(question, vector));
        });
    }

    /// Looks through all the folders for what is new, changed or gone.
    fn read(&mut self) {
        self.read_some(Reading::All);
    }

    /// Reads what the system has said changed, if nothing stands in the way.
    /// What it is not the time for waits: the next full look finds it.
    fn catch_up(&mut self) {
        if !self.pending.is_empty() && !self.reading && self.settings.read_automatically && self.memory().is_some() {
            let changed = std::mem::take(&mut self.pending);
            self.read_some(Reading::Changed(changed));
        }
    }

    /// Sends the photos and videos in the list that are only listed to be
    /// looked at, so that a search by what they show can find them; the
    /// list is filled again as they come in.
    fn look_into_the_list(&mut self) {
        if self.reading || (self.trail.is_empty() && self.asked.is_none()) {
            return;
        }
        let files: Vec<PathBuf> = self.shown.items.iter().filter(|(m, _)| m.light).filter_map(|(m, _)| m.source.clone()).filter(|p| !self.asked_after.contains(p)).take(LOOK_FOR_SEARCH).collect();
        if !files.is_empty() {
            self.asked_after.extend(files.iter().cloned());
            self.read_some(Reading::LookAt(files));
        }
    }

    /// Sets runners to listening to the rest of recordings that go on
    /// past their first stretch, if there are any and nothing else is
    /// being read. They take up again from where they stopped.
    fn hear_the_rest(&mut self) {
        if self.reading || !self.settings.read_automatically {
            return;
        }
        let Some((db, key)) = self.memory() else { return };
        // Asked of a store of its own, as this may be while the memory is locked.
        let waiting = Store::open_for(&db, &key, &*self.model).and_then(|s| s.unheard_count()).unwrap_or(0);
        if waiting > 0 {
            self.read_some(Reading::HearMore);
        }
    }

    /// Brings the memory up to date: with everything in the folders, with
    /// what changed, or by looking at some files that are only listed.
    fn read_some(&mut self, what: Reading) {
        let Some((db, key)) = self.memory() else { return };
        if self.reading {
            return;
        }
        // A model of a few gigabytes is not set to work on a computer with none to spare.
        self.held = (self.short_of_memory)();
        if self.held {
            return;
        }
        // A full look finds whatever changes were waiting.
        if matches!(what, Reading::All) {
            self.pending.clear();
        }
        self.reading = true;
        self.stop_reading = Arc::default();
        self.progress = Progress::default();
        let (model, roots, stop, short, ahead) = (self.model.clone(), self.settings.roots(), self.stop_reading.clone(), self.short_of_memory, self.settings.look_ahead);
        self.work(move |send| {
            let mut report = |p: &Progress| {
                send(Msg::Progress(p.clone()));
                // Between files is where it can stop without losing anything.
                if short() {
                    send(Msg::Held);
                    return false;
                }
                !stop.load(Ordering::Relaxed)
            };
            let done = match (Store::open_for(&db, &key, &*model), what) {
                (Ok(mut store), Reading::All) => index::run(&mut store, &*model, &roots, ahead, &mut report),
                (Ok(mut store), Reading::Changed(changed)) => index::update(&mut store, &*model, &roots, &changed, ahead, &mut report),
                (Ok(mut store), Reading::LookAt(files)) => index::look_at_files(&mut store, &*model, files, &mut report),
                (Ok(mut store), Reading::HearMore) => index::hear_more(&mut store, &*model, neo_apollo_core::hear::listener().runners(), &mut report),
                (Err(e), _) => Progress { trouble: Some(e), ..Progress::default() },
            };
            // Done for now: give the memory of the model that sees back.
            model.rest();
            send(Msg::ReadDone(done));
        });
    }

    /// Notes what was done with the file that was being worked on, now
    /// that the next has begun or the reading is over.
    fn log_done(&mut self, failed_before: usize, p: &Progress) {
        if let Some((path, since, looked)) = self.working.take() {
            let trouble = (p.failed > failed_before).then(|| p.trouble.clone().unwrap_or_default());
            self.log.push_back(Logged { path, looked, seconds: since.elapsed().as_secs_f32(), trouble });
            while self.log.len() > LOGGED {
                self.log.pop_front();
            }
        }
    }

    /// About how long looking at one file takes, from those looked at lately.
    fn seconds_a_look(&self) -> Option<f32> {
        let looks: Vec<f32> = self.log.iter().filter(|l| l.looked && l.trouble.is_none()).map(|l| l.seconds).collect();
        (!looks.is_empty()).then(|| looks.iter().sum::<f32>() / looks.len() as f32)
    }

    /// Has the system tell of changes in the folders that are read, so
    /// that a new screenshot is remembered in moments and without looking
    /// through everything else. Asked again whenever the folders change.
    fn watch(&mut self) {
        self.watcher = None;
        if let Some(proxy) = self.proxy.clone().filter(|_| self.settings.read_automatically) {
            self.watcher = listen(&self.settings.roots(), move |changed| proxy.send(Msg::Changed(changed)));
        }
    }

    /// One line on what Apollo is doing, for the sidebar.
    fn status(&self) -> String {
        match &self.ready {
            Engine::Checking => "Starting the model…".into(),
            Engine::Trouble(_) => "The model is not running".into(),
            Engine::Missing(_) => "The model needs downloading".into(),
            Engine::Pulling { .. } => "Downloading the model…".into(),
            Engine::Ready if self.reading && self.progress.listening => format!("Listening to the rest of {}", count(self.progress.total.saturating_sub(self.progress.done).max(1) as u32, "recording", "recordings")),
            Engine::Ready if self.reading && self.progress.total > 0 => format!("{} {} of {}", if self.progress.looking { "Looking at" } else { "Listing" }, (self.progress.done + 1).min(self.progress.total), self.progress.total),
            Engine::Ready if self.reading => "Looking through folders…".into(),
            Engine::Ready if self.stale.is_some() => "Memory is another model's".into(),
            Engine::Ready if self.held => "Paused: memory is short".into(),
            Engine::Ready => "Up to date".into(),
        }
    }
}

impl App for Apollo {
    type Message = Msg;

    fn title(&self) -> String {
        match self.page {
            Page::Ask => "Apollo".into(),
            Page::Memory => "Memory · Apollo".into(),
            Page::Sources => "Sources · Apollo".into(),
            Page::Activity => "Activity · Apollo".into(),
        }
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1080.0, 720.0), min_size: Some(Size::new(760.0, 480.0)), app_id: Some("org.neo.Apollo".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn window_state(&self) -> WindowState {
        WindowState { visible: self.in_sight, ..WindowState::default() }
    }

    /// Set to stay ready, closing the window only puts it out of sight:
    /// Apollo goes on answering the search bar, and Quit ends it.
    fn on_close(&self) -> Option<Msg> {
        self.settings.background.then_some(Msg::Hide)
    }

    /// Its icon in the Dock, clicked while it waits out of sight, brings the window back.
    fn on_reopen(&self) -> Option<Msg> {
        Some(Msg::Show)
    }

    fn should_exit(&self) -> bool {
        self.quit
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn menus(&self) -> Vec<Menu<Msg>> {
        let ready = self.ready == Engine::Ready;
        vec![
            Menu::new("File")
                .push(MenuEntry::new("New Conversation", Msg::NewTalk).shortcut(Shortcut::command("n")).enabled(!self.talk.is_empty() && !self.answering))
                .separator()
                .push(MenuEntry::new("Add Folder…", Msg::AddFolder))
                .push(MenuEntry::new("Read Folders Now", Msg::Read).shortcut(Shortcut::command("r")).enabled(ready && !self.reading))
                .separator()
                .push(MenuEntry::new("Quit Apollo", Msg::Quit)),
            Menu::new("View").push(MenuEntry::new("Ask", Msg::Page(Page::Ask)).shortcut(Shortcut::command("1"))).push(MenuEntry::new("Memory", Msg::Page(Page::Memory)).shortcut(Shortcut::command("2"))).push(MenuEntry::new("Sources", Msg::Page(Page::Sources)).shortcut(Shortcut::command("3"))).push(MenuEntry::new("Activity", Msg::Page(Page::Activity)).shortcut(Shortcut::command("4"))),
            Menu::new("Memory")
                .push(MenuEntry::new("Unlock…", Msg::Unlock).enabled(self.store.is_none() && !self.unlocking))
                .push(MenuEntry::new("Lock", Msg::Lock).shortcut(Shortcut::command("l")).enabled(self.store.is_some()))
                .separator()
                .push(MenuEntry::new("Back", Msg::Back).shortcut(Shortcut::command("[")).enabled(self.at > 0))
                .push(MenuEntry::new("Forward", Msg::Forward).shortcut(Shortcut::command("]")).enabled(self.at + 1 < self.history.len())),
        ]
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        // Tests do their work in place, where what comes of it can be looked at.
        if !cfg!(test) {
            self.proxy = Some(proxy);
        }
        if self.key.is_none() {
            match key::database_key() {
                Ok(key) => self.key = Some(key),
                Err(e) => self.trouble = Some(e),
            }
        }
        self.check_model();
        self.watch();
        if self.proxy.is_some() {
            self.keep_watch();
            keep_at_startup(self.settings.background);
        }
        // Questions from the search bar, and from an Apollo started with one
        // while this is running.
        if let Some(proxy) = self.proxy.clone() {
            std::thread::spawn(move || {
                let Ok(questions) = neo_desktop::apollo::Questions::open() else { return };
                while let Ok(asked) = questions.next() {
                    let heard = match asked {
                        neo_desktop::apollo::Asked::Question(question) if !question.trim().is_empty() => Msg::Ask(question),
                        _ => Msg::Show,
                    };
                    if !proxy.send(heard) {
                        break;
                    }
                }
            });
        }
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        if self.settings.warn {
            subs.push(Subscription::every(WATCH_EVERY, Msg::Watch));
        }
        if self.store.is_some() && self.settings.stay_unlocked > 0 {
            subs.push(Subscription::every(Duration::from_secs(20), Msg::Tick));
        }
        if self.ready == Engine::Ready && !self.reading && self.settings.read_automatically {
            subs.push(Subscription::every(if self.held { RETRY_EVERY } else { READ_EVERY }, Msg::Read));
        }
        subs
    }

    fn on_exit(&mut self) {
        // Whatever is under way stops at the next file or the next word.
        self.stop_reading.store(true, Ordering::Relaxed);
        self.stop_answer.store(true, Ordering::Relaxed);
        // And the model's few gigabytes are given back at once.
        if !cfg!(test) {
            self.model.rest_all();
        }
    }

    fn view(&self) -> Element<Msg> {
        self.whole_view()
    }

    fn update(&mut self, m: Msg) {
        self.apply(m);
        // The icon goes into the menu bar once the event loop is running,
        // which the first message shows, and its menu is kept in step.
        if !self.tray_tried && self.proxy.is_some() {
            self.tray_tried = true;
            self.add_tray();
        }
        let state = self.tray_now();
        if state != self.tray_state {
            if let Some(tray) = &self.tray {
                tray.update(&state);
            }
            self.tray_state = state;
        }
    }
}

impl Apollo {
    fn tray_now(&self) -> TrayState {
        TrayState { status: self.status(), unlocked: self.store.is_some(), can_read: self.ready == Engine::Ready && !self.reading }
    }

    /// Adds the menu bar icon. No icon is not fatal: the search bar and
    /// opening Apollo again still bring the window.
    fn add_tray(&mut self) {
        let Some(proxy) = self.proxy.clone() else { return };
        self.tray_state = self.tray_now();
        match Tray::new(&self.tray_state, move |action| {
            proxy.send(Msg::Tray(action));
        }) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("neo-apollo: no menu bar icon: {e}"),
        }
    }

    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Page(page) => self.page = page,
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }

            Msg::Ready(ready, dims) => {
                let first = self.ready != Engine::Ready && ready == Engine::Ready;
                self.ready = ready;
                if dims > 0 {
                    self.dims = dims;
                }
                // The model is there: see what is new in the folders.
                if first {
                    // Ahead of the first question, so that it does not wait on a model coming in.
                    if self.settings.background && self.server.is_some() {
                        let model = self.model.clone();
                        self.work(move |_| model.warm());
                    }
                    self.ask_what_waits();
                }
                if first && self.settings.read_automatically {
                    self.read();
                }
            }
            Msg::CheckModel => self.check_model(),
            Msg::Pull => self.pull(),

            Msg::Draft(text) => self.draft = text,
            Msg::Send => self.send(),
            Msg::Suggest(text) => {
                self.draft = text;
                self.send();
            }
            Msg::Ask(question) => {
                self.in_sight = true;
                self.page = Page::Ask;
                self.waiting_question = Some(question);
                self.ask_what_waits();
            }
            Msg::Piece(piece) => {
                self.looking = None;
                if let Some(last) = self.talk.last_mut().filter(|s| s.role == Role::Assistant && self.answering) {
                    last.text.push_str(&piece);
                    self.said += 1;
                }
            }
            Msg::Answered(answer) => {
                if !self.answering {
                    return;
                }
                self.answering = false;
                self.looking = None;
                self.last_used = std::time::Instant::now();
                self.said += 1;
                // One that came in the meantime is asked when this is dealt with.
                let waits = self.waiting_question.is_some();
                let Some(last) = self.talk.last_mut() else { return };
                match answer {
                    Ok(a) => {
                        // Pictures of the photos and videos it drew on, to show beneath it.
                        let files = if self.store.is_some() { assistant::sources(&a.listed, &a.recalled).into_iter().filter(|m| matches!(m.kind, Kind::Photo | Kind::Video)).filter_map(|m| m.source.clone()).take(TILES).collect() } else { vec![] };
                        (last.text, last.recalled, last.listed) = (a.text, a.recalled, a.listed);
                        self.picture(files);
                    }
                    Err(e) => {
                        // What could not be answered is put back to be asked again.
                        self.talk.pop();
                        if let Some(asked) = self.talk.pop() {
                            self.draft = asked.text;
                        }
                        self.trouble = Some(e);
                    }
                }
                self.show();
                if waits {
                    self.ask_what_waits();
                }
            }
            Msg::StopAnswer => self.stop_answer.store(true, Ordering::Relaxed),
            Msg::NewTalk => {
                if !self.answering {
                    self.talk.clear();
                    self.page = Page::Ask;
                }
            }

            Msg::Unlock => self.unlock(),
            Msg::Unlocked(outcome) => {
                self.unlocking = false;
                match outcome {
                    Ok(()) => {
                        self.last_used = std::time::Instant::now();
                        self.open_memory();
                        // Checked, but the memory would not open: not asked for again for this question.
                        self.declined = self.store.is_none();
                    }
                    Err(e) => {
                        self.trouble = Some(e);
                        self.declined = true;
                    }
                }
                // The check came up over everything and has gone; the answer is here.
                if self.waiting_question.is_some() {
                    self.come_forward();
                }
                self.ask_what_waits();
            }
            Msg::Lock => self.lock(),
            Msg::Query(text) => self.query = text,
            Msg::Search => self.search(),
            Msg::Asked(question, vector) => {
                self.searching = false;
                match vector {
                    Ok(vector) => {
                        self.asked = Some((question, vector));
                        self.go(vec![]);
                        self.show();
                    }
                    Err(e) => self.trouble = Some(e),
                }
            }
            Msg::Kind(kind) => {
                self.kind = kind;
                self.show();
            }
            Msg::Word(word) => {
                // A picked word again goes back to it; a new one is picked too.
                let mut trail = self.trail.clone();
                match trail.iter().position(|w| *w == word) {
                    Some(i) => trail.truncate(i + 1),
                    None => trail.push(word),
                }
                self.go(trail);
                self.last_used = std::time::Instant::now();
                self.asked = None;
                self.query.clear();
                self.show();
            }
            Msg::Back | Msg::Forward => {
                let to = if matches!(m, Msg::Back) { self.at.checked_sub(1) } else { Some(self.at + 1).filter(|i| *i < self.history.len()) };
                if let Some(to) = to {
                    (self.at, self.trail) = (to, self.history[to].clone());
                    self.asked = None;
                    self.query.clear();
                    self.show();
                }
            }
            Msg::Clear => {
                self.go(vec![]);
                self.asked = None;
                self.query.clear();
                self.show();
            }
            Msg::Open(path) => {
                // Tests must not start whatever viewers this computer has.
                if !cfg!(test)
                    && let Err(e) = neo_desktop::fs::open_file(&path)
                {
                    self.trouble = Some(format!("Couldn't open {}: {e}", path.display()));
                }
            }
            Msg::Reveal(path) => {
                if !cfg!(test) {
                    let _ = neo_desktop::fs::reveal(&path);
                }
            }
            Msg::Thumb(path, pixels) => {
                // One that comes after the memory was locked is not kept.
                if self.store.is_some() {
                    self.thumbs.insert(path, pixels.map(|(w, h, rgba)| Image::new(w, h, rgba)));
                }
            }
            Msg::Forget(id) => {
                if let Some(store) = &mut self.store {
                    if let Err(e) = store.forget(id).and_then(|()| store.tidy_words()) {
                        self.trouble = Some(e);
                    }
                    self.show();
                }
            }

            Msg::Read => self.read(),
            Msg::Tray(action) => match action {
                TrayAction::Open => self.come_forward(),
                TrayAction::Lock => self.lock(),
                TrayAction::Read => self.read(),
                TrayAction::Quit => self.quit = true,
            },
            Msg::Hide => self.in_sight = false,
            Msg::Show => self.in_sight = true,
            Msg::Quit => self.quit = true,
            Msg::Tick => {
                // Unused for long enough, the memory is locked again.
                let keep = self.settings.stay_unlocked;
                if self.store.is_some() && keep > 0 && !self.answering && self.last_used.elapsed() >= Duration::from_secs(u64::from(keep) * 60) {
                    self.lock();
                }
            }
            Msg::Background(on) => {
                self.settings.background = on;
                self.save_settings();
                // Asked for here, it is wanted at login again whatever Settings was told before.
                if on && !cfg!(test) {
                    let _ = neo_desktop::autostart::set_declined("org.neo.Apollo", false);
                }
                keep_at_startup(on);
                self.serve_anew_quietly();
            }
            Msg::StayUnlocked(minutes) => {
                self.settings.stay_unlocked = minutes;
                self.last_used = std::time::Instant::now();
                self.save_settings();
            }
            Msg::Watch => self.keep_watch(),
            Msg::Warned(found) => self.warn(found),
            Msg::WarnMe(on) => {
                self.settings.warn = on;
                self.save_settings();
            }
            Msg::GetHearing => {
                if self.hearing.is_none() {
                    self.hearing = Some((0, neo_apollo_core::hear::listener().bytes));
                    self.work(|send| {
                        let mut last = std::time::Instant::now();
                        let fetched = neo_apollo_core::hear::download(&mut |done, total| {
                            if last.elapsed() > Duration::from_millis(300) {
                                last = std::time::Instant::now();
                                send(Msg::Hearing(Ok(Some((done, total)))));
                            }
                            true
                        });
                        send(Msg::Hearing(fetched.map(|()| None)));
                    });
                }
            }
            Msg::Hearing(how) => match how {
                Ok(Some(far)) => self.hearing = Some(far),
                // There now: what was left unlistened to is gone back to.
                Ok(None) => {
                    self.hearing = None;
                    self.read();
                }
                Err(e) => {
                    self.hearing = None;
                    self.trouble = Some(e);
                }
            },
            Msg::Looking(i, n) => self.looking = Some((i, n)),
            Msg::LookAhead(on) => {
                self.settings.look_ahead = on;
                self.save_settings();
                if on {
                    self.read();
                }
            }
            Msg::Progress(p) => {
                // What has just been read shows up without waiting for the end.
                let more = p.read != self.progress.read;
                self.log_done(self.progress.failed, &p);
                self.working = p.now.clone().map(|path| (path, std::time::Instant::now(), p.looking));
                self.progress = p;
                if more && self.page == Page::Memory {
                    self.show();
                }
            }
            Msg::ReadDone(mut p) => {
                self.reading = false;
                // Keys and tokens it came on lying in the files it read.
                self.warn(p.secrets.iter().map(|(file, what)| warden::secret_warning(file, what)).collect());
                self.log_done(self.progress.failed, &p);
                // Made with another model: said once, with what to do about it.
                if let Some(why) = p.trouble.take_if(|t| t.starts_with(neo_apollo_core::store::OTHER_MODEL)) {
                    self.stale = Some(why);
                }
                self.progress = p;
                self.show();
                // Whatever changed while that was going on.
                self.catch_up();
                self.look_into_the_list();
                // And with nothing else to do, the rest of what is long is listened to.
                self.hear_the_rest();
            }
            Msg::Held => self.held = true,
            Msg::ModelDraft(i, name) => {
                if let Some(draft) = self.drafts.get_mut(i) {
                    *draft = name;
                }
            }
            Msg::UseModels => {
                self.typing = None;
                self.use_models();
            }
            Msg::Installed(have) => self.installed = have,
            Msg::ChooseModel(i, under) => {
                self.typing = None;
                self.choosing = Some((i, Point::new(under.x, under.bottom() + 4.0)));
            }
            Msg::CloseChoice => self.choosing = None,
            Msg::PickModel(HEARS, id, _) => {
                self.choosing = None;
                self.settings.hear_model = id;
                neo_apollo_core::hear::use_listener(&self.settings.hear_model);
                self.save_settings();
                // Not here yet: fetched now, so that it is in use and not only chosen.
                if !cfg!(test) && !neo_apollo_core::hear::listener().here() {
                    self.update(Msg::GetHearing);
                }
            }
            Msg::PickModel(i, name, away) => {
                self.choosing = None;
                if i < 3 {
                    (self.drafts[i], self.drafts_away[i]) = (name, away);
                    self.use_models();
                }
            }
            Msg::OtherModel(i, away) => {
                (self.choosing, self.typing) = (None, Some(i));
                if i < 3 {
                    self.drafts_away[i] = away;
                }
            }
            Msg::RemoteDraft(address) => self.remote_draft = address,
            Msg::UseRemote => self.use_remote(),
            Msg::Changed(paths) => {
                for path in paths {
                    if !self.pending.contains(&path) {
                        self.pending.push(path);
                    }
                }
                self.catch_up();
            }
            Msg::AskReset => self.asking_reset = self.stale.is_some(),
            Msg::CancelReset => self.asking_reset = false,
            Msg::Reset => self.reset(),
            Msg::StopReading => self.stop_reading.store(true, Ordering::Relaxed),
            Msg::AddFolder => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Add a Folder for Apollo to Read").pick_folder() {
                    if !self.settings.folders.iter().any(|f| f.path == dir) {
                        self.settings.folders.push(Folder { path: dir, on: true });
                        self.save_settings();
                        self.watch();
                    }
                    self.read();
                }
            }
            Msg::FolderOn(i, on) => {
                if let Some(f) = self.settings.folders.get_mut(i) {
                    f.on = on;
                    self.save_settings();
                    self.watch();
                    self.read();
                }
            }
            Msg::RemoveFolder(i) => {
                if i < self.settings.folders.len() {
                    self.settings.folders.remove(i);
                    self.save_settings();
                    self.watch();
                    self.read();
                }
            }
            Msg::ReadAutomatically(on) => {
                self.settings.read_automatically = on;
                self.save_settings();
                self.watch();
                if on {
                    self.read();
                }
            }
            Msg::RememberTalk(on) => {
                self.settings.remember_conversations = on;
                self.save_settings();
            }
        }
    }

    /// The window's content, with whatever is over it.
    fn whole_view(&self) -> Element<Msg> {
        let content = if self.asking_reset { self.reset_sheet() } else { self.content() };
        self.desktop.with_settings(content, "Apollo Settings", Msg::Desktop, vec![])
    }
}

impl Apollo {
    fn content(&self) -> Element<Msg> {
        let main = match self.page {
            Page::Ask => self.ask_page(),
            Page::Memory => self.memory_page(),
            Page::Sources => self.sources_page(),
            Page::Activity => self.activity_page(),
        };
        let page = split(self.sidebar(), main);
        // The menu of models hangs over everything, from the control that opened it.
        match self.choosing {
            Some((job, at)) => stack().width(Length::Fill).height(Length::Fill).push(page).push(popup_menu(at, self.model_menu(job), Msg::CloseChoice)).into(),
            None => page,
        }
    }

    /// The models that can do one of the three jobs (answer, see, embed),
    /// of those the server has. One whose abilities are not known is offered
    /// for every job.
    fn models_for(&self, job: usize, away: bool) -> Vec<&str> {
        let need = ["completion", "vision", "embedding"][job.min(2)];
        self.installed.iter().filter(|(_, can, there)| *there == away && (can.is_empty() || (can.iter().any(|c| c == need) && (job == 2 || !can.iter().all(|c| c == "embedding"))))).map(|(name, _, _)| name.as_str()).collect()
    }

    fn model_menu(&self, job: usize) -> Vec<MenuItem<Msg>> {
        // The models that listen are a few known ones, fetched by Apollo itself.
        if job == HEARS {
            let now = neo_apollo_core::hear::listener();
            return neo_apollo_core::hear::LISTENERS.iter().map(|l| if *l == now { MenuItem::new(l.name, Msg::CloseChoice).icon(icons::CHECK) } else { MenuItem::new(format!("{}  ·  {}{}", l.name, neo_desktop::fs::human_size(l.bytes), if l.here() { "" } else { ", to fetch" }), Msg::PickModel(HEARS, l.id.to_owned(), false)) }).collect();
        }
        let job = job.min(2);
        let (now, now_away) = (self.chosen()[job].clone(), self.away()[job]);
        // Had as "name:latest", chosen as "name": the same model.
        let same = |a: &str, b: &str| a == b || a.strip_suffix(":latest") == Some(b) || b.strip_suffix(":latest") == Some(a);
        let listed = |away: bool| -> Vec<MenuItem<Msg>> {
            let mut items: Vec<MenuItem<Msg>> = self.models_for(job, away).into_iter().map(|name| if same(name, &now) && away == now_away { MenuItem::new(name, Msg::CloseChoice).icon(icons::CHECK) } else { MenuItem::new(name, Msg::PickModel(job, name.to_owned(), away)) }).collect();
            if items.is_empty() {
                items.push(MenuItem::disabled("None yet that can do this"));
            }
            items
        };
        let mut items = listed(false);
        if self.settings.remote.is_empty() {
            items.push(MenuItem::separator());
            items.push(MenuItem::new("Another, by name…", Msg::OtherModel(job, false)).icon(icons::DOWNLOAD));
            return items;
        }
        // With another computer to hand, its models follow under its name.
        let there = self.remote_name();
        items.insert(0, MenuItem::disabled("On this computer"));
        items.push(MenuItem::separator());
        items.push(MenuItem::disabled(format!("On {there}")));
        items.extend(listed(true));
        items.push(MenuItem::separator());
        items.push(MenuItem::new("Another here, by name…", Msg::OtherModel(job, false)).icon(icons::DOWNLOAD));
        items.push(MenuItem::new(format!("Another on {there}, by name…"), Msg::OtherModel(job, true)).icon(icons::DOWNLOAD));
        items
    }

    /// Asks before everything remembered is thrown away.
    fn reset_sheet(&self) -> Element<Msg> {
        let buttons = row().spacing(8.0).push(Space::new(Length::Fill, 0.0)).push(Button::new(text("Cancel")).on_press(Msg::CancelReset)).push(Button::new(text("Forget and Read Again")).kind(ButtonKind::Accent).on_press(Msg::Reset));
        let sheet = column()
            .spacing(10.0)
            .width(Length::Fill)
            .push(text("Start Apollo's memory afresh?").role(TextRole::Title))
            .push(text(format!("Everything Apollo remembers will be forgotten, including notes and conversations, which cannot be read back from any file. Your folders will then be read again with {}, which takes as long as it did the first time.", self.settings.embed_model)).tone(Tone::Muted).width(Length::Fill))
            .push(Space::new(0.0, 4.0))
            .push(buttons);
        neo_desktop::ui::modal(self.content(), sheet, 460.0, Msg::CancelReset)
    }

    fn sidebar(&self) -> Element<Msg> {
        let busy = self.reading || matches!(self.ready, Engine::Checking | Engine::Pulling { .. });
        let tone = if matches!(self.ready, Engine::Trouble(_) | Engine::Missing(_)) { Tone::Warn } else { Tone::Muted };
        // What it is doing, which is also the way to the page that says more.
        let status = Button::new(row().spacing(8.0).align(Align::Center).push(icon(if busy { icons::REFRESH_CW } else { icons::SPARKLES }).size(13.0).tone(tone)).push(text(self.status()).role(TextRole::Caption).tone(tone).no_wrap())).kind(ButtonKind::Ghost).selected(self.page == Page::Activity).padding([10.0, 6.0]).width(Length::Fill).align_x(Align::Start).on_press(Msg::Page(Page::Activity));
        column()
            .spacing(2.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(section("Apollo"))
            .push(nav_item(icons::MESSAGE_CIRCLE, "Ask", self.page == Page::Ask, Msg::Page(Page::Ask)))
            .push(nav_item(if self.store.is_some() { icons::BRAIN } else { icons::LOCK }, "Memory", self.page == Page::Memory, Msg::Page(Page::Memory)))
            .push(nav_item(icons::FOLDER_SEARCH, "Sources", self.page == Page::Sources, Msg::Page(Page::Sources)))
            .push(Space::new(0.0, Length::Fill))
            .push(status)
            .into()
    }

    /// What stands in for the question box until the model can be used.
    fn setup(&self) -> Option<Element<Msg>> {
        let card = |glyph, title: &str, body: String, action: Option<Element<Msg>>| -> Element<Msg> {
            let mut words = column().spacing(3.0).width(Length::Fill).push(text(title).role(TextRole::Strong)).push(text(body).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill));
            if let Engine::Pulling { done, total, .. } = &self.ready {
                words = words.push(Space::new(0.0, 4.0)).push(progress_bar(if *total > 0 { *done as f32 / *total as f32 } else { 0.0 }).width(Length::Fill));
            }
            let mut line = row().spacing(14.0).align(Align::Center).width(Length::Fill).push(icon(glyph).size(22.0).tone(Tone::Accent)).push(words);
            if let Some(action) = action {
                line = line.push(action);
            }
            container(line).surface(Surface::Well).radius(12.0).padding([16.0, 14.0]).width(Length::Fill).into()
        };
        Some(match &self.ready {
            Engine::Ready => return None,
            Engine::Checking => card(icons::SPARKLES, "Starting Apollo's model", "It runs on this computer; the first start takes a moment.".into(), None),
            Engine::Trouble(why) => card(icons::TRIANGLE_ALERT, "Apollo's model is not running", why.clone(), Some(Button::new(text("Try Again")).on_press(Msg::CheckModel).into())),
            Engine::Missing(models) => card(icons::DOWNLOAD, "Apollo needs to download its model", format!("{} will be downloaded once, a few gigabytes, and then run on this computer. Nothing you ask or keep here is sent anywhere.", models.join(" and ")), Some(Button::new(text("Download")).kind(ButtonKind::Accent).on_press(Msg::Pull).into())),
            Engine::Pulling { model, status, done, total } => {
                let how_far = if *total > 0 { format!("{} of {}", neo_desktop::fs::human_size(*done), neo_desktop::fs::human_size(*total)) } else { status.clone() };
                card(icons::DOWNLOAD, &format!("Downloading {model}"), how_far, None)
            }
        })
    }

    fn ask_page(&self) -> Element<Msg> {
        let mut page = column().spacing(12.0).padding(18.0).width(Length::Fill).height(Length::Fill);
        if self.talk.is_empty() {
            let suggest = |q: &str| -> Element<Msg> { Button::new(text(q).role(TextRole::Caption)).kind(ButtonKind::Ghost).padding([12.0, 6.0]).radius(14.0).on_press_maybe((self.ready == Engine::Ready).then(|| Msg::Suggest(q.to_owned()))).into() };
            let about = if self.store.is_some() { "Ask about anything, or about what is on this computer." } else { "Ask about anything. To ask about what is on this computer, unlock Apollo's memory first." };
            let mut welcome = column().spacing(8.0).align(Align::Center).push(icon(icons::SPARKLES).size(34.0).tone(Tone::Accent)).push(text("Ask Apollo").role(TextRole::Heading)).push(text(about).tone(Tone::Muted)).push(Space::new(0.0, 6.0));
            welcome = welcome.push(row().spacing(6.0).push(suggest("What videos do I have from this month?")).push(suggest("Which screenshots show code?")));
            welcome = welcome.push(row().spacing(6.0).push(suggest("Remember that the spare key is with Sam")).push(suggest("What did I ask you last time?")));
            if self.store.is_none() {
                welcome = welcome.push(Space::new(0.0, 6.0)).push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::LOCK_OPEN).size(14.0)).push(text("Unlock Memory…"))).on_press_maybe((!self.unlocking).then_some(Msg::Unlock)));
            }
            page = page.push(container(welcome).width(Length::Fill).height(Length::Fill).center());
        } else {
            let mut said = column().spacing(14.0).width(Length::Fill).padding([0.0, 4.0]);
            for s in &self.talk {
                said = said.push(self.turn(s));
            }
            page = page.push(scrollable(said).height(Length::Fill).reveal(self.said, self.talk.len().saturating_sub(1)));
        }
        if let Some(why) = &self.trouble {
            page = page.push(notice(Tone::Bad, why.clone()));
        }
        match self.setup() {
            Some(setup) => page = page.push(setup),
            None => {
                let button = if self.answering { icon_button(icons::SQUARE, 34.0).on_press(Msg::StopAnswer) } else { icon_button(icons::ARROW_UP, 34.0).kind(ButtonKind::Accent).on_press_maybe((!self.draft.trim().is_empty()).then_some(Msg::Send)) };
                let hint = if self.store.is_some() { "Ask Apollo" } else { "Ask Apollo (memory locked)" };
                page = page.push(row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text_input(hint, self.draft.clone()).on_input(Msg::Draft).on_submit(Msg::Send).autofocus(true).width(Length::Fill)).push(button));
            }
        }
        page.into()
    }

    /// One thing said: the user's on the right, Apollo's across the page
    /// with the files it drew on underneath.
    fn turn(&self, s: &Said) -> Element<Msg> {
        if s.role == Role::User {
            let bubble = container(text(s.text.clone())).surface(Surface::Raised).radius(14.0).padding([14.0, 9.0]).max_width(560.0);
            return row().width(Length::Fill).push(Space::new(Length::Fill, 0.0)).push(bubble).into();
        }
        let waiting = s.text.is_empty() && self.answering;
        let wait = match self.looking {
            Some((i, n)) => format!("Looking at {} of {} {} this may be about…", i + 1, n, if n == 1 { "file" } else { "files" }),
            None => "Thinking…".to_owned(),
        };
        let mut answer = column().spacing(8.0).width(Length::Fill).push(text(if waiting { wait } else { assistant::plain(&s.text) }).tone(if waiting { Tone::Muted } else { Tone::Inherit }).width(Length::Fill));
        // The files the answer was given to draw on, each a picture that shows it in Files.
        let files = assistant::sources(&s.listed, &s.recalled);
        if !files.is_empty() && !(self.answering && std::ptr::eq(s, self.talk.last().unwrap_or(s))) {
            let mut tiles = row().spacing(8.0);
            for m in files.into_iter().take(TILES) {
                let Some(path) = m.source.clone() else { continue };
                let tile: Element<Msg> = match self.thumbs.get(&path).and_then(Option::as_ref) {
                    Some(small) => container(picture(small).fit(Fit::Cover).width(TILE.0).height(TILE.1)).width(TILE.0).height(TILE.1).into(),
                    None => container(icon(kind_icon(m.kind)).size(22.0).tone(Tone::Accent)).surface(Surface::Well).radius(8.0).width(TILE.0).height(TILE.1).center().into(),
                };
                tiles = tiles.push(mouse_area(tile).on_press(move || Msg::Reveal(path.clone())));
            }
            answer = answer.push(tiles);
        }
        row().spacing(12.0).width(Length::Fill).push(container(icon(icons::SPARKLES).size(16.0).tone(Tone::Accent)).padding([0.0, 2.0, 0.0, 0.0])).push(answer).into()
    }

    fn memory_page(&self) -> Element<Msg> {
        if self.store.is_none() {
            let (glyph, title, body) = if self.unlocking { (icons::FINGERPRINT, "Waiting for you", "Use Touch ID or type your password in the window that has come up.") } else { (icons::LOCK, "Apollo's memory is locked", "What Apollo remembers of your pictures, videos, documents and conversations is kept encrypted. Looking through it takes your login.") };
            let mut lock = column().spacing(8.0).align(Align::Center).push(icon(glyph).size(40.0).tone(Tone::Accent)).push(text(title).role(TextRole::Heading)).push(container(text(body).tone(Tone::Muted).align(Align::Center).width(Length::Fill)).max_width(440.0)).push(Space::new(0.0, 8.0));
            lock = lock.push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::LOCK_OPEN).size(15.0)).push(text("Unlock…"))).kind(ButtonKind::Accent).on_press_maybe((!self.unlocking).then_some(Msg::Unlock)));
            if let Some(why) = &self.trouble {
                lock = lock.push(Space::new(0.0, 6.0)).push(notice(Tone::Bad, why.clone()));
            }
            return container(lock).width(Length::Fill).height(Length::Fill).center().into();
        }
        let search = text_input("Search by meaning: “a dog on a beach”, “the invoice from March”", self.query.clone()).on_input(Msg::Query).on_submit(Msg::Search).on_cancel(Msg::Clear).width(Length::Fill);
        let top = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(icon(icons::SEARCH).size(16.0).tone(Tone::Muted)).push(search).push(icon_button(icons::LOCK, 30.0).kind(ButtonKind::Ghost).on_press(Msg::Lock));
        let kinds = segmented(std::iter::once("All").chain(Kind::ALL.iter().map(|k| k.plural())), Some(self.kind.map_or(0, |k| 1 + Kind::ALL.iter().position(|a| *a == k).unwrap_or(0))), |i| Msg::Kind(i.checked_sub(1).and_then(|i| Kind::ALL.get(i).copied())));

        let cloud: Element<Msg> = if self.cloud.is_empty() {
            let why = if self.reading { "Nothing is remembered yet. Apollo is reading your folders; words will gather here as it goes." } else { "Nothing is remembered yet. Give Apollo folders to read in Sources." };
            container(text(why).tone(Tone::Muted)).width(Length::Fill).height(120.0).center().into()
        } else if self.graph.is_empty() {
            Element::new(Cloud::new(self.cloud.clone()).height(CLOUD_H).on_pick(Msg::Word))
        } else {
            Element::new(Graph::new(self.graph.clone()).height(CLOUD_H).on_pick(Msg::Word))
        };
        let summary = format!("{} · {} from {}", count(self.stats.total(), "memory", "memories"), count(self.stats.words, "word", "words"), count(self.stats.files, "file", "files"));
        let steps = row().spacing(2.0).push(icon_button(icons::ARROW_LEFT, 28.0).kind(ButtonKind::Ghost).on_press_maybe((self.at > 0).then_some(Msg::Back))).push(icon_button(icons::ARROW_RIGHT, 28.0).kind(ButtonKind::Ghost).on_press_maybe((self.at + 1 < self.history.len()).then_some(Msg::Forward)));
        let mut heading = row().spacing(10.0).align(Align::Center).width(Length::Fill).push(steps).push(text(self.shown.heading.clone()).role(TextRole::Title)).push(text(if self.searching { "Searching…".to_owned() } else { summary }).role(TextRole::Caption).tone(Tone::Muted)).push(Space::new(Length::Fill, 0.0));
        if !self.trail.is_empty() || self.asked.is_some() {
            heading = heading.push(Button::new(text(if self.trail.is_empty() { "Clear" } else { "Back to the Cloud" }).role(TextRole::Caption)).kind(ButtonKind::Ghost).padding([10.0, 4.0]).on_press(Msg::Clear));
        }
        let mut list = column().spacing(2.0).width(Length::Fill);
        for (memory, distance) in &self.shown.items {
            list = list.push(self.memory_row(memory, *distance));
        }
        if self.shown.items.is_empty() {
            list = list.push(container(text("Nothing here.").tone(Tone::Muted)).padding([0.0, 12.0]));
        }
        let mut body = column().spacing(12.0).width(Length::Fill).push(container(cloud).surface(Surface::Well).radius(14.0).padding(10.0).width(Length::Fill)).push(heading);
        if let Some(why) = &self.trouble {
            body = body.push(notice(Tone::Bad, why.clone()));
        }
        column().spacing(12.0).padding(18.0).width(Length::Fill).height(Length::Fill).push(top).push(kinds).push(scrollable(body.push(list)).height(Length::Fill)).into()
    }

    fn memory_row(&self, m: &Memory, distance: Option<f32>) -> Element<Msg> {
        let when = neo_desktop::fs::friendly_time(std::time::UNIX_EPOCH + Duration::from_secs(m.created));
        let place = m.source.as_deref().and_then(Path::parent).map(|p| format!("{}  ·  ", short_path(p))).unwrap_or_default();
        let title: Element<Msg> = if m.kind == Kind::Video { row().spacing(6.0).align(Align::Center).push(icon(icons::FILM).size(13.0).tone(Tone::Accent)).push(text(m.title.clone()).role(TextRole::Strong).no_wrap()).into() } else { text(m.title.clone()).role(TextRole::Strong).no_wrap().into() };
        let words = column().spacing(2.0).width(Length::Fill).push(title).push(text(snippet(&m.text, 210)).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).push(text(format!("{place}{when}")).role(TextRole::Caption).tone(Tone::Faint).no_wrap());
        // A photo or a video is shown by a small picture of it; the rest, and
        // those whose picture is not made yet, by what kind of thing they are.
        let lead: Element<Msg> = match m.source.as_ref().and_then(|p| self.thumbs.get(p)).and_then(Option::as_ref) {
            Some(small) => container(picture(small).fit(Fit::Cover).width(THUMB.0).height(THUMB.1)).width(THUMB.0).height(THUMB.1).into(),
            None if matches!(m.kind, Kind::Photo | Kind::Video) => container(icon(kind_icon(m.kind)).size(18.0).tone(Tone::Accent)).surface(Surface::Well).radius(6.0).width(THUMB.0).height(THUMB.1).center().into(),
            None => container(icon(kind_icon(m.kind)).size(18.0).tone(Tone::Accent)).width(THUMB.0).center().into(),
        };
        let mut line = row().spacing(12.0).align(Align::Center).width(Length::Fill).push(lead).push(words);
        if let Some(d) = distance {
            // How near in meaning, as a share: the same is 100%.
            line = line.push(text(format!("{:.0}%", ((1.0 - d) * 100.0).clamp(0.0, 100.0))).role(TextRole::Caption).tone(Tone::Muted));
        }
        if let Some(path) = &m.source {
            line = line.push(icon_button(icons::FOLDER_OPEN, 28.0).kind(ButtonKind::Ghost).on_press(Msg::Reveal(path.clone())));
        }
        line = line.push(icon_button(icons::TRASH_2, 28.0).kind(ButtonKind::Ghost).on_press(Msg::Forget(m.id)));
        let inside: Element<Msg> = container(line).padding([10.0, 8.0]).width(Length::Fill).into();
        // The row opens the file, anywhere its buttons are not.
        match m.source.clone() {
            // A folder is shown in Files; anything else is opened.
            Some(path) if m.kind == Kind::Folder => mouse_area(inside).on_press(move || Msg::Reveal(path.clone())).into(),
            Some(path) => mouse_area(inside).on_press(move || Msg::Open(path.clone())).into(),
            None => inside,
        }
    }

    /// What stands in the way of listening to videos and recordings, said
    /// with what to do about it; nothing when there is hearing.
    fn hearing_notice(&self) -> Option<Element<Msg>> {
        use neo_apollo_core::hear::{self, Missing};
        if let Some((done, total)) = self.hearing {
            let words = column().spacing(6.0).width(Length::Fill).push(text(format!("Fetching the model that writes down what is said: {} of {}", neo_desktop::fs::human_size(done), neo_desktop::fs::human_size(total))).role(TextRole::Caption).tone(Tone::Muted)).push(progress_bar(if total > 0 { done as f32 / total as f32 } else { 0.0 }).width(Length::Fill));
            return Some(container(words).surface(Surface::Well).radius(10.0).padding([14.0, 10.0]).width(Length::Fill).into());
        }
        // Tests are not to depend on what this computer has installed.
        let missing = if cfg!(test) { None } else { hear::missing() };
        Some(match missing? {
            Missing::Tool => notice(Tone::Warn, "What is said in videos and recordings is not being listened to. For that, install whisper-cli: cargo xtask deps --install"),
            Missing::Ffmpeg => notice(Tone::Warn, "What is said in videos and recordings is not being listened to. For that, install ffmpeg: cargo xtask deps --install"),
            Missing::Model => row()
                .spacing(12.0)
                .align(Align::Center)
                .width(Length::Fill)
                .push(text(format!("What is said in videos and recordings is not being listened to yet. The model that writes it down is fetched once, about {}, and runs on this computer.", neo_desktop::fs::human_size(hear::listener().bytes))).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill))
                .push(Button::new(text("Fetch It")).on_press(Msg::GetHearing))
                .into(),
        })
    }

    /// What Apollo is doing and has done: how the reading stands, why it
    /// takes the time it does, and the files it has been through lately.
    fn activity_page(&self) -> Element<Msg> {
        let p = &self.progress;
        let mut page = column().spacing(14.0).padding(22.0).width(Length::Fill);
        page = page.push(text("Activity").role(TextRole::Heading));
        let how = if self.settings.look_ahead {
            "Apollo is set to look at every photo and video ahead of time. Listing a file takes a moment; looking at one means showing it to the model that can see, which takes some seconds for each picture and several times that for a video, looked at in a few places along its length. That is what takes the time."
        } else {
            "Apollo lists every file by its name, place and date as soon as it finds it, which takes moments, and reads documents in full. A photo or a video is looked at (shown to the model that can see, some seconds each) only when a question or a search turns it up, and it is remembered from then on."
        };
        page = page.push(text(how).tone(Tone::Muted).width(Length::Fill));

        // Now.
        let pace = self.seconds_a_look();
        let (what, detail) = if self.ready != Engine::Ready {
            (self.status(), "Nothing is read until the model is running.".to_owned())
        } else if self.reading {
            let left = p.total.saturating_sub(p.done);
            let name = self.working.as_ref().and_then(|(path, _, _)| path.file_name()).map(|n| n.to_string_lossy().into_owned());
            let mut detail = name.map_or_else(|| "Seeing what is new.".to_owned(), |n| format!("Now: {n}"));
            if let (true, Some(each)) = (p.looking, pace) {
                detail.push_str(&format!("  ·  about {:.0} s a file, so {} more", each, how_long(each * left as f32)));
            }
            (self.status(), detail)
        } else if self.held {
            (self.status(), "Held off while the computer is short of memory, and taken up again when there is room.".to_owned())
        } else {
            ("Nothing under way".to_owned(), format!("Last time: {}, {} listed{}.", count(p.read as u32, "file read", "files read"), p.listed, if p.failed > 0 { format!(", {} could not be", p.failed) } else { String::new() }))
        };
        let action: Element<Msg> = if self.reading { Button::new(text("Stop")).on_press(Msg::StopReading).into() } else { Button::new(text("Read Now")).on_press_maybe((self.ready == Engine::Ready).then_some(Msg::Read)).into() };
        let mut now = column().spacing(8.0).width(Length::Fill).push(setting(&what, &detail, action));
        if self.reading && p.total > 0 {
            now = now.push(progress_bar(p.done as f32 / p.total as f32).width(Length::Fill));
        }
        page = page.push(container(now).surface(Surface::Well).radius(12.0).padding([16.0, 14.0]).width(Length::Fill));

        // What is in the memory, which is only said once it is unlocked.
        page = page.push(section("In the memory"));
        if self.store.is_some() {
            let seen = self.stats.of(Kind::Photo) + self.stats.of(Kind::Video);
            let figures = [
                (count(self.stats.files, "file", "files"), "taken in"),
                (count(seen.saturating_sub(self.stats.light), "photo or video", "photos and videos"), "looked at"),
                (count(self.stats.light, "photo or video", "photos and videos"), "listed, not looked at yet"),
                (count(self.stats.of(Kind::Document), "passage", "passages"), "of documents read"),
                (count(self.stats.words, "word", "words"), "they are about"),
            ];
            let mut tiles = row().spacing(10.0).width(Length::Fill);
            for (figure, what) in figures {
                tiles = tiles.push(container(column().spacing(2.0).push(text(figure).role(TextRole::Strong).no_wrap()).push(text(what).role(TextRole::Caption).tone(Tone::Muted))).surface(Surface::Well).radius(10.0).padding([12.0, 10.0]).width(Length::Fill));
            }
            page = page.push(tiles);
            if self.stats.light > 0 && !self.settings.look_ahead {
                let all = pace.map_or(String::new(), |each| format!(" At the pace so far that would take {}.", how_long(each * self.stats.light as f32)));
                page = page.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text(format!("To look at the rest now rather than as they are asked after, turn on Look at everything up front.{all}")).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).push(Button::new(text("Look at Everything")).on_press(Msg::LookAhead(true))));
            }
        } else {
            page = page.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text("How much has been taken in is part of the memory, and shown once it is unlocked.").tone(Tone::Muted).width(Length::Fill)).push(Button::new(text("Unlock…")).on_press_maybe((!self.unlocking).then_some(Msg::Unlock))));
        }
        if let Some(hearing) = self.hearing_notice() {
            page = page.push(hearing);
        }
        for tool in p.needs.iter().filter(|t| !t.starts_with("whisper")) {
            page = page.push(notice(Tone::Warn, format!("Some files are being left for want of {tool}: cargo xtask deps --install")));
        }

        // What it has warned of, newest first.
        if !self.warnings.is_empty() {
            page = page.push(section("Warnings"));
            for w in self.warnings.iter().rev() {
                let words = column().spacing(2.0).width(Length::Fill).push(text(w.title.clone()).role(TextRole::Strong)).push(text(w.body.clone()).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill));
                let mut line = row().spacing(12.0).align(Align::Center).width(Length::Fill).push(icon(if w.security { icons::SHIELD_ALERT } else { icons::TRIANGLE_ALERT }).size(18.0).tone(if w.security { Tone::Bad } else { Tone::Warn })).push(words);
                if let Some(file) = &w.file {
                    line = line.push(icon_button(icons::FOLDER_OPEN, 28.0).kind(ButtonKind::Ghost).on_press(Msg::Reveal(file.clone())));
                }
                page = page.push(container(line).surface(Surface::Well).radius(10.0).padding([14.0, 10.0]).width(Length::Fill));
            }
        }

        // Lately, newest first.
        page = page.push(section("Lately"));
        if self.log.is_empty() {
            page = page.push(text("Nothing has been read since Apollo was opened.").tone(Tone::Muted));
        }
        let mut list = column().spacing(2.0).width(Length::Fill);
        for done in self.log.iter().rev() {
            let name = done.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let place = done.path.parent().map(short_path).unwrap_or_default();
            let photo_or_video = matches!(index::classify(&done.path), Some(index::What::Photo | index::What::Video | index::What::Audio));
            let (said, tone) = match (&done.trouble, done.looked, photo_or_video) {
                (Some(why), _, _) => (format!("Could not be read: {why}"), Tone::Bad),
                (None, true, _) => (format!("Looked at, {:.0} s", done.seconds.max(1.0)), Tone::Good),
                (None, false, true) => ("Listed".to_owned(), Tone::Muted),
                (None, false, false) => ("Read".to_owned(), Tone::Good),
            };
            let line =
                row().spacing(12.0).align(Align::Center).width(Length::Fill).push(icon(neo_desktop::fs::file_icon(&done.path, false)).size(16.0).tone(Tone::Accent)).push(column().spacing(1.0).width(Length::Fill).push(text(name).role(TextRole::Strong).no_wrap()).push(text(place).role(TextRole::Caption).tone(Tone::Faint).no_wrap())).push(text(said).role(TextRole::Caption).tone(tone).no_wrap());
            let path = done.path.clone();
            list = list.push(mouse_area(container(line).padding([8.0, 6.0]).width(Length::Fill)).on_press(move || Msg::Reveal(path.clone())));
        }
        scrollable(page.push(list)).into()
    }

    fn sources_page(&self) -> Element<Msg> {
        let p = &self.progress;
        let mut page = column().spacing(14.0).padding(22.0).width(Length::Fill);
        page = page.push(text("Sources").role(TextRole::Heading)).push(text("The folders Apollo reads. Each picture is described, each video looked at in a few places along its length and listened to, each recording listened to, and each document read in passages; what it finds goes into its memory.").tone(Tone::Muted).width(Length::Fill));

        // How the reading is going.
        let (what, detail) = if self.ready != Engine::Ready {
            (self.status(), "Apollo reads once its model is running.".to_owned())
        } else if self.reading {
            let name = p.now.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
            (self.status(), name.map_or_else(|| "Seeing what is new.".to_owned(), |n| format!("Now: {n}")))
        } else {
            let mut parts = vec![count(p.read as u32, "file read", "files read") + " last time"];
            if p.forgotten > 0 {
                parts.push(format!("{} forgotten", p.forgotten));
            }
            if p.failed > 0 {
                parts.push(format!("{} could not be read", p.failed));
            }
            if self.held { (self.status(), "Reading is held off while the computer is short of memory, and taken up again when there is room.".to_owned()) } else { ("Up to date".to_owned(), parts.join("  ·  ")) }
        };
        let action: Element<Msg> = if self.reading { Button::new(text("Stop")).on_press(Msg::StopReading).into() } else { Button::new(text("Read Now")).on_press_maybe((self.ready == Engine::Ready).then_some(Msg::Read)).into() };
        let mut reading = column().spacing(8.0).width(Length::Fill).push(setting(&what, &detail, action));
        if self.reading && p.total > 0 {
            reading = reading.push(progress_bar(p.done as f32 / p.total as f32).width(Length::Fill));
        }
        page = page.push(container(reading).surface(Surface::Well).radius(12.0).padding([16.0, 14.0]).width(Length::Fill));
        if let Some(setup) = self.setup() {
            page = page.push(setup);
        }
        if let Some(hearing) = self.hearing_notice() {
            page = page.push(hearing);
        }
        for tool in &p.needs {
            let what = match *tool {
                "pdftotext" => "PDFs are being left unread. To read them, install pdftotext: cargo xtask deps --install",
                "ffmpeg" => "Videos are being left unread. To read them, install ffmpeg: cargo xtask deps --install",
                // Said once, with what to do about it, by the notice above.
                t if t.starts_with("whisper") => continue,
                _ => "Some documents are being left unread. To read them, install pandoc.",
            };
            page = page.push(notice(Tone::Warn, what));
        }
        if let Some(why) = p.trouble.as_ref().or(self.trouble.as_ref()) {
            page = page.push(notice(Tone::Bad, why.clone()));
        }

        page = page.push(setting("Read by itself", "When Apollo opens, and now and then while it is open. Reading keeps a model of a few gigabytes in memory until it is done; turned off, the folders are read only when you press Read Now.", toggle(self.settings.read_automatically, Msg::ReadAutomatically)));
        page = page.push(setting(
            "Look at everything up front",
            "Photos and videos are listed by name as soon as they are found, which takes moments, and looked at when a question or a search turns them up. Turned on, every one is looked at ahead of time instead: searches by what a picture shows then find everything at once, at the cost of some seconds a picture, with the large model in memory all the while.",
            toggle(self.settings.look_ahead, Msg::LookAhead),
        ));
        page = page.push(section("Folders"));
        for (i, f) in self.settings.folders.iter().enumerate() {
            let help = if !f.path.is_dir() {
                "This folder is not there."
            } else if f.on {
                "Read, with everything inside it."
            } else {
                "Left alone; what was remembered of it is forgotten."
            };
            let controls = row().spacing(8.0).align(Align::Center).push(toggle(f.on, move |on| Msg::FolderOn(i, on))).push(icon_button(icons::TRASH_2, 28.0).kind(ButtonKind::Ghost).on_press(Msg::RemoveFolder(i)));
            page = page.push(setting(&short_path(&f.path), help, controls));
        }
        if self.settings.folders.is_empty() {
            page = page.push(text("No folders yet.").tone(Tone::Muted));
        }
        page = page.push(row().push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::PLUS).size(14.0)).push(text("Add Folder…"))).on_press(Msg::AddFolder)));

        let unlocked_for = segmented(STAY_UNLOCKED.iter().map(|(_, label)| *label), STAY_UNLOCKED.iter().position(|(m, _)| *m == self.settings.stay_unlocked), |i| Msg::StayUnlocked(STAY_UNLOCKED[i.min(3)].0));
        page = page
            .push(section("Ready when asked"))
            .push(setting(
                "Stay ready in the background",
                &format!("Closing the window leaves Apollo running out of sight, with the model that answers kept in memory (about a gigabyte), so a question from the search bar ({}) is answered at once. It starts at login the same way. Quit Apollo, in the File menu, ends it.", if cfg!(target_os = "macos") { "⌘⇧A" } else { "Ctrl+Shift+A" }),
                toggle(self.settings.background, Msg::Background),
            ))
            .push(setting("Keep the memory unlocked for", "Unlocked once, the memory stays so while questions keep coming, and locks again when it has gone unused this long. A question asked while it is locked brings up the login check first.", unlocked_for));
        page = page.push(section("Warnings")).push(setting(
            "Warn me",
            "While it is open Apollo keeps an eye on the computer and says, through NeoShell, when memory or the disk is running out and what is using it, and when something stands open: a disk that is not encrypted, a firewall that is off, a model server the whole network can use, a key or a token lying in a file it reads. It looks and tells; it changes nothing.",
            toggle(self.settings.warn, Msg::WarnMe),
        ));
        page = page.push(section("Conversations")).push(setting("Remember what I ask", "What you ask Apollo and what it answers go into its memory, so it can be brought up later.", toggle(self.settings.remember_conversations, Msg::RememberTalk)));
        // The models, which can be any the servers have or can get.
        let anything_away = self.away().contains(&true);
        let running = text(if self.ready == Engine::Ready { "Running" } else { "Not running" }).role(TextRole::Caption).tone(if self.ready == Engine::Ready { Tone::Good } else { Tone::Warn });
        let where_it_runs =
            if anything_away { format!("Some of Apollo's work is given to {}: what is asked of a model there is sent to it. The rest is done by Ollama on this computer.", self.remote_name()) } else { "Apollo's models are served by Ollama here, and nothing is sent anywhere. Each job below can be given to any model that is here, or to another by name, which is downloaded.".to_owned() };
        page = page.push(section("Models")).push(setting("On this computer", &where_it_runs, running));
        let jobs = [("Answers with", "The model that holds the conversation."), ("Describes pictures with", "A model that can see: it is shown each picture and each frame of a video."), ("Places text by meaning with", "An embedding model. Memories made with one cannot be searched with another, so changing it means starting the memory afresh.")];
        let away = self.away();
        for (i, (title, help)) in jobs.into_iter().enumerate() {
            // A menu of the models that are there to choose; or, for one that is not, its name typed.
            let control: Element<Msg> = if self.typing == Some(i) {
                text_input("name, as Ollama lists it", self.drafts[i].clone()).on_input(move |name| Msg::ModelDraft(i, name)).on_submit(Msg::UseModels).autofocus(true).width(MODEL_W).into()
            } else {
                let name = if away[i] { format!("{}  ·  {}", self.chosen()[i], self.remote_name()) } else { self.chosen()[i].clone() };
                let face = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text(name).no_wrap().width(Length::Fill)).push(icon(icons::CHEVRONS_UP_DOWN).size(14.0).tone(Tone::Muted));
                mouse_area(container(face).surface(Surface::Raised).radius(8.0).padding([12.0, 7.0]).width(MODEL_W)).on_press_in(move |at| Msg::ChooseModel(i, at)).into()
            };
            page = page.push(setting(title, help, control));
        }
        // The one that listens: a few sizes of Whisper, the larger the surer of the words.
        let listens = neo_apollo_core::hear::listener();
        let face = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text(listens.name).no_wrap().width(Length::Fill)).push(icon(icons::CHEVRONS_UP_DOWN).size(14.0).tone(Tone::Muted));
        page = page.push(setting("Listens with", &format!("The model that writes down what is said in videos and recordings. {} It is fetched once and runs on this computer; what was listened to before stays as it was heard.", listens.note), mouse_area(container(face).surface(Surface::Raised).radius(8.0).padding([12.0, 7.0]).width(MODEL_W)).on_press_in(|at| Msg::ChooseModel(HEARS, at))));
        if let Some(i) = self.typing {
            let changed = (self.drafts.clone().map(|m| m.trim().to_owned()) != self.chosen() || self.drafts_away != away) && self.drafts.iter().all(|m| !m.trim().is_empty());
            let onto = if self.drafts_away[i.min(2)] { format!("Type a model's name and press Return. It is downloaded onto {} if it is not there yet.", self.remote_name()) } else { "Type a model's name and press Return. It is downloaded if it is not here yet.".to_owned() };
            page = page.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text(onto).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).push(Button::new(text("Use This Model")).on_press_maybe(changed.then_some(Msg::UseModels))));
        }
        // Another computer, for models this one has no room for.
        let address = text_input("studio.local, or 192.168.1.20:11434", self.remote_draft.clone()).on_input(Msg::RemoteDraft).on_submit(Msg::UseRemote).width(MODEL_W);
        let set = self.remote_draft.trim().trim_end_matches('/') != self.settings.remote;
        page = page.push(setting(
            "Another computer",
            "A machine of yours that runs Ollama, for models too large for this one. Its models then appear in the menus above. What a model there is asked (your questions, or the pictures it describes) is sent to that machine over the network, unprotected unless the address begins with https; the memory itself stays here. On that machine, start Ollama with OLLAMA_HOST=0.0.0.0 ollama serve.",
            row().spacing(8.0).align(Align::Center).push(address).push(Button::new(text(if self.remote_draft.trim().is_empty() && !self.settings.remote.is_empty() { "Remove" } else { "Use" })).on_press_maybe(set.then_some(Msg::UseRemote))),
        ));
        if let Some(why) = &self.stale {
            page = page.push(notice(Tone::Warn, format!("{why} Go back to the model it was made with, or start the memory afresh."))).push(row().push(Button::new(text("Start Memory Afresh…")).on_press(Msg::AskReset)));
        }
        page = page.push(section("Where it all is"));
        page = page.push(setting("Encrypted memory", &format!("{}, encrypted with a key in your keychain. Looking through it takes your login.", short_path(&self.db)), text(if self.store.is_some() { "Unlocked" } else { "Locked" }).role(TextRole::Caption).tone(Tone::Muted)));
        scrollable(page).into()
    }
}

/// Has Apollo start at login, out of sight, for an installed copy that
/// is set to stay ready; and not, once it is set not to.
fn keep_at_startup(wanted: bool) {
    let installed = std::env::current_exe().is_ok_and(|p| !p.components().any(|c| c.as_os_str() == "target"));
    let Ok(program) = std::env::current_exe() else { return };
    if !installed || cfg!(test) {
        return;
    }
    let entry = neo_desktop::autostart::Entry { id: "org.neo.Apollo", name: "Apollo", program: &program, args: &["--hidden"] };
    // Turned off in Settings, it stays off though it is set to stay ready.
    let wanted = wanted && !neo_desktop::autostart::declined(entry.id);
    match (wanted, neo_desktop::autostart::is_enabled(&entry)) {
        (true, false) => drop(neo_desktop::autostart::enable(&entry)),
        (false, true) => drop(neo_desktop::autostart::disable(&entry)),
        _ => {}
    }
}

/// Reads the folders without a window, saying how it goes: for a script,
/// or to run at login.
fn read_folders() -> Result<(), String> {
    let settings = Settings::load();
    let model = Ollama::from_settings(&settings);
    model.start()?;
    let missing = model.missing()?;
    if !missing.is_empty() {
        return Err(format!("{} must be downloaded first: open Apollo, or run `ollama pull` for each.", missing.join(" and ")));
    }
    let mut store = Store::open_for(&neo_apollo_core::dir().join("memory.db"), &key::database_key()?, &model)?;
    let done = index::run(&mut store, &model, &settings.roots(), settings.look_ahead, &mut |p: &Progress| {
        if let Some(now) = &p.now {
            println!("[{} of {}] {}", p.done + 1, p.total, now.display());
        }
        true
    });
    println!("{} read, {} forgotten, {} could not be read.", done.read, done.forgotten, done.failed);
    for tool in &done.needs {
        println!("Some files were left for want of {tool}.");
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    if args.iter().any(|a| a == "--index") {
        if let Err(e) = read_folders() {
            eprintln!("neo-apollo: {e}");
            std::process::exit(1);
        }
        return;
    }
    // `neo-apollo --ask "question"` asks it: of the Apollo that is running,
    // if one is, and otherwise of this one once it is up.
    let question = args.iter().position(|a| a == "--ask").and_then(|i| args.get(i + 1)).filter(|q| !q.trim().is_empty()).cloned();
    if let Some(question) = &question
        && neo_desktop::apollo::send_to(&neo_desktop::apollo::port_file(), question)
    {
        return;
    }
    // Opened again while it runs out of sight, the one that is running shows itself.
    if question.is_none() && !args.iter().any(|a| a == "--hidden") && neo_desktop::apollo::show() {
        return;
    }
    let mut app = Apollo::new();
    // Started at login, it waits out of sight for the search bar.
    app.in_sight = !args.iter().any(|a| a == "--hidden") || question.is_some();
    app.waiting_question = question.clone();
    app.draft = question.unwrap_or_default();
    if let Err(e) = neo::run(app) {
        eprintln!("neo-apollo: {e}");
        std::process::exit(1);
    }
}

/// An Apollo for snapshots and tests: a model that answers at once, a
/// database of its own in the temporary folder, and a check that passes.
fn sample(name: &str, filed: bool) -> (Apollo, PathBuf) {
    use neo_apollo_core::model::fake::Fake;
    use neo_apollo_core::store::New;
    let dir = std::env::temp_dir().join(format!("neo-apollo-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let model = Arc::new(Fake::default());
    let key = [5u8; 32];
    let mut store = Store::open(&dir.join("memory.db"), &key, model.dims().unwrap()).expect("a database");
    let things: [(Kind, &str, &str, &str, &[&str]); 12] = [
        (Kind::Photo, "~/Pictures/Trips/IMG_2041.jpg", "A golden retriever running along a beach at low tide, with waves breaking behind it.", "dog beach sea", &["dog", "beach", "sea", "waves", "golden retriever", "summer"]),
        (Kind::Photo, "~/Pictures/Trips/IMG_2057.jpg", "Two people and a dog sitting on a picnic blanket on the sand, a kite in the sky.", "dog beach sand", &["dog", "beach", "picnic", "kite", "friends", "summer"]),
        (Kind::Photo, "~/Pictures/Trips/IMG_2110.jpg", "A snow-covered mountain peak above a pine forest, under a clear sky.", "mountain snow", &["mountain", "snow", "forest", "hiking", "sky"]),
        (Kind::Video, "~/Movies/Neo Recording 2026-10-06 at 09.42.30.mov", "A video 1:12 long. At 0:11: A code editor with a Rust file open and a terminal below it. At 0:36: A file manager showing a folder of pictures.", "code editor terminal screenshot", &["code", "editor", "terminal", "rust", "screen recording", "file manager"]),
        (Kind::Video, "~/Movies/Hike.mp4", "A video 3:40 long. At 0:33: A trail climbing through a forest. At 1:50: A dog drinking from a stream. At 3:07: A view from a mountain peak over a valley.", "mountain hiking dog", &["hiking", "mountain", "dog", "forest", "trail", "stream"]),
        (Kind::Photo, "~/Pictures/Neo Screenshot 2026-10-06 at 09.41.07.png", "A screenshot of a code editor showing a function that draws a word cloud, with a hover popup.", "code editor screenshot", &["screenshot", "code", "editor", "rust"]),
        (Kind::Document, "~/Documents/Invoices/March.md", "Invoice 4411 from Hartley Supplies. Payment of 240.00 is due to the supplier's bank by the end of March.", "invoice payment supplier bank", &["invoice", "payment", "supplier", "march"]),
        (Kind::Document, "~/Documents/Invoices/April.md", "Invoice 4460 from Hartley Supplies for paper and ink. Payment is due in April.", "invoice payment supplier", &["invoice", "payment", "supplier", "paper"]),
        (Kind::Document, "~/Documents/Recipes/Soup.txt", "Leek and potato soup. Soften the leeks in butter, add potatoes and stock, simmer for twenty minutes and blend.", "soup", &["soup", "recipe", "leek", "potato", "cooking"]),
        (Kind::Photo, "~/Pictures/Cats/Mog.jpg", "A tabby cat asleep on a windowsill in the sun, beside a potted fern.", "cat", &["cat", "window", "sleeping", "plant", "sun"]),
        (Kind::Note, "", "The spare key is with Sam next door.", "key", &["spare", "sam", "key"]),
        (Kind::Conversation, "", "Asked: Which videos show hiking?\nAnswered: Hike.mp4 shows a trail through a forest and a view from a mountain peak.", "hiking mountain", &["hiking", "videos"]),
    ];
    let home = neo_desktop::fs::home_dir();
    for (i, (kind, file, text, subject, words)) in things.iter().enumerate() {
        let path = (!file.is_empty()).then(|| home.join(file.trim_start_matches("~/")));
        let title = match (&path, kind) {
            (Some(p), _) => p.file_name().unwrap().to_string_lossy().into_owned(),
            (None, Kind::Conversation) => "Which videos show hiking?".into(),
            (None, _) => (*text).to_owned(),
        };
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let created = 1_791_400_000 + i as u64 * 4000;
        store.add(&New { kind: *kind, source: path.as_deref(), title: &title, text, part: 0, created, words: &words }, &neo_apollo_core::model::fake::vector(subject)).expect("a memory");
        // Snapshots show the files counted; tests' reading would forget them, as they are not there.
        if let Some(path) = path.as_ref().filter(|_| filed) {
            store.set_file(path, "sample", 1).expect("a file");
        }
    }
    drop(store);
    let mut app = Apollo::with(model, dir.join("memory.db"), Some(key), Arc::new(|_| Ok(())));
    app.drafts = app.chosen();
    // Shown in snapshots only: a test that reads must not read the real ones.
    if filed {
        app.settings.folders = ["Pictures", "Movies", "Documents"].iter().map(|d| Folder { path: home.join(d), on: true }).collect();
    }
    (app, dir)
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    type Shot<'a> = (&'a str, neo_desktop::SchemePref, &'a dyn Fn(&mut Apollo));
    let shots: [Shot; 7] = [
        ("apollo-ask", neo_desktop::SchemePref::Light, &|a| {
            a.update(Msg::Unlock);
            a.update(Msg::Suggest("Which photos show a dog on the beach?".into()));
        }),
        ("apollo-locked", neo_desktop::SchemePref::Dark, &|a| a.update(Msg::Page(Page::Memory))),
        ("apollo-memory", neo_desktop::SchemePref::Dark, &|a| {
            a.update(Msg::Page(Page::Memory));
            a.update(Msg::Unlock);
        }),
        ("apollo-word", neo_desktop::SchemePref::Light, &|a| {
            a.update(Msg::Page(Page::Memory));
            a.update(Msg::Unlock);
            a.update(Msg::Kind(Some(Kind::Photo)));
            a.update(Msg::Word("dog".into()));
            a.update(Msg::Word("beach".into()));
        }),
        ("apollo-sources", neo_desktop::SchemePref::Light, &|a| a.update(Msg::Page(Page::Sources))),
        ("apollo-activity", neo_desktop::SchemePref::Dark, &|a| {
            a.update(Msg::Unlock);
            a.update(Msg::Page(Page::Activity));
            let home = neo_desktop::fs::home_dir();
            for (file, looked, seconds, trouble) in [("Documents/Invoices/March.md", false, 0.1, None), ("Pictures/Trips/IMG_2041.jpg", false, 0.1, None), ("Pictures/Trips/IMG_2057.jpg", true, 8.4, None), ("Movies/Hike.mp4", true, 31.0, None), ("Pictures/old scan.tiff", true, 0.4, Some("It is not a picture that can be read."))] {
                a.log.push_back(Logged { path: home.join(file), looked, seconds, trouble: trouble.map(str::to_owned) });
            }
            a.stats.light = 4;
            (a.reading, a.working) = (true, Some((home.join("Pictures/Trips/IMG_2110.jpg"), std::time::Instant::now(), true)));
            a.progress = Progress { done: 46, total: 170, looking: true, read: 12, listed: 124, ..Progress::default() };
        }),
        ("apollo-models", neo_desktop::SchemePref::Dark, &|a| {
            a.update(Msg::Page(Page::Sources));
            let can = |name: &str, what: &[&str], away: bool| (name.to_owned(), what.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>(), away);
            a.settings.remote = "http://studio.local:11434".into();
            a.update(Msg::Installed(vec![can("gemma3:1b", &["completion"], false), can("gemma3:4b", &["completion", "vision"], false), can("nomic-embed-text:latest", &["embedding"], false), can("muse-glimmer:30b", &["completion", "vision", "tools"], true), can("qwen3:32b", &["completion", "tools"], true)]));
            a.update(Msg::ChooseModel(0, Rect::new(826.0, 250.0, MODEL_W, 34.0)));
        }),
    ];
    for (name, scheme, set) in shots {
        let (mut app, scratch) = sample(name, true);
        app.desktop.appearance.scheme = scheme;
        set(&mut app);
        // The sample's files are not there to take pictures from: stand-ins, each its own colours.
        let listed: Vec<PathBuf> = app.shown.items.iter().filter(|(m, _)| matches!(m.kind, Kind::Photo | Kind::Video)).filter_map(|(m, _)| m.source.clone()).collect();
        for (n, path) in listed.into_iter().enumerate() {
            let (w, h) = (96u32, 64u32);
            let rgba = (0..w * h).flat_map(|i| [(40 + n as u32 * 53 + i % w) as u8, (90 + i / w * 2) as u8, (200 - n as u32 * 37 % 160) as u8, 255]).collect();
            app.thumbs.insert(path, Some(Image::new(w, h, rgba)));
        }
        let mut h = Harness::new(app, Size::new(1080.0, 720.0)).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
        drop(h);
        let _ = std::fs::remove_dir_all(scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo::testing::Harness;
    use neo::{Key, Modifiers, Point};

    const WINDOW: Size = Size::new(1080.0, 720.0);

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app(name: &str) -> (Apollo, Scratch) {
        let (app, dir) = sample(name, false);
        (app, Scratch(dir))
    }

    fn titles(a: &Apollo) -> Vec<&str> {
        a.shown.items.iter().map(|(m, _)| m.title.as_str()).collect()
    }

    #[test]
    fn the_memory_is_locked_until_the_login_check_passes() {
        let (mut a, _scratch) = app("lock");
        a.check = Arc::new(|reason| {
            assert_eq!(reason, "look through Apollo's memory");
            Err("Authentication was cancelled.".into())
        });
        a.update(Msg::Page(Page::Memory));
        a.update(Msg::Unlock);
        assert!(a.store.is_none() && !a.unlocking, "refused, it stays locked");
        assert_eq!(a.trouble.as_deref(), Some("Authentication was cancelled."));
        assert!(a.shown.items.is_empty() && a.cloud.is_empty() && a.stats.total() == 0, "and nothing of it is in the app");
        // Searching while locked finds nothing and asks nothing.
        a.update(Msg::Query("dog".into()));
        a.update(Msg::Search);
        a.update(Msg::Word("dog".into()));
        assert!(a.shown.items.is_empty());
        a.update(Msg::Clear);

        a.check = Arc::new(|_| Ok(()));
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.render(1.0);
        // The Unlock button, under the words in the middle of the page.
        let before = h.app().store.is_some();
        for y in (300..460).step_by(8) {
            h.click(Point::new(neo_desktop::ui::SIDEBAR_W + (WINDOW.w - neo_desktop::ui::SIDEBAR_W) / 2.0, y as f32));
            if h.app().store.is_some() {
                break;
            }
        }
        assert!(!before && h.app().store.is_some(), "the button unlocks it");
        let a = h.app_mut();
        assert_eq!(a.trouble, None);
        assert!(a.stats.total() == 12 && !a.cloud.is_empty() && a.shown.heading == "Newest" && !a.shown.items.is_empty());
        assert_eq!(a.cloud[0].weight, 1.0);
        h.render(1.0);
        // Locked again, it is all gone from the app.
        h.app_mut().update(Msg::Lock);
        let a = h.app();
        assert!(a.store.is_none() && a.shown.items.is_empty() && a.cloud.is_empty() && a.stats.total() == 0);
        h.render(1.0);
    }

    #[test]
    fn the_memory_is_searched_by_meaning_kind_and_word() {
        let (mut a, _scratch) = app("search");
        a.update(Msg::Page(Page::Memory));
        a.update(Msg::Unlock);
        a.update(Msg::Query("  a puppy at the seaside ".into()));
        a.update(Msg::Search);
        assert_eq!(a.shown.heading, "Nearest to “a puppy at the seaside”");
        assert!(titles(&a)[0].starts_with("IMG_20"), "the beach photos of the dog come first: {:?}", titles(&a));
        assert!(a.shown.items[0].1.is_some_and(|d| d < 0.2));
        // Only videos: the search is not asked again, only narrowed.
        a.update(Msg::Kind(Some(Kind::Video)));
        assert!(!titles(&a).is_empty() && a.shown.items.iter().all(|(m, _)| m.kind == Kind::Video));
        assert_eq!(titles(&a)[0], "Hike.mp4", "the one with a dog in it");
        a.update(Msg::Kind(None));
        // The cloud is of the kind chosen: the videos' words, not the invoices'.
        let everything = a.cloud.len();
        a.update(Msg::Clear);
        a.update(Msg::Kind(Some(Kind::Video)));
        let words: Vec<&str> = a.cloud.iter().map(|w| w.word.as_str()).collect();
        assert!(words.contains(&"hiking") && words.contains(&"screen recording") && !words.contains(&"invoice") && !words.contains(&"cat") && words.len() < everything, "{words:?}");
        a.update(Msg::Word("dog".into()));
        let round: Vec<&str> = a.graph[0].related.iter().map(|(w, _)| w.as_str()).collect();
        assert!(round.contains(&"trail") && !round.contains(&"beach"), "and so are the words that go with one: {round:?}");
        a.update(Msg::Kind(Some(Kind::Document)));
        assert!(a.cloud.iter().any(|w| w.word == "invoice") && a.cloud.iter().all(|w| w.word != "hiking"));
        a.update(Msg::Clear);
        a.update(Msg::Kind(None));
        assert_eq!(a.cloud.len(), everything);
        // A word from the cloud: what is about it.
        a.update(Msg::Word("invoice".into()));
        assert_eq!((a.shown.heading.as_str(), a.trail.as_slice(), a.query.as_str()), ("About “invoice”", ["invoice".to_owned()].as_slice(), ""));
        let found = titles(&a);
        assert!(found.contains(&"March.md") && found.contains(&"April.md") && found.iter().all(|t| t.ends_with(".md")), "{found:?}");
        a.update(Msg::Clear);
        assert_eq!((a.shown.heading.as_str(), a.trail.is_empty(), a.graph.is_empty(), a.asked.is_none()), ("Newest", true, true, true));
        // An empty search is the same as clearing.
        a.update(Msg::Word("dog".into()));
        a.update(Msg::Query("   ".into()));
        a.update(Msg::Search);
        assert_eq!(a.shown.heading, "Newest");
        // Forgetting one takes it from the list and the counts.
        let (before, first) = (a.stats.total(), a.shown.items[0].0.id);
        a.update(Msg::Forget(first));
        assert_eq!(a.stats.total(), before - 1);
        assert!(a.shown.items.iter().all(|(m, _)| m.id != first));
    }

    #[test]
    fn photos_and_videos_in_the_list_show_small_pictures() {
        use neo_apollo_core::store::New;
        let (mut a, scratch) = app("thumbs");
        let photo = scratch.0.join("red.png");
        image::RgbImage::from_fn(300, 200, |x, y| image::Rgb([220, (x % 40) as u8, (y % 40) as u8])).save(&photo).unwrap();
        {
            let mut store = Store::open(&a.db, &a.key.unwrap(), a.dims).unwrap();
            store.add(&New { kind: Kind::Photo, source: Some(&photo), title: "red.png", text: "A red picture.", part: 0, created: 1_800_000_000, words: &[] }, &neo_apollo_core::model::fake::vector("red")).unwrap();
        }
        a.update(Msg::Page(Page::Memory));
        assert!(a.thumbs.is_empty(), "none while it is locked");
        a.update(Msg::Unlock);
        assert_eq!(a.shown.items[0].0.title, "red.png");
        let small = a.thumbs.get(&photo).expect("asked for").as_ref().expect("and made");
        assert_eq!((small.width(), small.height()), (THUMB_SIDE, THUMB_SIDE * 2 / 3));
        // Those whose files are not there have none, and are not asked for twice.
        let asked = a.thumbs.len();
        assert!(asked > 1 && a.thumbs.values().filter(|t| t.is_some()).count() == 1);
        assert!(a.thumbs.keys().all(|p| !p.to_string_lossy().ends_with(".md")), "documents have none to make");
        a.update(Msg::Kind(Some(Kind::Photo)));
        assert!(a.thumbs.len() <= asked);
        let mut h = Harness::new(a, WINDOW).unwrap();
        let px = h.render(1.0);
        // The first row's picture, at the left of the list, is the red of the file.
        let red = (0..px.len() / 4).filter(|i| px[i * 4] > 200 && px[i * 4 + 1] < 60 && px[i * 4 + 2] < 60).count();
        assert!(red > 1500, "the picture is drawn: {red} red pixels");
        // A picture that arrives after locking is not kept, and locking lets go of the rest.
        h.app_mut().update(Msg::Lock);
        h.app_mut().update(Msg::Thumb(photo.clone(), Some((1, 1, vec![0; 4]))));
        assert!(h.app().thumbs.is_empty());
    }

    #[test]
    fn a_word_in_the_cloud_can_be_clicked() {
        let (mut a, _scratch) = app("click");
        a.update(Msg::Page(Page::Memory));
        a.update(Msg::Unlock);
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.render(1.0);
        // Somewhere in the cloud's middle there is a word under the pointer.
        let left = neo_desktop::ui::SIDEBAR_W + 30.0;
        let spot = (0..40).map(|i| Point::new(left + 300.0 + i as f32 * 6.0, 330.0)).find(|p| {
            h.move_to(*p);
            h.cursor() == neo::CursorIcon::Pointer
        });
        let spot = spot.expect("a word says it can be clicked");
        h.click(spot);
        let a = h.app();
        assert_eq!(a.trail.len(), 1, "the word is picked");
        let word = a.trail[0].clone();
        assert!(a.cloud.iter().any(|w| w.word == word) && a.graph.len() == 1 && a.graph[0].word == word);
        assert_eq!(a.shown.heading, format!("About “{word}”"));
        assert!(!a.shown.items.is_empty());
        h.render(1.0);
    }

    #[test]
    fn picking_words_grows_a_graph_of_the_words_that_go_together() {
        let (mut a, _scratch) = app("graph");
        a.update(Msg::Page(Page::Memory));
        a.update(Msg::Unlock);
        a.update(Msg::Word("dog".into()));
        assert_eq!(a.graph.len(), 1);
        let round: Vec<&str> = a.graph[0].related.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(round[0], "beach", "the word that shares most memories with it first: {round:?}");
        assert!(round.contains(&"hiking") && round.contains(&"summer") && !round.contains(&"invoice") && !round.contains(&"dog"));
        assert!(a.graph[0].related[0].1 == 1.0 && a.graph[0].related.iter().all(|(_, w)| *w > 0.0 && *w <= 1.0));
        // One of those picked too: both are in the graph, and the list is of the newer.
        a.update(Msg::Word("beach".into()));
        assert_eq!((a.trail.clone(), a.graph.iter().map(|h| h.word.as_str()).collect::<Vec<_>>(), a.shown.heading.as_str()), (vec!["dog".to_owned(), "beach".to_owned()], vec!["dog", "beach"], "About “beach”"));
        assert!(a.graph[1].related.iter().any(|(w, _)| w == "dog") && a.graph[1].related.iter().any(|(w, _)| w == "picnic"));
        a.update(Msg::Word("picnic".into()));
        a.update(Msg::Word("kite".into()));
        assert_eq!((a.trail.len(), a.graph.len(), a.graph[0].word.as_str()), (4, HUBS, "beach"), "the graph keeps to the last few");
        // One step back at a time, and forward again.
        a.update(Msg::Back);
        assert_eq!((a.trail.len(), a.shown.heading.as_str()), (3, "About “picnic”"));
        a.update(Msg::Back);
        a.update(Msg::Back);
        assert_eq!(a.trail, ["dog".to_owned()]);
        a.update(Msg::Back);
        assert!(a.trail.is_empty() && a.graph.is_empty() && a.shown.heading == "Newest", "back to the cloud it began from");
        a.update(Msg::Back);
        assert_eq!(a.at, 0, "and no further");
        a.update(Msg::Forward);
        a.update(Msg::Forward);
        assert_eq!((a.trail.clone(), a.shown.heading.as_str()), (vec!["dog".to_owned(), "beach".to_owned()], "About “beach”"));
        // Picking from here drops what was ahead.
        a.update(Msg::Word("sea".into()));
        assert_eq!((a.history.len(), a.at), (4, 3));
        a.update(Msg::Forward);
        assert_eq!(a.trail.last().map(String::as_str), Some("sea"), "nothing ahead to go to");
        a.update(Msg::Back);
        a.update(Msg::Forward);
        a.update(Msg::Forward);
        a.update(Msg::Back);
        a.update(Msg::Word("picnic".into()));
        a.update(Msg::Word("kite".into()));
        // A picked word again goes back to it.
        a.update(Msg::Word("beach".into()));
        assert_eq!((a.trail.clone(), a.shown.heading.as_str()), (vec!["dog".to_owned(), "beach".to_owned()], "About “beach”"));
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.render(1.0);
        // The two picked words sit side by side; clicking the first goes back to it.
        let (left, wide) = (neo_desktop::ui::SIDEBAR_W + 28.0, WINDOW.w - neo_desktop::ui::SIDEBAR_W - 12.0 - 56.0);
        let first = Point::new(left + wide / 4.0, 170.0 + 10.0 + CLOUD_H / 2.0);
        h.move_to(first);
        assert_eq!(h.cursor(), neo::CursorIcon::Pointer);
        h.click(first);
        assert_eq!(h.app().trail, ["dog".to_owned()]);
        h.app_mut().update(Msg::Clear);
        assert!(h.app().graph.is_empty() && h.app().trail.is_empty(), "and back to the cloud");
        // Which is itself a step: back from it is the graph again.
        h.app_mut().update(Msg::Back);
        assert_eq!(h.app().trail, ["dog".to_owned()]);
        h.app_mut().update(Msg::Lock);
        assert_eq!((h.app().history.len(), h.app().at), (1, 0));
        h.render(1.0);
    }

    #[test]
    fn apollo_answers_from_memory_only_once_it_is_unlocked() {
        let (mut a, _scratch) = app("ask");
        a.settings.remember_conversations = false;
        a.update(Msg::Draft("Where is the photo of the dog on the beach?".into()));
        a.update(Msg::Send);
        assert_eq!((a.talk.len(), a.answering, a.draft.as_str()), (2, false, ""));
        assert_eq!(a.talk[1].text, "You asked: Where is the photo of the dog on the beach? I looked at 0 memories.");
        assert!(a.talk[1].recalled.is_empty(), "locked, it is shown nothing");
        a.update(Msg::Unlock);
        let before = a.stats.total();
        a.update(Msg::Suggest("Where is the photo of the dog on the beach?".into()));
        let answer = a.talk.last().unwrap();
        assert!(!answer.recalled.is_empty() && answer.text.ends_with("memories.") && !answer.text.contains(" 0 memories"));
        assert!(answer.recalled[0].memory.title.starts_with("IMG_20"));
        // The photos it listed are to be shown beneath it as pictures, not named.
        let drawn_on = assistant::sources(&answer.listed, &answer.recalled);
        assert!(!drawn_on.is_empty() && drawn_on.iter().all(|m| m.kind == Kind::Photo && a.thumbs.contains_key(m.source.as_ref().unwrap())), "pictures of them were asked for");
        assert_eq!(a.stats.total(), before, "with remembering off, what was asked is not kept");
        // With it on, it is, and what Apollo is told goes in as a note.
        a.settings.remember_conversations = true;
        a.update(Msg::Suggest("What about the mountain?".into()));
        assert_eq!((a.stats.total(), a.stats.of(Kind::Conversation)), (before + 1, 2));
        a.update(Msg::Suggest("Remember that the bins go out on Thursday".into()));
        assert_eq!(a.talk.last().unwrap().text, "I'll remember that.");
        assert_eq!(a.stats.of(Kind::Note), 2);
        assert_eq!(a.store.as_ref().unwrap().recent(1, Some(Kind::Note)).unwrap()[0].text, "the bins go out on Thursday");
        // Nothing is sent while one is being answered, or with nothing to ask.
        let said = a.talk.len();
        a.update(Msg::Draft("   ".into()));
        a.update(Msg::Send);
        assert_eq!(a.talk.len(), said);
        a.update(Msg::NewTalk);
        assert!(a.talk.is_empty());
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.render(1.0);
        h.type_text("What dog?");
        h.key(Key::Enter, Modifiers::default());
        assert_eq!(h.app().talk.len(), 2, "typed and sent from the keyboard");
        h.render(1.0);
    }

    #[test]
    fn a_question_from_outside_is_asked_when_it_can_be() {
        let (mut a, _scratch) = app("outside");
        // Ready and free: asked at once, on the page where the answer shows.
        a.update(Msg::Page(Page::Sources));
        a.update(Msg::Ask("What dog?".into()));
        assert_eq!((a.page, a.talk.len(), a.talk[0].text.as_str(), a.waiting_question.clone()), (Page::Ask, 2, "What dog?", None));
        // With the model not there yet, it waits in the box, and is asked when the model is.
        a.update(Msg::Ready(Engine::Checking, 0));
        a.update(Msg::Ask("And the beach?".into()));
        assert_eq!((a.talk.len(), a.draft.as_str(), a.waiting_question.as_deref()), (2, "And the beach?", Some("And the beach?")));
        a.update(Msg::Ready(Engine::Ready, 8));
        assert_eq!((a.talk.len(), a.talk[2].text.as_str(), a.waiting_question.clone(), a.draft.as_str()), (4, "And the beach?", None, ""));
        // One that comes while another is being answered is asked after it.
        a.answering = true;
        a.talk.push(Said { role: Role::User, text: "First".into(), recalled: vec![], listed: vec![] });
        a.talk.push(Said { role: Role::Assistant, text: String::new(), recalled: vec![], listed: vec![] });
        a.update(Msg::Ask("Second".into()));
        assert_eq!((a.talk.len(), a.waiting_question.as_deref()), (6, Some("Second")));
        a.update(Msg::Answered(Ok(Answer { text: "Done.".into(), recalled: vec![], listed: vec![] })));
        assert_eq!((a.talk.len(), a.talk[6].text.as_str(), a.talk[5].text.as_str(), a.waiting_question.clone()), (8, "Second", "Done.", None));
    }

    #[test]
    fn apollo_waits_out_of_sight_and_the_memory_locks_itself_when_left() {
        let (mut a, scratch) = app("background");
        a.settings_file = Some(scratch.0.join("settings"));
        assert!(a.settings.background && a.settings.stay_unlocked == 15 && a.window_state().visible);
        // Closing the window only puts it away; a question brings it back.
        let closing = a.on_close();
        assert!(matches!(closing, Some(Msg::Hide)));
        a.update(Msg::Hide);
        assert!(!a.window_state().visible && !a.should_exit());
        // A click on its Dock icon brings the window back, as a question does.
        let mut h = Harness::new(a, WINDOW).unwrap();
        assert!(!h.window_state().visible);
        h.reopen();
        assert!(h.window_state().visible, "shown again");
        let mut a = std::mem::replace(h.app_mut(), sample("background-swap", false).0);
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("neo-apollo-app-background-swap-{}", std::process::id())));
        a.update(Msg::Hide);
        // Locked, a question brings up the login check once, and is answered from the memory.
        static CHECKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        a.check = Arc::new(|_| {
            CHECKS.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        a.update(Msg::Ask("Which photos show a dog on the beach?".into()));
        assert!(a.window_state().visible && a.store.is_some());
        assert_eq!((CHECKS.load(Ordering::Relaxed), a.talk.len(), a.talk[1].recalled.is_empty() && a.talk[1].listed.is_empty()), (1, 2, false));
        // Another soon after needs no check.
        a.update(Msg::Hide);
        a.update(Msg::Ask("And the mountain?".into()));
        assert_eq!((CHECKS.load(Ordering::Relaxed), a.talk.len()), (1, 4));
        // Left unused for as long as it is set to stay unlocked, it locks; not before, and not mid-answer.
        a.update(Msg::Tick);
        assert!(a.store.is_some());
        a.last_used = std::time::Instant::now().checked_sub(Duration::from_secs(16 * 60)).unwrap();
        a.answering = true;
        a.update(Msg::Tick);
        assert!(a.store.is_some());
        a.answering = false;
        a.update(Msg::Tick);
        assert!(a.store.is_none(), "locked again");
        // The next question asks for the login again; refused, it is answered without the memory.
        a.check = Arc::new(|_| Err("Authentication was cancelled.".into()));
        a.update(Msg::Ask("What about the cat?".into()));
        assert_eq!((a.talk.len(), a.store.is_none(), a.declined, a.waiting_question.clone()), (6, true, false, None));
        assert!(a.talk[5].text.ends_with("0 memories."));
        // From the menu bar: open the window, lock the memory, read, quit.
        a.check = Arc::new(|_| Ok(()));
        a.update(Msg::Unlock);
        a.update(Msg::Hide);
        assert_eq!(a.tray_now(), TrayState { status: "Up to date".into(), unlocked: true, can_read: true });
        a.update(Msg::Tray(TrayAction::Open));
        assert!(a.window_state().visible);
        a.update(Msg::Tray(TrayAction::Lock));
        assert!(a.store.is_none() && !a.tray_now().unlocked);
        a.update(Msg::Tray(TrayAction::Read));
        assert!(!a.reading && a.tray.is_none(), "a reading done; and no icon is made in a test");
        // Set to stay unlocked until it quits, time does not lock it.
        a.check = Arc::new(|_| Ok(()));
        a.update(Msg::Unlock);
        a.update(Msg::StayUnlocked(0));
        a.last_used = std::time::Instant::now().checked_sub(Duration::from_secs(3 * 3600)).unwrap();
        a.update(Msg::Tick);
        assert!(a.store.is_some() && Settings::load_from(&scratch.0.join("settings")).stay_unlocked == 0);
        // Set not to stay ready, closing the window ends it; and Quit always does.
        a.update(Msg::Background(false));
        assert!(a.on_close().is_none() && !Settings::load_from(&scratch.0.join("settings")).background);
        a.update(Msg::Show);
        a.update(Msg::Tray(TrayAction::Quit));
        assert!(a.should_exit());
    }

    #[test]
    fn an_answer_that_fails_puts_the_question_back() {
        let (mut a, _scratch) = app("fail");
        a.update(Msg::Draft("Hello".into()));
        a.answering = true;
        a.talk.push(Said { role: Role::User, text: "Hello".into(), recalled: vec![], listed: vec![] });
        a.talk.push(Said { role: Role::Assistant, text: String::new(), recalled: vec![], listed: vec![] });
        a.update(Msg::Piece("Hi".into()));
        assert_eq!(a.talk[1].text, "Hi", "an answer shows as it comes");
        a.update(Msg::Answered(Err("The model server did not answer.".into())));
        assert_eq!((a.talk.len(), a.draft.as_str(), a.trouble.as_deref(), a.answering), (0, "Hello", Some("The model server did not answer."), false));
        // A piece that comes after the end is not added to anything.
        a.update(Msg::Piece("late".into()));
        assert!(a.talk.is_empty());
    }

    #[test]
    fn folders_are_read_and_the_list_of_them_is_kept() {
        let (mut a, scratch) = app("read");
        let folder = scratch.0.join("things");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("trail.txt"), "Notes on hiking the mountain trail in the snow.").unwrap();
        let file = scratch.0.join("settings");
        a.settings_file = Some(file.clone());
        a.settings.folders = vec![Folder { path: folder.clone(), on: true }];
        a.update(Msg::Unlock);
        let before = a.stats.of(Kind::Document);
        a.update(Msg::Read);
        assert_eq!((a.reading, a.progress.read, a.progress.total, a.progress.trouble.clone()), (false, 1, 1, None));
        assert_eq!(a.stats.of(Kind::Document), before + 1, "what was read shows at once in a memory that is open");
        assert_eq!(a.status(), "Up to date");
        // Short of memory, nothing is read, and it says why; with room again, it is.
        std::fs::write(folder.join("more.txt"), "Another note about the beach and the sea.").unwrap();
        a.short_of_memory = || true;
        a.update(Msg::Read);
        assert_eq!((a.reading, a.held, a.status(), a.stats.of(Kind::Document)), (false, true, "Paused: memory is short".to_owned(), before + 1));
        a.short_of_memory = || false;
        a.update(Msg::Read);
        assert_eq!((a.held, a.status(), a.stats.of(Kind::Document)), (false, "Up to date".to_owned(), before + 2));
        std::fs::remove_file(folder.join("more.txt")).unwrap();
        a.update(Msg::Read);
        assert_eq!(a.stats.of(Kind::Document), before + 1);
        // Told of a change, only that file is read, and nothing else is looked for.
        std::fs::write(folder.join("told.txt"), "A note about a cat and a kitten.").unwrap();
        std::fs::write(folder.join("untold.txt"), "A note nobody mentioned, about code.").unwrap();
        a.update(Msg::Changed(vec![folder.join("told.txt"), folder.join("told.txt")]));
        assert_eq!((a.progress.read, a.progress.total, a.stats.of(Kind::Document), a.pending.len()), (1, 1, before + 2, 0));
        // While memory is short, changes wait; a full look later finds them and what was not mentioned.
        std::fs::write(folder.join("later.txt"), "Another, about snow on a peak.").unwrap();
        a.short_of_memory = || true;
        a.update(Msg::Changed(vec![folder.join("later.txt")]));
        assert_eq!((a.held, a.stats.of(Kind::Document)), (true, before + 2));
        a.short_of_memory = || false;
        a.update(Msg::Read);
        assert_eq!((a.progress.read, a.stats.of(Kind::Document), a.pending.len()), (2, before + 4, 0));
        // Gone again, told of: forgotten.
        for name in ["told.txt", "untold.txt", "later.txt"] {
            std::fs::remove_file(folder.join(name)).unwrap();
        }
        a.update(Msg::Changed(vec![folder.join("told.txt"), folder.join("untold.txt"), folder.join("later.txt")]));
        assert_eq!((a.progress.forgotten, a.stats.of(Kind::Document)), (3, before + 1));
        // With reading by itself turned off, changes are not acted on.
        a.settings.read_automatically = false;
        std::fs::write(folder.join("quiet.txt"), "Not to be read unasked.").unwrap();
        a.update(Msg::Changed(vec![folder.join("quiet.txt")]));
        assert_eq!(a.stats.of(Kind::Document), before + 1);
        std::fs::remove_file(folder.join("quiet.txt")).unwrap();
        a.settings.read_automatically = true;
        a.pending.clear();
        // Turned off, the folder's memories are forgotten; the choice is saved.
        a.update(Msg::FolderOn(0, false));
        assert_eq!((a.stats.of(Kind::Document), a.progress.forgotten), (before, 1));
        assert_eq!(Settings::load_from(&file).folders, [Folder { path: folder.clone(), on: false }]);
        a.update(Msg::RememberTalk(false));
        a.update(Msg::RemoveFolder(0));
        a.update(Msg::RemoveFolder(7));
        let saved = Settings::load_from(&file);
        assert!(saved.folders.is_empty() && !saved.remember_conversations);
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.app_mut().update(Msg::Page(Page::Sources));
        h.render(1.0);
    }

    #[test]
    fn a_model_that_is_not_there_is_said_and_nothing_is_asked_of_it() {
        let (mut a, _scratch) = app("model");
        for (ready, status) in [(Engine::Checking, "Starting the model…"), (Engine::Trouble("Ollama is not installed.".into()), "The model is not running"), (Engine::Missing(vec!["gemma3:4b".into()]), "The model needs downloading"), (Engine::Pulling { model: "gemma3:4b".into(), status: "pulling".into(), done: 1 << 30, total: 3 << 30 }, "Downloading the model…")] {
            a.update(Msg::Ready(ready.clone(), 0));
            assert_eq!(a.status(), status);
            assert!(a.setup().is_some());
            a.update(Msg::Suggest("Hello".into()));
            a.update(Msg::Read);
            assert!(a.talk.is_empty() && !a.reading && a.draft == "Hello", "nothing is asked of a model that is not there");
            let mut h = Harness::new(a, WINDOW).unwrap();
            h.render(1.0);
            h.app_mut().update(Msg::Page(Page::Sources));
            h.render(1.0);
            (a, _) = (std::mem::replace(h.app_mut(), sample("model-swap", false).0), ());
            a.update(Msg::Page(Page::Ask));
        }
        // Once it is there, the folders are read straight away.
        a.update(Msg::Ready(Engine::Ready, 8));
        assert_eq!((a.status(), a.setup().is_none()), ("Up to date".into(), true));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("neo-apollo-app-model-swap-{}", std::process::id())));
    }

    /// By hand, with the real model, on a folder of your choosing:
    /// `NEO_APOLLO_PROBE=folder NEO_APOLLO_ASK="question" NEO_SNAPSHOT_DIR=dir cargo test -p neo-apollo probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_a_real_folder() {
        let folder = PathBuf::from(std::env::var("NEO_APOLLO_PROBE").expect("NEO_APOLLO_PROBE"));
        let dir = std::env::temp_dir().join(format!("neo-apollo-probe-{}", std::process::id()));
        let model = Arc::new(Ollama::from_settings(&Settings::default()));
        model.start().unwrap();
        let mut a = Apollo::with(model, dir.join("memory.db"), Some([9; 32]), Arc::new(|_| Ok(())));
        a.settings.folders = vec![Folder { path: folder, on: true }];
        a.settings.remember_conversations = false;
        // Not on a computer that is short of memory.
        a.short_of_memory = neo_apollo_core::pressure::short_of_memory;
        let began = std::time::Instant::now();
        a.update(Msg::Read);
        println!("read in {:?}: {:?}", began.elapsed(), a.progress);
        a.update(Msg::Page(Page::Memory));
        a.update(Msg::Unlock);
        println!("{:?}", a.stats);
        for m in a.store.as_ref().unwrap().recent(50, None).unwrap() {
            println!("- {} [{}] {} :: {:?}", m.title, m.kind.name(), snippet(&m.text, 300), a.store.as_ref().unwrap().words_of(m.id).unwrap());
        }
        println!("cloud: {:?}", a.cloud.iter().map(|w| (w.word.as_str(), w.count, &w.also)).collect::<Vec<_>>());
        let shots = std::env::var("NEO_SNAPSHOT_DIR").ok().map(PathBuf::from);
        let shoot = |a: Apollo, name: &str| -> Apollo {
            let Some(dir) = &shots else { return a };
            let mut h = Harness::new(a, WINDOW).unwrap();
            h.save_png(dir.join(format!("{name}.png")), 1.0).unwrap();
            std::mem::replace(h.app_mut(), sample("probe-swap", false).0)
        };
        a = shoot(a, "probe-cloud");
        if let Some(word) = a.cloud.first().map(|w| w.word.clone()) {
            a.update(Msg::Word(word));
            println!("graph: {:?}", a.graph);
            if let Some(next) = a.graph[0].related.first().map(|r| r.0.clone()) {
                a.update(Msg::Word(next));
            }
            a = shoot(a, "probe-graph");
            a.update(Msg::Clear);
        }
        for q in std::env::var("NEO_APOLLO_SEARCH").unwrap_or_default().split('|').filter(|q| !q.is_empty()) {
            a.update(Msg::Query(q.into()));
            a.update(Msg::Search);
            println!("search {q:?}: {:?}", a.shown.items.iter().take(5).map(|(m, d)| (m.title.as_str(), d.map(|d| (d * 1000.0).round() / 1000.0))).collect::<Vec<_>>());
        }
        a.update(Msg::Clear);
        if let Ok(q) = std::env::var("NEO_APOLLO_ASK") {
            let began = std::time::Instant::now();
            a.update(Msg::Page(Page::Ask));
            a.update(Msg::Suggest(q));
            let said = a.talk.last().unwrap();
            println!("answered in {:?}: {}\n  from {:?}; trouble {:?}", began.elapsed(), said.text, said.recalled.iter().map(|h| (h.memory.title.as_str(), h.distance)).collect::<Vec<_>>(), a.trouble);
            println!("  shown as: {}", assistant::plain(&said.text));
            println!("  listed {:?}; named {:?}", said.listed.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), assistant::sources(&said.listed, &said.recalled).iter().map(|m| m.title.as_str()).collect::<Vec<_>>());
            a.model.rest_all();
            shoot(a, "probe-ask");
        }
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("neo-apollo-app-probe-swap-{}", std::process::id())));
    }

    /// The test model under another name: its embeddings are the same
    /// length, but a memory made with one must not be used with the other.
    struct Renamed(neo_apollo_core::model::fake::Fake);
    impl Model for Renamed {
        fn name(&self) -> String {
            "another".into()
        }
        fn dims(&self) -> Result<usize, String> {
            self.0.dims()
        }
        fn embed(&self, texts: &[String], question: bool) -> Result<Vec<Vec<f32>>, String> {
            self.0.embed(texts, question)
        }
        fn describe(&self, jpeg: &[u8]) -> Result<neo_apollo_core::model::Seen, String> {
            self.0.describe(jpeg)
        }
        fn chat(&self, turns: &[Turn], piece: &mut dyn FnMut(&str) -> bool) -> Result<String, String> {
            self.0.chat(turns, piece)
        }
    }

    #[test]
    fn models_can_be_changed_and_a_memory_keeps_to_the_one_it_was_made_with() {
        let (mut a, scratch) = app("models");
        let file = scratch.0.join("settings");
        a.settings_file = Some(file.clone());
        assert_eq!(a.drafts, ["gemma3:1b".to_owned(), "gemma3:4b".to_owned(), "nomic-embed-text".to_owned()]);
        // Typed and put to use, the choice is saved; one left empty is not taken.
        a.update(Msg::ModelDraft(0, " qwen3:8b ".into()));
        a.update(Msg::ModelDraft(1, "   ".into()));
        a.update(Msg::UseModels);
        assert_eq!(a.settings.chat_model, "gemma3:1b", "not with one of them empty");
        a.update(Msg::ModelDraft(1, "qwen3-vl:8b".into()));
        a.update(Msg::UseModels);
        assert_eq!((a.settings.chat_model.as_str(), a.settings.vision_model.as_str(), a.settings.embed_model.as_str()), ("qwen3:8b", "qwen3-vl:8b", "nomic-embed-text"));
        let saved = Settings::load_from(&file);
        assert_eq!((saved.chat_model, saved.vision_model), ("qwen3:8b".to_owned(), "qwen3-vl:8b".to_owned()));
        // The menu for each job offers the models that can do it.
        let can = |name: &str, what: &[&str]| (name.to_owned(), what.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>(), false);
        a.update(Msg::Installed(vec![can("qwen3:8b", &["completion", "tools"]), can("gemma3:4b", &["completion", "vision"]), can("nomic-embed-text:latest", &["embedding"]), can("mystery", &[])]));
        assert_eq!((a.models_for(0, false), a.models_for(1, false), a.models_for(2, false)), (vec!["qwen3:8b", "gemma3:4b", "mystery"], vec!["gemma3:4b", "mystery"], vec!["nomic-embed-text:latest", "mystery"]));
        a.update(Msg::ChooseModel(1, Rect::new(700.0, 400.0, 230.0, 32.0)));
        assert_eq!(a.choosing, Some((1, Point::new(700.0, 436.0))), "the menu hangs under its control");
        assert_eq!(a.model_menu(1).len(), 4, "the two that can see, a line, and another by name");
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.app_mut().update(Msg::Page(Page::Sources));
        h.render(1.0);
        h.key(Key::Escape, Modifiers::default());
        let mut a = std::mem::replace(h.app_mut(), sample("models-menu", false).0);
        assert_eq!(a.choosing, None, "Escape closes it");
        // Picked from the menu, a model is put to use at once.
        a.update(Msg::ChooseModel(1, Rect::new(700.0, 400.0, 230.0, 32.0)));
        a.update(Msg::PickModel(1, "gemma3:4b".into(), false));
        assert_eq!((a.choosing, a.settings.vision_model.as_str(), Settings::load_from(&file).vision_model.as_str()), (None, "gemma3:4b", "gemma3:4b"));
        // One that is not in the menu is typed.
        a.update(Msg::OtherModel(1, false));
        assert_eq!(a.typing, Some(1));
        a.update(Msg::ModelDraft(1, "qwen3-vl:8b".into()));
        a.update(Msg::UseModels);
        assert_eq!((a.typing, a.settings.vision_model.as_str()), (None, "qwen3-vl:8b"));
        // With another computer named, its models are offered under its name, and a job can be given to it.
        assert_eq!(a.model_menu(0).len(), 5, "without one, only what is here");
        a.update(Msg::RemoteDraft(" http://studio.local:11434/ ".into()));
        a.update(Msg::UseRemote);
        assert_eq!((a.settings.remote.as_str(), a.remote_name().as_str(), Settings::load_from(&file).remote.as_str()), ("http://studio.local:11434", "studio.local", "http://studio.local:11434"));
        let mut have = a.installed.clone();
        have.push(("muse-glimmer:30b".into(), vec!["completion".into(), "vision".into()], true));
        a.update(Msg::Installed(have));
        assert_eq!((a.models_for(0, true), a.models_for(2, true)), (vec!["muse-glimmer:30b"], Vec::<&str>::new()));
        assert_eq!(a.model_menu(0).len(), 10, "this computer's under one heading, the other's under another, and a name for either");
        a.update(Msg::PickModel(0, "muse-glimmer:30b".into(), true));
        assert_eq!((a.settings.chat_model.as_str(), a.away(), Settings::load_from(&file).chat_remote), ("muse-glimmer:30b", [true, false, false], true));
        // The same model picked here instead comes back; so does everything when the address is taken away.
        a.update(Msg::OtherModel(1, true));
        a.update(Msg::ModelDraft(1, "llava:34b".into()));
        a.update(Msg::UseModels);
        assert_eq!((a.settings.vision_model.as_str(), a.away()), ("llava:34b", [true, true, false]));
        a.update(Msg::PickModel(1, "llava:34b".into(), false));
        assert_eq!(a.away(), [true, false, false]);
        a.update(Msg::RemoteDraft(String::new()));
        a.update(Msg::UseRemote);
        let saved = Settings::load_from(&file);
        assert_eq!((a.away(), saved.remote.as_str(), saved.chat_remote), ([false; 3], "", false), "nothing is left pointing at a computer that is not named");
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("neo-apollo-app-models-menu-{}", std::process::id())));

        // The memory is marked as the first model's when it is first used.
        let folder = scratch.0.join("things");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("trail.txt"), "Notes on hiking the mountain trail.").unwrap();
        a.settings.folders = vec![Folder { path: folder, on: true }];
        a.update(Msg::Read);
        assert_eq!((a.progress.read, a.stale.clone()), (1, None));
        // With another model's embeddings it is not read into, searched or opened.
        a.model = Arc::new(Renamed(Default::default()));
        a.update(Msg::Read);
        let why = a.stale.clone().expect("said to be another model's");
        assert_eq!(why, "The memory was made with fake, and another is chosen now.");
        assert_eq!((a.status(), a.progress.trouble.clone()), ("Memory is another model's".to_owned(), None));
        a.update(Msg::Unlock);
        assert!(a.store.is_none() && a.memory().is_none());
        a.update(Msg::Suggest("Where is the dog?".into()));
        assert!(a.talk.last().unwrap().recalled.is_empty(), "it answers, but not from a memory it cannot read");
        // Asked first; cancelled, nothing is lost.
        a.update(Msg::AskReset);
        assert!(a.asking_reset);
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.app_mut().update(Msg::Page(Page::Sources));
        h.render(1.0);
        h.key(Key::Escape, Modifiers::default());
        let mut a = std::mem::replace(h.app_mut(), sample("models-swap", false).0);
        assert!(!a.asking_reset && a.stale.is_some() && a.db.is_file());
        // Agreed to, the memory is begun afresh with the new model and read into.
        a.update(Msg::AskReset);
        a.update(Msg::Reset);
        assert_eq!((a.stale.clone(), a.asking_reset, a.progress.read), (None, false, 1));
        a.update(Msg::Unlock);
        assert_eq!((a.stats.total(), a.stats.of(Kind::Document)), (1, 1), "only what was read again; the rest is gone");
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("neo-apollo-app-models-swap-{}", std::process::id())));
    }

    /// A picture the test model takes for a dog on a beach (red), a mountain (green) or the sea (blue).
    fn paint(path: &Path, colour: [u8; 3]) {
        image::RgbImage::from_fn(240, 160, |x, y| if x < 8 && y < 8 { image::Rgb(colour) } else { image::Rgb([colour[0] ^ ((x * 7 + y * 13) % 31) as u8, colour[1] ^ ((x * 3 + y * 5) % 29) as u8, colour[2] ^ ((x + y * 11) % 23) as u8]) }).save(path).unwrap();
    }

    #[test]
    fn pictures_are_listed_at_once_and_looked_at_when_something_asks_after_them() {
        let (mut a, scratch) = app("lazy");
        let folder = scratch.0.join("Trips");
        std::fs::create_dir_all(&folder).unwrap();
        for (i, colour) in [[220u8, 40, 40], [40, 220, 40], [40, 40, 220]].into_iter().enumerate() {
            paint(&folder.join(format!("IMG_{i}.png")), colour);
        }
        std::fs::write(folder.join("plan.txt"), "The invoice from the supplier.").unwrap();
        a.settings.folders = vec![Folder { path: folder.clone(), on: true }];
        a.update(Msg::Unlock);
        let photos = a.stats.of(Kind::Photo);
        // Reading lists the pictures and reads the document; nothing is looked at.
        a.update(Msg::Read);
        assert_eq!((a.progress.read, a.progress.listed, a.stats.light, a.stats.of(Kind::Photo)), (1, 3, 3, photos + 3));
        assert_eq!(a.log.iter().map(|l| (l.path.file_name().unwrap().to_str().unwrap(), l.looked)).collect::<Vec<_>>(), [("plan.txt", false), ("IMG_0.png", false), ("IMG_1.png", false), ("IMG_2.png", false)], "the Activity page's account of it");
        assert_eq!(a.seconds_a_look(), None);
        // A search turns them up by name; they are looked at behind it, and the list is of what they show.
        a.update(Msg::Page(Page::Memory));
        a.update(Msg::Query("IMG trips".into()));
        a.update(Msg::Search);
        assert_eq!(a.stats.light, 0, "the three it turned up were looked at");
        assert!(a.log.iter().filter(|l| l.looked).count() == 3 && a.seconds_a_look().is_some());
        a.update(Msg::Query("a dog on the beach".into()));
        a.update(Msg::Search);
        assert_eq!((a.shown.items[0].0.text.as_str(), a.shown.items[0].0.light), ("A dog running on a beach.", false), "and so found by what they show");
        assert_eq!(a.asked_after.len(), 3, "each sent once");
        // A question looks at what it turns up before answering, and says so while it does.
        // (With no search showing: one that is would have it looked at for its list.)
        a.update(Msg::Clear);
        paint(&folder.join("IMG_9.png"), [220, 40, 40]);
        a.update(Msg::Changed(vec![folder.join("IMG_9.png")]));
        assert_eq!((a.progress.listed, a.stats.light), (1, 1), "a new picture is listed as it arrives");
        a.update(Msg::Looking(0, 2));
        assert_eq!(a.looking, Some((0, 2)));
        a.update(Msg::Suggest("What photos do I have from this year?".into()));
        assert_eq!((a.stats.light, a.looking, a.answering), (0, None, false));
        assert!(a.talk.last().unwrap().listed.iter().all(|m| !m.light));
        // Chosen, everything is looked at up front, and what arrives after is too.
        a.settings_file = Some(scratch.0.join("settings"));
        paint(&folder.join("IMG_10.png"), [40, 220, 40]);
        a.update(Msg::LookAhead(true));
        assert_eq!((a.stats.light, a.progress.looking, Settings::load_from(&scratch.0.join("settings")).look_ahead), (0, true, true));
        paint(&folder.join("IMG_11.png"), [40, 40, 220]);
        a.update(Msg::Changed(vec![folder.join("IMG_11.png")]));
        assert_eq!((a.stats.light, a.progress.read), (0, 1));
        // The page that tells of all this is reached from the status in the corner, and draws locked or not.
        assert_eq!(a.status(), "Up to date");
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.render(1.0);
        h.click(Point::new(80.0, WINDOW.h - 34.0));
        assert_eq!((h.app().page, h.title().as_str()), (Page::Activity, "Activity · Apollo"));
        h.render(1.0);
        h.app_mut().update(Msg::Lock);
        h.render(1.0);
    }

    #[test]
    fn warnings_are_told_once_and_kept_for_the_activity_page() {
        use std::sync::atomic::AtomicUsize;
        static TOLD: AtomicUsize = AtomicUsize::new(0);
        let (mut a, scratch) = app("warn");
        a.tell = |w| {
            assert!(!w.title.is_empty());
            TOLD.fetch_add(1, Ordering::Relaxed);
            true
        };
        let using = [warden::Using { name: "MeadowBoot".into(), processes: 8, bytes: 73 << 30 }];
        let short = warden::memory_warnings(4, 16 << 30, &using);
        a.update(Msg::Warned(short.clone()));
        a.update(Msg::Warned(short.clone()));
        assert_eq!((TOLD.load(Ordering::Relaxed), a.warnings.len(), a.warnings[0].title.as_str()), (1, 1, "Memory is running short"), "the same thing is not said twice running");
        // Another kind of thing is said; and the first again once it has been quiet long enough.
        a.update(Msg::Warned(warden::disk_warnings(1 << 30, 460 << 30)));
        *a.warned.get_mut("memory").unwrap() = std::time::Instant::now().checked_sub(Duration::from_secs(16 * 60)).unwrap();
        a.update(Msg::Warned(short.clone()));
        assert_eq!((TOLD.load(Ordering::Relaxed), a.warnings.len()), (3, 3));
        // A key lying in a file that is read is warned of, with the file, and left out of the memory.
        let folder = scratch.0.join("docs");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("deploy.md"), "To deploy:\n\nexport TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789\n").unwrap();
        a.settings.folders = vec![Folder { path: folder.clone(), on: true }];
        a.update(Msg::Read);
        let last = a.warnings.last().unwrap();
        assert_eq!((last.title.as_str(), last.file.clone(), last.security, TOLD.load(Ordering::Relaxed)), ("deploy.md holds what looks like a GitHub token", Some(folder.join("deploy.md")), true, 4));
        a.update(Msg::Unlock);
        assert!(a.store.as_ref().unwrap().recent(1, Some(Kind::Document)).unwrap()[0].text.contains("[a GitHub token, left out]"));
        // The page shows them; turned off, nothing more is said or looked for.
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.app_mut().update(Msg::Page(Page::Activity));
        h.render(1.0);
        let mut a = std::mem::replace(h.app_mut(), sample("warn-swap", false).0);
        a.settings_file = Some(scratch.0.join("settings"));
        a.update(Msg::WarnMe(false));
        a.update(Msg::Warned(warden::disk_warnings(1 << 20, 460 << 30)));
        a.update(Msg::Watch);
        assert_eq!((a.warnings.len(), TOLD.load(Ordering::Relaxed), Settings::load_from(&scratch.0.join("settings")).warn), (4, 4, false));
        assert_eq!((quiet_for(&short[0]), quiet_for(last_of(&a))), (Duration::from_secs(900), Duration::from_secs(86_400)));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("neo-apollo-app-warn-swap-{}", std::process::id())));
    }

    fn last_of(a: &Apollo) -> &Warning {
        a.warnings.last().unwrap()
    }

    #[test]
    fn the_system_tells_of_a_file_that_changes() {
        let dir = std::env::temp_dir().join(format!("neo-apollo-listen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        // As the system names it, which on macOS is not as it was asked for.
        let dir = dir.canonicalize().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = listen(std::slice::from_ref(&dir), move |changed| tx.send(changed).is_ok()).expect("this system tells of changes");
        // Watching takes a moment to begin.
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(dir.join("inner/new.txt"), "one").unwrap();
        std::fs::write(dir.join("inner/new.txt"), "one, and more").unwrap();
        let told: Vec<PathBuf> = rx.recv_timeout(Duration::from_secs(15)).expect("told of the change");
        assert!(told.iter().any(|p| p.ends_with("inner/new.txt")), "{told:?}");
        assert_eq!(told.iter().filter(|p| p.ends_with("inner/new.txt")).count(), 1, "a file written twice in a burst is told of once");
        drop(watcher);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_model_that_listens_can_be_chosen() {
        let (mut a, scratch) = app("listens");
        a.settings_file = Some(scratch.0.join("settings"));
        a.update(Msg::ChooseModel(HEARS, Rect::new(700.0, 500.0, 230.0, 32.0)));
        let menu = a.model_menu(HEARS);
        assert_eq!(menu.len(), neo_apollo_core::hear::LISTENERS.len(), "one entry for each size there is");
        // Picked, it is saved; and it is the one in use (put back after, as other tests listen with it).
        let before = neo_apollo_core::hear::listener();
        a.update(Msg::PickModel(HEARS, "turbo".into(), false));
        assert_eq!((a.choosing, a.settings.hear_model.as_str(), Settings::load_from(&scratch.0.join("settings")).hear_model.as_str(), neo_apollo_core::hear::listener().id), (None, "turbo", "turbo", "turbo"));
        neo_apollo_core::hear::use_listener(before.id);
        let mut h = Harness::new(a, WINDOW).unwrap();
        h.app_mut().update(Msg::Page(Page::Sources));
        h.render(1.0);
    }

    #[test]
    fn numbers_and_snippets_read_well() {
        assert_eq!((count(1, "memory", "memories"), count(0, "word", "words"), count(1204, "memory", "memories"), count(1_000_000, "file", "files")), ("1 memory".into(), "0 words".into(), "1,204 memories".into(), "1,000,000 files".into()));
        assert_eq!((how_long(20.0), how_long(70.0), how_long(700.0), how_long(3.0 * 3600.0)), ("under a minute".into(), "about a minute".into(), "about 12 minutes".into(), "about 3 hours".into()));
        assert_eq!(snippet("one\n  two   three", 40), "one two three");
        assert_eq!(snippet("abcdefghij", 6), "abcde…");
        assert_eq!(short_path(&neo_desktop::fs::home_dir().join("Pictures/a.png")), "~/Pictures/a.png");
        assert_eq!(short_path(Path::new("/Volumes/Disk/a")), "/Volumes/Disk/a");
    }
}
