//! Package inventory, metadata, and reviewed removal plans.
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;
use wait_timeout::ChildExt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Debian,
    Snap,
    Flatpak,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Debian => "Debian",
            Self::Snap => "Snap",
            Self::Flatpak => "Flatpak",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: Source,
    pub description: String,
    pub size: u64,
    pub protected: bool,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub scope: String,
    pub install_date: Option<String>,
    pub application: bool,
    /// Either an image path or an icon name from a desktop entry.
    pub icon: String,
}

impl Package {
    fn new(name: String, version: String, source: Source) -> Self {
        Self {
            name,
            version,
            source,
            description: String::new(),
            size: 0,
            protected: false,
            ref_name: String::new(),
            scope: String::new(),
            install_date: None,
            application: false,
            icon: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Inventory {
    pub packages: Vec<Package>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalPlan {
    pub names: Vec<String>,
    pub command: Vec<String>,
}

fn run(args: &[String], timeout: Option<Duration>) -> Result<String, String> {
    let Some(program) = args.first() else {
        return Err("No command was provided".to_owned());
    };

    let mut child = Command::new(program)
        .args(&args[1..])
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not run {program}: {error}"))?;

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));

    let status = match timeout {
        Some(limit) => match child
            .wait_timeout(limit)
            .map_err(|error| format!("Could not wait for {program}: {error}"))?
        {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "{program} did not finish within {} seconds",
                    limit.as_secs()
                ));
            }
        },
        None => child
            .wait()
            .map_err(|error| format!("Could not wait for {program}: {error}"))?,
    };

    let stdout = String::from_utf8_lossy(&stdout_reader.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default()).into_owned();
    if !status.success() {
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(if message.is_empty() {
            format!("{program} exited with status {}", status_code(status))
        } else {
            message.to_owned()
        });
    }
    Ok(stdout)
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = reader.read_to_end(&mut bytes);
    bytes
}

fn status_code(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
}

fn command(args: &[&str]) -> Vec<String> {
    args.iter().map(|part| (*part).to_owned()).collect()
}

fn executable_exists(name: &str) -> bool {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .any(|path| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::metadata(path).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
            }
            #[cfg(not(unix))]
            {
                path.is_file()
            }
        })
}

fn desktop_entry_values(path: &Path) -> HashMap<String, String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let mut values = HashMap::new();
    let mut in_entry = false;
    for line in contents.lines().map(str::trim) {
        if line == "[Desktop Entry]" {
            in_entry = true;
        } else if line.starts_with('[') && line.ends_with(']') {
            if in_entry {
                break;
            }
        } else if in_entry && !line.starts_with('#') {
            if let Some((key, value)) = line.split_once('=') {
                values
                    .entry(key.to_owned())
                    .or_insert_with(|| value.to_owned());
            }
        }
    }
    values
}

fn debian_desktop_info(output: &str) -> (HashSet<String>, HashMap<String, String>) {
    let mut packages = HashSet::new();
    let mut icons = HashMap::new();
    for line in output.lines() {
        let Some((owner, path)) = line.split_once(": ") else {
            continue;
        };
        if owner.is_empty() || !path.ends_with(".desktop") {
            continue;
        }
        let unqualified = owner.split(':').next().unwrap_or(owner);
        packages.insert(owner.to_owned());
        packages.insert(unqualified.to_owned());
        if let Some(icon) = desktop_entry_values(Path::new(path)).get("Icon") {
            if !icon.is_empty() {
                icons
                    .entry(owner.to_owned())
                    .or_insert_with(|| icon.clone());
                icons
                    .entry(unqualified.to_owned())
                    .or_insert_with(|| icon.clone());
            }
        }
    }
    (packages, icons)
}

