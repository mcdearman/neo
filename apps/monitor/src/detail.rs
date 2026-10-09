//! The process detail view: memory, swap and a live table of threads.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use neo::Proxy;
use neo::prelude::*;
use neo_desktop::fs::human_bytes_binary;
use sysinfo::Pid;

use crate::inspect::{self, Denied, ThreadInfo, ThreadState};
use crate::{Monitor, Msg};

/// Thread rows beyond this are not built.
const MAX_THREADS: usize = 400;
/// How often to measure swap again. On macOS each measurement walks the
/// process's whole address space.
const SWAP_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadSort {
    Name,
    Id,
    State,
    Cpu,
    Time,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Swap {
    Measuring,
    Known(u64),
    /// The system does not report it, or the process belongs to someone else.
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct ThreadRow {
    pub info: ThreadInfo,
    /// Percent of one processor core since the previous sample. `None`
    /// until the thread has been seen twice.
    pub cpu: Option<f32>,
}

/// The process being inspected and what has been measured about it.
pub struct Inspect {
    pub pid: Pid,
    pub threads: Result<Vec<ThreadRow>, Denied>,
    /// Each thread's processor time at the previous sample.
    prev: HashMap<u64, Duration>,
    prev_at: Instant,
    pub swap: Swap,
    swap_at: Option<Instant>,
    swap_pending: bool,
    pub sort: ThreadSort,
    pub descending: bool,
}

impl Inspect {
    pub fn new(pid: Pid) -> Self {
        Self { pid, threads: Ok(vec![]), prev: HashMap::new(), prev_at: Instant::now(), swap: Swap::Measuring, swap_at: None, swap_pending: false, sort: ThreadSort::Cpu, descending: true }
    }

    /// Reads the threads again and works out each one's processor use since
    /// the last call. Starts a swap measurement when one is due.
    pub fn refresh(&mut self, proxy: Option<&Proxy<Msg>>) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.prev_at).as_secs_f32();
        let pid = self.pid.as_u32();
        self.threads = inspect::threads(pid).map(|list| {
            let rows: Vec<ThreadRow> = list
                .into_iter()
                .map(|info| {
                    let cpu = self.prev.get(&info.id).filter(|_| elapsed > 0.05).map(|before| info.cpu_time.saturating_sub(*before).as_secs_f32() / elapsed * 100.0);
                    ThreadRow { info, cpu }
                })
                .collect();
            self.prev = rows.iter().map(|r| (r.info.id, r.info.cpu_time)).collect();
            self.prev_at = now;
            rows
        });

        let due = self.swap_at.is_none_or(|t| now.duration_since(t) >= SWAP_EVERY);
        if inspect::QUICK_SWAP {
            self.set_swap(inspect::swap(pid));
        } else if due && !self.swap_pending {
            match proxy {
                Some(proxy) => {
                    self.swap_pending = true;
                    let proxy = proxy.clone();
                    std::thread::spawn(move || proxy.send(Msg::Swap(pid, inspect::swap(pid))));
                }
                None => self.set_swap(inspect::swap(pid)),
            }
        }
    }

    pub fn set_swap(&mut self, bytes: Option<u64>) {
        self.swap_pending = false;
        self.swap_at = Some(Instant::now());
        self.swap = bytes.map_or(Swap::Unavailable, Swap::Known);
    }

    fn sorted(&self) -> Vec<&ThreadRow> {
        let Ok(rows) = &self.threads else { return vec![] };
        let mut v: Vec<&ThreadRow> = rows.iter().collect();
        v.sort_by(|a, b| {
            let o = match self.sort {
                // Unnamed threads go after named ones.
                ThreadSort::Name => a.info.name.is_empty().cmp(&b.info.name.is_empty()).then_with(|| a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase())),
                ThreadSort::Id => a.info.id.cmp(&b.info.id),
                ThreadSort::State => a.info.state.label().cmp(b.info.state.label()),
                ThreadSort::Cpu => a.cpu.unwrap_or(0.0).total_cmp(&b.cpu.unwrap_or(0.0)).then_with(|| a.info.cpu_time.cmp(&b.info.cpu_time)),
                ThreadSort::Time => a.info.cpu_time.cmp(&b.info.cpu_time),
            };
            (if self.descending { o.reverse() } else { o }).then_with(|| a.info.id.cmp(&b.info.id))
        });
        v
    }
}

/// `2 h 05 min`, `3 min 20 s`, `4.2 s` or `15 ms`.
pub fn cpu_time(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        3600.. => format!("{} h {:02} min", s / 3600, s / 60 % 60),
        60.. => format!("{} min {:02} s", s / 60, s % 60),
        1.. => format!("{:.1} s", d.as_secs_f32()),
        _ => format!("{} ms", d.as_millis()),
    }
}

