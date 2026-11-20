# Linux Application Remover

A **Programs and Features** style application remover for Linux, written in Rust with GTK4 and
libadwaita. It lists the applications you actually installed, previews what an uninstall will change,
and removes them with the package manager that owns them. There is no custom package manager: it
drives `apt-get`/`dpkg`, `dnf`/`rpm`, `pacman`, `zypper`, `flatpak` and `snapd`.

- Compact sortable table: Name (with icon), Publisher, Installed On, Size, Version, Source
- Instant search (name, publisher, package id, description) plus Source and Category filters
- **Show System Programs: OFF/ON** toggle; libraries, kernels, drivers and dependencies hidden by default
- Removal preview (what else would be removed), confirmation, Polkit authentication, live output
- Settings: theme, icons, show system by default, remember window size, confirm, refresh on launch

## Supported package managers

| Backend | Distros | Listing | Removal |
|---|---|---|---|
| APT | Debian, Ubuntu, Mint, Pop!_OS | `dpkg-query`, `/var/lib/apt/extended_states`, `.list` files | `apt-get remove` (preview: `apt-get -s`) |
| DNF | Fedora, RHEL family | `rpm -qa` | `dnf remove` (preview: `--assumeno`) |
| Zypper | openSUSE | `rpm -qa` | `zypper remove` (preview: `--dry-run`) |
| Pacman | Arch, Manjaro, EndeavourOS | `/var/lib/pacman/local` | `pacman -R` (preview: `-R --print`) |
| Flatpak | any | `flatpak list` | `flatpak uninstall` |
| Snap | Ubuntu and others | snapd REST API (`/run/snapd.socket`) | `snap remove` |

Backends are detected at runtime; missing ones are simply skipped. Packages from different sources
are never merged.

## How applications are told apart from system packages

`services/classify.rs` combines: a visible `.desktop` entry owned by the package (strongest signal),
the package manager's own role (Flatpak runtime vs app, Snap `type`), dpkg `Section`/`Priority`/`Essential`,
name patterns (`lib*N`, `-dev`, `-devel`, `-debuginfo`, `python3-*`, firmware, kernels, drivers…) and, where
known, whether the user installed it explicitly (APT manual mark, pacman install reason) together with a
binary in `/usr/bin`. A package without a desktop entry is **not** assumed to be a system component when the
user explicitly installed a command-line tool; and a package with a desktop entry is never assumed safe:
a separate *critical* check (`critical_reason`) flags kernels, systemd, the package managers, the desktop
shell and similar. Rpm-based systems do not record explicit installs, so command-line tools without
desktop entries only appear with the System toggle on.

## Safety model

- The GUI never runs as root. Privileged commands go through `pkexec` (Polkit) only when the backend says
  it is needed (Flatpak authenticates through its own Polkit helper; no root process is spawned for it).
- Every command is an argument vector; no shell string is ever built. Package ids are validated
  (`security::validate_package_id`, `validate_flatpak_ref`) and always follow a `--` separator.
- Nothing is removed without a confirmation dialog showing name, version, source and the packages the
  transaction will also remove. Critical transactions require ticking an extra acknowledgement. The
  "Confirm before uninstalling" setting can skip only the plain dialog for single-package, non-critical
  removals. If the package manager refuses the removal in its preview, the app refuses too.
- Package manager safeguards are never bypassed: no `--force`, no `--nodeps`, no `--purge`, no
  autoremove/`-s`/`--clean-deps`. Only the selected package (plus what the manager itself requires) goes.
- One transaction at a time (process-wide lock), and the window cannot be closed while one runs.
- Cancellation: only offered when it is safe (Flatpak). Interrupting apt/dnf/pacman/zypper/snap mid
  transaction can corrupt the package database, so you cancel by dismissing the authentication prompt.

## Building

Requires stable Rust (≥ 1.80), GTK 4.12+ and libadwaita 1.5+ development files.

```bash
# Debian / Ubuntu
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev
# Fedora
sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel
# Arch
sudo pacman -S base-devel gtk4 libadwaita
# openSUSE
sudo zypper install gcc pkg-config gtk4-devel libadwaita-devel
```

```bash
cargo run --release          # build and launch
cargo test                   # unit tests
cargo test --lib --no-default-features   # core logic only, no GTK needed
```

Developer aid: `cargo run --example scan --no-default-features [-- --system] [-- --preview <package-id>]`
scans the real system and prints what was detected (read-only; `--preview` only simulates a removal).
`LAR_SCREENSHOT=out.png [LAR_SYSTEM=1] [LAR_SELECT=n] linux-application-remover` renders the window to a PNG and exits.

Files: settings in `~/.config/linux-application-remover/settings.json`, package cache in
`~/.cache/linux-application-remover/packages.json` (shown immediately on launch, then refreshed).

## Testing

- Unit tests cover classification, parsers for dpkg, rpm, DNF4/DNF5, Zypper, pacman, Flatpak and snapd
  output, filtering/sorting, id validation, the transaction lock, and settings.
- `tests/integration/run.sh [debian|fedora|arch|opensuse|all]` scans a package installed **inside a
  disposable container** (podman or docker, repository mounted read-only) with the real backend and checks
  the removal *preview*. Automated tests never execute a destructive removal; never run one on your own machine.

## Packaging

DEB, RPM, AppImage and Flatpak for x86_64 and aarch64: see
[docs/PACKAGING.md](docs/PACKAGING.md). GitHub Actions builds and attaches them to releases when a `v*` tag is pushed.

## Keyboard

`Delete` uninstall · `F5`/`Ctrl+R` refresh · `Ctrl+F` search · `Ctrl+,` settings · double-click/Enter for details.

## License

GPL-3.0-or-later.
