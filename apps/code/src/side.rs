//! The strip of pages down the left edge, and the pages other than the
//! Explorer: for now, Source Control.

use super::*;
use crate::git::{self, Change, Status};

/// A page of the side pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Explorer,
    SourceControl,
}

impl Page {
    pub const ALL: [Page; 2] = [Page::Explorer, Page::SourceControl];

    fn glyph(self) -> neo::theme::Icon {
        match self {
            Page::Explorer => icons::FILES,
            Page::SourceControl => icons::GIT_BRANCH,
        }
    }
}

/// What is known of the folder's repository, and the commit being written.
#[derive(Default)]
pub struct Source {
    /// `None` until it has been looked at; an error if the folder is in
    /// no repository, or git is not to be had.
    pub status: Option<Result<Status, String>>,
    /// The message for the next commit.
    pub message: String,
    /// What git is being asked to do just now.
    pub busy: Option<String>,
    /// How the last thing asked of it went, if that is worth saying.
    pub said: Option<Result<String, String>>,
    /// The top of the repository, which may be above the folder that is
    /// open. Paths in the status are counted from here.
    pub top: Option<PathBuf>,
    /// Which reading of the status this is, so that a late one is dropped.
    reading: u64,
}

/// Something asked of git from the Source Control page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitDo {
    Refresh,
    Stage(String),
    Unstage(String),
    StageAll,
    UnstageAll,
    Commit,
    Push,
    Pull,
}

impl NeoCode {
    /// How many files differ from the last commit, for the badge.
    fn changed(&self) -> usize {
        self.source.status.as_ref().and_then(|s| s.as_ref().ok()).map_or(0, |s| s.changes.len())
    }

    /// Shows a page of the side pane; the one already showing is put away,
    /// and brought back the same way.
    pub(crate) fn show_page(&mut self, page: Page) {
        if self.page == page && self.side_shown {
            self.side_shown = false;
            return;
        }
        self.page = page;
        self.side_shown = true;
        if page == Page::SourceControl {
            self.git_do(GitDo::Refresh);
        }
    }

