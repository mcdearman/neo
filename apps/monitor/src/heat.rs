//! The Sensors page: temperatures by part, fan speeds, and every sensor.

use std::time::Duration;

use neo::prelude::*;

use crate::sensors::{Fan, Kind, Reading, Sensor};
use crate::{Monitor, Msg, panel};

/// How often the sensors are read while the page is open.
pub const EVERY: Duration = Duration::from_secs(2);
/// The top of the temperature scale for bars and charts, in °C.
const SCALE: f32 = 110.0;

/// Normal, warm or hot, by the limits for that kind of part.
fn tone(kind: Kind, celsius: f32) -> Tone {
    let (warm, hot) = kind.limits();
    if celsius >= hot {
        Tone::Bad
    } else if celsius >= warm {
        Tone::Warn
    } else {
        Tone::Inherit
    }
}

/// The bar colour: the accent until a part is warm.
fn bar_tone(kind: Kind, celsius: f32) -> Tone {
    match tone(kind, celsius) {
        Tone::Inherit => Tone::Accent,
        t => t,
    }
}

/// `3,742` for fan speeds.
pub fn thousands(n: f32) -> String {
    let digits = (n.round().max(0.0) as u64).to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn tile(kind: Kind, reading: &Reading) -> Element<Msg> {
    let temps: Vec<f32> = reading.awake(kind).map(|s| s.celsius).collect();
    let idle = reading.of(kind).filter(|s| s.idle).count();
    let (value, value_tone, caption) = match reading.hottest(kind) {
        Some(hottest) => {
            let average = temps.iter().sum::<f32>() / temps.len() as f32;
            // Short enough for one line in the tile.
            let caption = match (temps.len(), idle) {
                (n, 0) if n > 1 => format!("Hottest of {n} · average {average:.0}°"),
                (_, 0) => "One sensor".to_string(),
                (n, idle) => format!("Hottest of {n} · {idle} idle"),
            };
            (format!("{hottest:.0}°C"), tone(kind, hottest), caption)
        }
        // No core is reporting: show the chips beside the processor.
        None if idle > 0 && kind == Kind::Cpu && reading.beside_cpu().is_some() => {
            let nearby = reading.beside_cpu().unwrap_or_default();
            (format!("{nearby:.0}°C"), tone(kind, nearby), "Core sensors idle · power chips beside them".into())
        }
        None if idle > 0 => ("—".into(), Tone::Faint, "Idle".into()),
        None => ("—".into(), Tone::Faint, "No sensor reported".into()),
    };
    container(column().spacing(4.0).width(Length::Fill).push(text(kind.name()).role(TextRole::Label).tone(Tone::Muted)).push(text(value).mono().role(TextRole::Heading).tone(value_tone).no_wrap()).push(text(caption).role(TextRole::Caption).tone(Tone::Muted))).surface(Surface::Well).padding([16.0, 14.0]).width(Length::Fill).into()
}

fn fan_row(fan: &Fan) -> Element<Msg> {
    // How far between its slowest and fastest the fan is turning.
    let share = match (fan.min, fan.max) {
        (Some(min), Some(max)) if max > min => (fan.rpm - min) / (max - min),
        (_, Some(max)) if max > 0.0 => fan.rpm / max,
        _ => fan.rpm / 6000.0,
    };
    let range = match (fan.min, fan.max) {
        (Some(min), Some(max)) => format!("Runs at {}–{} RPM", thousands(min), thousands(max)),
        _ => String::new(),
    };
    column()
        .spacing(6.0)
        .width(Length::Fill)
        .push(row().width(Length::Fill).align(Align::Center).push(text(fan.name.clone()).role(TextRole::Strong)).push(Space::fill_x()).push(if fan.rpm < 1.0 { text("Off").tone(Tone::Muted) } else { text(format!("{} RPM", thousands(fan.rpm))).mono() }))
        .push(progress_bar(share.clamp(0.0, 1.0)).height(8.0))
        .push(text(range).mono().role(TextRole::Caption).tone(Tone::Faint))
        .into()
}

fn sensor_row(s: &Sensor) -> Element<Msg> {
    let mut name = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text(s.name.clone()).no_wrap());
    // The system's own name, where ours differs from it.
    if s.id != s.name {
        name = name.push(text(s.id.clone()).mono().role(TextRole::Caption).tone(Tone::Faint));
    }
    if s.idle {
        // Nothing to show until the chip is busy again.
        return row().spacing(12.0).align(Align::Center).width(Length::Fill).push(name).push(text("Idle").role(TextRole::Caption).tone(Tone::Faint).align(Align::End).width(70.0)).into();
    }
    row().spacing(12.0).align(Align::Center).width(Length::Fill).push(name).push(progress_bar((s.celsius / SCALE).clamp(0.0, 1.0)).width(120.0).height(6.0).tone(bar_tone(s.kind, s.celsius))).push(text(format!("{:.1}°C", s.celsius)).mono().role(TextRole::Caption).tone(tone(s.kind, s.celsius)).align(Align::End).width(70.0)).into()
}