fn tile(label: &str, value: String, caption: String) -> Element<Msg> {
    container(column().spacing(4.0).width(Length::Fill).push(text(label).role(TextRole::Label).tone(Tone::Muted)).push(text(value).mono().role(TextRole::Heading).no_wrap()).push(text(caption).role(TextRole::Caption).tone(Tone::Muted))).surface(Surface::Well).padding([16.0, 14.0]).width(Length::Fill).into()
}

fn message(glyph: neo::theme::Icon, title: &str, body: String) -> Element<Msg> {
    let col = column().spacing(8.0).align(Align::Center).push(icon(glyph).size(34.0).tone(Tone::Faint)).push(text(title).role(TextRole::Strong)).push(container(text(body).tone(Tone::Muted).align(Align::Center)).max_width(440.0));
    container(col).width(Length::Fill).height(Length::Fill).center().into()
}

impl Monitor {
    pub(crate) fn detail(&self, inspect: &Inspect) -> Element<Msg> {
        let proc = self.procs.iter().find(|p| p.pid == inspect.pid);
        let name = proc.map(|p| p.name.clone()).unwrap_or_else(|| "Process".into());
        let who = proc.map(|p| if p.user.is_empty() { format!("PID {}", p.pid) } else { format!("PID {} · {}", p.pid, p.user) }).unwrap_or_else(|| format!("PID {}", inspect.pid));
        let toolbar = row().spacing(10.0).align(Align::Center).width(Length::Fill).padding([12.0, 10.0]).push(icon_button(icons::ARROW_LEFT, 34.0).kind(ButtonKind::Ghost).on_press(Msg::CloseInspect)).push(text(name.clone()).role(TextRole::Title).no_wrap()).push(text(who).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x());

        let rows = inspect.sorted();
        let running = rows.iter().filter(|r| r.info.state == ThreadState::Running).count();
        let (swap_value, swap_note) = match &inspect.swap {
            Swap::Known(b) => (human_bytes_binary(*b), if cfg!(target_os = "macos") { "Compressed or swapped".to_string() } else { "Swapped out".to_string() }),
            Swap::Measuring => ("…".into(), "Measuring".into()),
            Swap::Unavailable => ("—".into(), if cfg!(windows) { "Windows does not report it".to_string() } else { "Not available for this process".to_string() }),
        };
        let system_swap = if self.sys.total_swap() > 0 { format!("The whole system is using {} of {} swap.", human_bytes_binary(self.sys.used_swap()), human_bytes_binary(self.sys.total_swap())) } else { "This system has no swap.".into() };
        let count = match (&inspect.threads, proc.and_then(|p| p.threads)) {
            (Ok(t), _) if !t.is_empty() => t.len().to_string(),
            (_, Some(n)) => n.to_string(),
            _ => "—".into(),
        };
        let tiles = row()
            .spacing(12.0)
            .width(Length::Fill)
            .push(tile("CPU", proc.map_or("—".into(), |p| format!("{:.1}%", p.cpu)), "Of one core; over 100% is more than one".into()))
            .push(tile("Memory", proc.map_or("—".into(), |p| human_bytes_binary(p.memory)), proc.map_or(String::new(), |p| format!("In RAM: {} · Virtual: {}", human_bytes_binary(p.resident), human_bytes_binary(p.virtual_memory)))))
            .push(tile("Swap", swap_value, swap_note))
            .push(tile("Threads", count, if inspect.threads.is_ok() { format!("{running} running now") } else { String::new() }));
        let note = text(format!("{system_swap} Threads share their process's memory, so memory and swap are measured for the whole process.")).role(TextRole::Caption).tone(Tone::Muted);
        let top = column().spacing(12.0).width(Length::Fill).padding([16.0, 14.0, 16.0, 10.0]).push(tiles).push(note);

        let body: Element<Msg> = match &inspect.threads {
            Err(Denied::Gone) => message(icons::CIRCLE_STOP, "This process has exited", format!("{name} is no longer running.")),
            Err(Denied::Permission) => message(icons::LOCK, "These threads can't be read", format!("{name} belongs to another user or the system. Its threads are visible only to an administrator.")),
            Err(Denied::Unsupported) => message(icons::INFO, "Not supported here", "Neo can't list threads on this operating system yet.".into()),
            Ok(_) => self.thread_table(inspect, &rows),
        };
        column().width(Length::Fill).height(Length::Fill).push(toolbar).push(Divider::horizontal()).push(top).push(Divider::horizontal()).push(body).into()
    }

