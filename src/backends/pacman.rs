//! Pacman (Arch Linux, Manjaro, EndeavourOS, ...). Reads the local package
//! database directly, which is much faster than spawning `pacman -Qi`.

use super::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct PacmanBackend;

const LOCAL_DB: &str = "/var/lib/pacman/local";

impl Backend for PacmanBackend {
    fn source(&self) -> Source {
        Source::Pacman
    }

    fn is_available(&self) -> bool {
        cmd::exists("pacman") && Path::new(LOCAL_DB).is_dir()
    }

    fn list_installed(&self, desktop: &DesktopIndex) -> Result<Vec<Package>> {
        let rd = fs::read_dir(LOCAL_DB).map_err(|e| Error::Other(format!("cannot read {LOCAL_DB}: {e}")))?;
        let mut pkgs = Vec::new();
        for ent in rd.flatten() {
            let dir = ent.path();
            let Ok(desc) = fs::read_to_string(dir.join("desc")) else { continue };
            let Some(mut p) = parse_desc(&desc) else { continue };
            if let Ok(files) = fs::read_to_string(dir.join("files")) {
                scan_files(&files, &mut p, desktop);
            }
            pkgs.push(p);
        }
        Ok(pkgs)
    }

    fn details(&self, pkg: &Package) -> Result<Package> {
        security::validate_package_id(&pkg.id)?;
        let mut p = pkg.clone();
        // `pacman -Ql` lists owned files; pick the first binary as the location.
        if let Ok(out) = cmd::run("pacman", &["-Ql", "--", &pkg.id]) {
            p.location = out
                .lines()
                .filter_map(|l| l.split_once(' ').map(|x| x.1))
                .find(|l| crate::services::desktop::is_binary_path(l))
                .map(String::from);
        }
        Ok(p)
    }

    fn plan_removal(&self, pkg: &Package) -> Result<RemovalPlan> {
        security::validate_package_id(&pkg.id)?;
        let (ok, out, err) =
            cmd::run_allow_failure("pacman", &["-R", "--print", "--print-format", "%n %v", "--", &pkg.id])?;
        Ok(parse_pacman_print(&pkg.id, &out, &err, ok))
    }

    fn removal_command(&self, pkg: &Package) -> Result<CommandSpec> {
        security::validate_package_id(&pkg.id)?;
        Ok(CommandSpec {
            program: "pacman".into(),
            // -R only: no -s (recursive) so shared dependencies stay.
            args: vec!["-R".into(), "--noconfirm".into(), "--".into(), pkg.id.clone()],
            env: vec![],
            privileged: true,
            cancellable: false,
        })
    }
}

fn scan_files(text: &str, p: &mut Package, desktop: &DesktopIndex) {
    let mut entries = Vec::new();
    for line in text.lines() {
        if crate::services::desktop::is_desktop_file_path(line) {
            let abs = format!("/{}", line.trim_start_matches('/'));
            if let Some(e) = desktop.get(Path::new(&abs)) {
                entries.push(e);
            }
        } else if !p.has_binary && crate::services::desktop::is_binary_path(line) {
            p.has_binary = true;
        }
    }
    attach_desktop(p, &entries);
}

/// Split a pacman desc/files document into `%SECTION%` -> lines.
fn sections(text: &str) -> HashMap<&str, Vec<&str>> {
    let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut cur: Option<&str> = None;
    for line in text.lines() {
        if line.starts_with('%') && line.ends_with('%') && line.len() > 2 {
            cur = Some(&line[1..line.len() - 1]);
            map.entry(cur.unwrap()).or_default();
        } else if !line.is_empty() {
            if let Some(c) = cur {
                map.entry(c).or_default().push(line);
            }
        }
    }
    map
}

/// Parse one local database `desc` file.
pub fn parse_desc(text: &str) -> Option<Package> {
    let s = sections(text);
    let first = |k: &str| s.get(k).and_then(|v| v.first()).copied();
    let name = first("NAME")?;
    Some(Package {
        source: Some(Source::Pacman),
        id: name.to_string(),
        name: name.to_string(),
        version: first("VERSION").unwrap_or("").to_string(),
        description: first("DESC").and_then(none_if_empty),
        arch: first("ARCH").and_then(none_if_empty),
        publisher: first("PACKAGER").and_then(strip_email),
        installed_on: first("INSTALLDATE").and_then(|v| v.parse().ok()),
        size: first("SIZE").and_then(|v| v.parse().ok()),
        // %REASON% is 1 for dependencies and absent/0 for explicit installs.
        explicit: Some(first("REASON").map(|r| r.trim() != "1").unwrap_or(true)),
        section: s.get("GROUPS").map(|g| g.join(",")),
        ..Default::default()
    })
}