impl Monitor {
    pub(crate) fn sensors(&self) -> Element<Msg> {
        let Some(reading) = &self.heat else {
            let waiting = column().spacing(10.0).align(Align::Center).push(icon(icons::THERMOMETER).size(34.0).tone(Tone::Faint)).push(text("Reading sensors…").tone(Tone::Muted));
            return container(waiting).width(Length::Fill).height(Length::Fill).center().into();
        };
        if reading.sensors.is_empty() && reading.fans.is_empty() {
            let why = if cfg!(windows) { "Windows reports temperatures only where the firmware provides them, and often only to administrators." } else { "This computer reports no temperature or fan sensors. In a virtual machine that is expected." };
            let none = column().spacing(10.0).align(Align::Center).push(icon(icons::THERMOMETER).size(34.0).tone(Tone::Faint)).push(text("No sensors found").role(TextRole::Strong)).push(container(text(why).tone(Tone::Muted).align(Align::Center)).max_width(440.0));
            return container(none).width(Length::Fill).height(Length::Fill).center().into();
        }

        let tiles = Kind::SUMMARY.iter().fold(row().spacing(12.0).width(Length::Fill), |r, kind| r.push(tile(*kind, reading)));

        let fans: Element<Msg> = if reading.fans.is_empty() {
            let why = if cfg!(target_os = "macos") {
                "No fans reported. This Mac may not have any."
            } else if cfg!(windows) {
                "Windows does not report fan speeds without the maker's own driver."
            } else {
                "No fan sensors found. Some laptops report them only with a vendor kernel module."
            };
            text(why).tone(Tone::Muted).into()
        } else {
            reading.fans.iter().fold(column().spacing(14.0).width(Length::Fill), |c, f| c.push(fan_row(f))).into()
        };
        let fan_detail = if reading.fans.is_empty() { String::new() } else { format!("{} fans", reading.fans.len()) };

        let chart = |label: &str, history: &std::collections::VecDeque<f32>, tone: Tone| -> Element<Msg> {
            let now = history.back().map_or(String::new(), |t| format!("{t:.0}°C"));
            column().spacing(6.0).width(Length::Fill).push(row().width(Length::Fill).push(text(label).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x()).push(text(now).mono().role(TextRole::Caption))).push(sparkline(Vec::from(history.clone()), 20.0, SCALE).height(64.0).tone(tone)).into()
        };
        let mut history = row().spacing(16.0).width(Length::Fill);
        if !self.cpu_heat.is_empty() {
            history = history.push(chart("CPU, hottest sensor", &self.cpu_heat, Tone::Accent));
        }
        if !self.gpu_heat.is_empty() {
            history = history.push(chart("GPU, hottest sensor", &self.gpu_heat, Tone::Warn));
        }

        let mut all = column().spacing(8.0).width(Length::Fill);
        for kind in Kind::ALL {
            let mut of_kind = reading.of(kind).peekable();
            if of_kind.peek().is_none() {
                continue;
            }
            all = all.push(container(text(kind.name()).role(TextRole::Label).tone(Tone::Muted)).padding([0.0, 6.0, 0.0, 0.0]));
            for s in of_kind {
                all = all.push(sensor_row(s));
            }
        }

        let mut page = column().spacing(16.0).width(Length::Fill).padding(18.0).push(tiles);
        let mut middle = row().spacing(16.0).width(Length::Fill).push(panel("Fans", fan_detail, fans));
        if !self.cpu_heat.is_empty() || !self.gpu_heat.is_empty() {
            middle = middle.push(panel("History", format!("every {} s", EVERY.as_secs()), history));
        }
        page = page.push(middle);
        if !reading.sensors.is_empty() {
            page = page.push(panel("All sensors", format!("{} sensors", reading.sensors.len()), all));
        }
        scrollable(page).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        assert_eq!(thousands(3742.4), "3,742");
        assert_eq!(thousands(980.0), "980");
        assert_eq!(thousands(12_000.0), "12,000");
    }

    #[test]
    fn warns_by_the_kind_of_part() {
        // 65 °C is nothing for a processor and hot for a drive.
        assert_eq!(tone(Kind::Cpu, 65.0), Tone::Inherit);
        assert_eq!(tone(Kind::Storage, 65.0), Tone::Warn);
        assert_eq!(tone(Kind::Cpu, 101.0), Tone::Bad);
        assert_eq!(tone(Kind::Battery, 46.0), Tone::Bad);
    }
}
