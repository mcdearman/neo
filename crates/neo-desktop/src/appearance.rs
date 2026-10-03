use std::path::PathBuf;
use std::time::SystemTime;

use neo::{Accent, Glass, Scheme, Theme};

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
/// Stored as `key = value` lines in `appearance.conf` under [`config_dir`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Appearance {
    pub scheme: SchemePref,
    pub accent: Accent,
    pub radius: f32,
    pub text_scale: f32,
    pub reduce_motion: bool,
    pub glass: Glass,
}

impl Default for Appearance {
    fn default() -> Self {
        let t = Theme::default();
        Self { scheme: SchemePref::Auto, accent: t.accent, radius: t.radius, text_scale: 1.0, reduce_motion: false, glass: t.glass }
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
            let (l, d) = v.split_once('/')?;
            let hex = |s: &str| u32::from_str_radix(s.strip_prefix('#')?, 16).ok().map(neo::Color::hex);
            Accent::Custom { light: hex(l)?, dark: hex(d)? }
        }
    })
}

impl Appearance {
    pub fn path() -> PathBuf {
        config_dir().join("appearance.conf")
    }

    pub(crate) fn modified() -> Option<SystemTime> {
        std::fs::metadata(Self::path()).and_then(|m| m.modified()).ok()
    }

    /// Reads the settings file. Missing or unreadable values keep their defaults.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    pub fn parse(src: &str) -> Self {
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
                _ => {}
            }
        }
        a
    }

    pub fn serialize(&self) -> String {
        format!(
            "# Neo appearance. Written by Settings; every Neo app reloads it on change.\n\
             scheme = {}\naccent = {}\nradius = {}\ntext-scale = {}\nreduce-motion = {}\nglass = {}\nglass-opacity = {}\nglass-blur = {}\n",
            self.scheme.key(),
            accent_key(self.accent),
            self.radius,
            self.text_scale,
            self.reduce_motion,
            self.glass.enabled,
            self.glass.opacity,
            self.glass.blur,
        )
    }

    /// Writes the settings atomically, so readers never see half a file.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, self.serialize())?;
        std::fs::rename(tmp, path)
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let a = Appearance {
            scheme: SchemePref::Dark,
            accent: Accent::Custom { light: neo::Color::hex(0x123456), dark: neo::Color::hex(0xABCDEF) },
            radius: 12.0,
            text_scale: 1.25,
            reduce_motion: true,
            glass: Glass { enabled: true, opacity: 0.5, blur: 20.0 },
        };
        assert_eq!(Appearance::parse(&a.serialize()), a);
    }

    #[test]
    fn ignores_junk_and_clamps() {
        let a = Appearance::parse("scheme = purple\nradius = 900\nnonsense\naccent = teal\n");
        assert_eq!(a.scheme, SchemePref::Auto);
        assert_eq!(a.radius, 40.0);
        assert_eq!(a.accent, Accent::Teal);
    }
}
