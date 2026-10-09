//! System Monitor: processes, CPU, memory, disks and network.
//!
//!     cargo run -p neo-monitor
//!     cargo run -p neo-monitor -- --snapshot target/snapshots

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ask;
mod detail;
mod evidence;
mod heat;
mod inspect;
mod net;
mod power;
mod sensors;
#[cfg(target_os = "macos")]
mod smc;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use neo::prelude::*;
use neo::{Key, KeyEvent, Proxy, Size};
use neo_desktop::Desktop;
use neo_desktop::fs::{human_bytes_binary, human_size};
use neo_desktop::ui::{nav_item, notice, section, split};
use sysinfo::{Disks, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind, Users};

const HISTORY: usize = 60;
const SAMPLE: Duration = Duration::from_millis(1500);
const MAX_PROCESSES: usize = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Processes,
    Resources,
    Sensors,
    Network,
    Storage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortBy {
    Name,
    User,
    Cpu,
    Memory,
    Swap,
    Energy,
    Threads,
    Pid,
}

/// One row of the process table, copied out of sysinfo on each sample.
#[derive(Clone, Debug)]
struct Proc {
    pid: Pid,
    name: String,
    user: String,
    /// Percent of the whole machine, 0 to 100.
    cpu: f32,
    memory: u64,
    virtual_memory: u64,
    /// The part of `memory` that is in RAM right now.
    resident: u64,
    threads: Option<u32>,
    /// Swapped-out bytes, on systems that report it cheaply for every process.
    swap: Option<u64>,
    /// The power it drew since the last sample, in watts, where that is known.
    energy: Option<f32>,
}

struct Monitor {
    desktop: Desktop,
    sys: System,
    networks: Networks,
    disks: Disks,
    users: Users,
    last_sample: Instant,
    page: Page,
    cpu: VecDeque<f32>,
    memory: VecDeque<f32>,
    /// Memory pressure from 0 to 100, and how the system grades it now.
    pressure: VecDeque<f32>,
    pressure_now: Option<power::Pressure>,
    meter: power::Meter,
    rx: VecDeque<f32>,
    tx: VecDeque<f32>,
    procs: Vec<Proc>,
    query: String,
    sort: SortBy,
    descending: bool,
    selected: Option<Pid>,
    /// Where a selection of several began: they are those from here to
    /// `selected`, as the list is sorted. `None` when one is selected.
    anchor: Option<Pid>,
    confirm_end: bool,
    status: Option<(Tone, String)>,
    /// The process whose threads are shown in place of the process list.
    inspect: Option<detail::Inspect>,
    proxy: Option<Proxy<Msg>>,
    /// The latest temperatures and fan speeds, once the reader has answered.
    heat: Option<sensors::Reading>,
    cpu_heat: VecDeque<f32>,
    gpu_heat: VecDeque<f32>,
    /// The interfaces worth showing, as last read.
    interfaces: Vec<net::Interface>,
    /// Which programs are using the network, once it has been asked.
    net_using: Option<Vec<net::Using>>,
    /// Tells the thread that asks whether anyone is looking, as with the sensors.
    watching_net: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Tells the sensor thread whether anyone is looking, so it rests otherwise.
    watching_heat: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Clone, Debug)]
enum Msg {
    Page(Page),
    /// Which programs are using the network, just asked.
    NetUsing(Vec<net::Using>),
    Sample,
    Query(String),
    Sort(SortBy),
    Select(Pid),
    /// Show the selected process's threads, memory and swap.
    Inspect,
    CloseInspect,
    ThreadSort(detail::ThreadSort),
    /// A swap measurement finished, for the process with this ID.
    Swap(u32, Option<u64>),
    /// A reading from the sensor thread.
    Heat(sensors::Reading),
    Move(isize),
    /// Move the selection's end, keeping where it began: Shift with an arrow.
    Extend(isize),
    /// Ask Apollo what the selected processes are.
    AskApollo,
    /// What was found out about them, to send with the question.
    FoundOut(String, String),
    End,
    ConfirmEnd,
    CancelEnd,
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
}

/// Where a process's program is. The system does not always say, of one
/// that is not the user's own; `ps` is asked then, which may.
fn program_of(sys: &System, pid: Pid) -> Option<String> {
    let known = sys.process(pid).and_then(|p| p.exe()).map(|e| e.display().to_string()).filter(|e| !e.is_empty());
    known.or_else(|| {
        let out = std::process::Command::new("ps").args(["-p", &pid.as_u32().to_string(), "-o", "comm="]).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|p| p.starts_with('/'))
    })
}

fn filled(n: usize) -> VecDeque<f32> {
    std::iter::repeat_n(0.0, n).collect()
}

fn push(q: &mut VecDeque<f32>, v: f32) {
    q.push_back(v);
    while q.len() > HISTORY {
        q.pop_front();
    }
}

