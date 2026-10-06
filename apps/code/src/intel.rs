//! What language servers add to the editor: problems underlined as you
//! type, a note about what is under the caret, jumping to where something
//! is defined, completions, and formatting.

use std::collections::HashMap;

use super::*;
use crate::lsp::{self, Client, Completion, Event, Incoming, Severity};

/// The language servers in use, and the search for them.
#[derive(Default)]
pub struct Servers {
    /// The folders servers are looked for in, once that is known. Working
    /// it out asks the login shell, so it happens off the main thread.
    pub dirs: Option<Vec<PathBuf>>,
    /// The running servers by name, each with the number of its launch, so
    /// that word from one that has been replaced can be told apart.
    pub clients: HashMap<&'static str, (u64, Client)>,
    /// Copies of a server still to try if the one running will not start.
    spares: HashMap<&'static str, Vec<lsp::Found>>,
    /// Why a server is not running.
    pub problems: HashMap<&'static str, String>,
    pub proxy: Option<Proxy<Msg>>,
    launches: u64,
}

impl Servers {
    /// Stops every server, as when another folder is opened.
    pub fn reset(&mut self) {
        self.clients.clear();
        self.spares.clear();
        self.problems.clear();
    }
}

/// What is showing at the caret.
pub enum Popup {
    /// What there is to say, and where in the text if not at the caret.
    Hover(String, Option<Pos>),
    /// Everything the server offered, and which of those that still match
    /// what has been typed is picked.
    Complete { all: Vec<Completion>, selected: usize },
}

/// Something to ask the server about the caret's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Hover,
    Definition,
    Complete,
    Format,
}

fn spec_named(name: &str) -> Option<&'static lsp::ServerSpec> {
    lsp::all().iter().find(|s| s.name == name)
}

/// How NeoCode colours a kind of token a server names. `None` leaves the
/// text to the built-in highlighter.
pub fn kind_of(name: &str) -> Option<SyntaxKind> {
    Some(match name {
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "namespace" | "builtinType" | "typeAlias" | "union" | "trait" | "selfTypeKeyword" => SyntaxKind::Type,
        "function" | "method" => SyntaxKind::Function,
        "macro" => SyntaxKind::Macro,
        "keyword" | "modifier" | "selfKeyword" => SyntaxKind::Keyword,
        "string" | "regexp" | "character" | "escapeSequence" => SyntaxKind::String,
        "number" | "enumMember" | "constant" | "boolean" => SyntaxKind::Number,
        "comment" => SyntaxKind::Comment,
        "decorator" | "attribute" | "lifetime" | "label" => SyntaxKind::Attribute,
        "operator" | "punctuation" => SyntaxKind::Punct,
        // Plain names read best in the text's own colour.
        "variable" | "parameter" | "property" => SyntaxKind::Plain,
        _ => return None,
    })
}

/// Where the word that ends at `col` begins.
fn word_start(line: &str, col: usize) -> usize {
    line[..col.min(line.len())].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map_or(col.min(line.len()), |(i, _)| i)
}

/// The completions that fit what has been typed: those that start with it,
/// then those that merely contain it.
pub fn matching<'a>(all: &'a [Completion], typed: &str) -> Vec<&'a Completion> {
    let typed = typed.to_lowercase();
    let key = |c: &Completion| c.label.to_lowercase();
    let mut out: Vec<&Completion> = all.iter().filter(|c| key(c).starts_with(&typed)).collect();
    out.extend(all.iter().filter(|c| !key(c).starts_with(&typed) && key(c).contains(&typed)));
    out.truncate(100);
    out
}

impl NeoCode {
    /// Starts looking for language servers. The answer comes back as
    /// [`Msg::LspDirs`].
    pub(crate) fn find_servers(&mut self, proxy: Proxy<Msg>) {
        self.servers.proxy = Some(proxy.clone());
        // Tests must not start whatever servers this computer happens to have.
        if cfg!(test) {
            self.servers.dirs = Some(vec![]);
            return;
        }
        std::thread::spawn(move || {
            proxy.send(Msg::LspDirs(lsp::search_dirs()));
        });
    }

