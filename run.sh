#!/bin/sh
cd "$(dirname "$(readlink -f "$0")")" || exit 1
exec /usr/bin/python3 -m application_remover "$@"
