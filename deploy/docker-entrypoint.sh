#!/bin/sh
set -eu

if [ "$(id -u)" = "0" ] && [ "${RUSTIC_RUN_AS_ROOT:-0}" != "1" ]; then
    mkdir -p /data
    if [ "$(stat -c %u /data)" != "1000" ]; then
        chown -R rustic:rustic /data
    fi
    exec setpriv --reuid=rustic --regid=rustic --init-groups \
        env HOME=/home/rustic USER=rustic rustic-server "$@"
fi

exec rustic-server "$@"