    /// The search is done: start servers for the files already open.
    pub(crate) fn servers_found(&mut self, dirs: Vec<PathBuf>) {
        self.servers.dirs = Some(dirs);
        for i in 0..self.tabs.len() {
            self.lsp_open(i);
        }
    }

    /// Starts the server for `spec`, trying each copy found until one runs.
    fn launch(&mut self, spec: &'static lsp::ServerSpec, root: &Path) -> bool {
        let (Some(dirs), Some(proxy)) = (&self.servers.dirs, &self.servers.proxy) else { return false };
        if self.servers.problems.contains_key(spec.name) {
            return false;
        }
        let first_try = !self.servers.spares.contains_key(spec.name);
        let spares = self.servers.spares.entry(spec.name).or_insert_with(|| {
            let mut found = lsp::find_in(dirs, spec);
            found.reverse();
            found
        });
        let none_installed = first_try && spares.is_empty();
        while let Some(found) = spares.pop() {
            self.servers.launches += 1;
            let (launch, name, proxy) = (self.servers.launches, spec.name, proxy.clone());
            if let Ok(client) = Client::spawn(&found, root, dirs, move |m| {
                proxy.send(Msg::Lsp(name, launch, m));
            }) {
                self.servers.clients.insert(name, (launch, client));
                return true;
            }
        }
        let why = if none_installed { format!("No language server for {} was found", spec.language) } else { format!("{} would not start", spec.name) };
        self.servers.problems.insert(spec.name, why);
        false
    }

    /// Tells the right server about a newly opened file, starting it first
    /// if need be.
    pub(crate) fn lsp_open(&mut self, i: usize) {
        let Some(root) = self.root.clone() else { return };
        let Some(disk) = self.tabs.get(i).and_then(|t| t.disk.clone()) else { return };
        let Some((spec, language)) = lsp::spec_for(&disk) else { return };
        self.tabs[i].server = Some(spec.name);
        if !self.servers.clients.contains_key(spec.name) && !self.launch(spec, &root) {
            return;
        }
        let Self { tabs, servers, .. } = self;
        let (t, Some((_, client))) = (&mut tabs[i], servers.clients.get_mut(spec.name)) else { return };
        t.version = 1;
        t.synced = t.doc.revision();
        client.did_open(&disk, language, t.version, &t.doc.text());
        client.tokens(&disk);
    }

    fn client_of(&mut self, i: usize) -> Option<(&mut Tab, &mut Client)> {
        let Self { tabs, servers, .. } = self;
        let t = tabs.get_mut(i)?;
        let (_, client) = servers.clients.get_mut(t.server?)?;
        Some((t, client))
    }

    /// Sends the file's text to its server if it has changed since last sent.
    pub(crate) fn lsp_sync(&mut self, i: usize) {
        let Some((t, client)) = self.client_of(i) else { return };
        let Some(disk) = &t.disk else { return };
        if t.doc.revision() != t.synced {
            t.version += 1;
            t.synced = t.doc.revision();
            client.did_change(disk, t.version, &t.doc.text());
            client.tokens(disk);
        }
    }

    pub(crate) fn lsp_saved(&mut self, i: usize) {
        self.lsp_sync(i);
        if let Some((t, client)) = self.client_of(i)
            && let Some(disk) = &t.disk
        {
            client.did_save(disk);
        }
    }

    pub(crate) fn lsp_closing(&mut self, i: usize) {
        if let Some((t, client)) = self.client_of(i)
            && let Some(disk) = &t.disk
        {
            client.did_close(disk);
        }
    }

    /// Word from a server, or that its output has ended.
    pub(crate) fn lsp_incoming(&mut self, name: &'static str, launch: u64, incoming: Incoming) {
        // A server that has since been replaced has nothing to say.
        if self.servers.clients.get(name).is_none_or(|(l, _)| *l != launch) {
            return;
        }
        match incoming {
            Incoming::Message(message) => {
                let events = self.servers.clients.get_mut(name).map(|(_, c)| c.handle(&message)).unwrap_or_default();
                for event in events {
                    self.lsp_event(name, event);
                }
            }
            Incoming::Closed => self.server_gone(name),
        }
    }

