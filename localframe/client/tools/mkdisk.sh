#!/bin/sh
# Makes an 800K HFS disk image holding MacBinary files, with hfsutils:
#
#   tools/mkdisk.sh HFSUTILS_DIR OUT.dsk "Volume name" FILE.bin...
set -e
bin=$1 out=$2 name=$3
shift 3
rm -f "$out"
dd if=/dev/zero of="$out" bs=1024 count=800 2>/dev/null
"$bin/hformat" -l "$name" "$out" >/dev/null
"$bin/hcopy" -m "$@" :
"$bin/humount"
