//! Flatpak (system and per-user installations).
//!
//! Flatpak apps have no reverse dependencies; runtimes shared with other apps
//! are kept by `flatpak uninstall` and can be cleaned later with `--unused`.
//! The CLI offers no dry-run, so the preview is the single ref being removed.

use super::*;
use std::path::PathBuf;

pub struct FlatpakBackend;

const COLUMNS: &str = "application,name,description,version,branch,arch,origin,installation,size,options";

impl Backend for FlatpakBackend {
    fn source(&self) -> Source {
        Source::Flatpak
    }

    fn is_available(&self) -> bool {
        cmd::exists("flatpak")
    }

    fn list_installed(&self, desktop: &DesktopIndex) -> Result<Vec<Package>> {
        let out = cmd::run("flatpak", &["list", &format!("--columns={COLUMNS}")])?;
        let mut pkgs = parse_flatpak_list(&out);
        for p in &mut pkgs {
            let base = installation_base(p.scope.as_deref().unwrap_or("system"));
            let dir = base.join("app").join(&p.id).join(p.arch.as_deref().unwrap_or("")).join(branch_of(p));
            let dir = if dir.exists() {
                dir
            } else {
                base.join("runtime").join(&p.id).join(p.arch.as_deref().unwrap_or("")).join(branch_of(p))
            };
            if dir.exists() {
                p.location = Some(dir.to_string_lossy().into_owned());
                p.installed_on = std::fs::metadata(dir.join("active"))
                    .or_else(|_| std::fs::metadata(&dir))
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                if p.role == Role::App {
                    for name in [format!("{}.metainfo.xml", p.id), format!("{}.appdata.xml", p.id)] {
                        let f = dir.join("active/files/share/metainfo").join(name);
                        if let Ok(xml) = std::fs::read_to_string(f) {
                            if let Some(dev) = parse_developer(&xml) {
                                p.publisher = Some(dev);
                                break;
                            }
                        }
                    }
                }
            }
            if p.role == Role::App {
                let entries = desktop.by_id(&p.id);
                attach_desktop(p, &entries);
                if p.icon.is_none() {
                    p.icon = Some(p.id.clone());
                }
            }
        }
        Ok(pkgs)
    }

    fn details(&self, pkg: &Package) -> Result<Package> {
        Ok(pkg.clone())
    }

    fn is_user_facing(&self, pkg: &Package) -> bool {
        pkg.role == Role::App
    }

    fn plan_removal(&self, pkg: &Package) -> Result<RemovalPlan> {
        let r = reference(pkg)?;
        let mut plan = RemovalPlan { simulated: false, ..Default::default() };
        plan.packages.push(PlannedPackage {
            name: pkg.id.clone(),
            version: Some(branch_of(pkg).to_string()),
            role: PlanRole::Target,
        });
        plan.warnings.push(
            "Flatpak keeps runtimes that other apps may use. Your app data in ~/.var/app is kept; unused runtimes can be removed later with 'flatpak uninstall --unused'."
                .into(),
        );
        let _ = r;
        Ok(plan)
    }

    fn removal_command(&self, pkg: &Package) -> Result<CommandSpec> {
        let r = reference(pkg)?;
        let scope = match pkg.scope.as_deref() {
            Some("user") => "--user",
            _ => "--system",
        };
        Ok(CommandSpec {
            program: "flatpak".into(),
            // System installations authenticate through Flatpak's own polkit
            // helper, so the GUI never needs a root process for Flatpak.
            args: vec!["uninstall".into(), "-y".into(), "--noninteractive".into(), scope.into(), "--".into(), r],
            env: vec![],
            privileged: false,
            cancellable: true,
        })
    }
}

fn branch_of(p: &Package) -> &str {
    p.reference.as_deref().and_then(|r| r.rsplit('/').next()).unwrap_or("stable")
}

fn reference(p: &Package) -> Result<String> {
    security::validate_package_id(&p.id)?;
    let r = p.reference.clone().ok_or_else(|| Error::InvalidId(p.id.clone()))?;
    security::validate_flatpak_ref(&r)?;
    Ok(r)
}

fn installation_base(scope: &str) -> PathBuf {
    if scope == "user" {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".local/share/flatpak")
    } else {
        PathBuf::from("/var/lib/flatpak")
    }
}

/// Parse sizes such as "715.3 kB", "1.5 GB", "12 bytes" (SI units).
pub fn parse_flatpak_size(s: &str) -> Option<u64> {
    let s = s.trim().replace('\u{a0}', " ");
    let (num, unit) = s.split_once(' ')?;
    let v: f64 = num.replace(',', ".").parse().ok()?;
    let mult = match unit.trim().to_ascii_lowercase().as_str() {
        "bytes" | "byte" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => return None,
    };
    Some((v * mult) as u64)
}

