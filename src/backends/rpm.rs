//! RPM based systems: DNF (Fedora, RHEL family) and Zypper (openSUSE).
//! Both read the same rpm database; they differ in how removal is planned
//! and executed.

use super::*;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Dnf,
    Zypper,
}

pub struct RpmBackend {
    flavor: Flavor,
}

const RPM_FORMAT: &str =
    "%{NAME}\\t%{VERSION}-%{RELEASE}\\t%{SIZE}\\t%{ARCH}\\t%{VENDOR}\\t%{INSTALLTIME}\\t%{SUMMARY}\\t%{GROUP}\\t%{PACKAGER}\\n";

impl RpmBackend {
    pub fn new(flavor: Flavor) -> Self {
        Self { flavor }
    }

    fn dnf_program() -> Option<&'static str> {
        ["dnf", "dnf5", "yum"].into_iter().find(|p| cmd::exists(p))
    }

    /// Specifier passed to the package manager: `name.arch`.
    fn spec_for(pkg: &Package) -> String {
        match pkg.arch.as_deref() {
            Some(a) if !a.is_empty() => format!("{}.{}", pkg.id, a),
            _ => pkg.id.clone(),
        }
    }
}

impl Backend for RpmBackend {
    fn source(&self) -> Source {
        match self.flavor {
            Flavor::Dnf => Source::Dnf,
            Flavor::Zypper => Source::Zypper,
        }
    }

    fn is_available(&self) -> bool {
        if !cmd::exists("rpm") {
            return false;
        }
        match self.flavor {
            Flavor::Zypper => cmd::exists("zypper"),
            // A system with zypper is handled by the Zypper backend.
            Flavor::Dnf => !cmd::exists("zypper") && Self::dnf_program().is_some(),
        }
    }

    fn list_installed(&self, desktop: &DesktopIndex) -> Result<Vec<Package>> {
        let out = cmd::run("rpm", &["-qa", "--qf", RPM_FORMAT])?;
        let mut pkgs = parse_rpm_qa(&out, self.source());
        link_desktop_entries(&mut pkgs, desktop);
        Ok(pkgs)
    }

    fn details(&self, pkg: &Package) -> Result<Package> {
        security::validate_package_id(&pkg.id)?;
        let spec = Self::spec_for(pkg);
        let mut p = pkg.clone();
        if let Ok(d) = cmd::run("rpm", &["-q", "--qf", "%{DESCRIPTION}", &spec]) {
            if let Some(d) = none_if_empty(&d) {
                p.description = Some(d.replace("\n\n", "\u{0}").replace('\n', " ").replace('\u{0}', "\n"));
            }
        }
        if let Ok(files) = cmd::run("rpm", &["-ql", &spec]) {
            p.location = files.lines().find(|l| crate::services::desktop::is_binary_path(l)).map(String::from);
        }
        Ok(p)
    }

    fn plan_removal(&self, pkg: &Package) -> Result<RemovalPlan> {
        security::validate_package_id(&pkg.id)?;
        let spec = Self::spec_for(pkg);
        match self.flavor {
            Flavor::Dnf => {
                let prog = Self::dnf_program().ok_or(Error::Unavailable("dnf"))?;
                let (ok, out, err) = cmd::run_allow_failure(
                    prog,
                    &["remove", "--assumeno", "--setopt=clean_requirements_on_remove=0", "--", &spec],
                )?;
                Ok(parse_dnf_remove(&pkg.id, &out, &err, ok))
            }
            Flavor::Zypper => {
                let (ok, out, err) =
                    cmd::run_allow_failure("zypper", &["--non-interactive", "remove", "--dry-run", "--", &spec])?;
                Ok(parse_zypper_remove(&pkg.id, &out, &err, ok))
            }
        }
    }

