"""Package inventory and explicit removal plans. Never invoke a shell."""
from dataclasses import dataclass, replace
from datetime import datetime
import gzip
from pathlib import Path
import os
import re
import shutil
import subprocess


@dataclass(frozen=True)
class Package:
    name: str
    version: str
    source: str
    description: str = ''
    size: int = 0
    protected: bool = False
    ref: str = ''
    scope: str = ''
    install_date: str | None = None
    application: bool = False
    icon: str = ''


def run(args, timeout=60):
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout,
                            env={**os.environ, 'LC_ALL': 'C'})
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip() or
                           f'{args[0]} exited with status {result.returncode}')
    return result.stdout


def parse_debian(output, application_packages=(), application_icons=None):
    packages = []
    application_packages = set(application_packages)
    application_icons = application_icons or {}
    for line in output.splitlines():
        fields = line.split('\t', 7)
        if len(fields) != 8:
            continue
        name, status, version, size, essential, protected, section, description = fields
        if status != 'installed':
            continue
        is_application = name in application_packages or name.split(':')[0] in application_packages
        packages.append(Package(name, version, 'Debian', description,
                                int(size or 0) * 1024, essential == 'yes' or protected == 'yes',
                                application=is_application,
                                icon=application_icons.get(name, application_icons.get(name.split(':')[0], ''))))
    return packages


def desktop_entry_values(path):
    """Read keys from the main desktop-entry group without parsing localized values."""
    values = {}
    in_entry = False
    try:
        with open(path, encoding='utf-8', errors='replace') as desktop_file:
            for line in desktop_file:
                line = line.strip()
                if line == '[Desktop Entry]':
                    in_entry = True
                elif line.startswith('[') and line.endswith(']'):
                    if in_entry:
                        break
                elif in_entry and '=' in line and not line.startswith('#'):
                    key, value = line.split('=', 1)
                    values.setdefault(key, value)
    except OSError:
        pass
    return values


def debian_desktop_info(output):
    """Return Debian packages with desktop launchers and their declared icons."""
    packages, icons = set(), {}
    for line in output.splitlines():
        owner, separator, path = line.partition(': ')
        if not separator or not owner or not path.endswith('.desktop'):
            continue
        packages.update((owner, owner.split(':')[0]))
        icon = desktop_entry_values(path).get('Icon', '')
        if icon:
            icons.setdefault(owner, icon)
            icons.setdefault(owner.split(':')[0], icon)
    return packages, icons


def debian_application_packages(output):
    """Return Debian package names that own a desktop launcher."""
    return debian_desktop_info(output)[0]


def snap_desktop_icons(directory='/var/lib/snapd/desktop/applications'):
    """Read icons from Snap-generated desktop entries."""
    icons = {}
    for path in sorted(Path(directory).glob('*.desktop')):
        values = desktop_entry_values(path)
        snap_name = values.get('X-SnapInstanceName', '') or path.stem.split('_', 1)[0]
        icon = values.get('Icon', '')
        if snap_name and icon:
            icons.setdefault(snap_name, icon)
    return icons


def flatpak_icon(ref, scope, home=None, environ=None):
    """Find an exported icon for a Flatpak application, if one is available."""
    parts = ref.split('/')
    if len(parts) < 2 or parts[0] != 'app':
        return ''
    app_id = parts[1]
    environ = os.environ if environ is None else environ
    data_home = Path(environ.get('XDG_DATA_HOME', Path(home or Path.home()) / '.local/share'))
    roots = ([data_home / 'flatpak/exports/share'] if scope == 'user'
             else [Path('/var/lib/flatpak/exports/share')])
    icon_suffixes = ('.png', '.svg', '.webp', '.xpm')
    preferred_sizes = ('128x128', 'scalable', '96x96', '64x64', '48x48', '32x32', '256x256')
    for root in roots:
        hicolor = root / 'icons/hicolor'
        for size in preferred_sizes:
            for suffix in icon_suffixes:
                candidate = hicolor / size / 'apps' / f'{app_id}{suffix}'
                if candidate.is_file():
                    return str(candidate)
        for candidate in sorted(hicolor.glob(f'*/apps/{app_id}.*')):
            if candidate.suffix.lower() in icon_suffixes and candidate.is_file():
                return str(candidate)
    return ''


def _limit_lines(text, limit=80):
    lines = text.splitlines()
    if len(lines) > limit:
        lines = lines[:limit] + [f'… {len(lines) - limit} more lines']
    return '\n'.join(lines)


