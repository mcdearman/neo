//! Work shared out between agents: a workflow.
//!
//! A workflow is a graph. What is asked comes in at one end; each node
//! does something with what reaches it and passes on what it made; the
//! answer comes out at the other. A node that is an agent has
//! instructions of its own and may have a model of its own, so that a
//! small quick model can sort and a larger one can write. A node of
//! workers has the model split what reaches it into pieces and sets a
//! worker to each.
//!
//! Workflows are kept as TOML, one to a file, in `workflows` in Apollo's
//! folder, to be read, shared and written by hand as much as drawn.

use std::path::PathBuf;

use neo_desktop::config::File;

use crate::Model;
use crate::model::{Role, Turn};
use crate::store::Store;

/// What a node does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Where what is asked comes in.
    Input,
    /// A model given instructions, and what reaches the node to work on.
    Agent,
    /// A model that splits what reaches it into pieces of work and sets a
    /// worker, with the node's instructions, to each.
    Workers,
    /// Apollo's memory of the user's files, searched for what reaches it.
    Memory,
    /// Where the answer comes out.
    Output,
}

impl NodeKind {
    pub const ALL: [NodeKind; 5] = [NodeKind::Input, NodeKind::Agent, NodeKind::Workers, NodeKind::Memory, NodeKind::Output];

    pub fn id(self) -> &'static str {
        match self {
            NodeKind::Input => "input",
            NodeKind::Agent => "agent",
            NodeKind::Workers => "workers",
            NodeKind::Memory => "memory",
            NodeKind::Output => "output",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NodeKind::Input => "Question",
            NodeKind::Agent => "Agent",
            NodeKind::Workers => "Workers",
            NodeKind::Memory => "Memory",
            NodeKind::Output => "Answer",
        }
    }

    /// Whether a model does this node's work, and so one can be chosen for it.
    pub fn thinks(self) -> bool {
        matches!(self, NodeKind::Agent | NodeKind::Workers)
    }
}

/// One step of a workflow.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: u32,
    pub kind: NodeKind,
    pub title: String,
    /// The model that does its work, by name; empty for the one Apollo
    /// answers with.
    pub model: String,
    /// What it is told to do, for a node that thinks.
    pub prompt: String,
    /// The most workers a node of workers sets to work.
    pub workers: u32,
    /// Where it is drawn.
    pub x: f32,
    pub y: f32,
}

/// The most workers one node may set to work, whatever it is told.
pub const MOST_WORKERS: u32 = 8;

/// A graph of nodes, each passing what it makes to those it leads to.
#[derive(Clone, Debug, PartialEq)]
pub struct Workflow {
    pub name: String,
    pub nodes: Vec<Node>,
    /// From one node to another, by their IDs.
    pub edges: Vec<(u32, u32)>,
}

/// Something that happens as a workflow runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Started(u32),
    /// A node set a worker to a piece of work: which worker, and the piece.
    Worker(u32, usize, String),
    /// More of what a node is making, as it comes.
    Piece(u32, String),
    Done(u32, String),
    Failed(u32, String),
}

impl Workflow {
    /// A workflow to begin from: what is asked is split between workers,
    /// and what they find is put together into one answer.
    pub fn starter(name: &str) -> Self {
        let node = |id, kind: NodeKind, title: &str, prompt: &str, (x, y): (f32, f32)| Node { id, kind, title: title.into(), model: String::new(), prompt: prompt.into(), workers: 3, x, y };
        Self {
            name: name.to_owned(),
            nodes: vec![
                node(1, NodeKind::Input, "Question", "", (20.0, 80.0)),
                node(2, NodeKind::Workers, "Workers", "Do this one piece of work thoroughly and say what you found, in a few sentences.", (215.0, 20.0)),
                node(3, NodeKind::Agent, "Sum up", "Put what the workers found together into one clear answer to the question. Do not mention the workers.", (410.0, 80.0)),
                node(4, NodeKind::Output, "Answer", "", (410.0, 190.0)),
            ],
            edges: vec![(1, 2), (2, 3), (1, 3), (3, 4)],
        }
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: u32) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Adds a node of a kind, at a place, and says which it is.
    pub fn add(&mut self, kind: NodeKind, x: f32, y: f32) -> u32 {
        let id = self.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        let prompt = match kind {
            NodeKind::Agent => "Answer from what you are given, briefly.",
            NodeKind::Workers => "Do this one piece of work thoroughly and say what you found, in a few sentences.",
            _ => "",
        };
        self.nodes.push(Node { id, kind, title: kind.name().to_owned(), model: String::new(), prompt: prompt.into(), workers: 3, x, y });
        id
    }

