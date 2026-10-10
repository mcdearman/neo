//! Agents: workflows drawn, kept and run.
//!
//! A workflow shares a piece of work out between agents: see
//! `neo_apollo_core::flow`. This page lists the ones that are kept, shows
//! the one that is open as a graph to rearrange, lets each node be given
//! instructions and a model of its own, and runs it on a question,
//! showing each node's work as it is done.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use neo::prelude::*;
use neo::{Point, Rect};
use neo_apollo_core::flow::{NodeKind, Step, Workflow, MOST_WORKERS};
use neo_desktop::ui::{notice, section};

use crate::flow_view::{flow_view, FlowEvent, Shown, Stage};

#[derive(Clone, Debug)]
pub enum AgentMsg {
    /// Open one of the workflows that are kept, by name.
    Open(String),
    New,
    Save,
    Delete,
    Name(String),
    Graph(FlowEvent),
    Add(NodeKind),
    /// Take out the node that is chosen.
    Remove,
    /// Unjoin the first from the second.
    Unjoin(u32, u32),
    Title(String),
    Instructions(Action),
    Workers(f64),
    /// Open the list of models under the control at this place, or put it away.
    ChooseModel(Rect),
    CloseMenu,
    Model(String),
    Question(String),
    Run,
    Stop,
    Step(Step),
    Finished(Result<String, String>),
}

/// What a node has made of a run, so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Made {
    pub stage: Stage,
    pub text: String,
    /// The pieces of work a node of workers shared out.
    pub pieces: Vec<String>,
}

/// A run to be started: the caller has the model and the memory.
pub struct ToRun {
    pub flow: Workflow,
    pub question: String,
    pub stop: Arc<AtomicBool>,
}

pub struct Agents {
    /// Where workflows are kept. Tests and snapshots keep none.
    pub folder: Option<PathBuf>,
    pub kept: Vec<Workflow>,
    pub open: Workflow,
    /// The name it was kept under when it was opened, to keep it over that one.
    kept_as: Option<String>,
    /// Changed since it was last kept.
    pub changed: bool,
    pub chosen: Option<u32>,
    /// The chosen node's instructions, as they are being written.
    instructions: Document,
    pub question: String,
    pub running: bool,
    stop: Arc<AtomicBool>,
    pub made: HashMap<u32, Made>,
    pub answer: Option<String>,
    pub trouble: Option<String>,
    /// The list of models, open under this place.
    menu: Option<Point>,
}

impl Agents {
    pub fn new(folder: Option<PathBuf>) -> Self {
        let kept = folder.as_deref().map(Workflow::saved_in).unwrap_or_default();
        let open = kept.first().cloned().unwrap_or_else(|| Workflow::starter("Research with workers"));
        let kept_as = kept.first().map(|w| w.name.clone());
        Self { folder, kept, open, kept_as, changed: false, chosen: None, instructions: Document::new(""), question: String::new(), running: false, stop: Arc::default(), made: HashMap::new(), answer: None, trouble: None, menu: None }
    }

    fn choose(&mut self, id: Option<u32>) {
        self.chosen = id.filter(|id| self.open.node(*id).is_some());
        self.instructions = Document::new(self.chosen.and_then(|id| self.open.node(id)).map_or("", |n| n.prompt.as_str()));
        self.menu = None;
    }

    fn show(&mut self, flow: Workflow, kept_as: Option<String>) {
        (self.open, self.kept_as, self.changed) = (flow, kept_as, false);
        (self.made, self.answer, self.trouble) = (HashMap::new(), None, None);
        self.choose(None);
    }

    /// A name no kept workflow has: the one asked for, or that with a number after it.
    fn free_name(&self, wanted: &str) -> String {
        let taken = |name: &str| self.kept.iter().any(|w| w.name.eq_ignore_ascii_case(name));
        (1..).map(|n| if n == 1 { wanted.to_owned() } else { format!("{wanted} {n}") }).find(|name| !taken(name)).unwrap_or_default()
    }

