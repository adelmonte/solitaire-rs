//! Tiny INI/.desktop reader and the persistent settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type Ini = BTreeMap<String, BTreeMap<String, String>>;

pub fn parse_ini(text: &str) -> Ini {
    let mut ini = Ini::new();
    let mut group = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            group = line[1..line.len() - 1].to_string();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            ini.entry(group.clone())
                .or_default()
                .insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    ini
}

pub fn read_ini(path: &Path) -> Option<Ini> {
    std::fs::read_to_string(path).ok().map(|t| parse_ini(&t))
}

pub fn get<'a>(ini: &'a Ini, group: &str, key: &str) -> Option<&'a str> {
    ini.get(group)?.get(key).map(String::as_str)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn env_dir(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_dir() -> PathBuf {
    env_dir("XDG_CONFIG_HOME", ".config")
}

/// XDG data directories, user dir first (that's where KNewStuff installs
/// downloaded decks and themes).
pub fn data_dirs() -> Vec<PathBuf> {
    let mut v = vec![env_dir("XDG_DATA_HOME", ".local/share")];
    let sys = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty());
    for d in sys
        .as_deref()
        .unwrap_or("/usr/local/share:/usr/share")
        .split(':')
    {
        if !d.is_empty() {
            let p = PathBuf::from(d);
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    v
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub deck: String,
    pub theme: String,
    pub card_width: i32,
    pub draw_one: bool,
    pub autodrop: bool,
    pub solver: bool,
    pub status_bar: bool,
    pub win_w: i32,
    pub win_h: i32,
}

const GENERAL: &str = "General Settings";

impl Settings {
    fn path() -> PathBuf {
        config_dir().join("solitaire-rsrc")
    }

    /// Our own rc file; on first run, seed it from KPat's kpatrc so the
    /// chosen deck, background and card size carry over.
    pub fn load() -> Settings {
        let mut s = Settings {
            deck: "svg-oxygen-air".into(),
            theme: "greenblaze".into(),
            card_width: 0,
            draw_one: true,
            autodrop: true,
            solver: true,
            status_bar: true,
            win_w: 0,
            win_h: 0,
        };
        let ini = read_ini(&Self::path()).or_else(|| read_ini(&config_dir().join("kpatrc")));
        let Some(ini) = ini else { return s };
        let b = |k: &str, d: bool| get(&ini, GENERAL, k).map_or(d, |v| v == "true" || v == "1");
        let i = |g: &str, k: &str| get(&ini, g, k).and_then(|v| v.parse::<i32>().ok());
        if let Some(v) = get(&ini, GENERAL, "Cardname") {
            s.deck = v.to_string();
        }
        if let Some(v) = get(&ini, "KgTheme", "Theme") {
            s.theme = theme_id(v);
        }
        s.card_width = i(GENERAL, "CardWidth").unwrap_or(0).max(0);
        s.draw_one = b("KlondikeEasy", true);
        s.autodrop = b("Autodrop", true);
        s.solver = b("Solver", true);
        s.status_bar = b("StatusBar", true);
        s.win_w = i("Window", "Width").unwrap_or(0);
        s.win_h = i("Window", "Height").unwrap_or(0);
        s
    }

    pub fn save(&self) {
        let t = format!(
            "[{GENERAL}]\nCardname={}\nCardWidth={}\nKlondikeEasy={}\nAutodrop={}\nSolver={}\nStatusBar={}\n\n[KgTheme]\nTheme={}\n\n[Window]\nWidth={}\nHeight={}\n",
            self.deck,
            self.card_width,
            self.draw_one,
            self.autodrop,
            self.solver,
            self.status_bar,
            self.theme,
            self.win_w,
            self.win_h
        );
        let p = Self::path();
        let _ = std::fs::create_dir_all(config_dir());
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// KGameThemeProvider stores either a bare name or a path to the .desktop
/// file; reduce both to the bare name.
pub fn theme_id(v: &str) -> String {
    let base = v.rsplit('/').next().unwrap_or(v);
    base.strip_suffix(".desktop").unwrap_or(base).to_string()
}
