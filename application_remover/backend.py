"""Thin JSON bridge from the GTK frontend to the Rust package backend."""
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import subprocess


PROJECT_ROOT = Path(__file__).resolve().parent.parent


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

    @classmethod
    def from_json(cls, value):
        return cls(**value)

    def to_json(self):
        return {
            'name': self.name,
            'version': self.version,
            'source': self.source,
            'description': self.description,
            'size': self.size,
            'protected': self.protected,
            'ref': self.ref,
            'scope': self.scope,
            'install_date': self.install_date,
            'application': self.application,
            'icon': self.icon,
        }


@dataclass(frozen=True)
class RemovalPlan:
    names: tuple[str, ...]
    command: tuple[str, ...]

    @classmethod
    def from_json(cls, value):
        return cls(tuple(value['names']), tuple(value['command']))

    def to_json(self):
        return {'names': list(self.names), 'command': list(self.command)}


def _backend_executable():
    configured = os.environ.get('APPLICATION_REMOVER_BACKEND')
    if configured:
        executable = Path(configured).expanduser()
        if executable.is_file():
            return executable
        raise RuntimeError(f'Rust backend executable not found: {executable}')

    target_dir = os.environ.get('CARGO_TARGET_DIR')
    if target_dir:
        target = Path(target_dir).expanduser()
        if not target.is_absolute():
            target = PROJECT_ROOT / target
        candidates = [target / 'release/application-remover',
                      target / 'debug/application-remover']
    else:
        cache_home = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache'))
        cache_target = cache_home / 'application-remover/cargo-target'
        candidates = [cache_target / 'release/application-remover',
                      cache_target / 'debug/application-remover',
                      PROJECT_ROOT / 'target/release/application-remover',
                      PROJECT_ROOT / 'target/debug/application-remover']
    for candidate in candidates:
        if candidate.is_file():
            return candidate

    installed = shutil.which('application-remover')
    if installed:
        return Path(installed)

    raise RuntimeError(
        'The Rust backend is not built. Run `cargo build --release` from the project directory, '
        'or set APPLICATION_REMOVER_BACKEND to the backend executable.'
    )


def _request(operation, **values):
    request = {'operation': operation, **values}
    try:
        process = subprocess.run(
            [str(_backend_executable()), '--backend'],
            input=json.dumps(request),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as error:
        raise RuntimeError(f'Could not start the Rust backend: {error}') from error

    try:
        response = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        detail = process.stderr.strip() or process.stdout.strip()
        raise RuntimeError(detail or 'The Rust backend returned an invalid response.') from error

    if process.returncode:
        detail = process.stderr.strip() or response.get('error') or 'The Rust backend failed.'
        raise RuntimeError(detail)
    if not response.get('ok'):
        raise RuntimeError(response.get('error', 'The Rust backend failed.'))
    return response['result']


def inventory():
    result = _request('inventory')
    packages = [Package.from_json(package) for package in result['packages']]
    return packages, result['warnings']


def package_details(package):
    return _request('details', package=package.to_json())


def removal_plan(package, packages):
    result = _request('removal_plan', package=package.to_json(),
                      packages=[item.to_json() for item in packages])
    return RemovalPlan.from_json(result)


def remove(package, packages, approved_plan):
    return _request('remove', package=package.to_json(),
                    packages=[item.to_json() for item in packages],
                    plan=approved_plan.to_json())