    /// A server stopped. If it never got going, another copy may; if it
    /// was working, say that it is gone.
    fn server_gone(&mut self, name: &'static str) {
        // Give its last words a moment to arrive before reading them.
        std::thread::sleep(std::time::Duration::from_millis(60));
        let gone = self.servers.clients.remove(name);
        let said = gone.as_ref().and_then(|(_, c)| c.last_error());
        let was_ready = gone.is_some_and(|(_, c)| c.ready());
        let mine: Vec<usize> = (0..self.tabs.len()).filter(|i| self.tabs[*i].server == Some(name)).collect();
        for i in &mine {
            self.tabs[*i].diagnostics.clear();
        }
        if was_ready {
            self.servers.problems.insert(name, format!("{name} stopped"));
        } else if let (Some(spec), Some(root)) = (spec_named(name), self.root.clone()) {
            if self.launch(spec, &root) {
                for i in mine {
                    self.lsp_open(i);
                }
            } else if let Some(said) = said {
                // No copy would start: pass on what the last one said.
                self.servers.problems.insert(name, format!("{name} would not start: {said}"));
            }
        }
    }

    fn lsp_event(&mut self, name: &'static str, event: Event) {
        let utf8 = self.servers.clients.get(name).is_some_and(|(_, c)| c.utf8);
        let tab_at = |tabs: &[Tab], path: &Path| tabs.iter().position(|t| t.disk.as_deref() == Some(path));
        match event {
            // Now it is known whether the server can colour: ask for the
            // files already open.
            Event::Ready => self.lsp_event(name, Event::RefreshTokens),
            Event::Diagnostics { path, items } => {
                if let Some(i) = tab_at(&self.tabs, &path) {
                    self.tabs[i].diagnostics = items;
                }
            }
            Event::Hover(text) => {
                let Some(i) = self.active else { return };
                // An answer about the word the mouse was on: shown there,
                // under any problems, if the mouse has not moved on.
                if let Some(at) = self.tabs.get_mut(i).and_then(|t| t.hover_for.take()) {
                    let problems = self.problems_at(&self.tabs[i], at);
                    let t = &mut self.tabs[i];
                    if let (Some(text), true) = (text, t.pointed.is_some_and(|(p, _, _)| p == at)) {
                        t.popup = Some(Popup::Hover(if problems.is_empty() { text } else { format!("{problems}\n\n{text}") }, Some(at)));
                    }
                    return;
                }
                match (text, self.tabs.get_mut(i)) {
                    (Some(text), Some(t)) => t.popup = Some(Popup::Hover(text, None)),
                    _ => self.toast = Some("Nothing to say about that.".into()),
                }
            }
            Event::Definition(None) => self.toast = Some("No definition found.".into()),
            Event::Definition(Some((path, place))) => {
                let node = self.nodes.iter().position(|n| matches!(&n.source, Some(Source::Disk(p)) if *p == path));
                match node {
                    Some(n) => {
                        self.update(Msg::Open(n));
                        if let Some(t) = self.active.and_then(|i| self.tabs.get_mut(i)).filter(|t| t.disk.as_deref() == Some(path.as_path())) {
                            let pos = lsp::to_pos(t.doc.lines(), place, utf8);
                            t.doc.apply(Action::Click { pos, select: false });
                        }
                    }
                    None => self.toast = Some(format!("That is defined outside this folder, in {}.", path.display())),
                }
            }
            Event::Completions(items) => {
                if let Some(t) = self.active.and_then(|i| self.tabs.get_mut(i)) {
                    t.popup = (!items.is_empty()).then_some(Popup::Complete { all: items, selected: 0 });
                }
            }
            Event::Edits { path, edits } => {
                let Some(i) = tab_at(&self.tabs, &path) else { return };
                let t = &mut self.tabs[i];
                let edits: Vec<(Pos, Pos, String)> = edits.into_iter().map(|e| (lsp::to_pos(t.doc.lines(), e.from, utf8), lsp::to_pos(t.doc.lines(), e.to, utf8), e.text)).collect();
                let formatted = lsp::apply_edits(t.doc.lines(), &edits);
                if formatted == t.doc.text() {
                    self.toast = Some("Already formatted.".into());
                    return;
                }
                // Replace the text in one step, so one undo brings it back,
                // whatever keys are in use.
                let (keys, cursor) = (t.doc.keymap(), t.doc.cursor());
                t.doc.set_keymap(Keymap::Plain);
                t.doc.apply(Action::SelectAll);
                t.doc.apply(Action::Insert(formatted));
                t.doc.apply(Action::Click { pos: cursor, select: false });
                t.doc.set_keymap(keys);
                self.toast = Some(format!("Formatted {}", t.name));
                self.lsp_sync(i);
            }
            Event::Tokens { path, tokens } => {
                // Tokens for text that has since changed would land in the
                // wrong places; the request sent with the change is on its way.
                let Some(t) = tab_at(&self.tabs, &path).map(|i| &mut self.tabs[i]).filter(|t| t.doc.revision() == t.synced) else { return };
                let lines = t.doc.lines();
                t.tokens = tokens
                    .iter()
                    .filter_map(|k| {
                        let from = lsp::to_pos(lines, (k.line, k.start), utf8);
                        let to = lsp::to_pos(lines, (k.line, k.start + k.length), utf8);
                        Some(EditorToken { line: from.line, from: from.col, to: to.col, kind: kind_of(&k.kind)? })
                    })
                    .collect();
            }
            Event::RefreshTokens => {
                let Self { tabs, servers, .. } = self;
                if let Some((_, client)) = servers.clients.get_mut(name) {
                    for disk in tabs.iter().filter(|t| t.server == Some(name)).filter_map(|t| t.disk.as_deref()) {
                        client.tokens(disk);
                    }
                }
            }
            Event::Message(m) => self.toast = Some(m),
            Event::Failed(why) => {
                self.toast = Some(format!("{name}: {why}"));
                self.server_gone(name);
            }
        }
    }