fn parse_debian(
    output: &str,
    application_packages: &HashSet<String>,
    icons: &HashMap<String, String>,
) -> Vec<Package> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.splitn(8, '\t').collect::<Vec<_>>();
            if fields.len() != 8 || fields[1] != "installed" {
                return None;
            }
            let name = fields[0];
            let unqualified = name.split(':').next().unwrap_or(name);
            let mut package = Package::new(name.to_owned(), fields[2].to_owned(), Source::Debian);
            package.size = fields[3]
                .parse::<u64>()
                .unwrap_or_default()
                .saturating_mul(1024);
            package.protected = fields[4] == "yes" || fields[5] == "yes";
            package.description = fields[7].to_owned();
            package.application =
                application_packages.contains(name) || application_packages.contains(unqualified);
            package.icon = icons
                .get(name)
                .or_else(|| icons.get(unqualified))
                .cloned()
                .unwrap_or_default();
            Some(package)
        })
        .collect()
}

fn snap_desktop_icons(directory: &Path) -> HashMap<String, String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return HashMap::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "desktop")
        })
        .collect::<Vec<_>>();
    paths.sort();

    let mut icons = HashMap::new();
    for path in paths {
        let values = desktop_entry_values(&path);
        let snap_name = values
            .get("X-SnapInstanceName")
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .split('_')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            });
        if let Some(icon) = values.get("Icon").filter(|icon| !icon.is_empty()) {
            icons.entry(snap_name).or_insert_with(|| icon.clone());
        }
    }
    icons
}

fn flatpak_icon(ref_name: &str, scope: &str) -> String {
    let parts = ref_name.split('/').collect::<Vec<_>>();
    if parts.len() < 2 || parts[0] != "app" {
        return String::new();
    }
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("~/.local/share"));
    let roots = if scope == "user" {
        vec![data_home.join("flatpak/exports/share")]
    } else {
        vec![PathBuf::from("/var/lib/flatpak/exports/share")]
    };
    let suffixes = ["png", "svg", "webp"];
    let preferred_sizes = [
        "128x128", "scalable", "96x96", "64x64", "48x48", "32x32", "256x256",
    ];
    for root in roots {
        let hicolor = root.join("icons/hicolor");
        for size in preferred_sizes {
            for suffix in suffixes {
                let candidate = hicolor
                    .join(size)
                    .join("apps")
                    .join(format!("{}.{}", parts[1], suffix));
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
        let Ok(sizes) = fs::read_dir(&hicolor) else {
            continue;
        };
        let mut candidates = sizes
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("apps").join(format!("{}.*", parts[1])))
            .collect::<Vec<_>>();
        candidates.sort();
        for candidate in candidates {
            // The wildcard above is only a stem hint; enumerate that apps directory.
            let Some(parent) = candidate.parent() else {
                continue;
            };
            let Ok(entries) = fs::read_dir(parent) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let matching_stem = path.file_stem().is_some_and(|stem| stem == parts[1]);
                let supported = path
                    .extension()
                    .is_some_and(|ext| suffixes.contains(&ext.to_string_lossy().as_ref()));
                if matching_stem && supported && path.is_file() {
                    return path.to_string_lossy().into_owned();
                }
            }
        }
    }
    String::new()
}

fn icon_data_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        directories.push(PathBuf::from(data_home));
    } else if let Some(home) = env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share"));
    }
    for directory in env::var_os("XDG_DATA_DIRS")
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into())
        .to_string_lossy()
        .split(':')
    {
        if !directory.is_empty() {
            directories.push(PathBuf::from(directory));
        }
    }
    directories
}