    fn removal_command(&self, pkg: &Package) -> Result<CommandSpec> {
        security::validate_package_id(&pkg.id)?;
        let spec = Self::spec_for(pkg);
        let c = match self.flavor {
            Flavor::Dnf => CommandSpec {
                program: Self::dnf_program().ok_or(Error::Unavailable("dnf"))?.into(),
                // Do not clean up now-unused dependencies automatically.
                args: vec![
                    "remove".into(),
                    "-y".into(),
                    "--setopt=clean_requirements_on_remove=0".into(),
                    "--".into(),
                    spec,
                ],
                env: vec![],
                privileged: true,
                cancellable: false,
            },
            Flavor::Zypper => CommandSpec {
                program: "zypper".into(),
                args: vec!["--non-interactive".into(), "remove".into(), "--".into(), spec],
                env: vec![],
                privileged: true,
                cancellable: false,
            },
        };
        Ok(c)
    }
}

/// Resolve which package owns each desktop entry and attach it.
fn link_desktop_entries(pkgs: &mut [Package], desktop: &DesktopIndex) {
    let paths: Vec<std::path::PathBuf> = crate::services::desktop::application_dirs()
        .into_iter()
        .filter(|d| d.starts_with("/usr"))
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "desktop"))
        .collect();
    let owners = std::sync::Mutex::new(Vec::<(std::path::PathBuf, String, String)>::new());
    let chunk = paths.len().div_ceil(8).max(1);
    std::thread::scope(|s| {
        for part in paths.chunks(chunk) {
            let owners = &owners;
            s.spawn(move || {
                for path in part {
                    let Some(ps) = path.to_str() else { continue };
                    let Ok((true, out, _)) =
                        cmd::run_allow_failure("rpm", &["-qf", "--qf", "%{NAME}\\t%{ARCH}\\n", ps])
                    else {
                        continue;
                    };
                    for l in out.lines() {
                        if let Some((n, a)) = l.split_once('\t') {
                            owners.lock().unwrap().push((path.clone(), n.to_string(), a.to_string()));
                        }
                    }
                }
            });
        }
    });
    let owners = owners.into_inner().unwrap();
    for p in pkgs.iter_mut() {
        let entries: Vec<_> = owners
            .iter()
            .filter(|(_, n, a)| *n == p.id && Some(a.as_str()) == p.arch.as_deref())
            .filter_map(|(path, _, _)| desktop.get(Path::new(path)))
            .collect();
        attach_desktop(p, &entries);
    }
}

/// Parse `rpm -qa --qf RPM_FORMAT`.
pub fn parse_rpm_qa(text: &str, source: Source) -> Vec<Package> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 9 || f[0] == "gpg-pubkey" {
            continue;
        }
        out.push(Package {
            source: Some(source),
            id: f[0].to_string(),
            name: f[0].to_string(),
            version: f[1].to_string(),
            size: f[2].trim().parse().ok(),
            arch: none_if_empty(f[3]),
            publisher: none_if_empty(f[4]).or_else(|| strip_email(f[8])),
            installed_on: f[5].trim().parse().ok(),
            description: none_if_empty(f[6]),
            section: none_if_empty(f[7]),
            ..Default::default()
        });
    }
    out
}