/// Parse `flatpak list --columns=COLUMNS`.
pub fn parse_flatpak_list(text: &str) -> Vec<Package> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 10 {
            continue;
        }
        let options = f[9];
        let is_runtime = options.split(',').any(|o| o == "runtime");
        let scope = f[7].trim();
        let arch = f[5].trim();
        let branch = f[4].trim();
        let id = f[0].trim();
        if id.is_empty() {
            continue;
        }
        out.push(Package {
            source: Some(Source::Flatpak),
            id: id.to_string(),
            name: none_if_empty(f[1]).unwrap_or_else(|| id.to_string()),
            description: none_if_empty(f[2]),
            version: none_if_empty(f[3]).unwrap_or_else(|| branch.to_string()),
            arch: none_if_empty(arch),
            scope: none_if_empty(scope),
            size: parse_flatpak_size(f[8]),
            reference: Some(format!("{}/{}/{}/{}", if is_runtime { "runtime" } else { "app" }, id, arch, branch)),
            role: if is_runtime { Role::Component } else { Role::App },
            ..Default::default()
        });
    }
    out
}

/// Extract the developer name from AppStream metainfo XML.
pub fn parse_developer(xml: &str) -> Option<String> {
    fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
        let a = s.find(open)? + open.len();
        let b = s[a..].find(close)? + a;
        Some(&s[a..b])
    }
    let name = if let Some(dev_start) = xml.find("<developer ").or_else(|| xml.find("<developer>")) {
        let end = xml[dev_start..].find("</developer>").map(|e| e + dev_start).unwrap_or(xml.len());
        let block = &xml[dev_start..end];
        // First un-localised <name>.
        let mut found = None;
        let mut rest = block;
        while let Some(i) = rest.find("<name") {
            let tail = &rest[i..];
            if tail.starts_with("<name>") {
                found = between(tail, "<name>", "</name>");
                break;
            }
            rest = &rest[i + 5..];
        }
        found
    } else {
        between(xml, "<developer_name>", "</developer_name>")
    }?;
    none_if_empty(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "com.discordapp.Discord\tDiscord\tAll-in-one voice and text chat\t1.0.170\tstable\tx86_64\tflathub\tsystem\t597.8 MB\tsystem,current\n\
com.github.tchx84.Flatseal\tFlatseal\tManage Flatpak permissions\t2.4.1\tstable\tx86_64\tfedora\tsystem\t715.3 kB\tsystem,alt-id=47629aa0735a,current\n\
org.fedoraproject.Platform\tFedora Platform\tRuntime\t45\tf45\tx86_64\tfedora\tsystem\t1.5 GB\tsystem,alt-id=39ad4c934e95,runtime\n\
io.tangencylabs.solidcraft\tSolidCraft\t\t0.45.0\tstable\tx86_64\tsolidcraft-origin\tuser\t546.0 MB\tuser,current\n";

    #[test]
    fn parses_list() {
        let p = parse_flatpak_list(SAMPLE);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].id, "com.discordapp.Discord");
        assert_eq!(p[0].role, Role::App);
        assert_eq!(p[0].size, Some(597_800_000));
        assert_eq!(p[0].reference.as_deref(), Some("app/com.discordapp.Discord/x86_64/stable"));
        assert_eq!(p[1].size, Some(715_300));
        assert_eq!(p[2].role, Role::Component);
        assert_eq!(p[2].reference.as_deref(), Some("runtime/org.fedoraproject.Platform/x86_64/f45"));
        assert_eq!(p[3].scope.as_deref(), Some("user"));
        assert_eq!(p[3].description, None);
    }

    #[test]
    fn runtimes_hidden_apps_shown() {
        let mut p = parse_flatpak_list(SAMPLE);
        for x in &mut p {
            crate::services::classify::apply(x);
        }
        assert_eq!(p[0].kind, Kind::Application);
        assert_eq!(p[2].kind, Kind::System);
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_flatpak_size("12 bytes"), Some(12));
        assert_eq!(parse_flatpak_size("1.5 GB"), Some(1_500_000_000));
        assert_eq!(parse_flatpak_size("garbage"), None);
    }

    #[test]
    fn developer_parsing() {
        assert_eq!(
            parse_developer(
                "<component><developer id=\"com.discord\">\n<name>Discord Inc.</name>\n</developer></component>"
            )
            .as_deref(),
            Some("Discord Inc.")
        );
        assert_eq!(parse_developer("<developer_name>GNOME Project</developer_name>").as_deref(), Some("GNOME Project"));
        assert_eq!(parse_developer("<component></component>"), None);
    }

    #[test]
    fn removal_uses_scope_and_ref_and_no_root() {
        let p = parse_flatpak_list(SAMPLE);
        let c = FlatpakBackend.removal_command(&p[3]).unwrap();
        assert_eq!(
            c.args,
            vec!["uninstall", "-y", "--noninteractive", "--user", "--", "app/io.tangencylabs.solidcraft/x86_64/stable"]
        );
        assert!(!c.privileged);
        let mut bad = p[0].clone();
        bad.reference = Some("app/--evil/x86_64/stable".into());
        assert!(FlatpakBackend.removal_command(&bad).is_err());
    }
}
