//! APT / dpkg (Debian, Ubuntu, Linux Mint, Pop!_OS, ...).

use super::*;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub struct AptBackend;

const DPKG_FORMAT: &str = "${binary:Package}\\t${Package}\\t${Version}\\t${Installed-Size}\\t${Architecture}\\t${Maintainer}\\t${Section}\\t${Priority}\\t${Essential}\\t${db:Status-Abbrev}\\t${binary:Summary}\\n";

impl Backend for AptBackend {
    fn source(&self) -> Source {
        Source::Apt
    }

    fn is_available(&self) -> bool {
        cmd::exists("dpkg-query") && cmd::exists("apt-get")
    }

    fn list_installed(&self, desktop: &DesktopIndex) -> Result<Vec<Package>> {
        let out = cmd::run("dpkg-query", &["-W", "-f", DPKG_FORMAT])?;
        let mut pkgs = parse_dpkg_query(&out);
        let auto =
            fs::read_to_string("/var/lib/apt/extended_states").map(|t| parse_extended_states(&t)).unwrap_or_default();
        let have_states = !auto.is_empty();
        for p in &mut pkgs {
            if have_states {
                let name = p.id.split(':').next().unwrap_or(&p.id);
                let arch = p.arch.clone().unwrap_or_default();
                let auto_installed =
                    auto.contains(&(name.to_string(), arch)) || auto.contains(&(p.id.clone(), String::new()));
                p.explicit = Some(!auto_installed);
            }
            let list = Path::new("/var/lib/dpkg/info").join(format!("{}.list", p.id));
            let list = if list.exists() {
                list
            } else {
                // Native-arch packages are listed without the arch suffix.
                Path::new("/var/lib/dpkg/info").join(format!("{}.list", p.id.split(':').next().unwrap_or(&p.id)))
            };
            if let Ok(meta) = fs::metadata(&list) {
                p.installed_on = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
            }
            if let Ok(text) = fs::read_to_string(&list) {
                scan_file_list(&text, p, desktop);
            }
        }
        Ok(pkgs)
    }

    fn details(&self, pkg: &Package) -> Result<Package> {
        security::validate_package_id(&pkg.id)?;
        let mut p = pkg.clone();
        if let Ok(d) = cmd::run("dpkg-query", &["-W", "-f", "${Description}", &pkg.id]) {
            if let Some(d) = none_if_empty(&d) {
                p.description = Some(clean_description(&d));
            }
        }
        let list =
            Path::new("/var/lib/dpkg/info").join(format!("{}.list", pkg.id.split(':').next().unwrap_or(&pkg.id)));
        if let Ok(text) = fs::read_to_string(&list) {
            p.location = text
                .lines()
                .find(|l| crate::services::desktop::is_binary_path(l))
                .map(String::from)
                .or_else(|| text.lines().find(|l| l.starts_with("/opt/")).map(String::from));
        }
        Ok(p)
    }

    fn plan_removal(&self, pkg: &Package) -> Result<RemovalPlan> {
        security::validate_package_id(&pkg.id)?;
        // `-s` simulates; no root and no changes.
        let (ok, out, err) = cmd::run_allow_failure("apt-get", &["-s", "remove", "--", &pkg.id])?;
        Ok(parse_apt_simulation(&pkg.id, &out, &err, ok))
    }

    fn removal_command(&self, pkg: &Package) -> Result<CommandSpec> {
        security::validate_package_id(&pkg.id)?;
        Ok(CommandSpec {
            program: "apt-get".into(),
            // No --purge, no autoremove: only what the user asked for.
            args: vec!["remove".into(), "-y".into(), "--".into(), pkg.id.clone()],
            env: vec![("DEBIAN_FRONTEND".into(), "noninteractive".into()), ("LC_ALL".into(), "C".into())],
            privileged: true,
            cancellable: false,
        })
    }
}