impl Monitor {
    fn new() -> Self {
        let mut m = Self {
            desktop: Desktop::load(),
            sys: System::new(),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            users: Users::new_with_refreshed_list(),
            last_sample: Instant::now(),
            page: Page::Processes,
            cpu: filled(HISTORY),
            memory: filled(HISTORY),
            pressure: filled(HISTORY),
            pressure_now: None,
            meter: power::Meter::default(),
            rx: filled(HISTORY),
            tx: filled(HISTORY),
            procs: vec![],
            query: String::new(),
            sort: SortBy::Cpu,
            descending: true,
            selected: None,
            anchor: None,
            confirm_end: false,
            status: None,
            inspect: None,
            heat: None,
            cpu_heat: VecDeque::new(),
            gpu_heat: VecDeque::new(),
            interfaces: vec![],
            net_using: None,
            watching_net: Default::default(),
            watching_heat: Default::default(),
            proxy: None,
        };
        m.sample();
        m
    }

    fn sample(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory().with_user(UpdateKind::OnlyIfNotSet));
        self.networks.refresh(true);
        push(&mut self.cpu, self.sys.global_cpu_usage());
        let total = self.sys.total_memory().max(1) as f32;
        push(&mut self.memory, self.sys.used_memory() as f32 / total * 100.0);
        self.pressure_now = power::pressure();
        if let Some(p) = self.pressure_now {
            push(&mut self.pressure, p.percent);
        }
        let now = Instant::now();
        let secs = now.duration_since(self.last_sample).as_secs_f32().max(0.05);
        self.last_sample = now;
        let (rx, tx) = self.networks.list().values().fold((0u64, 0u64), |(r, t), n| (r + n.received(), t + n.transmitted()));
        push(&mut self.rx, rx as f32 / secs);
        push(&mut self.tx, tx as f32 / secs);
        self.interfaces = net::interfaces(&self.networks, secs);
        let pids: Vec<u32> = self.sys.processes().values().filter(|p| p.thread_kind().is_none()).map(|p| p.pid().as_u32()).collect();
        let quick = inspect::quick(&pids);
        let shares: std::collections::HashMap<u32, f32> = self.sys.processes().values().filter(|p| p.thread_kind().is_none()).map(|p| (p.pid().as_u32(), p.cpu_usage())).collect();
        let energy = self.meter.sample(&shares);
        self.procs = self
            .sys
            .processes()
            .values()
            .filter(|p| p.thread_kind().is_none())
            .map(|p| Proc {
                pid: p.pid(),
                name: p.name().to_string_lossy().into_owned(),
                user: p.user_id().and_then(|u| self.users.get_user_by_id(u)).map(|u| u.name().to_string()).unwrap_or_default(),
                // Of one core, as the system's own monitor and `top` give it: a
                // process busy on two cores reads 200%. Shared out over every
                // core instead, a process flat out on one of ten read 10% and
                // looked idle.
                cpu: p.cpu_usage(),
                // The footprint where the system keeps one, as its own
                // monitor shows; otherwise what is resident.
                memory: quick.get(&p.pid().as_u32()).and_then(|q| q.footprint).unwrap_or(p.memory()),
                virtual_memory: p.virtual_memory(),
                resident: p.memory(),
                threads: quick.get(&p.pid().as_u32()).and_then(|q| q.threads),
                swap: quick.get(&p.pid().as_u32()).and_then(|q| q.swap),
                energy: energy.get(&p.pid().as_u32()).copied(),
            })
            .collect();
        if let Some(i) = &mut self.inspect {
            i.refresh(self.proxy.as_ref());
        }
    }

    fn sorted(&self) -> Vec<&Proc> {
        let q = self.query.to_lowercase();
        let mut v: Vec<&Proc> = self.procs.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.pid.to_string() == q).collect();
        v.sort_by(|a, b| {
            let o = match self.sort {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::User => a.user.cmp(&b.user).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                SortBy::Cpu => a.cpu.total_cmp(&b.cpu),
                SortBy::Memory => a.memory.cmp(&b.memory),
                SortBy::Swap => a.swap.cmp(&b.swap),
                SortBy::Energy => a.energy.unwrap_or(-1.0).total_cmp(&b.energy.unwrap_or(-1.0)),
                SortBy::Threads => a.threads.cmp(&b.threads),
                SortBy::Pid => a.pid.cmp(&b.pid),
            };
            if self.descending { o.reverse() } else { o }
        });
        v
    }

    fn selected_proc(&self) -> Option<&Proc> {
        self.selected.and_then(|pid| self.procs.iter().find(|p| p.pid == pid))
    }

    /// Every process selected: the one, or those from where the selection
    /// began to where it ends, in the order the list shows them.
    fn chosen(&self) -> Vec<&Proc> {
        let v = self.sorted();
        let at = |pid: Option<Pid>| pid.and_then(|pid| v.iter().position(|p| p.pid == pid));
        match (at(self.selected), at(self.anchor)) {
            (Some(a), Some(b)) => v[a.min(b)..=a.max(b)].to_vec(),
            (Some(a), None) => vec![v[a]],
            // Filtered out of the list, it is still the one selected.
            (None, _) => self.selected_proc().into_iter().collect(),
        }
    }

    /// What is known of the selected processes, for asking Apollo about them.
    fn asked_about(&self) -> Vec<ask::About> {
        self.chosen().into_iter().map(|p| ask::About { name: p.name.clone(), pid: p.pid.as_u32(), user: p.user.clone(), program: program_of(&self.sys, p.pid), cpu: p.cpu, memory: p.memory }).collect()
    }
}

