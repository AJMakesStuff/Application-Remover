//! Package-manager backends. Each backend wraps a native tool or database;
//! there is no custom package manager here.

pub mod apt;
pub mod cmd;
pub mod flatpak;
pub mod pacman;
pub mod rpm;
pub mod snap;

use crate::models::*;
use crate::security;
use crate::services::classify;
use crate::services::desktop::DesktopIndex;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Interface every package manager implements. Adding a new package manager
/// means implementing this trait and registering it in [`all_backends`].
pub trait Backend: Send + Sync {
    fn source(&self) -> Source;

    /// Detect whether the package manager is present on this system.
    fn is_available(&self) -> bool;

    /// List installed packages, linked to desktop entries where possible.
    /// Packages are returned unclassified; see [`Backend::is_user_facing`].
    fn list_installed(&self, desktop: &DesktopIndex) -> Result<Vec<Package>>;

    /// Fetch extra metadata (long description, location) for one package.
    /// May be slow; call from a worker thread.
    fn details(&self, pkg: &Package) -> Result<Package>;

    /// Whether the package is a user-facing application.
    fn is_user_facing(&self, pkg: &Package) -> bool {
        classify::classify(pkg).0 == Kind::Application
    }

    /// Preview what removing `pkg` would do, without modifying the system.
    fn plan_removal(&self, pkg: &Package) -> Result<RemovalPlan>;

    /// The exact process to run to remove `pkg`.
    fn removal_command(&self, pkg: &Package) -> Result<CommandSpec>;

    /// Remove the package, streaming output lines to `sink`.
    fn remove(&self, pkg: &Package, cancel: &Arc<AtomicBool>, sink: &mut dyn FnMut(&str)) -> Result<()> {
        security::validate_package_id(&pkg.id)?;
        let spec = self.removal_command(pkg)?;
        cmd::run_streaming(&spec, cancel, sink)
    }
}

/// Every backend this build knows about (available or not).
pub fn all_backends() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(apt::AptBackend),
        Box::new(rpm::RpmBackend::new(rpm::Flavor::Dnf)),
        Box::new(rpm::RpmBackend::new(rpm::Flavor::Zypper)),
        Box::new(pacman::PacmanBackend),
        Box::new(flatpak::FlatpakBackend),
        Box::new(snap::SnapBackend),
    ]
}

/// Backends whose tools are installed here.
pub fn available_backends() -> Vec<Box<dyn Backend>> {
    all_backends().into_iter().filter(|b| b.is_available()).collect()
}

pub fn backend_for(source: Source) -> Option<Box<dyn Backend>> {
    all_backends().into_iter().find(|b| b.source() == source && b.is_available())
}

/// Strip an email address from "Name <email>" style maintainer fields.
pub(crate) fn strip_email(s: &str) -> Option<String> {
    let s = s.split('<').next().unwrap_or(s).trim();
    let s = s.trim_matches(|c| c == '"' || c == '\'');
    if s.is_empty() || s == "(none)" || s.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(s.to_string())
    }
}

pub(crate) fn none_if_empty(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s == "(none)" || s == "-" {
        None
    } else {
        Some(s.to_string())
    }
}

/// Fill desktop-entry derived fields on a package given the owned entry paths.
pub(crate) fn attach_desktop(pkg: &mut Package, entries: &[&crate::services::desktop::DesktopEntry]) {
    let visible: Vec<_> = entries.iter().filter(|e| !e.hidden).collect();
    if visible.is_empty() {
        return;
    }
    pkg.has_desktop_entry = true;
    // Prefer an entry whose id matches the package id, else the first one.
    let best = visible.iter().find(|e| e.id.eq_ignore_ascii_case(&pkg.id)).or_else(|| {
        if visible.len() == 1 {
            visible.first()
        } else {
            None
        }
    });
    if let Some(e) = best {
        pkg.name = e.name.clone();
        if pkg.description.is_none() {
            pkg.description = e.comment.clone();
        }
    }
    let icon_src = best.or_else(|| visible.first());
    if let Some(e) = icon_src {
        pkg.icon = e.icon.clone();
    }
    for e in &visible {
        if let Some(c) = crate::services::desktop::main_category(&e.categories) {
            pkg.category = Some(c);
            break;
        }
    }
}

/// Parse a "Removing"-style plan from a list of names, marking the target.
pub(crate) fn plan_from_names(target: &str, names: Vec<(String, Option<String>)>, simulated: bool) -> RemovalPlan {
    let mut plan = RemovalPlan { simulated, ..Default::default() };
    let mut seen_target = false;
    for (name, version) in names {
        let is_target = !seen_target && (name == target || name.split(':').next() == target.split(':').next());
        if is_target {
            seen_target = true;
        }
        plan.packages.push(PlannedPackage {
            name,
            version,
            role: if is_target { PlanRole::Target } else { PlanRole::Dependent },
        });
    }
    plan
}