    /// Asks the active file's server about the caret's position.
    pub(crate) fn ask(&mut self, what: Ask) {
        let Some(i) = self.active else { return };
        self.lsp_sync(i);
        let problem = self.tabs.get(i).and_then(|t| t.server).and_then(|s| self.servers.problems.get(s).cloned());
        let Some((t, client)) = self.client_of(i) else {
            self.toast = Some(problem.map_or("This file has no language server.".into(), |p| format!("{p}.")));
            return;
        };
        let Some(disk) = t.disk.clone() else { return };
        let place = lsp::to_place(t.doc.lines(), t.doc.cursor(), client.utf8);
        t.hover_for = None;
        match what {
            Ask::Hover => client.hover(&disk, place),
            Ask::Definition => client.definition(&disk, place),
            Ask::Complete => client.completion(&disk, place),
            Ask::Format if client.can_format => client.format(&disk, INDENT),
            Ask::Format => self.toast = Some("This language server cannot format.".into()),
        }
    }

    /// What has been typed of the word at the caret.
    fn typed_word(t: &Tab) -> &str {
        let c = t.doc.cursor();
        let line = &t.doc.lines()[c.line];
        &line[word_start(line, c.col)..c.col.min(line.len())]
    }

    /// Follows an edit: the server hears the new text, and the popup keeps
    /// up. `typed` says the edit added text at the caret.
    pub(crate) fn after_edit(&mut self, i: usize, typed: bool) {
        self.lsp_sync(i);
        let Some(t) = self.tabs.get(i) else { return };
        let typing_mode = t.doc.mode_status().is_none_or(|s| s.insert);
        let c = t.doc.cursor();
        let last = t.doc.lines()[c.line][..c.col].chars().next_back();
        let triggers = t.server.and_then(|s| self.servers.clients.get(s)).map(|(_, c)| c.triggers.clone()).unwrap_or_default();
        let wanted = typed && typing_mode && (!Self::typed_word(t).is_empty() || last.is_some_and(|l| triggers.contains(&l)));
        let t = &mut self.tabs[i];
        if !wanted {
            t.popup = None;
            return;
        }
        // Keep what is showing, narrowed by the new letter, until the
        // server's fresh answer arrives.
        if let Some(Popup::Complete { selected, .. }) = &mut t.popup {
            *selected = 0;
        } else {
            t.popup = None;
        }
        if t.server.is_some_and(|s| self.servers.clients.contains_key(s)) {
            self.ask(Ask::Complete);
        }
    }