fn clean_description(d: &str) -> String {
    d.lines().map(|l| l.trim()).map(|l| if l == "." { "" } else { l }).collect::<Vec<_>>().join("\n").trim().to_string()
}

fn scan_file_list(text: &str, p: &mut Package, desktop: &DesktopIndex) {
    let mut entries = Vec::new();
    for line in text.lines() {
        if crate::services::desktop::is_desktop_file_path(line) {
            if let Some(e) = desktop.get(Path::new(line)) {
                entries.push(e);
            }
        } else if !p.has_binary && crate::services::desktop::is_binary_path(line) {
            p.has_binary = true;
        }
    }
    attach_desktop(p, &entries);
}

/// Parse `dpkg-query -W -f DPKG_FORMAT` output.
pub fn parse_dpkg_query(text: &str) -> Vec<Package> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 11 {
            continue;
        }
        // Only fully installed packages ("ii"); skip config-files, half-installed...
        if !f[9].starts_with("ii") {
            continue;
        }
        let size_kib: Option<u64> = f[3].trim().parse().ok();
        out.push(Package {
            source: Some(Source::Apt),
            id: f[0].to_string(),
            name: f[1].to_string(),
            version: f[2].to_string(),
            size: size_kib.map(|k| k * 1024),
            arch: none_if_empty(f[4]),
            publisher: strip_email(f[5]),
            section: none_if_empty(f[6]),
            priority: none_if_empty(f[7]),
            essential: f[8].trim().eq_ignore_ascii_case("yes"),
            description: none_if_empty(f[10]),
            ..Default::default()
        });
    }
    out
}

/// Parse /var/lib/apt/extended_states: returns (name, arch) pairs of
/// automatically installed packages.
pub fn parse_extended_states(text: &str) -> HashSet<(String, String)> {
    let mut set = HashSet::new();
    for stanza in text.split("\n\n") {
        let mut name = None;
        let mut arch = String::new();
        let mut auto = false;
        for l in stanza.lines() {
            if let Some(v) = l.strip_prefix("Package: ") {
                name = Some(v.trim().to_string());
            } else if let Some(v) = l.strip_prefix("Architecture: ") {
                arch = v.trim().to_string();
            } else if let Some(v) = l.strip_prefix("Auto-Installed: ") {
                auto = v.trim() == "1";
            }
        }
        if let (Some(n), true) = (name, auto) {
            set.insert((n, arch));
        }
    }
    set
}

