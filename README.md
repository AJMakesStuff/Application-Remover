# Application Remover

A native GTK 4 / libadwaita GNOME application for Ubuntu and Debian. Search installed Debian packages (including packages installed from downloaded `.deb` files), Snaps, and Flatpak apps and runtimes. Separate type and source filters let you switch between desktop applications and Linux components, or narrow the list to Debian, Snap, or Flatpak. Versions, descriptions, installed Debian sizes, and user/system Flatpak scopes are included.

Packages are listed alphabetically by name. Use search, type, and source filters to narrow the list. Debian packages with installed desktop launchers and Flatpak application refs are shown as applications. Debian packages without desktop launchers and Flatpak runtimes are shown as Linux stuff. Platform snaps such as `core` and `gnome-42-2204` are also treated as Linux stuff. Available app icons are read from desktop entries and Flatpak exports; other items use a standard app or package icon.

Right-click an item and choose **Show dependencies and details…** to inspect its package metadata. Debian entries include declared dependencies and installed reverse dependencies; Flatpak entries show their runtime and permissions; Snap entries show their base metadata and connected interfaces.

Install dates come from retained Debian `dpkg.log` files, including rotated gzip logs. The latest install event is used; upgrades do not change the date. Dates are shown in the selected package details. Packages whose history has been deleted, and Snap/Flatpak entries, show **Unknown**.

## Run

```sh
./run.sh
```

Requires system Python 3, `python3-gi`, `gir1.2-gtk-4.0`, `gir1.2-adw-1`, and `pkexec`. Snap and Flatpak are detected automatically. No pip packages are needed. Run inside your graphical GNOME session as your normal user.

## Install in GNOME

Open this project folder in GNOME Files, right-click `install.sh`, choose **Properties → Permissions**, enable **Allow executing file as program**, then double-click `install.sh` and choose **Run** if prompted. The installer requests administrator authorization through PolicyKit if the required GTK libraries are missing, then installs the app for your user and adds a GNOME launcher. Find **Application Remover** in the application overview. Run the installer again after updating the source. `XDG_DATA_HOME` is respected.

## Removal behavior

Select a package, choose **Uninstall…**, review the removal list, and confirm. A modal progress dialog shows a spinner and the current step during dependency checks, uninstalling, and the final package-list refresh. It closes automatically when the operation finishes and cannot be dismissed while work is running. Debian removals are simulated with APT first, including dependencies that would also be removed. Essential and protected packages are blocked. The simulation is repeated before executing; APT performs its own final safety checks. Avoid running other package managers during an uninstall. Administrator authorization uses the system PolicyKit dialog; the application never handles passwords. No removals happen simply by selecting a package.

Debian removals use APT’s automatic dependency cleanup, keeping package configuration files. APT checks the complete installed dependency graph and removes automatically installed dependencies that become unused after the selected removal. Shared dependencies, manually installed libraries, and packages that were already unused before this operation are retained. The full removal list is shown before confirmation. This relies on declared package dependencies: manually copied software and undeclared dependencies cannot be detected. Snap removal uses Snap's default snapshot behavior. Flatpak removal keeps application data and does not automatically remove unused runtimes. Package-manager errors, locks, and cancelled authentication are displayed. The window stays open while an operation runs.

All dpkg-installed packages are listed, even without desktop launchers. Dpkg does not reliably record whether a package originally came from a downloaded file, so locally installed `.deb` packages appear under **Debian**, alongside repository packages. AppImages, manually copied binaries, language-specific environments, and software installed with `make install` are outside this app's scope. Flatpak custom named system installations are not enumerated; the default system and user installations are supported.

## Tests

```sh
/usr/bin/python3 -m unittest discover -s tests -v
```

Tests mock uninstall commands: they never remove real packages.

To uninstall this application, delete its `application-remover` directory and `applications/io.github.applicationremover.App.desktop` under your user data directory. The installer does not remove system dependencies it installed.