    /// Carries out what was asked. A run to start is handed back, for
    /// whoever has the model to run it and send back each [`Step`].
    pub fn update(&mut self, m: AgentMsg) -> Option<ToRun> {
        let before = self.open.clone();
        match m {
            AgentMsg::Open(name) => {
                if let Some(flow) = self.kept.iter().find(|w| w.name == name).cloned() {
                    self.show(flow, Some(name));
                }
                // As it was kept: nothing about it has been changed.
                return None;
            }
            AgentMsg::New => {
                let name = self.free_name("New workflow");
                self.show(Workflow::starter(&name), None);
                self.changed = true;
            }
            AgentMsg::Save => {
                if self.open.name.trim().is_empty() {
                    self.open.name = self.free_name("Workflow");
                }
                if let Some(folder) = &self.folder {
                    // Under a new name, the one of the old name goes.
                    if let Some(old) = self.kept_as.as_deref().filter(|old| Workflow::file_in(folder, old) != Workflow::file_in(folder, &self.open.name)) {
                        let _ = Workflow::delete_in(folder, old);
                    }
                    if let Err(e) = self.open.save_in(folder) {
                        self.trouble = Some(format!("Could not keep the workflow: {e}"));
                        return None;
                    }
                    self.kept = Workflow::saved_in(folder);
                } else {
                    let (old, name) = (self.kept_as.clone(), self.open.name.clone());
                    self.kept.retain(|w| Some(&w.name) != old.as_ref() && w.name != name);
                    self.kept.push(self.open.clone());
                    self.kept.sort_by_key(|w| w.name.to_lowercase());
                }
                (self.kept_as, self.changed, self.trouble) = (Some(self.open.name.clone()), false, None);
                return None;
            }
            AgentMsg::Delete => {
                if let Some(name) = self.kept_as.take() {
                    if let Some(folder) = &self.folder {
                        let _ = Workflow::delete_in(folder, &name);
                    }
                    self.kept.retain(|w| w.name != name);
                }
                match self.kept.first().cloned() {
                    Some(next) => self.show(next.clone(), Some(next.name)),
                    None => {
                        self.show(Workflow::starter("Research with workers"), None);
                        self.changed = true;
                    }
                }
                return None;
            }
            AgentMsg::Name(name) => self.open.name = name,
            AgentMsg::Graph(FlowEvent::Chose(id)) => self.choose(id),
            AgentMsg::Graph(FlowEvent::Moved(id, to)) => {
                if let Some(n) = self.open.node_mut(id) {
                    (n.x, n.y) = (to.x, to.y);
                }
            }
            AgentMsg::Graph(FlowEvent::Joined(from, to)) => self.trouble = self.open.connect(from, to).err(),
            AgentMsg::Add(kind) => {
                // Beside the one that is chosen, or after the last.
                let at = self.chosen.and_then(|id| self.open.node(id)).map_or_else(|| Point::new(40.0 + self.open.nodes.len() as f32 * 36.0, 170.0 + (self.open.nodes.len() % 3) as f32 * 30.0), |n| Point::new(n.x + 40.0, n.y + 90.0));
                let id = self.open.add(kind, at.x, at.y);
                self.choose(Some(id));
            }
            AgentMsg::Remove => {
                if let Some(id) = self.chosen {
                    self.open.remove(id);
                    self.made.remove(&id);
                    self.choose(None);
                }
            }
            AgentMsg::Unjoin(from, to) => self.open.disconnect(from, to),
            AgentMsg::Title(title) => {
                if let Some(n) = self.chosen.and_then(|id| self.open.node_mut(id)) {
                    n.title = title;
                }
            }
            AgentMsg::Instructions(action) => {
                self.instructions.apply(action);
                let written = self.instructions.text();
                if let Some(n) = self.chosen.and_then(|id| self.open.node_mut(id)) {
                    n.prompt = written;
                }
            }
            AgentMsg::Workers(n) => {
                if let Some(node) = self.chosen.and_then(|id| self.open.node_mut(id)) {
                    node.workers = (n.round() as u32).clamp(1, MOST_WORKERS);
                }
            }
            AgentMsg::ChooseModel(under) => self.menu = if self.menu.is_some() { None } else { Some(Point::new(under.x, under.bottom() + 4.0)) },
            AgentMsg::CloseMenu => self.menu = None,
            AgentMsg::Model(model) => {
                self.menu = None;
                if let Some(n) = self.chosen.and_then(|id| self.open.node_mut(id)) {
                    n.model = model;
                }
            }
            AgentMsg::Question(q) => self.question = q,
            AgentMsg::Run => {
                if self.running || self.question.trim().is_empty() {
                    return None;
                }
                if let Some(why) = self.open.trouble() {
                    self.trouble = Some(why);
                    return None;
                }
                (self.made, self.answer, self.trouble, self.running) = (HashMap::new(), None, None, true);
                self.stop = Arc::default();
                return Some(ToRun { flow: self.open.clone(), question: self.question.trim().to_owned(), stop: self.stop.clone() });
            }
            AgentMsg::Stop => self.stop.store(true, Ordering::Relaxed),
            AgentMsg::Step(step) => {
                fn made(made: &mut HashMap<u32, Made>, id: u32) -> &mut Made {
                    made.entry(id).or_insert(Made { stage: Stage::Running, text: String::new(), pieces: vec![] })
                }
                match step {
                    Step::Started(id) => drop(made(&mut self.made, id)),
                    Step::Worker(id, _, piece) => made(&mut self.made, id).pieces.push(piece),
                    Step::Piece(id, more) => made(&mut self.made, id).text.push_str(&more),
                    Step::Done(id, text) => {
                        let m = made(&mut self.made, id);
                        (m.stage, m.text) = (Stage::Done, text);
                    }
                    Step::Failed(id, why) => {
                        let m = made(&mut self.made, id);
                        (m.stage, m.text) = (Stage::Failed, why);
                    }
                }
            }
            AgentMsg::Finished(result) => {
                self.running = false;
                // What was under way when it stopped did not finish.
                for made in self.made.values_mut().filter(|m| m.stage == Stage::Running) {
                    made.stage = Stage::Failed;
                }
                match result {
                    Ok(answer) => self.answer = Some(answer),
                    Err(why) => self.trouble = Some(why),
                }
            }
        }
        self.changed |= self.open != before;
        None
    }