    /// Takes a node out, with whatever led to it or from it.
    pub fn remove(&mut self, id: u32) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|(a, b)| *a != id && *b != id);
    }

    /// The nodes that lead to a node, in the order they were joined.
    pub fn leading_to(&self, id: u32) -> Vec<u32> {
        self.edges.iter().filter(|(_, to)| *to == id).map(|(from, _)| *from).collect()
    }

    fn reaches(&self, from: u32, to: u32) -> bool {
        from == to || self.edges.iter().any(|(a, b)| *a == from && self.reaches(*b, to))
    }

    /// Joins one node to another, so that what the first makes reaches
    /// the second. Says why not, if it cannot be: a node to itself, into
    /// where the question comes in or out of where the answer goes, twice
    /// over, or round in a ring.
    pub fn connect(&mut self, from: u32, to: u32) -> Result<(), String> {
        let (Some(a), Some(b)) = (self.node(from), self.node(to)) else { return Err("There is no such node.".into()) };
        if from == to {
            return Err("A node cannot lead to itself.".into());
        }
        if b.kind == NodeKind::Input {
            return Err("Nothing leads into the question: it is where things start.".into());
        }
        if a.kind == NodeKind::Output {
            return Err("Nothing leads on from the answer: it is where things end.".into());
        }
        if self.edges.contains(&(from, to)) {
            return Err("Those two are joined already.".into());
        }
        if self.reaches(to, from) {
            return Err("That would go round in a ring, and never finish.".into());
        }
        self.edges.push((from, to));
        Ok(())
    }

    pub fn disconnect(&mut self, from: u32, to: u32) {
        self.edges.retain(|e| *e != (from, to));
    }

    /// The nodes in an order in which each comes after all that lead to it.
    pub fn order(&self) -> Vec<u32> {
        let mut done: Vec<u32> = vec![];
        while done.len() < self.nodes.len() {
            let ready: Vec<u32> = self.nodes.iter().filter(|n| !done.contains(&n.id) && self.leading_to(n.id).iter().all(|from| done.contains(from) || self.node(*from).is_none())).map(|n| n.id).collect();
            if ready.is_empty() {
                break;
            }
            done.extend(ready);
        }
        done
    }

    /// What is wrong with it, that would keep it from running: nothing, if nothing.
    pub fn trouble(&self) -> Option<String> {
        let count = |kind| self.nodes.iter().filter(|n| n.kind == kind).count();
        match (count(NodeKind::Input), count(NodeKind::Output)) {
            (0, _) => return Some("It has nowhere for the question to come in: add a Question.".into()),
            (_, 0) => return Some("It has nowhere for the answer to come out: add an Answer.".into()),
            _ => {}
        }
        if self.order().len() < self.nodes.len() {
            return Some("Some of it goes round in a ring, and would never finish.".into());
        }
        let stranded = self.nodes.iter().find(|n| n.kind != NodeKind::Input && self.leading_to(n.id).is_empty());
        stranded.map(|n| format!("Nothing leads to “{}”, so it would have nothing to work on.", n.title))
    }

    // --- keeping -----------------------------------------------------------

    /// As TOML: its name, a table to each node, and a table to each join.
    pub fn encode(&self) -> String {
        let quoted = |s: &str| toml_string(s);
        let mut out = format!("name = {}\n", quoted(&self.name));
        for n in &self.nodes {
            out.push_str(&format!("\n[[nodes]]\nid = {}\nkind = \"{}\"\ntitle = {}\n", n.id, n.kind.id(), quoted(&n.title)));
            if n.kind.thinks() {
                out.push_str(&format!("model = {}\nprompt = {}\n", quoted(&n.model), quoted(&n.prompt)));
            }
            if n.kind == NodeKind::Workers {
                out.push_str(&format!("workers = {}\n", n.workers));
            }
            out.push_str(&format!("x = {}\ny = {}\n", n.x.round(), n.y.round()));
        }
        for (from, to) in &self.edges {
            out.push_str(&format!("\n[[edges]]\nfrom = {from}\nto = {to}\n"));
        }
        out
    }

    /// Reads what [`encode`](Self::encode) wrote, or what was written by
    /// hand: what is missing has its usual value, and a join to a node
    /// that is not there, or that could not be made, is left out.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file = File::parse(text);
        if let Some(problem) = file.problem() {
            return Err(problem);
        }
        let mut flow = Workflow { name: file.text(&["name"]).unwrap_or("Workflow").to_owned(), nodes: vec![], edges: vec![] };
        for (i, n) in file.each(&["nodes"]).iter().enumerate() {
            let whole = |key: &str| n.number(&[key]);
            let kind = NodeKind::ALL.into_iter().find(|k| Some(k.id()) == n.text(&["kind"])).unwrap_or(NodeKind::Agent);
            let id = whole("id").map_or(i as u32 + 1, |v| v.max(1.0) as u32);
            if flow.node(id).is_some() {
                continue;
            }
            flow.nodes.push(Node {
                id,
                kind,
                title: n.text(&["title"]).filter(|t| !t.trim().is_empty()).unwrap_or(kind.name()).to_owned(),
                model: n.text(&["model"]).unwrap_or_default().trim().to_owned(),
                prompt: n.text(&["prompt"]).unwrap_or_default().to_owned(),
                workers: whole("workers").map_or(3, |v| (v as u32).clamp(1, MOST_WORKERS)),
                x: whole("x").unwrap_or(40.0 + i as f64 * 220.0) as f32,
                y: whole("y").unwrap_or(80.0) as f32,
            });
        }
        for e in file.each(&["edges"]) {
            if let (Some(from), Some(to)) = (e.number(&["from"]), e.number(&["to"])) {
                let _ = flow.connect(from as u32, to as u32);
            }
        }
        Ok(flow)
    }

    /// Where workflows are kept.
    pub fn folder() -> PathBuf {
        crate::dir().join("workflows")
    }

    /// The file a workflow of this name is kept in, in a folder: its
    /// name, in letters a file can have.
    pub fn file_in(folder: &std::path::Path, name: &str) -> PathBuf {
        let safe: String = name.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect();
        folder.join(format!("{}.toml", if safe.trim_matches('-').is_empty() { "workflow" } else { safe.trim_matches('-') }))
    }

    /// The workflows kept in a folder, by name.
    pub fn saved_in(folder: &std::path::Path) -> Vec<Workflow> {
        let mut all: Vec<Workflow> = std::fs::read_dir(folder).into_iter().flatten().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "toml")).filter_map(|e| Self::parse(&std::fs::read_to_string(e.path()).ok()?).ok()).collect();
        all.sort_by_key(|w| w.name.to_lowercase());
        all
    }

    pub fn save_in(&self, folder: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        std::fs::write(Self::file_in(folder, &self.name), self.encode())
    }

    pub fn delete_in(folder: &std::path::Path, name: &str) -> std::io::Result<()> {
        std::fs::remove_file(Self::file_in(folder, name))
    }
}