    fn thread_table(&self, inspect: &Inspect, rows: &[&ThreadRow]) -> Element<Msg> {
        let header_cell = |label: &str, by: ThreadSort, width: Length, align: Align| -> Element<Msg> {
            let active = inspect.sort == by;
            let mut r = row().spacing(4.0).align(Align::Center).push(text(label).role(TextRole::Label).tone(if active { Tone::Accent } else { Tone::Muted }));
            if active {
                r = r.push(icon(if inspect.descending { icons::CHEVRON_DOWN } else { icons::CHEVRON_UP }).size(12.0).tone(Tone::Accent));
            }
            Button::new(r).kind(ButtonKind::Ghost).padding([6.0, 6.0]).width(width).align_x(align).on_press(Msg::ThreadSort(by)).into()
        };
        let header = row()
            .spacing(12.0)
            .width(Length::Fill)
            .padding([4.0, 22.0, 4.0, 14.0])
            .push(header_cell("Thread", ThreadSort::Name, Length::Fill, Align::Start))
            .push(header_cell("State", ThreadSort::State, Length::Fixed(90.0), Align::Start))
            .push(header_cell("CPU", ThreadSort::Cpu, Length::Fixed(130.0), Align::End))
            .push(header_cell("CPU time", ThreadSort::Time, Length::Fixed(110.0), Align::End))
            .push(header_cell("ID", ThreadSort::Id, Length::Fixed(90.0), Align::End));
        let mut list = column().spacing(1.0).width(Length::Fill).padding([4.0, 28.0, 10.0, 20.0]);
        for r in rows.iter().take(MAX_THREADS) {
            let t = &r.info;
            let name = if t.name.is_empty() { text("Unnamed").tone(Tone::Faint) } else { text(t.name.clone()) };
            let state_tone = match t.state {
                ThreadState::Running => Tone::Good,
                ThreadState::Blocked | ThreadState::Stopped => Tone::Warn,
                _ => Tone::Muted,
            };
            let cpu = match r.cpu {
                Some(c) => row().spacing(8.0).align(Align::Center).width(130.0).push(Space::fill_x()).push(progress_bar((c / 100.0).min(1.0)).width(48.0).height(6.0)).push(text(format!("{c:.1}%")).mono().role(TextRole::Caption).align(Align::End).width(56.0)),
                None => row().width(130.0).push(Space::fill_x()).push(text("—").mono().role(TextRole::Caption).tone(Tone::Faint)),
            };
            list = list.push(
                row()
                    .spacing(12.0)
                    .align(Align::Center)
                    .width(Length::Fill)
                    .padding([0.0, 5.0])
                    .push(name.no_wrap().width(Length::Fill))
                    .push(container(text(t.state.label()).role(TextRole::Caption).tone(state_tone)).padding([0.0, 0.0, 0.0, 6.0]).width(90.0))
                    .push(cpu)
                    .push(text(cpu_time(t.cpu_time)).mono().role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(110.0))
                    .push(text(t.id.to_string()).mono().role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(90.0)),
            );
        }
        if rows.len() > MAX_THREADS {
            list = list.push(container(text(format!("Showing {MAX_THREADS} of {} threads.", rows.len())).role(TextRole::Caption).tone(Tone::Muted)).padding([0.0, 10.0]));
        }
        let caption = container(text("CPU is the share of one processor core each thread used since the last sample.").role(TextRole::Caption).tone(Tone::Muted)).padding([24.0, 10.0]);
        column().width(Length::Fill).height(Length::Fill).push(header).push(scrollable(list).height(Length::Fill)).push(Divider::horizontal()).push(caption).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_cpu_time() {
        assert_eq!(cpu_time(Duration::from_millis(15)), "15 ms");
        assert_eq!(cpu_time(Duration::from_millis(4200)), "4.2 s");
        assert_eq!(cpu_time(Duration::from_secs(200)), "3 min 20 s");
        assert_eq!(cpu_time(Duration::from_secs(7500)), "2 h 05 min");
    }

    #[test]
    fn measures_a_busy_thread_between_samples() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let (started, running) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("neo-spin".into())
            .spawn(move || {
                let _ = started.send(());
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            })
            .unwrap();
        // The first sample must see the thread, or there is nothing to
        // measure its second sample against.
        running.recv().unwrap();
        let mut inspect = Inspect::new(Pid::from_u32(std::process::id()));
        inspect.refresh(None);
        assert!(inspect.threads.as_ref().unwrap().iter().all(|r| r.cpu.is_none()), "nothing to compare against on the first sample");
        // A listing of a process's threads can miss one while others are
        // exiting, as the tests running beside this one do. A measurement
        // needs the thread in two listings in a row, so try again if not.
        let spinning = |i: &Inspect| i.threads.as_ref().unwrap().iter().find(|r| r.info.name == "neo-spin").and_then(|r| r.cpu);
        let mut cpu = None;
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(400));
            inspect.refresh(None);
            cpu = spinning(&inspect);
            if cpu.is_some() {
                break;
            }
        }
        let top = inspect.sorted()[0];
        assert_eq!(top.info.name, "neo-spin", "the busiest thread sorts first");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        worker.join().unwrap();
        let cpu = cpu.expect("the thread was in two listings in a row");
        assert!((60.0..=110.0).contains(&cpu), "a spinning thread uses about one core, measured {cpu:.0}%");
    }
}
