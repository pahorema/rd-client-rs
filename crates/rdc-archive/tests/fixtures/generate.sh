#!/bin/sh
# Regenerates the RAR fixtures used by the integration tests.
#
# Needs the `rar` command line tool, version 6.x (RAR 7 can no longer write RAR4 archives),
# available from https://www.rarlab.com/download.htm. Usage: `RAR=/path/to/rar ./generate.sh`.
#
# Fixtures:
#   nested.rar           RAR5: top.txt, folder/a.txt, folder/sub/b.txt
#   nested4.rar          RAR4: same content as nested.rar
#   multi.partN.rar      RAR5, 3 volumes, new-style names: big.txt (stored, 6 KiB volumes)
#   legacy.rar/.r00/.r01 RAR4, 3 volumes, old-style names: big.txt (stored, 6 KiB volumes)
# big.txt holds 1500 lines "line 00000" … "line 01499".
set -eu

RAR=${RAR:-rar}
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/folder/sub"
printf 'top\n' > "$work/top.txt"
printf 'alpha\n' > "$work/folder/a.txt"
printf 'beta\n' > "$work/folder/sub/b.txt"
i=0
while [ "$i" -lt 1500 ]; do
    printf 'line %05d\n' "$i"
    i=$((i + 1))
done > "$work/big.txt"

cd "$here"
rm -f nested.rar nested4.rar multi.part*.rar legacy.rar legacy.r0*
cd "$work"
"$RAR" a -idq -ma5 "$here/nested.rar" top.txt folder
"$RAR" a -idq -ma4 "$here/nested4.rar" top.txt folder
"$RAR" a -idq -ma5 -m0 -v6k "$here/multi.rar" big.txt
"$RAR" a -idq -ma4 -m0 -vn -v6k "$here/legacy.rar" big.txt
