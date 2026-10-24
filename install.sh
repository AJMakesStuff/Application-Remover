#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
python=/usr/bin/python3

show_error() {
    if command -v zenity >/dev/null 2>&1 && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
        zenity --error --title='Application Remover installer' --text="$1" 2>/dev/null || true
    elif command -v notify-send >/dev/null 2>&1 && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
        notify-send 'Application Remover installer' "$1" 2>/dev/null || true
    fi
    printf '%s\n' "$1" >&2
}

if [ ! -x "$python" ]; then
    show_error 'System Python 3 was not found at /usr/bin/python3.'
    exit 1
fi

if ! command -v pkexec >/dev/null 2>&1; then
    show_error 'PolicyKit (pkexec) is required for package removal. Install it with your distribution software manager, then run this installer again.'
    exit 1
fi

if ! "$python" -c "import gi; gi.require_version('Gtk', '4.0'); gi.require_version('Adw', '1'); from gi.repository import Gtk, Adw" >/dev/null 2>&1; then
    if ! command -v apt-get >/dev/null 2>&1; then
        show_error 'GTK 4 and libadwaita are required. Install python3-gi, gir1.2-gtk-4.0, and gir1.2-adw-1 with your distribution software manager, then run this installer again.'
        exit 1
    fi
    apt_get=$(command -v apt-get)
    if ! pkexec /usr/bin/env DEBIAN_FRONTEND=noninteractive "$apt_get" install -y python3-gi gir1.2-gtk-4.0 gir1.2-adw-1; then
        show_error 'Could not install the required GTK libraries. Check the package manager and try again.'
        exit 1
    fi
fi

"$python" "$project_dir/install.py"
if command -v notify-send >/dev/null 2>&1 && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    notify-send 'Application Remover installed' 'Find it in the GNOME application overview.' 2>/dev/null || true
fi
