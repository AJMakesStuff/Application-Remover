#!/usr/bin/env bash
# Integration tests in DISPOSABLE containers. Nothing here touches the host:
# the repository is mounted read-only and every container is removed afterwards.
#
#   tests/integration/run.sh [debian|fedora|arch|opensuse|all]
#
# Each run installs a small package inside the container, scans it with the
# real backend and checks that the removal *preview* (dry run) names it.
# Destructive removal is intentionally never executed by automated tests.
set -euo pipefail
cd "$(dirname "$0")/../.."
ENGINE=${CONTAINER_ENGINE:-$(command -v podman || command -v docker)}
target=${1:-all}

run_case() {
  local name=$1 image=$2 setup=$3 pkg=$4
  echo "=== $name ($image) ==="
  "$ENGINE" run --rm -v "$PWD":/src:ro,Z -e CARGO_TARGET_DIR=/tmp/target "$image" sh -ec "
    $setup
    cd /src
    cargo run -q --example scan --no-default-features -- --preview $pkg > /tmp/out.txt 2>&1 || { cat /tmp/out.txt; exit 1; }
    grep -q 'role: Target' /tmp/out.txt || { cat /tmp/out.txt; echo 'preview did not name the target'; exit 1; }
    grep -q 'blocked: None' /tmp/out.txt || { cat /tmp/out.txt; echo 'preview unexpectedly blocked'; exit 1; }
    echo OK
  "
}

want() { [ "$target" = all ] || [ "$target" = "$1" ]; }

want debian   && run_case debian   docker.io/library/debian:stable \
  'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq cargo gcc htop >/dev/null' htop
want fedora   && run_case fedora   registry.fedoraproject.org/fedora:latest \
  'dnf install -y -q cargo gcc htop >/dev/null' htop
want arch     && run_case arch     docker.io/library/archlinux:latest \
  'pacman -Syu --noconfirm --needed rust gcc htop >/dev/null' htop
want opensuse && run_case opensuse registry.opensuse.org/opensuse/tumbleweed:latest \
  'zypper -q --non-interactive install cargo gcc htop >/dev/null' htop
