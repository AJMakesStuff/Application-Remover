//! Decides whether a package is a user-facing application or a system
//! component, and whether removing it is dangerous.

use crate::models::{Kind, Package, Role};

/// Name fragments that identify libraries, devel/debug files, drivers etc.
fn name_is_support_package(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    // Debian style shared libraries: libfoo1, libfoo-2.0-0, libfoo2t64 ...
    let starts_lib = n.starts_with("lib");
    let ends_digitish = n.trim_end_matches("t64").chars().last().is_some_and(|c| c.is_ascii_digit());
    if starts_lib && ends_digitish {
        return Some("shared library");
    }
    const SUFFIXES: &[(&str, &str)] = &[
        ("-dev", "development files"),
        ("-devel", "development files"),
        ("-headers", "development files"),
        ("-static", "development files"),
        ("-dbg", "debug symbols"),
        ("-dbgsym", "debug symbols"),
        ("-debuginfo", "debug symbols"),
        ("-debugsource", "debug symbols"),
        ("-doc", "documentation"),
        ("-docs", "documentation"),
        ("-common", "shared files"),
        ("-data", "shared data"),
        ("-libs", "shared library"),
        ("-lib", "shared library"),
        ("-l10n", "localisation"),
        ("-langpack", "localisation"),
        ("-locale", "localisation"),
        ("-i18n", "localisation"),
        ("-dkms", "kernel driver"),
        ("-firmware", "firmware"),
        ("-fonts", "font"),
        ("-doc-html", "documentation"),
    ];
    for &(suf, why) in SUFFIXES {
        if n.ends_with(suf) {
            return Some(why);
        }
    }
    const PREFIXES: &[(&str, &str)] = &[
        ("python3-", "language library"),
        ("python-", "language library"),
        ("perl-", "language library"),
        ("lib32-", "library"),
        ("ruby-", "language library"),
        ("golang-", "language library"),
        ("node-", "language library"),
        ("fonts-", "font"),
        ("ttf-", "font"),
        ("otf-", "font"),
        ("xorg-x11-drv-", "display driver"),
        ("xserver-xorg-", "display driver"),
        ("xf86-", "display driver"),
        ("linux-image", "kernel"),
        ("linux-headers", "kernel"),
        ("linux-modules", "kernel"),
        ("linux-firmware", "firmware"),
        ("kernel", "kernel"),
        ("kmod", "kernel module"),
        ("akmod", "kernel module"),
        ("firmware-", "firmware"),
        ("gstreamer1.0-", "multimedia plugin"),
        ("gstreamer1-", "multimedia plugin"),
        ("hunspell-", "dictionary"),
        ("texlive-", "document component"),
    ];
    for &(pre, why) in PREFIXES {
        if n.starts_with(pre) {
            return Some(why);
        }
    }
    None
}

fn section_is_system(section: &str) -> Option<&'static str> {
    let s = section.to_ascii_lowercase();
    // dpkg sections may be prefixed with a component: "universe/libs"
    let s = s.rsplit('/').next().unwrap_or(&s).to_string();
    match s.as_str() {
        "libs" | "oldlibs" | "libdevel" => Some("library"),
        "kernel" => Some("kernel"),
        "metapackages" | "metapackage" => Some("meta-package"),
        "devel" | "debug" => Some("development files"),
        "python" | "perl" | "ruby" | "golang" | "rust" | "java" | "javascript" | "haskell" | "ocaml" | "lisp"
        | "php" | "interpreters" | "introspection" | "localization" | "fonts" | "doc" => Some("support package"),
        _ if s.starts_with("system environment/libraries")
            || s.starts_with("system environment/kernel")
            || s.starts_with("system environment/base")
            || s.starts_with("development/libraries")
            || s.starts_with("development/languages") =>
        {
            Some("system component")
        }
        _ => None,
    }
}