fn resolve_icon_name(icon: &str, data_directories: &[PathBuf]) -> String {
    if icon.is_empty() || Path::new(icon).is_file() {
        return icon.to_owned();
    }
    let icon_path = Path::new(icon);
    let stem = icon_path
        .file_stem()
        .unwrap_or_else(|| icon_path.as_os_str())
        .to_string_lossy();
    let explicit_extension = icon_path
        .extension()
        .map(|value| value.to_string_lossy().into_owned());
    let extensions = ["png", "svg", "webp"];
    let sizes = [
        "scalable", "256x256", "128x128", "96x96", "64x64", "48x48", "32x32", "24x24", "16x16",
    ];
    let categories = ["apps", "places", "mimetypes", "actions"];

    for data_directory in data_directories {
        let mut candidates = Vec::new();
        candidates.push(data_directory.join("pixmaps"));
        let icons_directory = data_directory.join("icons");
        if let Ok(themes) = fs::read_dir(&icons_directory) {
            let mut themes = themes
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect::<Vec<_>>();
            themes.sort();
            for theme in themes {
                for size in sizes {
                    for category in categories {
                        candidates.push(theme.join(size).join(category));
                    }
                }
            }
        }
        for directory in candidates {
            let check_extensions = explicit_extension
                .as_deref()
                .map(|extension| vec![extension])
                .unwrap_or_else(|| extensions.to_vec());
            for extension in check_extensions {
                let candidate = directory.join(format!("{stem}.{extension}"));
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
    }
    icon.to_owned()
}

fn resolve_icons(icons: &mut HashMap<String, String>, data_directories: &[PathBuf]) {
    let mut cache = HashMap::new();
    for icon in icons.values_mut() {
        let original = icon.clone();
        *icon = cache
            .entry(original.clone())
            .or_insert_with(|| resolve_icon_name(&original, data_directories))
            .clone();
    }
}

fn snap_is_application(name: &str) -> bool {
    if [
        "bare",
        "core",
        "core16",
        "core18",
        "core20",
        "core22",
        "core24",
        "snapd",
        "gtk-common-themes",
    ]
    .contains(&name)
    {
        return false;
    }
    if name.starts_with("gnome-") {
        let parts = name
            .strip_prefix("gnome-")
            .unwrap_or_default()
            .split('-')
            .collect::<Vec<_>>();
        if parts.len() >= 2
            && parts
                .last()
                .is_some_and(|year| year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()))
            && parts[..parts.len() - 1]
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        {
            return false;
        }
    }
    if let Some(year) = name.strip_prefix("mesa-") {
        if year.len() == 4 && year.chars().all(|character| character.is_ascii_digit()) {
            return false;
        }
    }
    !name.starts_with("kde-frameworks-")
}

fn dpkg_log_paths(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("dpkg.log"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn valid_dpkg_datetime(value: &str) -> bool {
    let mut fields = value.split(' ');
    let (Some(date), Some(time), None) = (fields.next(), fields.next(), fields.next()) else {
        return false;
    };
    let date_parts = date.split('-').collect::<Vec<_>>();
    let time_parts = time.split(':').collect::<Vec<_>>();
    if date_parts.len() != 3 || time_parts.len() != 3 || date_parts[0].len() != 4 {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day), Ok(hour), Ok(minute), Ok(second)) = (
        date_parts[0].parse::<i32>(),
        date_parts[1].parse::<u32>(),
        date_parts[2].parse::<u32>(),
        time_parts[0].parse::<u32>(),
        time_parts[1].parse::<u32>(),
        time_parts[2].parse::<u32>(),
    ) else {
        return false;
    };
    let leap_year = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days_in_month).contains(&day) && hour < 24 && minute < 60 && second < 60
}

pub fn debian_install_dates(directory: &Path) -> HashMap<String, String> {
    let mut events: HashMap<String, (String, String)> = HashMap::new();
    for path in dpkg_log_paths(directory) {
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut contents = String::new();
        let read_result = if path.extension().is_some_and(|extension| extension == "gz") {
            GzDecoder::new(file).read_to_string(&mut contents)
        } else {
            let mut file = file;
            file.read_to_string(&mut contents)
        };
        if read_result.is_err() {
            continue;
        }
        for line in contents.lines() {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 6 || !["install", "remove", "purge"].contains(&fields[2]) {
                continue;
            }
            let date = format!("{} {}", fields[0], fields[1]);
            if !valid_dpkg_datetime(&date) {
                continue;
            }
            let name = fields[3].to_owned();
            let action = fields[2].to_owned();
            if events
                .get(&name)
                .is_none_or(|(current, _)| date.as_str() >= current.as_str())
            {
                events.insert(name, (date, action));
            }
        }
    }
    let mut dates = events
        .iter()
        .filter(|(_, (_, action))| action == "install")
        .map(|(name, (date, _))| (name.clone(), date.clone()))
        .collect::<HashMap<_, _>>();
    let mut variants: HashMap<String, Vec<String>> = HashMap::new();
    for name in events.keys() {
        variants
            .entry(name.split(':').next().unwrap_or(name).to_owned())
            .or_default()
            .push(name.clone());
    }
    for (unqualified, names) in variants {
        if names.len() == 1 {
            if let Some(date) = dates.get(&names[0]).cloned() {
                dates.entry(unqualified).or_insert(date);
            }
        }
    }
    dates
}

pub fn inventory() -> Inventory {
    let mut packages = Vec::new();
    let mut warnings = Vec::new();

    if executable_exists("dpkg-query") {
        let format = "${binary:Package}\t${db:Status-Status}\t${Version}\t${Installed-Size}\t${Essential}\t${Protected}\t${Section}\t${binary:Summary}\n";
        match run(
            &[
                "dpkg-query".to_owned(),
                "-W".to_owned(),
                format!("-f={format}"),
            ],
            Some(Duration::from_secs(60)),
        ) {
            Ok(rows) => {
                let launchers = run(
                    &command(&["dpkg-query", "-S", "/usr/share/applications/*.desktop"]),
                    Some(Duration::from_secs(60)),
                )
                .unwrap_or_default();
                let (application_packages, mut icons) = debian_desktop_info(&launchers);
                let data_directories = icon_data_directories();
                resolve_icons(&mut icons, &data_directories);
                packages.extend(parse_debian(&rows, &application_packages, &icons));
            }
            Err(error) => warnings.push(format!("Debian: {error}")),
        }
    }

    if executable_exists("snap") {
        let mut icons = snap_desktop_icons(Path::new("/var/lib/snapd/desktop/applications"));
        resolve_icons(&mut icons, &icon_data_directories());
        match run(&command(&["snap", "list"]), Some(Duration::from_secs(30))) {
            Ok(listing) => {
                for line in listing.lines().skip(1) {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    if fields.len() >= 2 {
                        let mut package =
                            Package::new(fields[0].to_owned(), fields[1].to_owned(), Source::Snap);
                        package.application = snap_is_application(fields[0]);
                        package.icon = icons.get(fields[0]).cloned().unwrap_or_default();
                        packages.push(package);
                    }
                }
            }
            Err(error) => warnings.push(format!("Snap: {error}")),
        }
    }

    if executable_exists("flatpak") {
        for scope in ["user", "system"] {
            match run(
                &command(&[
                    "flatpak",
                    "list",
                    &format!("--{scope}"),
                    "--columns=ref,name,version,description",
                ]),
                Some(Duration::from_secs(60)),
            ) {
                Ok(listing) => {
                    for line in listing.lines() {
                        let fields = line.split('\t').collect::<Vec<_>>();
                        if fields.len() < 3 {
                            continue;
                        }
                        let mut package = Package::new(
                            fields[1].to_owned(),
                            fields[2].to_owned(),
                            Source::Flatpak,
                        );
                        package.ref_name = fields[0].to_owned();
                        package.scope = scope.to_owned();
                        package.description = fields.get(3).copied().unwrap_or_default().to_owned();
                        package.application = package.ref_name.starts_with("app/");
                        package.icon = flatpak_icon(&package.ref_name, scope);
                        packages.push(package);
                    }
                }
                Err(error) => warnings.push(format!("Flatpak ({scope}): {error}")),
            }
        }
    }

    let dates = debian_install_dates(Path::new("/var/log"));
    for package in &mut packages {
        if package.source == Source::Debian {
            package.install_date = dates.get(&package.name).cloned();
        }
    }
    packages.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.source.label().cmp(right.source.label()))
            .then_with(|| left.scope.cmp(&right.scope))
    });

    Inventory { packages, warnings }
}

