//! Neo System Monitor: processes, CPU, memory, disks and network.
//!
//!     cargo run -p neo-monitor
//!     cargo run -p neo-monitor -- --snapshot target/snapshots

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use neo::prelude::*;
use neo::{Key, KeyEvent, Size};
use neo_desktop::fs::{human_bytes_binary, human_size};
use neo_desktop::ui::{nav_item, notice, section, split};
use neo_desktop::Desktop;
use sysinfo::{Disks, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind, Users};

const HISTORY: usize = 60;
const SAMPLE: Duration = Duration::from_millis(1500);
const MAX_PROCESSES: usize = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Processes,
    Resources,
    Storage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortBy {
    Name,
    User,
    Cpu,
    Memory,
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
    rx: VecDeque<f32>,
    tx: VecDeque<f32>,
    procs: Vec<Proc>,
    query: String,
    sort: SortBy,
    descending: bool,
    selected: Option<Pid>,
    confirm_end: bool,
    status: Option<(Tone, String)>,
}

#[derive(Clone, Debug)]
enum Msg {
    Page(Page),
    Sample,
    Query(String),
    Sort(SortBy),
    Select(Pid),
    Move(isize),
    End,
    ConfirmEnd,
    CancelEnd,
    Poll,
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
            rx: filled(HISTORY),
            tx: filled(HISTORY),
            procs: vec![],
            query: String::new(),
            sort: SortBy::Cpu,
            descending: true,
            selected: None,
            confirm_end: false,
            status: None,
        };
        m.sample();
        m
    }

    fn sample(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory().with_user(UpdateKind::OnlyIfNotSet));
        self.networks.refresh(true);
        let cores = self.sys.cpus().len().max(1) as f32;
        push(&mut self.cpu, self.sys.global_cpu_usage());
        let total = self.sys.total_memory().max(1) as f32;
        push(&mut self.memory, self.sys.used_memory() as f32 / total * 100.0);
        let now = Instant::now();
        let secs = now.duration_since(self.last_sample).as_secs_f32().max(0.05);
        self.last_sample = now;
        let (rx, tx) = self.networks.list().values().fold((0u64, 0u64), |(r, t), n| (r + n.received(), t + n.transmitted()));
        push(&mut self.rx, rx as f32 / secs);
        push(&mut self.tx, tx as f32 / secs);
        self.procs = self
            .sys
            .processes()
            .values()
            .filter(|p| p.thread_kind().is_none())
            .map(|p| Proc {
                pid: p.pid(),
                name: p.name().to_string_lossy().into_owned(),
                user: p.user_id().and_then(|u| self.users.get_user_by_id(u)).map(|u| u.name().to_string()).unwrap_or_default(),
                cpu: p.cpu_usage() / cores,
                memory: p.memory(),
            })
            .collect();
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
                SortBy::Pid => a.pid.cmp(&b.pid),
            };
            if self.descending { o.reverse() } else { o }
        });
        v
    }

    fn selected_proc(&self) -> Option<&Proc> {
        self.selected.and_then(|pid| self.procs.iter().find(|p| p.pid == pid))
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

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll), Subscription::every(SAMPLE, Msg::Sample)]
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        match k.key {
            Key::Up => Some(Msg::Move(-1)),
            Key::Down => Some(Msg::Move(1)),
            Key::Delete => self.selected.map(|_| Msg::End),
            Key::Escape => Some(Msg::CancelEnd),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Page(p) => self.page = p,
            Msg::Sample => self.sample(),
            Msg::Query(q) => self.query = q,
            Msg::Sort(s) => {
                if self.sort == s {
                    self.descending = !self.descending;
                } else {
                    self.sort = s;
                    self.descending = matches!(s, SortBy::Cpu | SortBy::Memory);
                }
            }
            Msg::Select(pid) => {
                if self.selected != Some(pid) {
                    self.confirm_end = false;
                }
                self.selected = Some(pid);
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
                    self.confirm_end = false;
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
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        let cpu = *self.cpu.back().unwrap_or(&0.0);
        let mem = *self.memory.back().unwrap_or(&0.0);
        let mut side = column().spacing(2.0).width(Length::Fill).push(section("Monitor"));
        for (page, glyph, name) in [(Page::Processes, icons::LIST, "Processes"), (Page::Resources, icons::ACTIVITY, "Resources"), (Page::Storage, icons::HARD_DRIVE, "Storage")] {
            side = side.push(nav_item(glyph, name, self.page == page, Msg::Page(page)));
        }
        let mini = |label: &str, v: f32, tone: Tone| -> Element<Msg> {
            column()
                .spacing(6.0)
                .width(Length::Fill)
                .push(row().width(Length::Fill).push(text(label).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x()).push(text(format!("{v:.0}%")).mono().role(TextRole::Caption)))
                .push(progress_bar(v / 100.0).height(8.0).tone(tone))
                .into()
        };
        side = side.push(Space::fill_y()).push(container(column().spacing(12.0).width(Length::Fill).push(mini("CPU", cpu, Tone::Accent)).push(mini("Memory", mem, Tone::Good))).padding([10.0, 12.0]));
        let body = match self.page {
            Page::Processes => self.processes(),
            Page::Resources => self.resources(),
            Page::Storage => self.storage(),
        };
        split(side, body)
    }
}

