# Packaging

All artifacts are produced by `.github/workflows/release.yml` on a `v*` tag and attached to the GitHub
Release together with `SHA256SUMS`. Build locally with the commands below.

| Format | Tool | x86_64 | aarch64 |
|---|---|---|---|
| `.deb` | `cargo-deb` (metadata in `Cargo.toml`) | yes | yes (ubuntu-24.04-arm) |
| `.rpm` | `cargo-generate-rpm` in a Fedora container | yes | yes |
| `.AppImage` | `packaging/appimage/build.sh` (linuxdeploy + GTK plugin) | yes | yes |
| Flatpak | `packaging/flatpak/*.yml` with `flatpak-builder` | yes | yes |

Shared files: `packaging/io.github.linux_application_remover.desktop`, `.metainfo.xml` (AppStream),
`packaging/icons/*.svg`. Validate them with `desktop-file-validate` and `appstreamcli validate --no-net`.

## Runtime requirements

GTK ≥ 4.12, libadwaita ≥ 1.5 (Ubuntu 24.04, Debian 13, Fedora 40+, Arch, openSUSE Tumbleweed or newer), and
**Polkit** (`pkexec`) for administrator authentication. The package manager tools themselves are optional
and detected at runtime. Package dependency declarations:

- deb: `$auto, libgtk-4-1 (>= 4.10), libadwaita-1-0 (>= 1.5), policykit-1 | pkexec`
- rpm: `gtk4`, `libadwaita`, `polkit`

Because the app is linked against the build machine's GTK, build `.deb`/`.AppImage` on the oldest distribution
you want to support (CI uses Ubuntu 24.04).

## Commands

```bash
cargo install cargo-deb && cargo build --release && cargo deb --no-build        # target/debian/*.deb
cargo install cargo-generate-rpm && cargo build --release && cargo generate-rpm # target/generate-rpm/*.rpm
VERSION=0.1.0 packaging/appimage/build.sh                                       # dist/*.AppImage
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/io.github.linux_application_remover.yml
```

There are no install/uninstall scripts: package managers place and remove the files listed above, and the
app writes only to `~/.config/linux-application-remover` and `~/.cache/linux-application-remover`
(delete them to fully clean up). Update `rust-version`, the Flatpak runtime version and the AppStream release
entry when cutting a new release.

## Flatpak and host package management

A sandboxed Flatpak application **cannot** uninstall host packages: it has no access to the host's package
databases, and `pkexec`, `apt`, `dnf`… do not exist inside the sandbox. The only ways out are
`--talk-name=org.freedesktop.Flatpak` (lets the app run `flatpak-spawn --host <command>`, which is equivalent
to unrestricted access to the user's session and system) or `--filesystem=host`. The provided manifest uses the
first, plus read-only access to `/run/host` (via `host-os:ro`), `/var/lib/{dpkg,apt,pacman,flatpak}`,
the user's Flatpak data and the snapd socket so the backends can read the host's databases and desktop entries.
When the app detects `/.flatpak-info` it routes every command through `flatpak-spawn --host`
(`security::build_argv`, `backends::cmd`).

Consequences: this permission set will not pass Flathub review, so distribute the Flatpak as a side-loaded
bundle from GitHub Releases. Behaviour inside the sandbox is best effort and less tested than the native
packages; for general use prefer the deb/rpm/AppImage. Flatpak apps themselves can also be removed on
the host without the sandbox by installing the native package.

## Polkit

Privileged removals run `pkexec <package-manager> …` so the user sees the standard Polkit dialog. To show a
branded prompt, install a Polkit action with `org.freedesktop.policykit.exec.path` annotations for specific
package-manager binaries (`/usr/bin/apt-get`, `/usr/bin/dnf`, …) and `allow_gui` set; the code needs no change.