fn limit_lines(text: &str, limit: usize) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() <= limit {
        return text.to_owned();
    }
    format!(
        "{}\n… {} more lines",
        lines[..limit].join("\n"),
        lines.len() - limit
    )
}

pub fn package_details(package: &Package) -> Result<String, String> {
    let text = match package.source {
        Source::Debian => {
            let fields = "Package: ${binary:Package}\nVersion: ${Version}\nArchitecture: ${Architecture}\nStatus: ${db:Status-Status}\nSection: ${Section}\nInstalled size: ${Installed-Size} KiB\nEssential: ${Essential}\nProtected: ${Protected}\nPre-Depends: ${Pre-Depends}\nDepends: ${Depends}\nRecommends: ${Recommends}\nSuggests: ${Suggests}\nProvides: ${Provides}\nConflicts: ${Conflicts}\nBreaks: ${Breaks}\nReplaces: ${Replaces}\n";
            let metadata = run(
                &[
                    "dpkg-query".to_owned(),
                    "-W".to_owned(),
                    format!("-f={fields}"),
                    "--".to_owned(),
                    package.name.clone(),
                ],
                Some(Duration::from_secs(60)),
            )?;
            let reverse = run(
                &command(&["apt-cache", "rdepends", "--installed", &package.name]),
                Some(Duration::from_secs(60)),
            )
            .unwrap_or_else(|error| format!("Unavailable: {error}"));
            format!("{metadata}\nInstalled reverse dependencies:\n{reverse}")
        }
        Source::Flatpak => {
            let scope = format!("--{}", package.scope);
            let runtime = run(
                &command(&[
                    "flatpak",
                    "info",
                    &scope,
                    "--show-runtime",
                    &package.ref_name,
                ]),
                Some(Duration::from_secs(60)),
            )
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|error| format!("Unavailable: {error}"));
            let permissions = run(
                &command(&[
                    "flatpak",
                    "info",
                    &scope,
                    "--show-permissions",
                    &package.ref_name,
                ]),
                Some(Duration::from_secs(60)),
            )
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|error| format!("Unavailable: {error}"));
            format!(
                "Package: {}\nRef: {}\nVersion: {}\nInstallation: {}\n\nRuntime dependency:\n{}\n\nPermissions:\n{}",
                package.name,
                package.ref_name,
                package.version,
                package.scope,
                if runtime.is_empty() {
                    "(none)"
                } else {
                    &runtime
                },
                if permissions.is_empty() {
                    "(none reported)"
                } else {
                    &permissions
                },
            )
        }
        Source::Snap => {
            let manifest = Path::new("/snap")
                .join(&package.name)
                .join("current/meta/snap.yaml");
            let snap_metadata = match fs::read_to_string(manifest) {
                Ok(raw) => {
                    let wanted = [
                        "summary",
                        "base",
                        "confinement",
                        "grade",
                        "license",
                        "architectures",
                    ];
                    let values = raw
                        .lines()
                        .filter(|line| !line.chars().next().is_some_and(char::is_whitespace))
                        .filter_map(|line| line.split_once(':'))
                        .filter(|(key, value)| wanted.contains(key) && !value.trim().is_empty())
                        .map(|(key, value)| format!("{}: {}", title_case(key), value.trim()))
                        .collect::<Vec<_>>();
                    if values.is_empty() {
                        "(no fields found)".to_owned()
                    } else {
                        values.join("\n")
                    }
                }
                Err(_) => "unavailable".to_owned(),
            };
            let connections = run(
                &command(&["snap", "connections", &package.name]),
                Some(Duration::from_secs(60)),
            )
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|error| format!("Unavailable: {error}"));
            format!(
                "Package: {}\nVersion: {}\nSource: Snap\n\nSnap metadata:\n{}\n\nConnected interfaces:\n{}",
                package.name,
                package.version,
                snap_metadata,
                if connections.is_empty() {
                    "(none)"
                } else {
                    &connections
                },
            )
        }
    };
    Ok(limit_lines(&text, 80))
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn simulated_removals(output: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for line in output.lines() {
        if line.starts_with("Inst ") || line.starts_with("Conf ") {
            return Err("APT also proposes installing or configuring packages. Resolve pending package changes first.".to_owned());
        }
        if let Some(name) = line
            .strip_prefix("Remv ")
            .and_then(|line| line.split_whitespace().next())
        {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if "\\.^$|?*+()[]{}".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn removal_command(package: &Package) -> Result<Vec<String>, String> {
    match package.source {
        Source::Debian => Err("Debian removals require a reviewed dependency plan".to_owned()),
        Source::Snap => Ok(command(&[
            "pkexec",
            "/usr/bin/snap",
            "remove",
            &package.name,
        ])),
        Source::Flatpak => {
            let mut args = Vec::new();
            if package.scope == "system" {
                args.push("pkexec".to_owned());
            }
            args.extend(command(&[
                "/usr/bin/flatpak",
                "uninstall",
                "--noninteractive",
                &format!("--{}", package.scope),
                &package.ref_name,
            ]));
            Ok(args)
        }
    }
}

pub fn removal_plan(package: &Package, packages: &[Package]) -> Result<RemovalPlan, String> {
    if package.protected {
        return Err(
            "This is an essential or protected system package and cannot be removed here."
                .to_owned(),
        );
    }
    if package.source != Source::Debian {
        return Ok(RemovalPlan {
            names: vec![if package.ref_name.is_empty() {
                package.name.clone()
            } else {
                package.ref_name.clone()
            }],
            command: removal_command(package)?,
        });
    }

    let existing_orphans_output = run(
        &command(&["apt-get", "--simulate", "autoremove"]),
        Some(Duration::from_secs(60)),
    )?;
    let existing_orphans = simulated_removals(&existing_orphans_output)?;
    let mut args = vec!["apt-get".to_owned(), "--simulate".to_owned()];
    for name in existing_orphans {
        let base_name = name.split(':').next().unwrap_or(&name);
        args.push("-o".to_owned());
        args.push(format!(
            "APT::NeverAutoRemove::=^{}$",
            regex_escape(base_name)
        ));
    }
    args.extend(command(&["--autoremove", "remove", "--", &package.name]));
    let simulated = run(&args, Some(Duration::from_secs(60)))?;
    let names = simulated_removals(&simulated)?;
    if names.is_empty() {
        return Err("APT did not propose any removals. Refresh the package list.".to_owned());
    }
    let protected = packages
        .iter()
        .filter(|installed| installed.protected)
        .map(|installed| {
            installed
                .name
                .split(':')
                .next()
                .unwrap_or(&installed.name)
                .to_owned()
        })
        .collect::<HashSet<_>>();
    let blocked = names
        .iter()
        .filter(|name| protected.contains(name.split(':').next().unwrap_or(name)))
        .cloned()
        .collect::<Vec<_>>();
    if !blocked.is_empty() {
        return Err(format!(
            "Removal would affect essential or protected packages: {}",
            blocked.join(", ")
        ));
    }

    let mut actual = command(&["pkexec", "/usr/bin/apt-get", "--assume-yes"]);
    actual.extend(args.into_iter().skip(2));
    Ok(RemovalPlan {
        names,
        command: actual,
    })
}

pub fn remove(
    package: &Package,
    packages: &[Package],
    approved_plan: &RemovalPlan,
) -> Result<String, String> {
    if removal_plan(package, packages)? != *approved_plan {
        return Err("The removal plan changed. Refresh and review it again.".to_owned());
    }
    run(&approved_plan.command, None)
}