/// Classify a package. Pure function of the package's metadata.
pub fn classify(p: &Package) -> (Kind, Option<String>) {
    let sys = |why: &str| (Kind::System, Some(why.to_string()));
    match p.role {
        Role::Component => return sys("runtime or platform component"),
        Role::App => return (Kind::Application, None),
        Role::Unknown => {}
    }
    if p.essential {
        return sys("essential system package");
    }
    if let Some(prio) = p.priority.as_deref() {
        if matches!(prio, "required" | "important") {
            return sys("required by the system");
        }
    }
    if let Some(s) = p.section.as_deref().and_then(section_is_system) {
        return sys(s);
    }
    let by_name = name_is_support_package(&p.id);
    // Kernels, drivers and firmware stay system packages even if they ship a
    // desktop entry (e.g. driver control panels are separate packages).
    if let Some(why) = by_name {
        if matches!(why, "kernel" | "kernel module" | "firmware" | "kernel driver" | "display driver") {
            return sys(why);
        }
    }
    if p.has_desktop_entry {
        return (Kind::Application, None);
    }
    if let Some(why) = by_name {
        return sys(why);
    }
    // No desktop entry: a command-line tool the user explicitly installed is
    // still an application. Package managers that cannot tell stay hidden.
    if p.explicit == Some(true) && p.has_binary {
        return (Kind::Application, None);
    }
    sys("no desktop entry")
}

const CRITICAL_EXACT: &[&str] = &[
    "bash",
    "coreutils",
    "glibc",
    "libc6",
    "libc-bin",
    "systemd",
    "systemd-libs",
    "udev",
    "dbus",
    "dbus-broker",
    "dbus-daemon",
    "dbus-x11",
    "polkit",
    "policykit-1",
    "polkit-gnome",
    "sudo",
    "util-linux",
    "filesystem",
    "base",
    "shadow",
    "pam",
    "login",
    "dpkg",
    "apt",
    "apt-get",
    "rpm",
    "dnf",
    "dnf5",
    "yum",
    "pacman",
    "zypper",
    "flatpak",
    "snapd",
    "grub",
    "grub2",
    "grub-pc",
    "grub-efi",
    "shim",
    "dracut",
    "initramfs-tools",
    "networkmanager",
    "network-manager",
    "wpa_supplicant",
    "wpasupplicant",
    "openssh-server",
    "gdm",
    "gdm3",
    "sddm",
    "lightdm",
    "xorg-x11-server-xorg",
    "xserver-xorg",
    "xorg-server",
    "wayland",
    "mesa",
    "gnome-shell",
    "gnome-session",
    "plasma-desktop",
    "plasma-workspace",
    "kwin",
    "xfce4-session",
    "cinnamon",
    "mate-session-manager",
    "ubuntu-desktop",
    "ubuntu-minimal",
    "ubuntu-standard",
    "ubuntu-desktop-minimal",
    "linux-generic",
    "gtk4",
    "libgtk-4-1",
    "libadwaita",
    "libadwaita-1-0",
    "firewalld",
    "ufw",
    "selinux-policy",
    "apparmor",
    "plymouth",
    "pipewire",
    "wireplumber",
    "pulseaudio",
    "glib2",
    "libglib2.0-0",
    "python3",
    "python",
    "perl",
    "gcc-libs",
    "linux-firmware",
    "kernel",
    "kernel-core",
    "kernel-modules",
    "kernel-modules-core",
    "linux",
    "linux-lts",
    "linux-image-generic",
    "fedora-release",
    "fedora-release-common",
    "debian-archive-keyring",
    "ca-certificates",
    "tzdata",
];

const CRITICAL_PREFIXES: &[&str] = &[
    "linux-image",
    "linux-modules",
    "kernel-",
    "systemd-",
    "grub2-",
    "glibc-",
    "gnome-shell",
    "gnome-session",
    "plasma-desktop",
    "xorg-x11-server",
    "dbus-",
    "polkit-",
    "mesa-",
    "nvidia-driver",
    "pam-",
    "libpam",
    "network-manager",
    "NetworkManager",
];

/// Why removing this package is dangerous, if it is.
pub fn critical_reason(p: &Package) -> Option<String> {
    if p.essential {
        return Some("It is marked as essential by the package manager.".into());
    }
    if matches!(p.priority.as_deref(), Some("required")) {
        return Some("It is required for the system to operate.".into());
    }
    critical_name_reason(&p.id)
}