fn panel<M: 'static>(title: &str, detail: String, body: impl Into<Element<M>>) -> Element<M> {
    container(
        column()
            .spacing(12.0)
            .width(Length::Fill)
            .push(row().width(Length::Fill).align(Align::Center).push(text(title).role(TextRole::Title)).push(Space::fill_x()).push(text(detail).mono().role(TextRole::Caption).tone(Tone::Muted)))
            .push(body),
    )
    .surface(Surface::Well)
    .padding(18.0)
    .width(Length::Fill)
    .into()
}

fn rate(bytes_per_sec: f32) -> String {
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
        let header = row()
            .spacing(12.0)
            .width(Length::Fill)
            .padding([4.0, 22.0, 4.0, 14.0])
            .push(header_cell("Name", SortBy::Name, Length::Fill, Align::Start))
            .push(header_cell("User", SortBy::User, Length::Fixed(110.0), Align::Start))
            .push(header_cell("CPU", SortBy::Cpu, Length::Fixed(120.0), Align::End))
            .push(header_cell("Memory", SortBy::Memory, Length::Fixed(90.0), Align::End))
            .push(header_cell("PID", SortBy::Pid, Length::Fixed(70.0), Align::End));
        let mut rows = column().spacing(1.0).width(Length::Fill).padding([10.0, 4.0, 10.0, 10.0]);
        for p in v.iter().take(MAX_PROCESSES) {
            let cpu = row().spacing(8.0).align(Align::Center).width(120.0).push(Space::fill_x()).push(progress_bar((p.cpu / 50.0).min(1.0)).width(44.0).height(6.0)).push(text(format!("{:.1}%", p.cpu)).mono().role(TextRole::Caption).align(Align::End).width(50.0));
            let content = row()
                .spacing(12.0)
                .align(Align::Center)
                .width(Length::Fill)
                .push(text(p.name.clone()).no_wrap().width(Length::Fill))
                .push(text(p.user.clone()).role(TextRole::Caption).tone(Tone::Muted).no_wrap().width(110.0))
                .push(cpu)
                .push(text(human_bytes_binary(p.memory)).mono().role(TextRole::Caption).align(Align::End).width(90.0))
                .push(text(p.pid.to_string()).mono().role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(70.0));
            rows = rows.push(Button::new(content).kind(ButtonKind::Ghost).selected(self.selected == Some(p.pid)).padding([10.0, 6.0]).width(Length::Fill).align_x(Align::Start).on_press(Msg::Select(p.pid)));
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
                    None => r.push(text(selected.map(|p| format!("{} · PID {}", p.name, p.pid)).unwrap_or_else(|| "Select a process to end it.".into())).role(TextRole::Caption).tone(Tone::Muted)),
                };
                r.push(Space::fill_x()).push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::CIRCLE_STOP).size(15.0)).push(text("End process"))).on_press_maybe(selected.map(|_| Msg::End))).into()
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
                line = line.push(
                    column()
                        .spacing(4.0)
                        .width(Length::Fill)
                        .push(row().width(Length::Fill).push(text(format!("Core {n}")).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x()).push(text(format!("{:.0}%", c.cpu_usage())).mono().role(TextRole::Caption)))
                        .push(progress_bar(c.cpu_usage() / 100.0).height(6.0)),
                );
            }
            for _ in chunk.len()..4 {
                line = line.push(Space::fill_x());
            }
            grid = grid.push(line);
        }
        let cpu_panel = panel(
            "Processor",
            format!("{cpu:.0}% · load {:.2} {:.2} {:.2}", load.one, load.five, load.fifteen),
            column().spacing(16.0).width(Length::Fill).push(sparkline(Vec::from(self.cpu.clone()), 0.0, 100.0).height(90.0)).push(grid),
        );
        let (used, total) = (self.sys.used_memory(), self.sys.total_memory());
        let (swap_used, swap_total) = (self.sys.used_swap(), self.sys.total_swap());
        let mut mem_body = column().spacing(12.0).width(Length::Fill).push(sparkline(Vec::from(self.memory.clone()), 0.0, 100.0).height(70.0).tone(Tone::Good));
        if swap_total > 0 {
            mem_body = mem_body.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text("Swap").role(TextRole::Caption).tone(Tone::Muted).width(60.0)).push(progress_bar(swap_used as f32 / swap_total as f32).height(6.0).tone(Tone::Warn)).push(text(format!("{} of {}", human_bytes_binary(swap_used), human_bytes_binary(swap_total))).mono().role(TextRole::Caption).tone(Tone::Muted)));
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
            let tone = if frac > 0.9 { Tone::Bad } else if frac > 0.75 { Tone::Warn } else { Tone::Accent };
            let name = d.mount_point().display().to_string();
            col = col.push(
                container(
                    row()
                        .spacing(16.0)
                        .align(Align::Center)
                        .width(Length::Fill)
                        .push(icon(if d.is_removable() { icons::HARD_DRIVE } else { icons::DATABASE }).size(22.0).tone(Tone::Accent))
                        .push(
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
    for (name, page, scheme) in [("monitor-processes", Page::Processes, neo_desktop::SchemePref::Light), ("monitor-resources", Page::Resources, neo_desktop::SchemePref::Dark)] {
        let mut app = Monitor::new();
        app.desktop.appearance.scheme = scheme;
        app.page = page;
        for _ in 0..HISTORY {
            std::thread::sleep(Duration::from_millis(20));
            app.sample();
        }
        if let Some(first) = app.sorted().first().map(|p| p.pid) {
            app.selected = Some(first);
        }
        let mut h = Harness::new(app, Size::new(1060.0, 700.0)).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}