    pub(crate) fn popup_key(&mut self, key: PopupKey) {
        let Some(i) = self.active else { return };
        let Some(t) = self.tabs.get_mut(i) else { return };
        let typed = Self::typed_word(t).to_owned();
        let Some(Popup::Complete { all, selected }) = &mut t.popup else {
            t.popup = None;
            return;
        };
        let fits = matching(all, &typed);
        match key {
            PopupKey::Dismiss => t.popup = None,
            PopupKey::Up => *selected = if *selected == 0 { fits.len().saturating_sub(1) } else { *selected - 1 },
            PopupKey::Down => *selected = if *selected + 1 >= fits.len() { 0 } else { *selected + 1 },
            PopupKey::Accept => {
                let insert = fits.get(*selected).map(|c| c.insert.clone());
                t.popup = None;
                if let Some(insert) = insert {
                    // Replace what was typed of the word with the choice.
                    for _ in 0..typed.chars().count() {
                        t.doc.apply(Action::Backspace);
                    }
                    t.doc.apply(Action::Insert(insert));
                    self.lsp_sync(i);
                }
            }
        }
    }

    /// How long the mouse rests on a word before it is asked about.
    pub(crate) const REST: std::time::Duration = std::time::Duration::from_millis(350);

    /// The mouse came to rest on a word, or left it.
    pub(crate) fn point(&mut self, word: Option<Pos>) {
        let Some(t) = self.active.and_then(|i| self.tabs.get_mut(i)) else { return };
        t.pointed = word.map(|p| (p, std::time::Instant::now(), false));
        // What was said about the last word goes when the mouse does.
        if matches!(&t.popup, Some(Popup::Hover(_, Some(at))) if Some(*at) != word) {
            t.popup = None;
        }
    }

    /// Whether a word under the mouse is waiting to be asked about.
    pub(crate) fn hover_waiting(&self) -> bool {
        self.active.and_then(|i| self.tabs.get(i)).is_some_and(|t| matches!(t.pointed, Some((_, _, false))))
    }

    /// Once the mouse has rested long enough: shows the problems on that
    /// word at once, and asks the server what else there is to say.
    pub(crate) fn hover_rested(&mut self) {
        let Some(i) = self.active else { return };
        let Some((at, since, false)) = self.tabs.get(i).and_then(|t| t.pointed) else { return };
        // Not over a list of completions, which the keyboard is working.
        if since.elapsed() < Self::REST || matches!(self.tabs[i].popup, Some(Popup::Complete { .. })) {
            return;
        }
        self.lsp_sync(i);
        let problems = self.problems_at(&self.tabs[i], at);
        let t = &mut self.tabs[i];
        t.pointed = Some((at, since, true));
        if !problems.is_empty() {
            t.popup = Some(Popup::Hover(problems, Some(at)));
        }
        let Some((t, client)) = self.client_of(i) else { return };
        let Some(disk) = t.disk.clone() else { return };
        let place = lsp::to_place(t.doc.lines(), at, client.utf8);
        t.hover_for = Some(at);
        client.hover(&disk, place);
    }