/// Check a bare package name, used for both targets and transaction entries.
pub fn critical_name_reason(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    let base = n.split(':').next().unwrap_or(&n);
    if CRITICAL_EXACT.iter().any(|c| c.eq_ignore_ascii_case(base))
        || CRITICAL_PREFIXES.iter().any(|c| base.starts_with(&c.to_ascii_lowercase()))
    {
        return Some(format!("'{name}' is a core part of the operating system or desktop."));
    }
    None
}

/// Fill in `kind`, `system_reason` and `critical_reason` on a package.
pub fn apply(p: &mut Package) {
    let (k, why) = classify(p);
    p.kind = k;
    p.system_reason = why;
    p.critical_reason = critical_reason(p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Source;

    fn pkg(id: &str) -> Package {
        Package { source: Some(Source::Apt), id: id.into(), name: id.into(), ..Default::default() }
    }

    #[test]
    fn desktop_entry_means_application() {
        let mut p = pkg("firefox");
        p.has_desktop_entry = true;
        assert_eq!(classify(&p).0, Kind::Application);
    }

    #[test]
    fn libreoffice_is_not_mistaken_for_a_library() {
        let mut p = pkg("libreoffice-writer");
        p.has_desktop_entry = true;
        assert_eq!(classify(&p).0, Kind::Application);
        let mut p = pkg("libreoffice");
        p.explicit = Some(true);
        p.has_binary = true;
        assert_eq!(classify(&p).0, Kind::Application);
    }

    #[test]
    fn libraries_and_dev_packages_are_system() {
        for n in [
            "libssl3",
            "libgtk-4-1",
            "libc6",
            "zlib1g-dev",
            "openssl-devel",
            "libfoo2t64",
            "python3-requests",
            "fonts-dejavu-core",
            "glibc-debuginfo",
        ] {
            assert_eq!(classify(&pkg(n)).0, Kind::System, "{n}");
        }
    }

    #[test]
    fn kernel_and_drivers_are_system_even_with_desktop_entry() {
        let mut p = pkg("linux-image-6.8.0-45-generic");
        p.has_desktop_entry = true;
        assert_eq!(classify(&p).0, Kind::System);
        let mut p = pkg("kmod-nvidia");
        p.has_desktop_entry = true;
        assert_eq!(classify(&p).0, Kind::System);
    }

    #[test]
    fn essential_and_priority_are_system() {
        let mut p = pkg("bash");
        p.essential = true;
        p.has_desktop_entry = true;
        assert_eq!(classify(&p).0, Kind::System);
        let mut p = pkg("something");
        p.priority = Some("required".into());
        assert_eq!(classify(&p).0, Kind::System);
    }

    #[test]
    fn explicit_cli_tools_without_desktop_entry_are_applications() {
        let mut p = pkg("htop");
        p.explicit = Some(true);
        p.has_binary = true;
        assert_eq!(classify(&p).0, Kind::Application);
        // Dependencies stay hidden.
        p.explicit = Some(false);
        assert_eq!(classify(&p).0, Kind::System);
        // Unknown install reason (rpm) stays hidden without a desktop entry.
        p.explicit = None;
        assert_eq!(classify(&p).0, Kind::System);
    }

    #[test]
    fn package_manager_roles_win() {
        let mut p = pkg("org.freedesktop.Platform");
        p.role = Role::Component;
        assert_eq!(classify(&p).0, Kind::System);
        let mut p = pkg("firefox");
        p.role = Role::App;
        assert_eq!(classify(&p).0, Kind::Application);
    }

    #[test]
    fn sections() {
        let mut p = pkg("foo");
        p.section = Some("universe/libs".into());
        assert_eq!(classify(&p).0, Kind::System);
        p.section = Some("web".into());
        p.has_desktop_entry = true;
        assert_eq!(classify(&p).0, Kind::Application);
    }

    #[test]
    fn critical() {
        assert!(critical_name_reason("systemd").is_some());
        assert!(critical_name_reason("linux-image-6.8.0-45-generic").is_some());
        assert!(critical_name_reason("gnome-shell").is_some());
        assert!(critical_name_reason("vlc").is_none());
        assert!(critical_name_reason("libreoffice").is_none());
        let mut p = pkg("vlc");
        p.essential = true;
        assert!(critical_reason(&p).is_some());
    }
}