    fn shown(&self) -> Vec<Shown> {
        self.open
            .nodes
            .iter()
            .map(|n| {
                let under = match (n.kind.thinks(), n.model.as_str()) {
                    (true, "") => format!("{} · Apollo's model", n.kind.name()),
                    (true, model) => format!("{} · {model}", n.kind.name()),
                    (false, _) => n.kind.name().to_owned(),
                };
                Shown { id: n.id, title: n.title.clone(), under, at: Point::new(n.x, n.y), stage: self.made.get(&n.id).map(|m| m.stage), leads: n.kind != NodeKind::Output }
            })
            .collect()
    }

    /// The page. `models` are the ones there are to give a node.
    pub fn view(&self, models: &[String], ready: bool) -> Element<AgentMsg> {
        let flow = &self.open;
        // The workflows that are kept, and a new one.
        let mut kept = column().spacing(2.0).width(170.0).push(section("Workflows"));
        for w in &self.kept {
            kept = kept.push(Button::new(text(w.name.clone()).width(Length::Fill)).kind(ButtonKind::Ghost).selected(self.kept_as.as_deref() == Some(w.name.as_str())).width(Length::Fill).align_x(Align::Start).on_press(AgentMsg::Open(w.name.clone())));
        }
        if self.kept.is_empty() {
            kept = kept.push(container(text("None kept yet. Save this one to keep it.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).padding([10.0, 4.0]));
        }
        kept = kept.push(Space::new(0.0, 8.0)).push(button("New Workflow").on_press(AgentMsg::New));

        // Its name, keeping it, and the kinds of node there are to add.
        let mut bar = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text_input("Name", flow.name.clone()).on_input(AgentMsg::Name).width(240.0)).push(Button::new(text(if self.changed { "Save" } else { "Saved" }).role(TextRole::Strong)).kind(ButtonKind::Accent).on_press_maybe(self.changed.then_some(AgentMsg::Save)));
        if self.kept_as.is_some() {
            bar = bar.push(button("Delete").on_press(AgentMsg::Delete));
        }
        // The kinds of node there are to add, on a line of their own.
        let mut add = row().spacing(6.0).align(Align::Center).width(Length::Fill).push(text("Add a node").role(TextRole::Caption).tone(Tone::Muted));
        for kind in NodeKind::ALL {
            add = add.push(Button::new(text(kind.name()).role(TextRole::Caption).no_wrap()).on_press(AgentMsg::Add(kind)));
        }

        let graph = container(Element::new(flow_view(self.shown(), flow.edges.clone(), self.chosen, AgentMsg::Graph))).width(Length::Fill).height(280.0);

        // What the chosen node is, to change; or how to choose one.
        let chosen: Element<AgentMsg> = match self.chosen.and_then(|id| flow.node(id)) {
            None => text("Choose a node to give it instructions and a model. Drag a node to move it, and drag from the dot on its right edge to another node to join them.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill).into(),
            Some(n) => {
                let mut about = column().spacing(10.0).width(Length::Fill);
                let mut head = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text(n.kind.name()).role(TextRole::Label).tone(Tone::Muted)).push(text_input("Title", n.title.clone()).on_input(AgentMsg::Title).width(220.0));
                if n.kind.thinks() {
                    let model = if n.model.is_empty() { "Apollo's model".to_owned() } else { n.model.clone() };
                    head = head.push(mouse_area(container(row().spacing(8.0).align(Align::Center).push(icon(icons::CPU).size(14.0).tone(Tone::Muted)).push(text(model)).push(icon(icons::CHEVRON_DOWN).size(14.0).tone(Tone::Muted))).surface(Surface::Raised).padding([12.0, 7.0])).on_press_in(AgentMsg::ChooseModel));
                }
                head = head.push(Space::fill_x()).push(button("Remove").on_press(AgentMsg::Remove));
                about = about.push(head);
                if n.kind == NodeKind::Workers {
                    about = about.push(row().spacing(10.0).align(Align::Center).push(text("Workers at most")).push(number_field(f64::from(n.workers)).step(1.0).range(1.0..=f64::from(MOST_WORKERS)).width(70.0).on_change(AgentMsg::Workers)).push(text("It splits what reaches it into that many pieces of work, and gives each worker the instructions below.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)));
                }
                if n.kind.thinks() {
                    about = about.push(prompt(&self.instructions, AgentMsg::Instructions).placeholder("What this agent is to do with what reaches it").max_lines(5));
                } else {
                    about = about.push(text(match n.kind {
                        NodeKind::Input => "What you ask comes in here, and goes on to whatever it is joined to.",
                        NodeKind::Memory => "Looks up what reaches it in Apollo's memory of your files, and passes on what it finds. The memory has to be unlocked.",
                        _ => "What reaches this is the answer.",
                    }).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill));
                }
                // What leads into it, each to be unjoined.
                let from = flow.leading_to(n.id);
                if !from.is_empty() {
                    let mut joins = row().spacing(6.0).align(Align::Center).push(text("From").role(TextRole::Caption).tone(Tone::Muted));
                    for f in from {
                        let title = flow.node(f).map_or(String::new(), |x| x.title.clone());
                        joins = joins.push(Button::new(row().spacing(6.0).align(Align::Center).push(text(title).role(TextRole::Caption)).push(icon(icons::X).size(12.0).tone(Tone::Muted))).kind(ButtonKind::Ghost).on_press(AgentMsg::Unjoin(f, n.id)));
                    }
                    about = about.push(joins);
                }
                // What it made of the last run.
                if let Some(made) = self.made.get(&n.id) {
                    let said = match made.stage {
                        Stage::Running => "Working…",
                        Stage::Done => "What it made",
                        Stage::Failed => "It did not finish",
                    };
                    about = about.push(text(said).role(TextRole::Label).tone(Tone::Muted));
                    if !made.pieces.is_empty() {
                        about = about.push(text(format!("Shared out as {}: {}", made.pieces.len(), made.pieces.join("; "))).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill));
                    }
                    about = about.push(markdown(&made.text));
                }
                container(about).surface(Surface::Well).padding([14.0, 12.0]).width(Length::Fill).into()
            }
        };

        // What to ask of it, and what came out.
        let mut ask = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text_input("Ask something of this workflow", self.question.clone()).on_input(AgentMsg::Question).on_submit(AgentMsg::Run).width(Length::Fill));
        ask = ask.push(if self.running { button("Stop").on_press(AgentMsg::Stop) } else { Button::new(text("Run").role(TextRole::Strong)).kind(ButtonKind::Accent).on_press_maybe((ready && !self.question.trim().is_empty()).then_some(AgentMsg::Run)) });
        let mut below = column().spacing(12.0).width(Length::Fill).push(chosen).push(ask);
        if !ready {
            below = below.push(notice(Tone::Warn, "The model is not ready, so nothing can be run yet."));
        }
        if let Some(why) = &self.trouble {
            below = below.push(notice(Tone::Bad, why.clone()));
        }
        if let Some(answer) = &self.answer {
            below = below.push(text("Answer").role(TextRole::Label).tone(Tone::Muted)).push(container(markdown(answer)).surface(Surface::Well).padding([14.0, 12.0]).width(Length::Fill));
        }

        let main = column().spacing(12.0).width(Length::Fill).height(Length::Fill).push(text("Agents").role(TextRole::Heading)).push(bar).push(add).push(graph).push(scrollable(below));
        let page: Element<AgentMsg> = row().spacing(18.0).padding(22.0).width(Length::Fill).height(Length::Fill).push(kept).push(main).into();
        // The list of models hangs over everything, from the control that opened it.
        match self.menu {
            Some(at) => {
                let current = self.chosen.and_then(|id| flow.node(id)).map_or("", |n| n.model.as_str());
                let entry = |label: String, model: String| if model == current { MenuItem::new(label, AgentMsg::CloseMenu).icon(icons::CHECK) } else { MenuItem::new(label, AgentMsg::Model(model)) };
                let mut items = vec![entry("Apollo's model".to_owned(), String::new())];
                items.extend(models.iter().map(|m| entry(m.clone(), m.clone())));
                stack().width(Length::Fill).height(Length::Fill).push(page).push(popup_menu(at, items, AgentMsg::CloseMenu)).into()
            }
            None => page,
        }
    }
}