/// Parse `apt-get -s remove` output.
pub fn parse_apt_simulation(target: &str, stdout: &str, stderr: &str, ok: bool) -> RemovalPlan {
    let mut names = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Remv ") {
            let mut it = rest.split_whitespace();
            if let Some(name) = it.next() {
                let ver = it.next().map(|v| v.trim_matches(|c| c == '[' || c == ']').to_string());
                names.push((name.to_string(), ver));
            }
        }
    }
    let mut plan = plan_from_names(target, names, true);
    if !ok {
        let msg: Vec<&str> = stderr.lines().chain(stdout.lines()).filter(|l| l.starts_with("E:")).collect();
        plan.blocked = Some(if msg.is_empty() { "apt refused the removal".into() } else { msg.join("\n") });
    }
    if plan.packages.is_empty() && plan.blocked.is_none() {
        plan.blocked = Some("The package manager reports nothing to remove (is it still installed?).".into());
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    // Representative dpkg-query output (Ubuntu 24.04 / Debian 12).
    const SAMPLE: &str = "firefox\tfirefox\t128.0+build1-0ubuntu1\t263000\tamd64\tUbuntu Mozilla Team <ubuntu-mozillateam@lists.ubuntu.com>\tweb\toptional\t\tii \tSafe and easy web browser from Mozilla\n\
libc6:amd64\tlibc6\t2.39-0ubuntu8\t13300\tamd64\tUbuntu Developers <ubuntu-devel-discuss@lists.ubuntu.com>\tlibs\trequired\t\tii \tGNU C Library: Shared libraries\n\
bash\tbash\t5.2.21-2ubuntu4\t1800\tamd64\tUbuntu Developers <ubuntu-devel-discuss@lists.ubuntu.com>\tshells\trequired\tyes\tii \tGNU Bourne Again SHell\n\
oldpkg\toldpkg\t1.0\t10\tall\tSomeone\tmisc\toptional\t\trc \tRemoved but configured\n\
htop\thtop\t3.3.0-4\t420\tamd64\tDaniel Lange <dlange@debian.org>\tutils\toptional\t\tii \tinteractive processes viewer\n";

    #[test]
    fn parses_dpkg_query() {
        let p = parse_dpkg_query(SAMPLE);
        assert_eq!(p.len(), 4, "rc package skipped");
        assert_eq!(p[0].id, "firefox");
        assert_eq!(p[0].size, Some(263000 * 1024));
        assert_eq!(p[0].publisher.as_deref(), Some("Ubuntu Mozilla Team"));
        assert_eq!(p[1].id, "libc6:amd64");
        assert_eq!(p[1].name, "libc6");
        assert!(p[2].essential);
        assert_eq!(p[1].priority.as_deref(), Some("required"));
    }

    #[test]
    fn classification_of_sample() {
        let mut p = parse_dpkg_query(SAMPLE);
        for x in &mut p {
            if x.id == "firefox" {
                x.has_desktop_entry = true;
            }
            if x.id == "htop" {
                x.has_binary = true;
                x.explicit = Some(true);
            }
            crate::services::classify::apply(x);
        }
        let kinds: Vec<_> = p.iter().map(|x| (x.id.as_str(), x.kind)).collect();
        assert_eq!(kinds[0], ("firefox", Kind::Application));
        assert_eq!(kinds[1].1, Kind::System);
        assert_eq!(kinds[2].1, Kind::System);
        assert_eq!(kinds[3], ("htop", Kind::Application));
    }

    #[test]
    fn extended_states() {
        let t = "Package: libfoo1\nArchitecture: amd64\nAuto-Installed: 1\n\nPackage: htop\nArchitecture: amd64\nAuto-Installed: 0\n\nPackage: bar\nArchitecture: amd64\nAuto-Installed: 1\n";
        let s = parse_extended_states(t);
        assert!(s.contains(&("libfoo1".to_string(), "amd64".to_string())));
        assert!(!s.contains(&("htop".to_string(), "amd64".to_string())));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn parses_simulation() {
        let out = "Reading package lists...\nBuilding dependency tree...\nThe following packages will be REMOVED:\n  vlc vlc-plugin-base\n0 upgraded, 0 newly installed, 2 to remove and 0 not upgraded.\nRemv vlc [3.0.20-1]\nRemv vlc-plugin-base [3.0.20-1]\n";
        let plan = parse_apt_simulation("vlc", out, "", true);
        assert_eq!(plan.packages.len(), 2);
        assert_eq!(plan.packages[0].role, PlanRole::Target);
        assert_eq!(plan.packages[1].role, PlanRole::Dependent);
        assert_eq!(plan.packages[1].version.as_deref(), Some("3.0.20-1"));
        assert!(plan.blocked.is_none());
    }

    #[test]
    fn simulation_error_blocks() {
        let plan = parse_apt_simulation("bash", "", "E: Unable to locate package bash\n", false);
        assert!(plan.blocked.unwrap().contains("Unable to locate"));
    }

    #[test]
    fn removal_command_is_structured_and_safe() {
        let p = Package { source: Some(Source::Apt), id: "vlc".into(), ..Default::default() };
        let c = AptBackend.removal_command(&p).unwrap();
        assert_eq!(c.args, vec!["remove", "-y", "--", "vlc"]);
        assert!(c.privileged);
        let bad = Package { source: Some(Source::Apt), id: "--purge".into(), ..Default::default() };
        assert!(AptBackend.removal_command(&bad).is_err());
    }
}