impl App for Monitor {
    type Message = Msg;

    fn title(&self) -> String {
        "System Monitor".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1060.0, 700.0), min_size: Some(Size::new(760.0, 480.0)), app_id: Some("org.neo.Monitor".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll), Subscription::every(SAMPLE, Msg::Sample)]
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy.clone());
        // Asking which programs use the network takes the best part of a
        // second, so a thread does it, and only while the Network page is open.
        if let Some(proxy) = self.proxy.clone() {
            let watching = self.watching_net.clone();
            std::thread::spawn(move || {
                let mut watcher = net::Watcher::default();
                let mut last: Option<Instant> = None;
                loop {
                    if !watching.load(std::sync::atomic::Ordering::Relaxed) {
                        // Away from the page, what was counted goes stale: begin afresh on return.
                        if last.take().is_some() {
                            watcher = net::Watcher::default();
                        }
                    } else if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
                        last = Some(Instant::now());
                        match watcher.sample() {
                            Some(using) => {
                                if !proxy.send(Msg::NetUsing(using)) {
                                    return;
                                }
                            }
                            // This system does not say: nothing to keep asking.
                            None => return,
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
        // Reading sensors takes tens of milliseconds, so a thread does it,
        // and only while the Sensors page is open.
        let watching = self.watching_heat.clone();
        let brand = {
            let sys = System::new_with_specifics(sysinfo::RefreshKind::nothing().with_cpu(sysinfo::CpuRefreshKind::nothing()));
            sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default()
        };
        std::thread::spawn(move || {
            let mut reader = sensors::Reader::new(&brand);
            let mut last: Option<Instant> = None;
            loop {
                if !watching.load(std::sync::atomic::Ordering::Relaxed) {
                    last = None;
                } else if last.is_none_or(|t| t.elapsed() >= heat::EVERY) {
                    last = Some(Instant::now());
                    if !proxy.send(Msg::Heat(reader.read())) {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        });
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        if self.inspect.is_some() {
            return matches!(k.key, Key::Escape | Key::Backspace).then_some(Msg::CloseInspect);
        }
        match k.key {
            Key::Enter => self.selected.map(|_| Msg::Inspect),
            Key::Up if k.modifiers.shift => Some(Msg::Extend(-1)),
            Key::Down if k.modifiers.shift => Some(Msg::Extend(1)),
            Key::Up => Some(Msg::Move(-1)),
            Key::Down => Some(Msg::Move(1)),
            Key::Delete => self.selected.map(|_| Msg::End),
            Key::Escape => Some(Msg::CancelEnd),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Page(p) => {
                self.page = p;
                self.watching_heat.store(p == Page::Sensors, std::sync::atomic::Ordering::Relaxed);
                self.watching_net.store(p == Page::Network, std::sync::atomic::Ordering::Relaxed);
            }
            Msg::NetUsing(using) => self.net_using = Some(using),
            Msg::Heat(reading) => {
                // With no core sensor reporting, the chart follows the chips beside them.
                if let Some(t) = reading.hottest(sensors::Kind::Cpu).or_else(|| reading.beside_cpu()) {
                    push(&mut self.cpu_heat, t);
                }
                if let Some(t) = reading.hottest(sensors::Kind::Gpu) {
                    push(&mut self.gpu_heat, t);
                }
                self.heat = Some(reading);
            }
            Msg::Sample => self.sample(),
            Msg::Query(q) => self.query = q,
            Msg::Sort(s) => {
                if self.sort == s {
                    self.descending = !self.descending;
                } else {
                    self.sort = s;
                    self.descending = matches!(s, SortBy::Cpu | SortBy::Memory | SortBy::Swap | SortBy::Energy | SortBy::Threads);
                }
            }
            Msg::Select(pid) => {
                if self.selected != Some(pid) {
                    self.confirm_end = false;
                }
                self.selected = Some(pid);
                self.anchor = None;
            }
            Msg::Extend(d) => {
                // From the one selected, which stays one end of them.
                let began = self.anchor.or(self.selected);
                self.update(Msg::Move(d));
                self.anchor = began.filter(|b| Some(*b) != self.selected);
            }
            Msg::AskApollo => {
                let about = self.asked_about();
                let Some(question) = ask::question(&about) else { return };
                // What the system keeps of each, to hand: what started it, how, and when.
                // Each one's parents, command line, seconds running and threads.
                type Known = (Vec<String>, Vec<String>, u64, Option<u32>);
                let known: Vec<Known> = about
                    .iter()
                    .map(|a| {
                        let pid = Pid::from_u32(a.pid);
                        let mut parents = vec![];
                        let mut at = self.sys.process(pid).and_then(|p| p.parent());
                        while let Some(p) = at.and_then(|p| self.sys.process(p)).filter(|_| parents.len() < 6) {
                            parents.push(p.name().to_string_lossy().into_owned());
                            at = p.parent();
                        }
                        let process = self.sys.process(pid);
                        (parents, process.map(|p| p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect()).unwrap_or_default(), process.map_or(0, |p| p.run_time()), self.procs.iter().find(|p| p.pid == pid).and_then(|p| p.threads))
                    })
                    .collect();
                // One is looked at closely, which takes a second or two; several are only described.
                let closely = about.len() == 1 && !cfg!(test);
                let find = move || {
                    let found: Vec<Vec<String>> = about.iter().zip(known).map(|(a, (parents, command, running, threads))| evidence::gather(&evidence::Subject { about: a, parents, command, running, threads }, closely)).collect();
                    ask::set_out(&found)
                };
                match self.proxy.clone() {
                    Some(proxy) => {
                        self.status = Some((Tone::Good, "Looking at what it is doing, to ask Apollo…".into()));
                        std::thread::spawn(move || {
                            proxy.send(Msg::FoundOut(question, find()));
                        });
                    }
                    None => {
                        let found = find();
                        self.update(Msg::FoundOut(question, found));
                    }
                }
            }
            Msg::FoundOut(question, found) => {
                // Tests must not start Apollo, or put questions to the one that is running.
                let asked = if cfg!(test) { Ok(()) } else { neo_desktop::apollo::ask_about(&question, &found) };
                self.status = asked.err().map(|e| (Tone::Bad, format!("Apollo could not be asked: {e}")));
            }
            Msg::Move(d) => {
                let v = self.sorted();
                let pos = self.selected.and_then(|s| v.iter().position(|p| p.pid == s));
                let next = match pos {
                    None => 0,
                    Some(i) => (i as isize + d).clamp(0, v.len().saturating_sub(1) as isize) as usize,
                };
                if let Some(p) = v.get(next) {
                    self.selected = Some(p.pid);
                    self.anchor = None;
                    self.confirm_end = false;
                }
            }
            Msg::Inspect => {
                if let Some(pid) = self.selected {
                    let mut inspect = detail::Inspect::new(pid);
                    inspect.refresh(self.proxy.as_ref());
                    self.inspect = Some(inspect);
                    self.confirm_end = false;
                }
            }
            Msg::CloseInspect => self.inspect = None,
            Msg::ThreadSort(s) => {
                if let Some(i) = &mut self.inspect {
                    if i.sort == s {
                        i.descending = !i.descending;
                    } else {
                        i.sort = s;
                        i.descending = matches!(s, detail::ThreadSort::Cpu | detail::ThreadSort::Time);
                    }
                }
            }
            Msg::Swap(pid, bytes) => {
                // A result for a process that is no longer open is dropped.
                if let Some(i) = self.inspect.as_mut().filter(|i| i.pid.as_u32() == pid) {
                    i.set_swap(bytes);
                }
            }
            Msg::End => self.confirm_end = self.selected.is_some(),
            Msg::CancelEnd => self.confirm_end = false,
            Msg::ConfirmEnd => {
                self.confirm_end = false;
                let Some(pid) = self.selected else { return };
                let name = self.selected_proc().map(|p| p.name.clone()).unwrap_or_default();
                self.status = Some(match self.sys.process(pid).map(|p| p.kill_with(Signal::Term).unwrap_or_else(|| p.kill())) {
                    Some(true) => (Tone::Good, format!("Asked {name} ({pid}) to quit.")),
                    Some(false) => (Tone::Bad, format!("Could not end {name}. It may belong to another user.")),
                    None => (Tone::Warn, format!("{name} has already exited.")),
                });
            }
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "System Monitor Settings", Msg::Desktop, vec![])
    }
}

impl Monitor {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        let cpu = *self.cpu.back().unwrap_or(&0.0);
        let mem = *self.memory.back().unwrap_or(&0.0);
        let mut side = column().spacing(2.0).width(Length::Fill).push(section("Monitor"));
        for (page, glyph, name) in [(Page::Processes, icons::LIST, "Processes"), (Page::Resources, icons::ACTIVITY, "Resources"), (Page::Sensors, icons::THERMOMETER, "Sensors"), (Page::Network, icons::NETWORK, "Network"), (Page::Storage, icons::HARD_DRIVE, "Storage")] {
            side = side.push(nav_item(glyph, name, self.page == page, Msg::Page(page)));
        }
        let mini = |label: &str, v: f32, tone: Tone| -> Element<Msg> { column().spacing(6.0).width(Length::Fill).push(row().width(Length::Fill).push(text(label).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x()).push(text(format!("{v:.0}%")).mono().role(TextRole::Caption))).push(progress_bar(v / 100.0).height(8.0).tone(tone)).into() };
        let mut meters = column().spacing(12.0).width(Length::Fill).push(mini("CPU", cpu, Tone::Accent)).push(mini("Memory", mem, Tone::Good));
        // Swap across the whole system, on computers that have any.
        let swap_total = self.sys.total_swap();
        if swap_total > 0 {
            meters = meters.push(mini("Swap", self.sys.used_swap() as f32 / swap_total as f32 * 100.0, Tone::Warn));
        }
        // Memory pressure, coloured as the system grades it.
        if let Some(p) = self.pressure_now {
            meters = meters.push(mini("Pressure", p.percent, pressure_tone(p.level)));
        }
        side = side.push(Space::fill_y()).push(container(meters).padding([10.0, 12.0]));
        let body = match self.page {
            Page::Processes => match &self.inspect {
                Some(inspect) => self.detail(inspect),
                None => self.processes(),
            },
            Page::Resources => self.resources(),
            Page::Sensors => self.sensors(),
            Page::Network => self.network(),
            Page::Storage => self.storage(),
        };
        split(side, body)
    }
}

/// The colour for a memory pressure level: green, yellow and red, as
/// Activity Monitor has them.
fn pressure_tone(level: power::Level) -> Tone {
    match level {
        power::Level::Normal => Tone::Good,
        power::Level::Warning => Tone::Warn,
        power::Level::Critical => Tone::Bad,
    }
}

impl Monitor {
    /// Whether any process has an energy figure, so the table shows the column.
    fn has_energy(&self) -> bool {
        self.procs.iter().any(|p| p.energy.is_some())
    }
}

pub(crate) fn panel<M: 'static>(title: &str, detail: String, body: impl Into<Element<M>>) -> Element<M> {
    container(column().spacing(12.0).width(Length::Fill).push(row().width(Length::Fill).align(Align::Center).push(text(title).role(TextRole::Title)).push(Space::fill_x()).push(text(detail).mono().role(TextRole::Caption).tone(Tone::Muted))).push(body)).surface(Surface::Well).padding(18.0).width(Length::Fill).into()
}

pub(crate) fn rate(bytes_per_sec: f32) -> String {
    format!("{}/s", human_size(bytes_per_sec as u64))
}

impl Monitor {
    fn processes(&self) -> Element<Msg> {
        let v = self.sorted();
        let header_cell = |label: &str, by: SortBy, width: Length, align: Align| -> Element<Msg> {
            let active = self.sort == by;
            let mut r = row().spacing(4.0).align(Align::Center).push(text(label).role(TextRole::Label).tone(if active { Tone::Accent } else { Tone::Muted }));
            if active {
                r = r.push(icon(if self.descending { icons::CHEVRON_DOWN } else { icons::CHEVRON_UP }).size(12.0).tone(Tone::Accent));
            }
            Button::new(r).kind(ButtonKind::Ghost).padding([6.0, 6.0]).width(width).align_x(align).on_press(Msg::Sort(by)).into()
        };
        let mut header = row().spacing(12.0).width(Length::Fill).padding([4.0, 22.0, 4.0, 14.0]).push(header_cell("Name", SortBy::Name, Length::Fill, Align::Start)).push(header_cell("User", SortBy::User, Length::Fixed(110.0), Align::Start)).push(header_cell("CPU", SortBy::Cpu, Length::Fixed(120.0), Align::End)).push(header_cell("Memory", SortBy::Memory, Length::Fixed(90.0), Align::End));
        if inspect::QUICK_SWAP {
            header = header.push(header_cell("Swap", SortBy::Swap, Length::Fixed(80.0), Align::End));
        }
        let energy = self.has_energy();
        if energy {
            header = header.push(header_cell("Energy", SortBy::Energy, Length::Fixed(80.0), Align::End));
        }
        let header = header.push(header_cell("Threads", SortBy::Threads, Length::Fixed(76.0), Align::End)).push(header_cell("PID", SortBy::Pid, Length::Fixed(70.0), Align::End));
        let chosen: Vec<Pid> = self.chosen().iter().map(|p| p.pid).collect();
        let mut rows = column().spacing(1.0).width(Length::Fill).padding([10.0, 4.0, 10.0, 10.0]);
        for p in v.iter().take(MAX_PROCESSES) {
            let cpu = row().spacing(8.0).align(Align::Center).width(120.0).push(Space::fill_x()).push(progress_bar((p.cpu / 100.0).min(1.0)).width(44.0).height(6.0)).push(text(format!("{:.1}%", p.cpu)).mono().role(TextRole::Caption).align(Align::End).width(50.0));
            let mut content = row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text(p.name.clone()).no_wrap().width(Length::Fill)).push(text(p.user.clone()).role(TextRole::Caption).tone(Tone::Muted).no_wrap().width(110.0)).push(cpu).push(text(human_bytes_binary(p.memory)).mono().role(TextRole::Caption).align(Align::End).width(90.0));
            if inspect::QUICK_SWAP {
                content = content.push(text(p.swap.map_or("—".into(), human_bytes_binary)).mono().role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(80.0));
            }
            if energy {
                // A process drawing a watt or more stands out.
                let tone = match p.energy {
                    Some(w) if w >= 1.0 => Tone::Warn,
                    Some(_) => Tone::Inherit,
                    None => Tone::Faint,
                };
                content = content.push(text(p.energy.map_or("—".into(), power::watts)).mono().role(TextRole::Caption).tone(tone).align(Align::End).width(80.0));
            }
            let content = content.push(text(p.threads.map_or("—".into(), |n| n.to_string())).mono().role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(76.0)).push(text(p.pid.to_string()).mono().role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(70.0));
            rows = rows.push(Button::new(content).kind(ButtonKind::Ghost).selected(chosen.contains(&p.pid)).padding([10.0, 6.0]).width(Length::Fill).align_x(Align::Start).on_press(Msg::Select(p.pid)));
        }
        let toolbar = row()
            .spacing(10.0)
            .align(Align::Center)
            .width(Length::Fill)
            .padding([14.0, 12.0])
            .push(text("Processes").role(TextRole::Title))
            .push(text(format!("{} running", self.procs.len())).role(TextRole::Caption).tone(Tone::Muted))
            .push(Space::fill_x())
            .push(container(text_input("Filter by name or PID", self.query.clone()).on_input(Msg::Query).on_cancel(Msg::Query(String::new()))).width(240.0));
        let footer: Element<Msg> = match self.selected_proc() {
            Some(p) if self.confirm_end => row()
                .spacing(10.0)
                .align(Align::Center)
                .width(Length::Fill)
                .padding([16.0, 10.0])
                .push(icon(icons::TRIANGLE_ALERT).size(16.0).tone(Tone::Warn))
                .push(text(format!("End “{}”? Unsaved work in it may be lost.", p.name)).width(Length::Fill))
                .push(button("Cancel").on_press(Msg::CancelEnd))
                .push(Button::new(text("End process").role(TextRole::Strong)).kind(ButtonKind::Accent).on_press(Msg::ConfirmEnd))
                .into(),
            selected => {
                let mut r = row().spacing(10.0).align(Align::Center).width(Length::Fill).padding([16.0, 10.0]);
                r = match &self.status {
                    Some((tone, msg)) => r.push(notice(*tone, msg.clone())),
                    None if chosen.len() > 1 => r.push(text(format!("{} processes selected", chosen.len())).role(TextRole::Caption).tone(Tone::Muted)),
                    None => r.push(text(selected.map(|p| format!("{} · PID {}", p.name, p.pid)).unwrap_or_else(|| "Select a process to see its threads, end it, or ask Apollo what it is. Shift with the arrows selects several.".into())).role(TextRole::Caption).tone(Tone::Muted)),
                };
                r.push(Space::fill_x()).push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::SPARKLES).size(15.0)).push(text("Ask Apollo"))).on_press_maybe(selected.map(|_| Msg::AskApollo))).push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::CPU).size(15.0)).push(text("Threads and swap"))).on_press_maybe(selected.map(|_| Msg::Inspect))).push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::CIRCLE_STOP).size(15.0)).push(text("End process"))).on_press_maybe(selected.map(|_| Msg::End))).into()
            }
        };
        column().width(Length::Fill).height(Length::Fill).push(toolbar).push(Divider::horizontal()).push(header).push(scrollable(rows).height(Length::Fill)).push(Divider::horizontal()).push(footer).into()
    }

    fn resources(&self) -> Element<Msg> {
        let cpu = *self.cpu.back().unwrap_or(&0.0);
        let load = System::load_average();
        let cores = self.sys.cpus();
        let mut grid = column().spacing(10.0).width(Length::Fill);
        for chunk in cores.chunks(4).enumerate() {
            let (ci, chunk) = chunk;
            let mut line = row().spacing(16.0).width(Length::Fill);
            for (i, c) in chunk.iter().enumerate() {
                let n = ci * 4 + i + 1;
                line = line.push(column().spacing(4.0).width(Length::Fill).push(row().width(Length::Fill).push(text(format!("Core {n}")).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x()).push(text(format!("{:.0}%", c.cpu_usage())).mono().role(TextRole::Caption))).push(progress_bar(c.cpu_usage() / 100.0).height(6.0)));
            }
            for _ in chunk.len()..4 {
                line = line.push(Space::fill_x());
            }
            grid = grid.push(line);
        }
        let cpu_panel = panel("Processor", format!("{cpu:.0}% · load {:.2} {:.2} {:.2}", load.one, load.five, load.fifteen), column().spacing(16.0).width(Length::Fill).push(sparkline(Vec::from(self.cpu.clone()), 0.0, 100.0).height(90.0)).push(grid));
        let (used, total) = (self.sys.used_memory(), self.sys.total_memory());
        let (swap_used, swap_total) = (self.sys.used_swap(), self.sys.total_swap());
        let mut mem_body = column().spacing(12.0).width(Length::Fill).push(sparkline(Vec::from(self.memory.clone()), 0.0, 100.0).height(70.0).tone(Tone::Good));
        // Pressure, which says more than how full memory is: a healthy system
        // keeps most of it in use as cache.
        if let Some(p) = self.pressure_now {
            let tone = pressure_tone(p.level);
            mem_body = mem_body
                .push(row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text("Memory pressure").role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x()).push(text(p.level.label()).role(TextRole::Caption).tone(tone)).push(text(format!("{:.0}%", p.percent)).mono().role(TextRole::Caption)))
                .push(sparkline(Vec::from(self.pressure.clone()), 0.0, 100.0).height(44.0).tone(tone));
        }
        if swap_total > 0 {
            mem_body =
                mem_body.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text("Swap").role(TextRole::Caption).tone(Tone::Muted).width(60.0)).push(progress_bar(swap_used as f32 / swap_total as f32).height(6.0).tone(Tone::Warn)).push(text(format!("{} of {}", human_bytes_binary(swap_used), human_bytes_binary(swap_total))).mono().role(TextRole::Caption).tone(Tone::Muted)));
        }
        let mem_panel = panel("Memory", format!("{} of {}", human_bytes_binary(used), human_bytes_binary(total)), mem_body);
        let peak = self.rx.iter().chain(self.tx.iter()).copied().fold(1024.0f32, f32::max) * 1.15;
        let (rx, tx) = (*self.rx.back().unwrap_or(&0.0), *self.tx.back().unwrap_or(&0.0));
        let net_panel = panel(
            "Network",
            format!("↓ {}  ↑ {}", rate(rx), rate(tx)),
            row()
                .spacing(16.0)
                .width(Length::Fill)
                .push(column().spacing(6.0).width(Length::Fill).push(text("Download").role(TextRole::Caption).tone(Tone::Muted)).push(sparkline(Vec::from(self.rx.clone()), 0.0, peak).height(60.0)))
                .push(column().spacing(6.0).width(Length::Fill).push(text("Upload").role(TextRole::Caption).tone(Tone::Muted)).push(sparkline(Vec::from(self.tx.clone()), 0.0, peak).height(60.0).tone(Tone::Warn))),
        );
        scrollable(column().spacing(16.0).width(Length::Fill).padding(18.0).push(cpu_panel).push(row().spacing(16.0).width(Length::Fill).push(mem_panel).push(net_panel))).into()
    }

    fn storage(&self) -> Element<Msg> {
        let mut col = column().spacing(12.0).width(Length::Fill).padding(18.0).push(text("Storage").role(TextRole::Title));
        let mut seen = std::collections::HashSet::new();
        for d in self.disks.list() {
            let total = d.total_space();
            // Skip pseudo file systems and duplicate views of the same volume.
            if total == 0 || !seen.insert((d.name().to_os_string(), total)) {
                continue;
            }
            let used = total.saturating_sub(d.available_space());
            let frac = used as f32 / total as f32;
            let tone = if frac > 0.9 {
                Tone::Bad
            } else if frac > 0.75 {
                Tone::Warn
            } else {
                Tone::Accent
            };
            let name = d.mount_point().display().to_string();
            col = col.push(
                container(
                    row().spacing(16.0).align(Align::Center).width(Length::Fill).push(icon(if d.is_removable() { icons::HARD_DRIVE } else { icons::DATABASE }).size(22.0).tone(Tone::Accent)).push(
                        column()
                            .spacing(8.0)
                            .width(Length::Fill)
                            .push(row().width(Length::Fill).push(text(name).role(TextRole::Strong)).push(Space::fill_x()).push(text(format!("{} free of {}", human_size(d.available_space()), human_size(total))).role(TextRole::Caption).tone(Tone::Muted)))
                            .push(progress_bar(frac).height(8.0).tone(tone))
                            .push(text(format!("{} · {}", d.name().to_string_lossy(), d.file_system().to_string_lossy())).mono().role(TextRole::Caption).tone(Tone::Faint)),
                    ),
                )
                .surface(Surface::Well)
                .padding(16.0)
                .width(Length::Fill),
            );
        }
        scrollable(col).into()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    if let Err(e) = neo::run(Monitor::new()) {
        eprintln!("neo-monitor: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: std::path::PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, page, scheme) in [("monitor-processes", Page::Processes, neo_desktop::SchemePref::Light), ("monitor-resources", Page::Resources, neo_desktop::SchemePref::Dark), ("monitor-network", Page::Network, neo_desktop::SchemePref::Dark)] {
        let mut app = Monitor::new();
        app.desktop.appearance.scheme = scheme;
        app.page = page;
        for i in 0..HISTORY {
            // The last gap is long enough to measure energy over.
            std::thread::sleep(Duration::from_millis(if i + 1 == HISTORY { 1200 } else { 20 }));
            app.sample();
        }
        if let Some(first) = app.sorted().first().map(|p| p.pid) {
            app.selected = Some(first);
        }
        // The thread that asks which programs use the network is not running here: ask twice, for rates.
        if page == Page::Network {
            let mut watcher = net::Watcher::default();
            watcher.sample();
            std::thread::sleep(Duration::from_millis(1500));
            app.net_using = watcher.sample();
        }
        let mut h = Harness::new(app, Size::new(1060.0, 700.0)).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }

    // The detail view, for the readable process with the most threads.
    let mut app = Monitor::new();
    app.desktop.appearance.scheme = neo_desktop::SchemePref::Dark;
    let busiest = app.procs.iter().filter(|p| inspect::threads(p.pid.as_u32()).is_ok()).max_by_key(|p| p.threads).map(|p| p.pid);
    app.selected = busiest;
    let mut h = Harness::new(app, Size::new(1060.0, 700.0)).expect("GPU");
    h.app_mut().update(Msg::Inspect);
    // Two samples give each thread a processor share; the swap figure arrives from a worker.
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(250));
        h.app_mut().update(Msg::Sample);
        h.advance(Duration::from_millis(250));
    }
    let path = dir.join("monitor-threads.png");
    h.save_png(&path, 1.0).expect("write png");
    println!("wrote {}", path.display());

    // The Sensors page, once the sensor thread has answered a few times.
    let mut app = Monitor::new();
    app.desktop.appearance.scheme = neo_desktop::SchemePref::Light;
    let mut h = Harness::new(app, Size::new(1060.0, 700.0)).expect("GPU");
    h.app_mut().update(Msg::Page(Page::Sensors));
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(250));
        h.advance(Duration::from_millis(250));
    }
    let path = dir.join("monitor-sensors.png");
    h.save_png(&path, 1.0).expect("write png");
    println!("wrote {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is found out about a real process, to look at by hand:
    /// `NEO_ASK_PID=123 cargo test -p neo-monitor found_out -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn found_out() {
        let pid: u32 = std::env::var("NEO_ASK_PID").expect("NEO_ASK_PID").parse().expect("a number");
        let mut m = Monitor::new();
        m.update(Msg::Select(Pid::from_u32(pid)));
        let about = m.asked_about();
        let process = m.sys.process(Pid::from_u32(pid)).expect("a process");
        let parents = process.parent().and_then(|p| m.sys.process(p)).map(|p| vec![p.name().to_string_lossy().into_owned()]).unwrap_or_default();
        let facts = evidence::gather(&evidence::Subject { about: &about[0], parents, command: process.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect(), running: process.run_time(), threads: None }, true);
        println!("{}\n\n{}", ask::question(&about).unwrap(), ask::set_out(&[facts]));
    }

    #[test]
    fn several_processes_are_selected_with_shift_and_asked_about_together() {
        let mut m = Monitor::new();
        m.update(Msg::Sort(SortBy::Pid));
        let pids: Vec<Pid> = m.sorted().iter().map(|p| p.pid).collect();
        assert!(pids.len() >= 4, "this computer is running something");
        assert!(m.chosen().is_empty() && ask::question(&m.asked_about()).is_none(), "nothing is selected to begin with");
        m.update(Msg::AskApollo);
        m.update(Msg::Select(pids[1]));
        assert_eq!(m.chosen().iter().map(|p| p.pid).collect::<Vec<_>>(), [pids[1]]);
        // Shift with an arrow takes in the next, and the next; back again lets one go.
        let shift = |key| KeyEvent { key, pressed: true, repeat: false, modifiers: neo::Modifiers { shift: true, ..Default::default() }, text: None };
        assert!(matches!(m.on_key(&shift(Key::Down)), Some(Msg::Extend(1))) && matches!(m.on_key(&shift(Key::Up)), Some(Msg::Extend(-1))));
        m.update(Msg::Extend(1));
        m.update(Msg::Extend(1));
        assert_eq!(m.chosen().iter().map(|p| p.pid).collect::<Vec<_>>(), pids[1..=3]);
        let about = m.asked_about();
        assert_eq!((about.len(), about[0].pid, ask::question(&about).is_some()), (3, pids[1].as_u32(), true));
        m.update(Msg::Extend(-1));
        assert_eq!(m.chosen().len(), 2);
        // Above where it began, the same; back to where it began, the one.
        m.update(Msg::Extend(-1));
        m.update(Msg::Extend(-1));
        assert_eq!(m.chosen().iter().map(|p| p.pid).collect::<Vec<_>>(), pids[0..=1]);
        m.update(Msg::Extend(1));
        assert_eq!((m.chosen().len(), m.anchor), (1, None));
        // An arrow alone, or a click, selects one again; asking leaves no complaint.
        m.update(Msg::Extend(1));
        m.update(Msg::Move(1));
        assert_eq!(m.chosen().len(), 1);
        m.update(Msg::Extend(1));
        m.update(Msg::Select(pids[0]));
        assert_eq!((m.chosen().len(), m.anchor), (1, None));
        m.update(Msg::AskApollo);
        assert!(m.status.is_none());
    }
}
