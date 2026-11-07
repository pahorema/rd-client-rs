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

database_dir="$(dirname "${DATABASE_PATH:-/data/db/rd-client.db}")"
mkdir -p "$database_dir"
mkdir -p /data/downloads || true

if [ "${SKIP_CHOWN:-false}" != "true" ]; then
    # Only the database folder holds rd-client's own files; fix it when anything in it
    # belongs to someone else.
    if [ -n "$(find "$database_dir" ! -user "$PUID" -o ! -group "$PGID" | head -n 1)" ]; then
        chown -R "$PUID:$PGID" "$database_dir"
    fi
    # The download folder is often a media library, a read-only bind or a network share:
    # never walk it, and only take over the folder itself when the user cannot write to it.
    if ! su-exec "$PUID:$PGID" test -w /data/downloads; then
        chown "$PUID:$PGID" /data/downloads 2>/dev/null \
            || echo "User $PUID cannot write to /data/downloads; fix its permissions on the host" >&2
    fi
fi

exec su-exec "$PUID:$PGID" /app/rd-client "$@"