    /// Reads the repository's state again, as after a save or a commit.
    pub(crate) fn git_refresh(&mut self) {
        let Some(root) = self.root.clone() else {
            self.source.status = Some(Err("Open a folder to see its changes.".into()));
            return;
        };
        self.source.reading += 1;
        let reading = self.source.reading;
        let dirs = self.servers.dirs.clone().unwrap_or_default();
        // From the top of the repository, where its paths are counted from.
        let read = move || {
            let top = git::top(&root, &dirs);
            let status = git::status(top.as_deref().unwrap_or(&root), &dirs);
            (top, status)
        };
        match self.servers.proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let (top, status) = read();
                    proxy.send(Msg::GitRead(reading, top, status));
                });
            }
            // No event loop to report back to, as in most tests: read now.
            None => {
                let (top, status) = read();
                self.apply(Msg::GitRead(reading, top, status));
            }
        }
    }

    pub(crate) fn git_read(&mut self, reading: u64, top: Option<PathBuf>, status: Result<Status, String>) {
        if reading == self.source.reading {
            self.source.top = top;
            self.source.status = Some(status.map_err(|why| if why.contains("not a git repository") { "This folder is not in a git repository.".into() } else { why }));
        }
    }

    /// Carries out something asked for on the Source Control page.
    pub(crate) fn git_do(&mut self, what: GitDo) {
        if what == GitDo::Refresh {
            return self.git_refresh();
        }
        let Some(root) = self.source.top.clone().or_else(|| self.root.clone()) else { return };
        if self.source.busy.is_some() {
            return;
        }
        let message = self.source.message.trim().to_owned();
        let (doing, done, args): (&str, String, Vec<String>) = match &what {
            GitDo::Stage(path) => ("Staging…", String::new(), vec!["add".into(), "--".into(), path.clone()]),
            // Out of the index, and the file itself left as it is.
            GitDo::Unstage(path) => ("Unstaging…", String::new(), vec!["restore".into(), "--staged".into(), "--".into(), path.clone()]),
            GitDo::StageAll => ("Staging…", String::new(), vec!["add".into(), "--all".into()]),
            GitDo::UnstageAll => ("Unstaging…", String::new(), vec!["restore".into(), "--staged".into(), "--".into(), ".".into()]),
            GitDo::Commit => {
                if message.is_empty() {
                    self.source.said = Some(Err("Write a message for the commit first.".into()));
                    return;
                }
                if self.source.status.as_ref().and_then(|s| s.as_ref().ok()).is_none_or(|s| s.staged().next().is_none()) {
                    self.source.said = Some(Err("Nothing is staged. Stage the changes to go in this commit.".into()));
                    return;
                }
                ("Committing…", "Committed.".into(), vec!["commit".into(), "-m".into(), message])
            }
            GitDo::Push => ("Pushing…", "Pushed.".into(), vec!["push".into()]),
            GitDo::Pull => ("Pulling…", "Pulled.".into(), vec!["pull".into(), "--ff-only".into()]),
            GitDo::Refresh => unreachable!("handled above"),
        };
        // What is committed is what is on disk.
        if what == GitDo::Commit {
            for i in 0..self.tabs.len() {
                if self.tabs[i].dirty() && self.in_folder(&self.tabs[i]) {
                    let was = self.active;
                    self.active = Some(i);
                    self.apply(Msg::Save);
                    self.active = was;
                }
            }
        }
        self.source.busy = Some(doing.to_owned());
        self.source.said = None;
        let dirs = self.servers.dirs.clone().unwrap_or_default();
        let work = move || {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            git::run(&root, &dirs, &args).map(|_| done)
        };
        match self.servers.proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    proxy.send(Msg::GitDone(what, work()));
                });
            }
            None => {
                let result = work();
                self.apply(Msg::GitDone(what, result));
            }
        }
    }

    pub(crate) fn git_done(&mut self, what: GitDo, result: Result<String, String>) {
        self.source.busy = None;
        if what == GitDo::Commit && result.is_ok() {
            self.source.message.clear();
        }
        // Staging says nothing when it works; the list shows it.
        self.source.said = match result {
            Ok(said) if said.is_empty() => None,
            other => Some(other),
        };
        self.git_refresh();
    }

    /// Opens a changed file from the list.
    pub(crate) fn git_open(&mut self, path: &str) {
        let Some(root) = self.source.top.as_ref().or(self.root.as_ref()) else { return };
        let file = root.join(path);
        if !self.show_file(&file) {
            self.toast = Some(format!("{path} is not there to open."));
        }
    }

    /// The strip down the left edge: one button a page, and the pane's own
    /// on and off.
    pub(crate) fn activity_bar(&self) -> Element<Msg> {
        let mut bar = column().spacing(4.0).align(Align::Center).width(Length::Fill);
        for page in Page::ALL {
            let on = self.side_shown && self.page == page;
            let mut glyph: Element<Msg> = icon(page.glyph()).size(19.0).tone(if on { Tone::Accent } else { Tone::Muted }).into();
            // How many files have changed, beside the Source Control icon.
            if page == Page::SourceControl && self.changed() > 0 {
                let count = self.changed();
                glyph = column().spacing(1.0).align(Align::Center).push(glyph).push(text(if count > 99 { "99+".to_owned() } else { count.to_string() }).role(TextRole::Label).tone(Tone::Accent)).into();
            }
            bar = bar.push(Button::new(glyph).kind(ButtonKind::Ghost).selected(on).padding([8.0, 8.0]).radius(8.0).on_press(Msg::Page(page)));
        }
        container(bar).padding([4.0, 8.0]).width(46.0).height(Length::Fill).into()
    }

    fn change_row(&self, c: &Change, staged: bool) -> Element<Msg> {
        let letter = if staged { c.staged } else { c.unstaged }.unwrap_or(' ');
        let tone = match letter {
            'D' => Tone::Bad,
            'A' | '?' => Tone::Good,
            'U' => Tone::Warn,
            _ => Tone::Accent,
        };
        let label = row()
            .spacing(7.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(file_icon(c.name())).size(13.0).tone(Tone::Muted))
            .push(text(c.name().to_owned()).role(TextRole::Body).no_wrap())
            .push(container(text(c.folder().to_owned()).role(TextRole::Caption).tone(Tone::Faint).no_wrap()).width(Length::Fill))
            .push(text(if letter == '?' { "U".to_owned() } else { letter.to_string() }).mono().role(TextRole::Caption).tone(tone));
        let open = Button::new(label).kind(ButtonKind::Ghost).align_x(Align::Start).width(Length::Fill).padding([6.0, 4.0]).radius(6.0).on_press(Msg::GitOpen(c.path.clone()));
        let idle = self.source.busy.is_none();
        let act = if staged { GitDo::Unstage(c.path.clone()) } else { GitDo::Stage(c.path.clone()) };
        let side = icon_button(if staged { icons::MINUS } else { icons::PLUS }, 22.0).kind(ButtonKind::Ghost).on_press_maybe(idle.then_some(Msg::Git(act)));
        row().spacing(2.0).align(Align::Center).width(Length::Fill).push(open).push(side).into_element_keyed(&format!("{}{}", if staged { "s:" } else { "u:" }, c.path))
    }

    /// The Source Control page: the commit being written, then what is
    /// staged to go in it and what is not.
    pub(crate) fn source_page(&self) -> Element<Msg> {
        let idle = self.source.busy.is_none();
        let small = |glyph, what: GitDo, on: bool| icon_button(glyph, 22.0).kind(ButtonKind::Ghost).on_press_maybe((on && idle).then_some(Msg::Git(what)));
        let mut head = column().spacing(6.0).width(Length::Fill).push(row().align(Align::Center).width(Length::Fill).push(container(text("Source Control").role(TextRole::Label).tone(Tone::Muted)).width(Length::Fill)).push(small(icons::REFRESH_CW, GitDo::Refresh, true)));
        let mut body = column().spacing(1.0).width(Length::Fill);
        match &self.source.status {
            None => body = body.push(text("Reading the repository…").role(TextRole::Caption).tone(Tone::Muted)),
            Some(Err(why)) => body = body.push(text(why.clone()).role(TextRole::Caption).tone(Tone::Muted)),
            Some(Ok(status)) => {
                // The branch, and how far it is from the one it follows.
                let mut branch = row().spacing(6.0).align(Align::Center).width(Length::Fill).push(icon(icons::GIT_BRANCH).size(13.0).tone(Tone::Accent)).push(container(text(status.branch.clone()).role(TextRole::Strong).no_wrap()).width(Length::Fill));
                if status.upstream.is_some() {
                    let count = |n: u32| if n > 0 { n.to_string() } else { String::new() };
                    branch = branch
                        .push(text(count(status.behind)).role(TextRole::Caption).tone(Tone::Muted))
                        .push(small(icons::ARROW_DOWN, GitDo::Pull, true))
                        .push(text(count(status.ahead)).role(TextRole::Caption).tone(Tone::Muted))
                        .push(small(icons::ARROW_UP, GitDo::Push, true));
                }
                let staged: Vec<&Change> = status.staged().collect();
                let unstaged: Vec<&Change> = status.unstaged().collect();
                head = head
                    .push(branch)
                    .push(text_input("Message", self.source.message.clone()).on_input(Msg::GitMessage).on_submit(Msg::Git(GitDo::Commit)).width(Length::Fill))
                    .push(Button::new(row().spacing(6.0).align(Align::Center).push(icon(icons::CHECK).size(14.0)).push(text("Commit").role(TextRole::Strong))).kind(ButtonKind::Accent).width(Length::Fill).padding([10.0, 6.0]).radius(8.0).on_press_maybe((idle && !staged.is_empty() && !self.source.message.trim().is_empty()).then_some(Msg::Git(GitDo::Commit))));
                if let Some(doing) = &self.source.busy {
                    head = head.push(text(doing.clone()).role(TextRole::Caption).tone(Tone::Accent));
                }
                match &self.source.said {
                    Some(Ok(said)) => head = head.push(text(said.clone()).role(TextRole::Caption).tone(Tone::Good)),
                    Some(Err(why)) => head = head.push(text(why.clone()).role(TextRole::Caption).tone(Tone::Bad)),
                    None => {}
                }
                let group = |title: &str, count: usize, glyph, all: GitDo| row().align(Align::Center).width(Length::Fill).push(container(text(format!("{title}  {count}")).role(TextRole::Label).tone(Tone::Muted)).width(Length::Fill)).push(small(glyph, all, count > 0));
                if !staged.is_empty() {
                    body = body.push(group("Staged", staged.len(), icons::MINUS, GitDo::UnstageAll));
                    for c in &staged {
                        body = body.push(self.change_row(c, true));
                    }
                    body = body.push(Space::new(0.0, 8.0));
                }
                body = body.push(group("Changes", unstaged.len(), icons::PLUS, GitDo::StageAll));
                for c in &unstaged {
                    body = body.push(self.change_row(c, false));
                }
                if status.changes.is_empty() {
                    body = body.push(container(text("Nothing has changed since the last commit.").role(TextRole::Caption).tone(Tone::Faint)).padding([0.0, 6.0]));
                }
            }
        }
        container(column().spacing(10.0).width(Length::Fill).height(Length::Fill).push(head).push(scrollable(body))).padding([8.0, 8.0, 0.0, 8.0]).width(240.0).height(Length::Fill).into()
    }
}