/// Parse `dnf remove --assumeno` (dnf4 and dnf5 layouts).
pub fn parse_dnf_remove(target: &str, stdout: &str, stderr: &str, _ok: bool) -> RemovalPlan {
    let mut names: Vec<(String, Option<String>)> = Vec::new();
    let mut in_section = false;
    let mut pending: Option<String> = None;
    for line in stdout.lines() {
        let t = line.trim();
        if t.starts_with("Transaction Summary") {
            break;
        }
        if t.starts_with("Removing") && t.ends_with(':') {
            in_section = true;
            pending = None;
            continue;
        }
        if !in_section {
            continue;
        }
        if t.is_empty() || t.starts_with('=') || t.starts_with("Upgrading") || t.starts_with("Installing") {
            if t.is_empty() {
                continue;
            }
            in_section = false;
            continue;
        }
        if !line.starts_with(' ') {
            // Any other un-indented header ends the section.
            in_section = false;
            continue;
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        match (toks.len(), pending.take()) {
            (1, _) => pending = Some(toks[0].to_string()),
            (_, Some(name)) => {
                // dnf4 wrapped line: name on the previous line.
                names.push((name, toks.get(1).map(|s| s.to_string())));
            }
            (n, None) if n >= 3 => {
                names.push((toks[0].to_string(), Some(toks[2].trim_start_matches("0:").to_string())))
            }
            _ => {}
        }
    }
    let mut plan = plan_from_names(target, names, true);
    // Items beyond the first target in dnf's "dependent"/"unused" sections are
    // all "other packages"; plan_from_names already marks them Dependent.
    let combined = format!("{stdout}\n{stderr}");
    if combined.contains("protected packages") || combined.contains("protected package") {
        plan.blocked = Some("DNF refuses this removal because it would remove protected system packages.".into());
    } else if plan.packages.is_empty() {
        let msg: Vec<&str> = combined
            .lines()
            .map(str::trim)
            .filter(|l| {
                l.starts_with("Error")
                    || l.starts_with("Problem")
                    || l.contains("No match")
                    || l.contains("not installed")
            })
            .collect();
        plan.blocked =
            Some(if msg.is_empty() { "The package manager reports nothing to remove.".into() } else { msg.join("\n") });
    }
    plan
}

/// Parse `zypper remove --dry-run`.
pub fn parse_zypper_remove(target: &str, stdout: &str, stderr: &str, ok: bool) -> RemovalPlan {
    let mut names = Vec::new();
    let mut in_list = false;
    for line in stdout.lines() {
        if line.contains("to be REMOVED:") {
            in_list = true;
            continue;
        }
        if in_list {
            if line.starts_with("  ") {
                names.extend(line.split_whitespace().map(|n| (n.to_string(), None)));
            } else {
                in_list = false;
            }
        }
    }
    let mut plan = plan_from_names(target, names, true);
    if plan.packages.is_empty() {
        let msg = format!("{stderr}\n{stdout}");
        if msg.contains("Root privileges") || msg.contains("root privileges") {
            // Preview not possible as a normal user: show the target only.
            plan.packages.push(PlannedPackage { name: target.to_string(), version: None, role: PlanRole::Target });
            plan.simulated = false;
            plan.warnings.push(
                "Zypper could not preview dependencies without administrator rights; other packages may be affected."
                    .into(),
            );
        } else {
            plan.blocked = Some(
                msg.lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && !l.starts_with("Loading") && !l.starts_with("Reading"))
                    .unwrap_or("zypper refused the removal")
                    .to_string(),
            );
            let _ = ok;
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fedora 40 style output.
    const FEDORA: &str = "firefox\t128.0-1.fc40\t266000000\tx86_64\tFedora Project\t1717000000\tMozilla Firefox Web browser\tUnspecified\tFedora Project\n\
glibc\t2.39-17.fc40\t6000000\tx86_64\tFedora Project\t1700000000\tThe GNU libc libraries\tSystem Environment/Libraries\tFedora Project\n\
gpg-pubkey\t105ef944\t0\t(none)\t(none)\t1700000000\tgpg(Fedora)\tPublic Keys\t(none)\n\
foo\t1.0-1\t100\tnoarch\t(none)\t1700000001\tA tool\t(none)\tJane Dev <jane@example.org>\n";

    // openSUSE Tumbleweed style output.
    const SUSE: &str = "MozillaFirefox\t128.0-1.1\t250000000\tx86_64\topenSUSE\t1717000000\tMozilla Firefox Web Browser\tProductivity/Networking/Web/Browsers\topenSUSE\n";

    #[test]
    fn parses_fedora() {
        let p = parse_rpm_qa(FEDORA, Source::Dnf);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].publisher.as_deref(), Some("Fedora Project"));
        assert_eq!(p[0].installed_on, Some(1717000000));
        assert_eq!(p[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(p[2].publisher.as_deref(), Some("Jane Dev"));
        assert_eq!(p[2].section, None);
    }

    #[test]
    fn parses_suse_and_classifies() {
        let mut p = parse_rpm_qa(SUSE, Source::Zypper);
        p[0].has_desktop_entry = true;
        crate::services::classify::apply(&mut p[0]);
        assert_eq!(p[0].kind, Kind::Application);
        let mut g = parse_rpm_qa(FEDORA, Source::Dnf);
        crate::services::classify::apply(&mut g[1]);
        assert_eq!(g[1].kind, Kind::System);
    }

    #[test]
    fn parses_dnf5_preview() {
        let out = "Package    Arch   Version      Repository   Size\nRemoving:\n vlc        x86_64 1:3.0.20-1   @updates    5.2 MiB\nRemoving dependent packages:\n vlc-plugins x86_64 1:3.0.20-1 @updates 1.0 MiB\n\nTransaction Summary:\n Removing:          2 packages\n";
        let plan = parse_dnf_remove("vlc", out, "Operation aborted.\n", false);
        assert_eq!(plan.packages.len(), 2);
        assert_eq!(plan.packages[0].name, "vlc");
        assert_eq!(plan.packages[0].role, PlanRole::Target);
        assert_eq!(plan.packages[1].name, "vlc-plugins");
        assert!(plan.blocked.is_none());
    }

    #[test]
    fn parses_dnf4_preview_with_wrapped_names() {
        let out = "Dependencies resolved.\n================================\n Package   Arch  Version  Repository  Size\n================================\nRemoving:\n gnome-calculator-extra-long-name\n           x86_64 46.1-1.fc40 @fedora 3.1 M\nRemoving dependent packages:\n foo  x86_64 1.0 @fedora 1 k\n\nTransaction Summary\n================================\nRemove  2 Packages\n";
        let plan = parse_dnf_remove("gnome-calculator-extra-long-name", out, "", false);
        assert_eq!(plan.packages.len(), 2);
        assert_eq!(plan.packages[0].role, PlanRole::Target);
        assert_eq!(plan.packages[1].name, "foo");
    }

    #[test]
    fn dnf_protected_is_blocked() {
        let err = "Problem: The operation would result in removing the following protected packages: systemd\n";
        let plan = parse_dnf_remove("systemd", "", err, false);
        assert!(plan.blocked.is_some());
    }

    #[test]
    fn dnf_unknown_package_is_blocked() {
        let plan = parse_dnf_remove(
            "nope",
            "",
            "No match for argument: nope\nError: No packages marked for removal.\n",
            false,
        );
        assert!(plan.blocked.unwrap().contains("No match"));
    }

    #[test]
    fn parses_zypper_preview() {
        let out = "Reading installed packages...\nResolving package dependencies...\n\nThe following 2 packages are going to be REMOVED:\n  vlc vlc-lang\n  libvlc5\n\n2 packages to remove.\n";
        let plan = parse_zypper_remove("vlc", out, "", true);
        assert_eq!(plan.packages.len(), 3);
        assert_eq!(plan.packages[0].role, PlanRole::Target);
    }

    #[test]
    fn zypper_without_root_falls_back_to_target_with_warning() {
        let plan = parse_zypper_remove("vlc", "", "Root privileges are required for this operation.\n", false);
        assert_eq!(plan.packages.len(), 1);
        assert!(!plan.warnings.is_empty());
        assert!(!plan.simulated);
    }

    #[test]
    fn commands() {
        let p = Package {
            source: Some(Source::Zypper),
            id: "vlc".into(),
            arch: Some("x86_64".into()),
            ..Default::default()
        };
        let c = RpmBackend::new(Flavor::Zypper).removal_command(&p).unwrap();
        assert_eq!(c.args, vec!["--non-interactive", "remove", "--", "vlc.x86_64"]);
        assert!(c.privileged);
    }
}
