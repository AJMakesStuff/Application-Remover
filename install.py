#!/usr/bin/python3
"""Install an independent copy and GNOME launcher for the current user."""
from pathlib import Path
import os
import shutil

root = Path(__file__).resolve().parent
base = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share'))
target = base / 'application-remover'
target.mkdir(parents=True, exist_ok=True)
shutil.copytree(root / 'application_remover', target / 'application_remover', dirs_exist_ok=True,
                ignore=shutil.ignore_patterns('__pycache__'))
shutil.copy2(root / 'run.sh', target / 'run.sh')
applications = base / 'applications'
applications.mkdir(parents=True, exist_ok=True)
# Desktop Exec has its own quoting rules (including literal percent escapes).
executable = str(target / 'run.sh').replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
launcher = applications / 'io.github.applicationremover.App.desktop'
launcher.write_text('[Desktop Entry]\nType=Application\nName=Application Remover\n'
                    'Comment=View and uninstall installed packages, Snaps, and Flatpaks\n'
                    f'Exec="{executable}"\nIcon=system-software-install\n'
                    'Terminal=false\nCategories=System;PackageManager;\nKeywords=uninstall;remove;packages;deb;\n')
print(f'Installed to {target}\nGNOME launcher: {launcher}')