    /// What the server found wrong with the word starting at `at`.
    fn problems_at(&self, t: &Tab, at: Pos) -> String {
        let utf8 = t.server.and_then(|s| self.servers.clients.get(s)).is_some_and(|(_, c)| c.utf8);
        let Some(line) = t.doc.lines().get(at.line) else { return String::new() };
        let len = line.get(at.col..).map_or(0, |rest| rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').map(char::len_utf8).sum::<usize>());
        let end = Pos::new(at.line, at.col + len);
        let key = |p: Pos| (p.line, p.col);
        let said: Vec<&str> = t.diagnostics.iter().filter(|d| key(lsp::to_pos(t.doc.lines(), d.from, utf8)) < key(end) && key(lsp::to_pos(t.doc.lines(), d.to, utf8)) > key(at)).map(|d| d.message.as_str()).collect();
        said.join("\n\n")
    }

    /// The problems in a file, as underlines for the editor.
    pub(crate) fn editor_marks(&self, t: &Tab) -> Vec<EditorMark> {
        let utf8 = t.server.and_then(|s| self.servers.clients.get(s)).is_some_and(|(_, c)| c.utf8);
        t.diagnostics
            .iter()
            .map(|d| EditorMark {
                from: lsp::to_pos(t.doc.lines(), d.from, utf8),
                to: lsp::to_pos(t.doc.lines(), d.to, utf8),
                tone: match d.severity {
                    Severity::Error => Tone::Bad,
                    Severity::Warning => Tone::Warn,
                    Severity::Info => Tone::Accent,
                    Severity::Hint => Tone::Faint,
                },
            })
            .collect()
    }

    /// Where the popup goes, if not at the caret.
    pub(crate) fn editor_popup_at(&self, t: &Tab) -> Option<Pos> {
        match t.popup.as_ref()? {
            Popup::Hover(_, at) => *at,
            Popup::Complete { .. } => None,
        }
    }

    pub(crate) fn editor_popup(&self, t: &Tab) -> Option<EditorPopup> {
        match t.popup.as_ref()? {
            Popup::Hover(text, _) => Some(EditorPopup::Text(text.clone())),
            Popup::Complete { all, selected } => {
                let fits = matching(all, Self::typed_word(t));
                (!fits.is_empty()).then(|| EditorPopup::List { items: fits.iter().map(|c| (c.label.clone(), c.detail.clone())).collect(), selected: (*selected).min(fits.len() - 1) })
            }
        }
    }

    /// The problem on the caret's line, if there is one, worst first.
    pub(crate) fn problem_here<'a>(&self, t: &'a Tab) -> Option<&'a lsp::Diagnostic> {
        let line = t.doc.cursor().line as u32;
        t.diagnostics.iter().filter(|d| d.from.0 <= line && line <= d.to.0).min_by_key(|d| d.severity)
    }

    /// How the file's language server is doing, for the status bar.
    pub(crate) fn server_status(&self, t: &Tab) -> Option<(String, Tone)> {
        // The sample project is not on disk, where a server could read it.
        if t.disk.is_none() && lsp::spec_for(Path::new(&t.path)).is_some() {
            return Some(("Sample project: open a folder to use language servers".into(), Tone::Muted));
        }
        let name = t.server?;
        Some(match (self.servers.clients.get(name), self.servers.problems.get(name)) {
            (Some((_, c)), _) if c.ready() => (name.to_owned(), Tone::Muted),
            (Some(_), _) => (format!("{name} is starting…"), Tone::Muted),
            (None, Some(why)) => (why.clone(), Tone::Warn),
            (None, None) if self.servers.dirs.is_none() => ("Looking for language servers…".into(), Tone::Muted),
            (None, None) => return None,
        })
    }

    /// Which servers are installed here, for the settings panel.
    pub(crate) fn servers_summary(&self) -> String {
        let Some(dirs) = &self.servers.dirs else { return "Looking for language servers…".into() };
        let (found, missing): (Vec<_>, Vec<_>) = lsp::all().iter().partition(|s| !lsp::find_in(dirs, s).is_empty());
        let names = |list: &[&lsp::ServerSpec]| list.iter().map(|s| s.name).collect::<Vec<_>>().join(", ");
        let summary = match (found.is_empty(), missing.is_empty()) {
            (true, _) => format!("None found. NeoCode looks for: {}.", names(&missing)),
            (false, true) => format!("Found: {}.", names(&found)),
            (false, false) => format!("Found: {}. Not installed: {}.", names(&found), names(&missing)),
        };
        format!("{summary} Add your own in {}.", lsp::user_servers_file().display())
    }
}
