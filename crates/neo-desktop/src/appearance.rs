use std::path::PathBuf;
use std::time::SystemTime;

use neo::{Accent, Glass, Scheme, Theme};

use crate::config::{self, File};

/// Light, dark, or whatever the system prefers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SchemePref {
    #[default]
    Auto,
    Light,
    Dark,
}

impl SchemePref {
    pub const ALL: [SchemePref; 3] = [SchemePref::Auto, SchemePref::Light, SchemePref::Dark];

    fn key(self) -> &'static str {
        match self {
            SchemePref::Auto => "auto",
            SchemePref::Light => "light",
            SchemePref::Dark => "dark",
        }
    }
}

/// The user's appearance settings, shared by every Neo app.
///
/// Kept in `neo.toml` under [`config_dir`]: see [`crate::config`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Appearance {
    pub scheme: SchemePref,
    pub accent: Accent,
    pub radius: f32,
    pub text_scale: f32,
    pub reduce_motion: bool,
    pub glass: Glass,
    /// How long a notification stays on screen, in seconds.
    pub notification_seconds: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        let t = Theme::default();
        Self { scheme: SchemePref::Auto, accent: t.accent, radius: t.radius, text_scale: 1.0, reduce_motion: false, glass: t.glass, notification_seconds: 5.0 }
    }
}

/// Where Neo keeps its settings: `$NEO_CONFIG_DIR`, else `%APPDATA%\Neo`
/// on Windows, else `$XDG_CONFIG_HOME/neo`, else `~/.config/neo`.
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NEO_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    if cfg!(windows)
        && let Some(d) = std::env::var_os("APPDATA")
    {
        return PathBuf::from(d).join("Neo");
    }
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("neo");
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default();
    PathBuf::from(home).join(".config").join("neo")
}

fn accent_key(a: Accent) -> String {
    match a {
        Accent::Royal => "royal".into(),
        Accent::Teal => "teal".into(),
        Accent::Coral => "coral".into(),
        Accent::Amber => "amber".into(),
        Accent::Custom { light, dark } => {
            let [lr, lg, lb, _] = light.to_rgba8();
            let [dr, dg, db, _] = dark.to_rgba8();
            format!("#{lr:02x}{lg:02x}{lb:02x}/#{dr:02x}{dg:02x}{db:02x}")
        }
    }
}

fn parse_accent(v: &str) -> Option<Accent> {
    Some(match v {
        "royal" => Accent::Royal,
        "teal" => Accent::Teal,
        "coral" => Accent::Coral,
        "amber" => Accent::Amber,
        _ => {
            let hex = |s: &str| u32::from_str_radix(s.trim().strip_prefix('#').filter(|h| h.len() == 6)?, 16).ok().map(neo::Color::hex);
            match v.split_once('/') {
                // One for light windows and one for dark.
                Some((l, d)) => Accent::Custom { light: hex(l)?, dark: hex(d)? },
                // One colour, shifted as far as it needs to be to be read on each.
                None => Accent::from_color(hex(v)?),
            }
        }
    })
}

impl Appearance {
    /// The file these are kept in: `neo.toml`, under `[appearance]`,
    /// `[appearance.glass]` and `[notifications]`.
    pub fn path() -> PathBuf {
        config::desktop_path()
    }

    pub(crate) fn modified() -> Option<SystemTime> {
        config::modified(&Self::path())
    }

    /// Reads the settings file. Missing or unreadable values keep their defaults.
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    /// Reads the settings file, or says what is wrong with it: a mistake
    /// made editing it by hand is not to put everything back to defaults.
    pub fn try_load() -> Result<Self, String> {
        let mut file = File::desktop();
        if let Some(problem) = file.problem() {
            return Err(problem);
        }
        // Kept in a file of its own before: brought over, the once.
        if !file.has(&["appearance"])
            && let Ok(old) = std::fs::read_to_string(config_dir().join("appearance.conf"))
        {
            let a = Self::legacy(&old);
            a.write(&mut file);
            let _ = file.save();
            return Ok(a);
        }
        Ok(Self::read(&file))
    }

    /// The settings in a file. What is missing, or makes no sense, keeps
    /// its default, and numbers are kept within bounds.
    pub fn read(file: &File) -> Self {
        let mut a = Self::default();
        let num = |at: &[&str], lo: f32, hi: f32, or: f32| file.number(at).map_or(or, |n| (n as f32).clamp(lo, hi));
        a.scheme = file.text(&["appearance", "scheme"]).and_then(|v| SchemePref::ALL.into_iter().find(|s| s.key() == v)).unwrap_or(a.scheme);
        a.accent = file.text(&["appearance", "accent"]).and_then(parse_accent).unwrap_or(a.accent);
        a.radius = num(&["appearance", "radius"], 0.0, 40.0, a.radius);
        a.text_scale = num(&["appearance", "text-scale"], 0.75, 2.0, a.text_scale);
        a.reduce_motion = file.flag(&["appearance", "reduce-motion"]).unwrap_or(a.reduce_motion);
        a.glass.enabled = file.flag(&["appearance", "glass", "enabled"]).unwrap_or(a.glass.enabled);
        a.glass.opacity = num(&["appearance", "glass", "opacity"], 0.2, 1.0, a.glass.opacity);
        a.glass.blur = num(&["appearance", "glass", "blur"], 0.0, 60.0, a.glass.blur);
        a.notification_seconds = num(&["notifications", "seconds"], 1.0, 60.0, a.notification_seconds);
        a
    }

