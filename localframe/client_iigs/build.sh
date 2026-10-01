#!/bin/sh
# Builds the IIgs Teletekst client:
#
#   build/Teletekst      the GS/OS application (S16, OMF)
#   build/Teletekst.po   an 800K ProDOS disk image holding it
#
# Needs Merlin 32 (assembler) and Cadius (disk images). If they are not
# on the PATH (or named by $MERLIN32 / $CADIUS), they are fetched and
# built into tools/bin, which needs git, make and a C compiler.
set -e
cd "$(dirname "$0")"
HERE=$PWD

build_tool() { # name repo subdir binary
    if [ ! -x "tools/bin/$1" ]; then
        echo "== building $1 from $2"
        rm -rf "tools/src/$1"
        mkdir -p tools/src tools/bin
        git clone -q --depth 1 "$2" "tools/src/$1"
        make -C "tools/src/$1/$3" >"tools/src/$1.log" 2>&1 || { cat "tools/src/$1.log"; exit 1; }
        cp "tools/src/$1/$3/$4" "tools/bin/$1"
    fi
}

if [ -z "$MERLIN32" ]; then
    MERLIN32=$(command -v merlin32 || true)
    if [ -z "$MERLIN32" ]; then
        build_tool merlin32 https://github.com/apple2accumulator/merlin32 Source merlin32
        MERLIN32=$HERE/tools/bin/merlin32
    fi
fi
if [ -z "$CADIUS" ]; then
    CADIUS=$(command -v cadius || true)
    if [ -z "$CADIUS" ]; then
        build_tool cadius https://github.com/mach-kernel/cadius . bin/release/cadius
        CADIUS=$HERE/tools/bin/cadius
    fi
fi

rm -rf build
mkdir -p build
cp src/*.s build/
cd build
# The program uses no macro libraries; the folder argument is required.
"$MERLIN32" -V . link.s >merlin32.log || { cat merlin32.log; exit 1; }
grep -q "Creating OMF file" merlin32.log || { cat merlin32.log; exit 1; }
rm -f *.s _FileInformation.txt  # _Output.txt is the listing

# Cadius takes the ProDOS file type and aux type from the name: S16 = $B3.
cp Teletekst 'TELETEKST#B30000'
"$CADIUS" CREATEVOLUME Teletekst.po TELETEKST 800KB >/dev/null
"$CADIUS" ADDFILE Teletekst.po /TELETEKST 'TELETEKST#B30000' >/dev/null
rm 'TELETEKST#B30000'
echo "built build/Teletekst ($(wc -c <Teletekst) bytes) and build/Teletekst.po"
