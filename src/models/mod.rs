//! Shared data model: installed packages and removal transactions.

use serde::{Deserialize, Serialize};

/// Where a package was installed from. Packages from different sources are
/// never merged, even when they share a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Source {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Snap,
}

impl Source {
    pub const ALL: [Source; 6] =
        [Source::Apt, Source::Dnf, Source::Pacman, Source::Zypper, Source::Flatpak, Source::Snap];

    pub fn label(self) -> &'static str {
        match self {
            Source::Apt => "APT",
            Source::Dnf => "DNF",
            Source::Pacman => "Pacman",
            Source::Zypper => "Zypper",
            Source::Flatpak => "Flatpak",
            Source::Snap => "Snap",
        }
    }
}

/// Role reported by the package manager itself, when it knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Role {
    #[default]
    Unknown,
    /// The package manager says this is an application (Flatpak app, Snap app).
    App,
    /// Runtime, base, kernel, gadget, extension, etc.
    Component,
}

/// Result of classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Kind {
    /// A user-facing application, shown by default.
    Application,
    /// Library, driver, service, dependency or other system component.
    #[default]
    System,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Package {
    pub source: Option<Source>,
    /// Identifier handed to the package manager for removal (validated).
    pub id: String,
    /// Display name (desktop entry name when one exists, else package name).
    pub name: String,
    pub version: String,
    pub arch: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    /// Installed size in bytes.
    pub size: Option<u64>,
    /// Unix timestamp.
    pub installed_on: Option<i64>,
    pub location: Option<String>,
    /// Icon theme name or absolute path.
    pub icon: Option<String>,
    /// Main category derived from desktop entry categories.
    pub category: Option<String>,
    /// Flatpak installation ("system" / "user") or other scope information.
    pub scope: Option<String>,
    /// Full reference for sources that need one (Flatpak: `app/ID/arch/branch`).
    pub reference: Option<String>,
    /// Package-manager section/group (dpkg Section, rpm Group, pacman groups).
    pub section: Option<String>,
    pub priority: Option<String>,
    pub essential: bool,
    /// `Some(true)` when explicitly installed by the user, `Some(false)` when
    /// installed only as a dependency, `None` when unknown.
    pub explicit: Option<bool>,
    pub has_desktop_entry: bool,
    pub has_binary: bool,
    pub role: Role,
    pub kind: Kind,
    pub system_reason: Option<String>,
    /// Set when removal needs extra caution.
    pub critical_reason: Option<String>,
}

impl Package {
    pub fn source(&self) -> Source {
        self.source.expect("package without source")
    }

    /// Unique key; two packages with the same key are the same installation.
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.source.map(Source::label).unwrap_or("?"),
            self.id,
            self.arch.as_deref().unwrap_or(""),
            self.scope.as_deref().unwrap_or(""),
            self.reference.as_deref().unwrap_or("")
        )
    }

    pub fn is_system(&self) -> bool {
        self.kind == Kind::System
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanRole {
    /// The package the user asked to remove.
    Target,
    /// Removed because it depends on the target.
    Dependent,
    /// Removed because it becomes unused.
    Unused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPackage {
    pub name: String,
    pub version: Option<String>,
    pub role: PlanRole,
}

/// Preview of what a removal would do.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemovalPlan {
    pub packages: Vec<PlannedPackage>,
    pub warnings: Vec<String>,
    /// Package manager refuses the removal (e.g. pacman dependency breakage).
    pub blocked: Option<String>,
    /// Whether the preview came from the package manager's own simulation.
    pub simulated: bool,
}

impl RemovalPlan {
    pub fn extra_packages(&self) -> impl Iterator<Item = &PlannedPackage> {
        self.packages.iter().filter(|p| p.role != PlanRole::Target)
    }
}

/// A fully specified process invocation (never a shell string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Needs administrator rights, obtained through Polkit.
    pub privileged: bool,
    /// Safe to interrupt midway.
    pub cancellable: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} is not available on this system")]
    Unavailable(&'static str),
    #[error("failed to run {program}: {message}")]
    Spawn { program: String, message: String },
    #[error("{program} failed: {message}")]
    Command { program: String, message: String },
    #[error("invalid package identifier: {0}")]
    InvalidId(String),
    #[error("{0}")]
    Parse(String),
    #[error("another package transaction is already running")]
    Busy,
    #[error("authentication was cancelled or denied")]
    AuthDenied,
    #[error("the operation was cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, UNITS[i])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(10), "10 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(300 * 1024 * 1024), "300 MB");
    }

    #[test]
    fn keys_do_not_merge_sources() {
        let a = Package { source: Some(Source::Apt), id: "firefox".into(), ..Default::default() };
        let b = Package { source: Some(Source::Flatpak), id: "firefox".into(), ..Default::default() };
        assert_ne!(a.key(), b.key());
    }
}