def package_details(package):
    """Return locally available package metadata and dependency information."""
    if package.source == 'Debian':
        fields = ('Package: ${binary:Package}\nVersion: ${Version}\nArchitecture: ${Architecture}\n'
                  'Status: ${db:Status-Status}\nSection: ${Section}\n'
                  'Installed size: ${Installed-Size} KiB\nEssential: ${Essential}\nProtected: ${Protected}\n'
                  'Pre-Depends: ${Pre-Depends}\nDepends: ${Depends}\nRecommends: ${Recommends}\n'
                  'Suggests: ${Suggests}\nProvides: ${Provides}\nConflicts: ${Conflicts}\n'
                  'Breaks: ${Breaks}\nReplaces: ${Replaces}\n')
        metadata = run(['dpkg-query', '-W', f'-f={fields}', '--', package.name])
        try:
            reverse = run(['apt-cache', 'rdepends', '--installed', package.name])
        except Exception as error:
            reverse = f'Unavailable: {error}'
        return _limit_lines(f'{metadata}\nInstalled reverse dependencies:\n{reverse}')

    if package.source == 'Flatpak':
        scope = f'--{package.scope}'
        lines = [f'Package: {package.name}', f'Ref: {package.ref}', f'Version: {package.version}',
                 f'Installation: {package.scope}']
        try:
            runtime = run(['flatpak', 'info', scope, '--show-runtime', package.ref]).strip()
        except Exception as error:
            runtime = f'Unavailable: {error}'
        lines.extend(('', 'Runtime dependency:', runtime or '(none)'))
        try:
            permissions = run(['flatpak', 'info', scope, '--show-permissions', package.ref]).strip()
        except Exception as error:
            permissions = f'Unavailable: {error}'
        lines.extend(('', 'Permissions:', permissions or '(none reported)'))
        return _limit_lines('\n'.join(lines))

    if package.source == 'Snap':
        lines = [f'Package: {package.name}', f'Version: {package.version}', 'Source: Snap']
        manifest = Path('/snap') / package.name / 'current/meta/snap.yaml'
        try:
            raw = manifest.read_text(encoding='utf-8', errors='replace')
            wanted = {'summary', 'base', 'confinement', 'grade', 'license', 'architectures'}
            values = []
            for line in raw.splitlines():
                if line and not line[0].isspace() and ':' in line:
                    key, value = line.split(':', 1)
                    if key in wanted and value.strip():
                        values.append(f'{key.title()}: {value.strip()}')
            lines.extend(('', 'Snap metadata:', *values))
        except OSError:
            lines.extend(('', 'Snap metadata: unavailable'))
        try:
            connections = run(['snap', 'connections', package.name]).strip()
        except Exception as error:
            connections = f'Unavailable: {error}'
        lines.extend(('', 'Connected interfaces:', connections or '(none)'))
        return _limit_lines('\n'.join(lines))

    return f'Package: {package.name}\nVersion: {package.version}\nSource: {package.source}'


def snap_is_application(name):
    """Treat platform snaps as Linux components; regular snaps are applications."""
    if name in {'bare', 'core', 'core16', 'core18', 'core20', 'core22', 'core24',
                'snapd', 'gtk-common-themes'}:
        return False
    if re.fullmatch(r'gnome-\d+(?:-\d+)?-\d{4}', name):
        return False
    if re.fullmatch(r'mesa-\d{4}', name) or name.startswith('kde-frameworks-'):
        return False
    return True


def debian_install_dates(log_dir='/var/log'):
    """Read actual install events, never confuse updates or file mtimes with installs."""
    events = {}
    for path in Path(log_dir).glob('dpkg.log*'):
        opener = gzip.open if path.suffix == '.gz' else open
        try:
            with opener(path, 'rt', encoding='utf-8', errors='replace') as log:
                for line in log:
                    fields = line.split()
                    if len(fields) != 6 or fields[2] not in ('install', 'remove', 'purge'):
                        continue
                    date = ' '.join(fields[:2])
                    try:
                        datetime.strptime(date, '%Y-%m-%d %H:%M:%S')
                    except ValueError:
                        continue
                    name, action = fields[3], fields[2]
                    if name not in events or date >= events[name][0]:
                        events[name] = (date, action)
        except (OSError, EOFError):
            # Missing, rotated, or unreadable history means unknown, not a guess.
            continue
    dates = {name: date for name, (date, action) in events.items() if action == 'install'}
    # dpkg-query omits the architecture for some packages. Only alias names
    # when the log has no ambiguity between architectures.
    variants = {}
    for name in events:
        variants.setdefault(name.split(':')[0], []).append(name)
    for name, qualified in variants.items():
        if len(qualified) == 1 and qualified[0] in dates:
            dates.setdefault(name, dates[qualified[0]])
    return dates