/// A string as TOML writes one: on one line, with what needs it escaped.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// How many memories a memory node brings back.
const RECALLED: usize = 6;

/// Runs a workflow on `question` and gives its answer. `report` hears
/// each thing that happens and stops the run by returning false. The
/// models are asked one at a time: on one computer two large ones at
/// once would not fit.
pub fn run(flow: &Workflow, question: &str, model: &dyn Model, store: Option<&Store>, report: &mut dyn FnMut(Step) -> bool) -> Result<String, String> {
    if let Some(trouble) = flow.trouble() {
        return Err(trouble);
    }
    let stopped = || "Stopped.".to_owned();
    let mut made: Vec<(u32, String)> = vec![];
    for id in flow.order() {
        let Some(node) = flow.node(id) else { continue };
        if !report(Step::Started(id)) {
            return Err(stopped());
        }
        // What reaches it: what each node that leads to it made, named where there are several.
        let from: Vec<(&str, &str)> = flow.leading_to(id).iter().filter_map(|f| Some((flow.node(*f)?.title.as_str(), made.iter().find(|(m, _)| m == f)?.1.as_str()))).collect();
        let given = match from.as_slice() {
            [(_, only)] => (*only).to_owned(),
            several => several.iter().map(|(title, text)| format!("{title}:\n{text}")).collect::<Vec<_>>().join("\n\n"),
        };
        let named = (!node.model.trim().is_empty()).then(|| node.model.trim());
        let mut going = true;
        let result = match node.kind {
            NodeKind::Input => Ok(question.to_owned()),
            NodeKind::Output => Ok(given),
            NodeKind::Agent => model.chat_as(named, &[Turn::new(Role::System, node.prompt.clone()), Turn::new(Role::User, given)], &mut |piece| {
                going = report(Step::Piece(id, piece.to_owned()));
                going
            }),
            NodeKind::Workers => (|| {
                // The pieces first, a line to each; then a worker to each piece.
                let most = node.workers.clamp(1, MOST_WORKERS) as usize;
                let split = format!("Split the task you are given into at most {most} separate pieces of work that can each be done on its own. Write each piece as a short instruction on a line of its own, and nothing else: no numbers, no introduction.");
                let pieces = model.chat_as(named, &[Turn::new(Role::System, split), Turn::new(Role::User, given.clone())], &mut |_| true)?;
                let mut pieces: Vec<String> = pieces.lines().map(|l| l.trim().trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '-' | '*' | '.' | ')' | '•' | ' ')).trim().to_owned()).filter(|l| l.split_whitespace().count() >= 2).take(most).collect();
                // A small model will put them on one line with commas between: those are the pieces.
                if let ([only], true) = (pieces.as_slice(), most > 1) {
                    let parts: Vec<String> = only.split([',', ';']).map(|part| part.trim().trim_end_matches('.').to_owned()).filter(|part| !part.is_empty()).take(most).collect();
                    if parts.len() > 1 {
                        pieces = parts;
                    }
                }
                // A model that would not split it has the whole of it for one worker.
                if pieces.is_empty() {
                    pieces.push(given.lines().next().unwrap_or(&given).to_owned());
                }
                let mut found = vec![];
                for (i, piece) in pieces.iter().enumerate() {
                    if !report(Step::Worker(id, i, piece.clone())) {
                        going = false;
                        return Err(stopped());
                    }
                    let heading = format!("\n\n## {piece}\n");
                    going = report(Step::Piece(id, heading.trim_start_matches(if i == 0 { '\n' } else { '\0' }).to_owned()));
                    let did = model.chat_as(named, &[Turn::new(Role::System, node.prompt.clone()), Turn::new(Role::User, format!("Your piece of work: {piece}\n\nThe whole task, for what it is part of:\n{given}"))], &mut |text| {
                        going = going && report(Step::Piece(id, text.to_owned()));
                        going
                    })?;
                    found.push(format!("## {piece}\n{}", did.trim()));
                    if !going {
                        return Err(stopped());
                    }
                }
                Ok(found.join("\n\n"))
            })(),
            NodeKind::Memory => match store {
                None => Ok("(Apollo's memory is locked, so nothing could be looked up in it.)".to_owned()),
                Some(store) => (|| {
                    let asked = model.embed(std::slice::from_ref(&given), true)?.pop().ok_or("no embedding")?;
                    let hits = store.search_for(&asked, &crate::words::terms(&given), RECALLED, None)?;
                    if hits.is_empty() {
                        return Ok("(Nothing in Apollo's memory bears on this.)".to_owned());
                    }
                    Ok(hits.iter().enumerate().map(|(i, h)| format!("{}. {}: {}", i + 1, h.memory.title, h.memory.text.chars().take(600).collect::<String>())).collect::<Vec<_>>().join("\n"))
                })(),
            },
        };
        match result {
            // Stopped by whoever is listening: not a failure of the node's.
            _ if !going => return Err(stopped()),
            Ok(text) => {
                let text = text.trim().to_owned();
                if !report(Step::Done(id, text.clone())) {
                    return Err(stopped());
                }
                made.push((id, text));
            }
            Err(why) => {
                report(Step::Failed(id, why.clone()));
                return Err(format!("“{}” could not do its work: {why}", node.title));
            }
        }
    }
    // What came out: the first answer there is.
    let out = flow.nodes.iter().find(|n| n.kind == NodeKind::Output).and_then(|n| made.iter().find(|(id, _)| *id == n.id));
    Ok(out.map(|(_, text)| text.clone()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fake::Fake;

    #[test]
    fn a_workflow_is_written_as_toml_and_read_back() {
        let mut flow = Workflow::starter("Research, with workers");
        flow.node_mut(3).unwrap().model = "qwen3:8b".into();
        flow.node_mut(3).unwrap().prompt = "Put it together.\nSay \"done\" at the end.".into();
        flow.node_mut(2).unwrap().workers = 5;
        let text = flow.encode();
        assert!(text.starts_with("name = \"Research, with workers\"\n\n[[nodes]]\nid = 1\nkind = \"input\"\ntitle = \"Question\"\nx = 20\ny = 80\n"), "{text}");
        assert!(text.contains("model = \"qwen3:8b\"\nprompt = \"Put it together.\\nSay \\\"done\\\" at the end.\"\n") && text.contains("workers = 5\n") && text.ends_with("[[edges]]\nfrom = 3\nto = 4\n"), "{text}");
        assert_eq!(Workflow::parse(&text), Ok(flow.clone()));
        // By hand, with little said: the rest has its usual value, and what cannot be is left out.
        let few = Workflow::parse("name = \"Mine\"\n[[nodes]]\nkind = \"input\"\n[[nodes]]\nkind = \"agent\"\nmodel = \" gemma3:4b \"\n[[nodes]]\nkind = \"output\"\n[[nodes]]\nid = 2\nkind = \"memory\"\n[[edges]]\nfrom = 1\nto = 2\n[[edges]]\nfrom = 2\nto = 3\n[[edges]]\nfrom = 3\nto = 1\n[[edges]]\nfrom = 2\nto = 9\n").unwrap();
        assert_eq!((few.nodes.len(), few.edges.clone(), few.node(2).map(|n| (n.kind, n.model.as_str(), n.title.as_str(), n.workers))), (3, vec![(1, 2), (2, 3)], Some((NodeKind::Agent, "gemma3:4b", "Agent", 3))));
        assert!(Workflow::parse("name = = 3").unwrap_err().contains("mistake"));
        assert_eq!(Workflow::file_in(std::path::Path::new("/w"), " Research, with Workers! ").file_name().unwrap().to_string_lossy(), "research--with-workers.toml");
        assert_eq!(Workflow::file_in(std::path::Path::new("/w"), "///").file_name().unwrap().to_string_lossy(), "workflow.toml");
        // Kept in a folder, found there again by name, and removed.
        let dir = std::env::temp_dir().join(format!("neo-apollo-flows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(Workflow::saved_in(&dir).is_empty());
        flow.save_in(&dir).unwrap();
        Workflow::starter("another one").save_in(&dir).unwrap();
        std::fs::write(dir.join("broken.toml"), "name = = 3").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a workflow").unwrap();
        assert_eq!(Workflow::saved_in(&dir).iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["another one", "Research, with workers"]);
        Workflow::delete_in(&dir, "another one").unwrap();
        assert_eq!(Workflow::saved_in(&dir), [flow.clone()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nodes_are_joined_only_in_ways_that_can_run() {
        let mut flow = Workflow::starter("w");
        assert_eq!(flow.order(), [1, 2, 3, 4]);
        assert_eq!(flow.trouble(), None);
        let memory = flow.add(NodeKind::Memory, 0.0, 0.0);
        assert_eq!((memory, flow.trouble().unwrap().as_str()), (5, "Nothing leads to “Memory”, so it would have nothing to work on."));
        assert_eq!(flow.connect(1, memory), Ok(()));
        assert_eq!(flow.connect(memory, 3), Ok(()));
        assert_eq!((flow.leading_to(3), flow.order()), (vec![2, 1, 5], vec![1, 2, 5, 3, 4]));
        for (from, to, why) in [(3, 3, "itself"), (3, 1, "into the question"), (4, 3, "on from the answer"), (1, 2, "joined already"), (3, 2, "round in a ring"), (3, 99, "no such node")] {
            let said = flow.connect(from, to).unwrap_err();
            assert!(said.contains(why), "{from} to {to}: {said}");
        }
        // A node taken out takes its joins with it; with no answer, or no question, it cannot run.
        flow.remove(memory);
        assert_eq!(flow.edges, [(1, 2), (2, 3), (1, 3), (3, 4)]);
        flow.remove(4);
        assert!(flow.trouble().unwrap().contains("add an Answer"));
        flow.remove(1);
        assert!(flow.trouble().unwrap().contains("add a Question"));
    }

    #[test]
    fn a_workflow_runs_node_by_node_each_on_the_model_it_names() {
        let mut flow = Workflow::starter("w");
        flow.node_mut(2).unwrap().model = "small".into();
        flow.node_mut(3).unwrap().model = "large".into();
        let mut steps = vec![];
        let answer = run(&flow, "Which is taller, a giraffe or an elephant?", &Fake::default(), None, &mut |s| {
            steps.push(s);
            true
        })
        .unwrap();
        // The summing-up was done by the model its node names, on what the workers made and the question both.
        assert!(answer.starts_with("[large] "), "{answer}");
        let done: Vec<(u32, &str)> = steps.iter().filter_map(|s| if let Step::Done(id, text) = s { Some((*id, text.as_str())) } else { None }).collect();
        assert_eq!(done.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [1, 2, 3, 4], "each in its turn");
        assert_eq!((done[0].1, done[3].1), ("Which is taller, a giraffe or an elephant?", answer.as_str()), "the question goes in and the answer comes out");
        // The workers' node split the task and set a worker to each piece, on its own model.
        let workers: Vec<&String> = steps.iter().filter_map(|s| if let Step::Worker(2, _, piece) = s { Some(piece) } else { None }).collect();
        assert!(!workers.is_empty() && workers.len() <= 3 && done[1].1.matches("[small] ").count() == workers.len() && done[1].1.starts_with("## "), "{:?}\n{}", workers, done[1].1);
        assert!(steps.iter().any(|s| matches!(s, Step::Piece(3, _))), "what is being made is heard as it comes");
        // Stopped part of the way: it ends there and says so.
        let mut heard = 0;
        let stopped = run(&flow, "Anything", &Fake::default(), None, &mut |_| {
            heard += 1;
            heard < 4
        });
        assert_eq!(stopped, Err("Stopped.".into()));
        // One that cannot run says why, and runs nothing.
        flow.remove(4);
        assert!(run(&flow, "Anything", &Fake::default(), None, &mut |_| panic!("nothing is begun")).unwrap_err().contains("add an Answer"));
    }

    /// By hand, with the real model: the workflow to begin from, run on a question.
    /// `NEO_APOLLO_QUERY="…" cargo test -p neo-apollo-core real_workflow -- --ignored --nocapture`
    /// `NEO_APOLLO_SUMMARY_MODEL` gives the summing-up a model of its own.
    #[test]
    #[ignore]
    fn real_workflow() {
        let model = crate::model::Ollama::from_settings(&crate::settings::Settings::load());
        let mut flow = Workflow::starter("probe");
        if let Ok(other) = std::env::var("NEO_APOLLO_SUMMARY_MODEL") {
            flow.node_mut(3).unwrap().model = other;
        }
        let began = std::time::Instant::now();
        let answer = run(&flow, &std::env::var("NEO_APOLLO_QUERY").unwrap(), &model, None, &mut |step| {
            match step {
                Step::Started(id) => println!("[{:>5.1}s] started {}", began.elapsed().as_secs_f32(), flow.node(id).unwrap().title),
                Step::Worker(_, i, piece) => println!("[{:>5.1}s]   worker {}: {piece}", began.elapsed().as_secs_f32(), i + 1),
                Step::Done(id, text) => println!("[{:>5.1}s] done {} ({} characters)", began.elapsed().as_secs_f32(), flow.node(id).unwrap().title, text.len()),
                Step::Failed(id, why) => println!("failed {id}: {why}"),
                Step::Piece(..) => {}
            }
            true
        });
        println!("\n{answer:?}");
    }

    #[test]
    fn a_memory_node_looks_up_what_reaches_it() {
        let dir = std::env::temp_dir().join(format!("neo-apollo-flow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model = Fake::default();
        let mut store = Store::open(&dir.join("memory.db"), &[3; 32], model.dims().unwrap()).unwrap();
        for (title, text) in [("Beach.jpg", "A dog running on the beach by the sea."), ("Invoice.pdf", "An invoice from a supplier for payment by bank.")] {
            store.add(&crate::store::New { kind: crate::Kind::Document, source: None, title, text, part: 0, created: 0, words: &[] }, &model.embed(&[text.to_owned()], false).unwrap()[0]).unwrap();
        }
        let mut flow = Workflow { name: "m".into(), nodes: vec![], edges: vec![] };
        let (q, m, a) = (flow.add(NodeKind::Input, 0.0, 0.0), flow.add(NodeKind::Memory, 0.0, 0.0), flow.add(NodeKind::Output, 0.0, 0.0));
        flow.connect(q, m).unwrap();
        flow.connect(m, a).unwrap();
        let found = run(&flow, "the dog on the beach", &model, Some(&store), &mut |_| true).unwrap();
        assert!(found.starts_with("1. Beach.jpg: A dog running"), "{found}");
        // Locked, it says so and the rest goes on.
        assert!(run(&flow, "the dog on the beach", &model, None, &mut |_| true).unwrap().contains("memory is locked"));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
