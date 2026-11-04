#!/bin/sh
# Regenerates the RAR fixtures used by the integration tests.
#
# Needs the `rar` command line tool, version 6.x (RAR 7 can no longer write RAR4 archives),
# available from https://www.rarlab.com/download.htm. Usage: `RAR=/path/to/rar ./generate.sh`.
#
# Fixtures:
#   nested.rar           RAR5: top.txt, folder/a.txt, folder/sub/b.txt
#   nested4.rar          RAR4: same content as nested.rar
set -eu

RAR=${RAR:-rar}
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/folder/sub"
printf 'top\n' > "$work/top.txt"
printf 'alpha\n' > "$work/folder/a.txt"
printf 'beta\n' > "$work/folder/sub/b.txt"

cd "$here"
rm -f nested.rar nested4.rar
cd "$work"
"$RAR" a -idq -ma5 "$here/nested.rar" top.txt folder
"$RAR" a -idq -ma4 "$here/nested4.rar" top.txt folder