def inventory():
    packages, warnings = [], []
    if shutil.which('dpkg-query'):
        try:
            rows = run([
                'dpkg-query', '-W', '-f=${binary:Package}\t${db:Status-Status}\t${Version}\t'
                '${Installed-Size}\t${Essential}\t${Protected}\t${Section}\t${binary:Summary}\n'])
            try:
                launchers = run(['dpkg-query', '-S', '/usr/share/applications/*.desktop'])
            except Exception:
                launchers = ''
            application_packages, application_icons = debian_desktop_info(launchers)
            packages.extend(parse_debian(rows, application_packages, application_icons))
        except Exception as error:
            warnings.append(f'Debian: {error}')
    if shutil.which('snap'):
        try:
            snap_icons = snap_desktop_icons()
            for line in run(['snap', 'list'], timeout=30).splitlines()[1:]:
                fields = line.split()
                if len(fields) >= 2:
                    packages.append(Package(fields[0], fields[1], 'Snap',
                                            application=snap_is_application(fields[0]),
                                            icon=snap_icons.get(fields[0], '')))
        except Exception as error:
            warnings.append(f'Snap: {error}')
    if shutil.which('flatpak'):
        for scope in ('user', 'system'):
            try:
                for line in run(['flatpak', 'list', f'--{scope}',
                                 '--columns=ref,name,version,description']).splitlines():
                    fields = line.split('\t')
                    if len(fields) >= 3:
                        ref, name, version = fields[:3]
                        packages.append(Package(name, version, 'Flatpak',
                                                fields[3] if len(fields) > 3 else '',
                                                ref=ref, scope=scope,
                                                application=ref.startswith('app/'),
                                                icon=flatpak_icon(ref, scope)))
            except Exception as error:
                warnings.append(f'Flatpak ({scope}): {error}')
    dates = debian_install_dates()
    packages = [replace(p, install_date=dates.get(p.name)) if p.source == 'Debian' else p
                for p in packages]
    return sorted(packages, key=lambda p: (p.name.casefold(), p.source, p.scope)), warnings


@dataclass(frozen=True)
class RemovalPlan:
    names: tuple[str, ...]
    command: tuple[str, ...]


def simulated_removals(output):
    if re.search(r'^(Inst|Conf) ', output, re.MULTILINE):
        raise RuntimeError('APT also proposes installing or configuring packages. Resolve pending package changes first.')
    return re.findall(r'^Remv (\S+)', output, re.MULTILINE)


def removal_plan(package, packages):
    if package.protected:
        raise RuntimeError('This is an essential or protected system package and cannot be removed here.')
    if package.source != 'Debian':
        return RemovalPlan((package.ref or package.name,), tuple(removal_command(package)))

    # Root already-unused packages during this transaction, so cleanup is limited
    # to dependencies made unused by this removal. APT also retains their deps.
    existing_orphans = simulated_removals(run(['apt-get', '--simulate', 'autoremove']))
    options = []
    for name in sorted(existing_orphans):
        options.extend(['-o', 'APT::NeverAutoRemove::=^' + re.escape(name.split(':')[0]) + '$'])
    args = options + ['--autoremove', 'remove', '--', package.name]
    names = simulated_removals(run(['apt-get', '--simulate', *args]))
    if not names:
        raise RuntimeError('APT did not propose any removals. Refresh the package list.')
    protected = {p.name.split(':')[0] for p in packages if p.protected}
    blocked = [name for name in names if name.split(':')[0] in protected]
    if blocked:
        raise RuntimeError('Removal would affect essential or protected packages: ' + ', '.join(blocked))
    return RemovalPlan(tuple(names), tuple(['pkexec', '/usr/bin/apt-get', '--assume-yes', *args]))


def removal_command(package):
    if package.source == 'Debian':
        raise ValueError('Debian removals require a reviewed dependency plan')
    if package.source == 'Snap':
        return ['pkexec', '/usr/bin/snap', 'remove', package.name]
    if package.source == 'Flatpak':
        command = ['/usr/bin/flatpak', 'uninstall', '--noninteractive',
                   f'--{package.scope}', package.ref]
        return (['pkexec'] if package.scope == 'system' else []) + command
    raise ValueError('Unknown package source')


def remove(package, packages, approved_plan):
    # Recheck immediately before authentication; package state may have changed.
    if removal_plan(package, packages) != approved_plan:
        raise RuntimeError('The removal plan changed. Refresh and review it again.')
    return run(list(approved_plan.command), timeout=None)
