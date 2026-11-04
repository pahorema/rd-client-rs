#!/bin/sh
# Prepares the container environment and starts rd-client as PUID:PGID.
set -eu

# FILE__NAME=/path sets NAME to the contents of /path (Docker secrets).
for name in $(env | sed -n 's/^FILE__\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p'); do
    eval "file=\${FILE__$name}"
    if [ -f "$file" ]; then
        export "$name=$(cat "$file")"
    else
        echo "FILE__$name points to missing file $file" >&2
    fi
done

if [ -n "${UMASK:-}" ]; then
    umask "$UMASK"
fi

if [ "$(id -u)" != "0" ]; then
    exec /app/rd-client "$@"
fi

PUID="${PUID:-1000}"
PGID="${PGID:-1000}"

if ! getent group "$PGID" >/dev/null 2>&1; then
    addgroup -g "$PGID" rdclient
fi
if ! getent passwd "$PUID" >/dev/null 2>&1; then
    group="$(getent group "$PGID" | cut -d: -f1)"
    adduser -D -H -h /data -s /sbin/nologin -u "$PUID" -G "$group" rdclient
fi

mkdir -p /data/db /data/downloads "$(dirname "${DATABASE_PATH:-/data/db/rd-client.db}")"
if [ "${SKIP_CHOWN:-false}" != "true" ]; then
    chown -R "$PUID:$PGID" /data
fi

exec su-exec "$PUID:$PGID" /app/rd-client "$@"