    /// Puts the settings into a file, leaving whatever else is in it.
    pub fn write(&self, file: &mut File) {
        file.set(&["appearance", "scheme"], self.scheme.key());
        // A colour written by hand as one code stays as it was written, while it is still that colour.
        if file.text(&["appearance", "accent"]).and_then(parse_accent) != Some(self.accent) {
            file.set(&["appearance", "accent"], accent_key(self.accent));
        }
        file.set_number(&["appearance", "radius"], self.radius);
        file.set_number(&["appearance", "text-scale"], self.text_scale);
        file.set(&["appearance", "reduce-motion"], self.reduce_motion);
        file.set(&["appearance", "glass", "enabled"], self.glass.enabled);
        file.set_number(&["appearance", "glass", "opacity"], self.glass.opacity);
        file.set_number(&["appearance", "glass", "blur"], self.glass.blur);
        file.set_number(&["notifications", "seconds"], self.notification_seconds);
    }

    /// The settings in a file's text.
    pub fn parse(src: &str) -> Self {
        Self::read(&File::parse(src))
    }

    /// Reads the `key = value` lines of `appearance.conf`, which is where
    /// these were kept before `neo.toml`.
    fn legacy(src: &str) -> Self {
        let mut a = Self::default();
        for line in src.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let num = |lo: f32, hi: f32| v.parse::<f32>().ok().filter(|n| n.is_finite()).map(|n| n.clamp(lo, hi));
            let flag = || match v {
                "true" | "yes" | "on" => Some(true),
                "false" | "no" | "off" => Some(false),
                _ => None,
            };
            match k {
                "scheme" => a.scheme = SchemePref::ALL.into_iter().find(|s| s.key() == v).unwrap_or(a.scheme),
                "accent" => a.accent = parse_accent(v).unwrap_or(a.accent),
                "radius" => a.radius = num(0.0, 40.0).unwrap_or(a.radius),
                "text-scale" => a.text_scale = num(0.75, 2.0).unwrap_or(a.text_scale),
                "reduce-motion" => a.reduce_motion = flag().unwrap_or(a.reduce_motion),
                "glass" => a.glass.enabled = flag().unwrap_or(a.glass.enabled),
                "glass-opacity" => a.glass.opacity = num(0.2, 1.0).unwrap_or(a.glass.opacity),
                "glass-blur" => a.glass.blur = num(0.0, 60.0).unwrap_or(a.glass.blur),
                "notification-seconds" => a.notification_seconds = num(1.0, 60.0).unwrap_or(a.notification_seconds),
                _ => {}
            }
        }
        a
    }

    /// The settings as a file of their own would have them.
    pub fn serialize(&self) -> String {
        let mut file = File::parse("");
        self.write(&mut file);
        file.encode()
    }

    /// Writes the settings into `neo.toml`, atomically, so readers never see half a file.
    pub fn save(&self) -> std::io::Result<()> {
        let mut file = File::desktop();
        self.write(&mut file);
        file.save()
    }

    pub fn theme(&self, system: Scheme) -> Theme {
        Theme {
            scheme: match self.scheme {
                SchemePref::Auto => system,
                SchemePref::Light => Scheme::Light,
                SchemePref::Dark => Scheme::Dark,
            },
            accent: self.accent,
            radius: self.radius,
            text_scale: self.text_scale,
            reduce_motion: self.reduce_motion,
            glass: self.glass,
            syntax: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let a = Appearance { scheme: SchemePref::Dark, accent: Accent::Custom { light: neo::Color::hex(0x123456), dark: neo::Color::hex(0xABCDEF) }, radius: 12.0, text_scale: 1.25, reduce_motion: true, glass: Glass { enabled: true, opacity: 0.5, blur: 20.0 }, notification_seconds: 8.0 };
        assert_eq!(Appearance::parse(&a.serialize()), a);
    }

    #[test]
    fn ignores_junk_and_clamps() {
        let a = Appearance::parse("[appearance]\nscheme = \"purple\"\nradius = 900\nnonsense = 1\naccent = \"teal\"\ntext-scale = \"big\"\n[notifications]\nseconds = 9\n");
        assert_eq!((a.scheme, a.radius, a.accent, a.text_scale, a.notification_seconds), (SchemePref::Auto, 40.0, Accent::Teal, 1.0, 9.0));
        // An accent written as one colour is that colour, and stays as it was written.
        let mine = "[appearance]\naccent = \"#E0569B\" # pink\n";
        let a = Appearance::parse(mine);
        assert_eq!(a.accent, Accent::from_color(neo::Color::hex(0xE0569B)));
        let mut file = File::parse(mine);
        a.write(&mut file);
        assert!(file.encode().starts_with(mine), "{}", file.encode());
    }

    #[test]
    fn what_was_kept_in_appearance_conf_is_read() {
        let old = Appearance { scheme: SchemePref::Dark, accent: Accent::Coral, radius: 12.0, text_scale: 1.15, reduce_motion: true, glass: Glass { enabled: false, opacity: 0.5, blur: 20.0 }, notification_seconds: 8.0 };
        assert_eq!(Appearance::legacy("# Neo appearance.\nscheme = dark\naccent = coral\nradius = 12\ntext-scale = 1.15\nreduce-motion = true\nglass = false\nglass-opacity = 0.5\nglass-blur = 20\nnotification-seconds = 8\n"), old);
        assert_eq!(old.serialize(), "[appearance]\nscheme = \"dark\"\naccent = \"coral\"\nradius = 12\ntext-scale = 1.15\nreduce-motion = true\n\n[appearance.glass]\nenabled = false\nopacity = 0.5\nblur = 20\n\n[notifications]\nseconds = 8\n");
    }
}
