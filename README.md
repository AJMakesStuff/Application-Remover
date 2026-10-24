# Application Remover

Application Remover uses its original GTK 4 and libadwaita interface, with package discovery, metadata, dependency planning, and removal implemented in Rust. The Python frontend only presents the interface and exchanges JSON requests with the Rust backend executable.

The app lists installed Debian packages, Snap packages, and Flatpak applications and runtimes. Search, source, and type filters narrow the list. Package rows show an icon, version, description, source, installed size, and install date when available. Package details include metadata and dependencies. Uninstall previews show the full proposed removal list and require confirmation.

## Requirements

- Rust and Cargo (Rust 1.95 or newer)
- Python 3 with PyGObject
- GTK 4 and libadwaita 1 introspection data
- `dpkg-query` for Debian packages; Snap and Flatpak are detected automatically
- `pkexec` for privileged Debian, Snap, and system Flatpak removals

On Ubuntu or Debian, install the GTK introspection packages for GTK 4 and libadwaita along with PyGObject, then build and run the app as described below.

## Build and run

Build the application from the project directory:

```sh
cargo build --release
```

Cargo generates the `application-remover` executable at `target/release/application-remover`. Run that file to open the GTK app:

```sh
./target/release/application-remover
```

The generated executable starts the GTK frontend by default and uses itself in backend mode for package operations. Set `APPLICATION_REMOVER_PYTHON` if the Python interpreter with PyGObject is not named `python3`.

## Removal behavior

Essential and protected packages cannot be removed. Debian removal plans are simulated with APT, including dependencies that become unused because of the selected removal. The Rust backend checks the plan again immediately before execution and stops if it changed. APT performs its own final safety checks. Administrator authorization uses PolicyKit; run the app as your normal user.

Debian removals keep package configuration files. Shared dependencies, manually installed libraries, and packages that were already unused are retained. Snap removal uses Snap's default snapshot behavior. Flatpak removal keeps application data and does not automatically remove unused runtimes. Package-manager errors, locks, and cancelled authentication are displayed in the window.

All dpkg-installed packages are listed, including packages without desktop launchers and packages installed from downloaded `.deb` files. AppImages, manually copied binaries, language-specific environments, and software installed with `make install` are outside the app's scope. Flatpak custom named system installations are not enumerated; the default system and user installations are supported.
