//! `.desktop` entry discovery and parsing.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopEntry {
    pub path: PathBuf,
    /// File stem, e.g. `org.mozilla.firefox`.
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub icon: Option<String>,
    pub categories: Vec<String>,
    /// NoDisplay=true or Hidden=true or not Type=Application.
    pub hidden: bool,
    pub exec: Option<String>,
}

/// Parse the `[Desktop Entry]` group of a desktop file.
pub fn parse_desktop(path: &Path, text: &str) -> Option<DesktopEntry> {
    let mut in_group = false;
    let mut e = DesktopEntry {
        path: path.to_path_buf(),
        id: path.file_stem()?.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut is_app = false;
    let mut have_name = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Type" => is_app = v == "Application",
            "Name" => {
                e.name = v.to_string();
                have_name = true;
            }
            "Comment" => e.comment = Some(v.to_string()),
            "Icon" => e.icon = Some(v.to_string()),
            "Exec" => e.exec = Some(v.to_string()),
            "Categories" => e.categories = v.split(';').filter(|s| !s.is_empty()).map(String::from).collect(),
            "NoDisplay" | "Hidden" if v.eq_ignore_ascii_case("true") => e.hidden = true,
            _ => {}
        }
    }
    if !have_name {
        return None;
    }
    if !is_app {
        e.hidden = true;
    }
    Some(e)
}

/// Map desktop Categories to one of a small set of friendly categories.
pub fn main_category(categories: &[String]) -> Option<String> {
    const MAP: &[(&str, &str)] = &[
        ("Game", "Games"),
        ("Development", "Development"),
        ("Graphics", "Graphics"),
        ("Office", "Office"),
        ("Network", "Internet"),
        ("AudioVideo", "Multimedia"),
        ("Audio", "Multimedia"),
        ("Video", "Multimedia"),
        ("Education", "Education"),
        ("Science", "Science"),
        ("Settings", "Settings"),
        ("System", "System Tools"),
        ("Utility", "Utilities"),
        ("Accessibility", "Utilities"),
    ];
    for &(raw, nice) in MAP {
        if categories.iter().any(|c| c == raw) {
            return Some(nice.to_string());
        }
    }
    None
}

/// Directories scanned for desktop entries.
pub fn application_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        PathBuf::from("/var/lib/snapd/desktop/applications"),
    ];
    if crate::security::in_flatpak_sandbox() {
        // The host's /usr is visible under /run/host with --filesystem=host-os:ro.
        dirs.push(PathBuf::from("/run/host/usr/share/applications"));
        dirs.push(PathBuf::from("/run/host/usr/local/share/applications"));
    }
    if !home.is_empty() {
        dirs.push(Path::new(&home).join(".local/share/applications"));
        dirs.push(Path::new(&home).join(".local/share/flatpak/exports/share/applications"));
    }
    dirs
}

/// All desktop entries found on the system, indexed by path and id.
#[derive(Debug, Default, Clone)]
pub struct DesktopIndex {
    by_path: HashMap<PathBuf, DesktopEntry>,
    by_id: HashMap<String, Vec<PathBuf>>,
}

impl DesktopIndex {
    pub fn scan() -> Self {
        let mut idx = DesktopIndex::default();
        for dir in application_dirs() {
            idx.scan_dir(&dir, 0);
        }
        idx
    }

    fn scan_dir(&mut self, dir: &Path, depth: u8) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                if depth < 2 {
                    self.scan_dir(&p, depth + 1);
                }
            } else if p.extension().is_some_and(|e| e == "desktop") {
                if let Ok(text) = fs::read_to_string(&p) {
                    if let Some(entry) = parse_desktop(&p, &text) {
                        self.insert(entry);
                    }
                }
            }
        }
    }

    pub fn insert(&mut self, e: DesktopEntry) {
        self.by_id.entry(e.id.clone()).or_default().push(e.path.clone());
        self.by_path.insert(e.path.clone(), e);
    }

    pub fn get(&self, path: &Path) -> Option<&DesktopEntry> {
        // Resolve /usr/share vs /usr/lib64 symlink differences by canonical path
        // only when direct lookup misses.
        self.by_path
            .get(path)
            .or_else(|| {
                let c = fs::canonicalize(path).ok()?;
                self.by_path.get(&c)
            })
            .or_else(|| {
                // Inside a Flatpak sandbox host paths live under /run/host.
                self.by_path.get(&Path::new("/run/host").join(path.strip_prefix("/").unwrap_or(path)))
            })
    }

    pub fn by_id(&self, id: &str) -> Vec<&DesktopEntry> {
        self.by_id.get(id).map(|v| v.iter().filter_map(|p| self.by_path.get(p)).collect()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

/// True when a package-owned file path is a desktop entry.
pub fn is_desktop_file_path(path: &str) -> bool {
    path.ends_with(".desktop")
        && (path.starts_with("/usr/share/applications/")
            || path.starts_with("/usr/local/share/applications/")
            || path.starts_with("/opt/") && path.contains("/applications/")
            || path.starts_with("usr/share/applications/"))
}

/// True when a package-owned file path looks like a user-launchable binary.
pub fn is_binary_path(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    (p.starts_with("usr/bin/")
        || p.starts_with("usr/games/")
        || p.starts_with("usr/local/bin/")
        || p.starts_with("bin/"))
        && !p.ends_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "[Desktop Entry]\nVersion=1.0\nName=Firefox Web Browser\nName[de]=Firefox\nComment=Browse the World Wide Web\nExec=firefox %u\nIcon=firefox\nType=Application\nCategories=GNOME;GTK;Network;WebBrowser;\n\n[Desktop Action new-window]\nName=New Window\nExec=firefox --new-window\n";

    #[test]
    fn parses_desktop_entry() {
        let e = parse_desktop(Path::new("/usr/share/applications/firefox.desktop"), FIREFOX).unwrap();
        assert_eq!(e.name, "Firefox Web Browser");
        assert_eq!(e.icon.as_deref(), Some("firefox"));
        assert!(!e.hidden);
        assert_eq!(main_category(&e.categories).as_deref(), Some("Internet"));
        assert_eq!(e.id, "firefox");
    }

    #[test]
    fn nodisplay_is_hidden() {
        let t = "[Desktop Entry]\nName=Helper\nType=Application\nNoDisplay=true\n";
        assert!(parse_desktop(Path::new("/x/helper.desktop"), t).unwrap().hidden);
        let t = "[Desktop Entry]\nName=Link\nType=Link\n";
        assert!(parse_desktop(Path::new("/x/link.desktop"), t).unwrap().hidden);
    }

    #[test]
    fn path_helpers() {
        assert!(is_desktop_file_path("/usr/share/applications/a.desktop"));
        assert!(!is_desktop_file_path("/usr/share/doc/a.desktop"));
        assert!(is_binary_path("/usr/bin/git"));
        assert!(is_binary_path("usr/bin/git"));
        assert!(!is_binary_path("/usr/lib/git"));
    }
}