/// Parse `pacman -R --print --print-format '%n %v'`.
pub fn parse_pacman_print(target: &str, stdout: &str, stderr: &str, ok: bool) -> RemovalPlan {
    let mut names = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty()
            || t.starts_with("checking")
            || t.starts_with("error")
            || t.starts_with("warning")
            || t.starts_with("::")
        {
            continue;
        }
        let mut it = t.split_whitespace();
        if let (Some(n), v) = (it.next(), it.next()) {
            names.push((n.to_string(), v.map(String::from)));
        }
    }
    let mut plan = plan_from_names(target, names, true);
    if !ok || plan.packages.is_empty() {
        let msg: Vec<&str> = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .filter(|l| l.starts_with("error") || l.contains("breaks dependency") || l.contains("target not found"))
            .collect();
        plan.blocked = Some(if msg.is_empty() { "pacman refused the removal.".into() } else { msg.join("\n") });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = "%NAME%\nfirefox\n\n%VERSION%\n128.0-1\n\n%DESC%\nStandalone web browser from mozilla.org\n\n%URL%\nhttps://www.mozilla.org\n\n%ARCH%\nx86_64\n\n%BUILDDATE%\n1717000000\n\n%INSTALLDATE%\n1717100000\n\n%PACKAGER%\nArch Linux Pkg <pkg@archlinux.org>\n\n%SIZE%\n263000000\n\n%GROUPS%\nxfce4-goodies\n\n";
    const DEP: &str = "%NAME%\nlibfoo\n\n%VERSION%\n1.0-1\n\n%ARCH%\nx86_64\n\n%REASON%\n1\n\n%SIZE%\n1000\n\n";

    #[test]
    fn parses_desc() {
        let p = parse_desc(DESC).unwrap();
        assert_eq!(p.id, "firefox");
        assert_eq!(p.version, "128.0-1");
        assert_eq!(p.size, Some(263000000));
        assert_eq!(p.installed_on, Some(1717100000));
        assert_eq!(p.explicit, Some(true));
        assert_eq!(p.publisher.as_deref(), Some("Arch Linux Pkg"));
        assert_eq!(p.section.as_deref(), Some("xfce4-goodies"));
    }

    #[test]
    fn dependency_reason() {
        let p = parse_desc(DEP).unwrap();
        assert_eq!(p.explicit, Some(false));
        let mut p = p;
        p.has_binary = true;
        crate::services::classify::apply(&mut p);
        assert_eq!(p.kind, Kind::System);
    }

    #[test]
    fn files_detect_desktop_and_binary() {
        let mut p = parse_desc(DESC).unwrap();
        let mut idx = DesktopIndex::default();
        idx.insert(
            crate::services::desktop::parse_desktop(
                Path::new("/usr/share/applications/firefox.desktop"),
                "[Desktop Entry]\nName=Firefox\nType=Application\nIcon=firefox\nCategories=Network;\n",
            )
            .unwrap(),
        );
        scan_files("%FILES%\nusr/\nusr/bin/\nusr/bin/firefox\nusr/share/applications/firefox.desktop\n", &mut p, &idx);
        assert!(p.has_binary && p.has_desktop_entry);
        assert_eq!(p.name, "Firefox");
        assert_eq!(p.category.as_deref(), Some("Internet"));
    }

    #[test]
    fn preview_ok_and_blocked() {
        let plan = parse_pacman_print("vlc", "vlc 3.0.20-1\n", "", true);
        assert_eq!(plan.packages.len(), 1);
        let err = "checking dependencies...\nerror: failed to prepare transaction (could not satisfy dependencies)\n:: vlc-plugin: removing vlc breaks dependency 'vlc'\n";
        let plan = parse_pacman_print("vlc", "", err, false);
        assert!(plan.blocked.unwrap().contains("breaks dependency"));
    }

    #[test]
    fn command() {
        let p = Package { source: Some(Source::Pacman), id: "vlc".into(), ..Default::default() };
        assert_eq!(PacmanBackend.removal_command(&p).unwrap().args, vec!["-R", "--noconfirm", "--", "vlc"]);
    }
}
