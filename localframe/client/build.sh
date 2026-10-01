#!/bin/sh
# Builds the Mac programs into build/.
#
#   ./build.sh                    with the Retro68 Docker image
#   RETRO68=~/Retro68-build ./build.sh   with a locally built Retro68
#
# Output: build/{LFTest,Teletekst,Tanks}.{bin,dsk,APPL}.
set -e
cd "$(dirname "$0")"
mkdir -p build
TOOLCHAIN=toolchain/m68k-apple-macos/cmake/retro68.toolchain.cmake
if [ -n "$RETRO68" ]; then
    cd build
    cmake .. -DCMAKE_TOOLCHAIN_FILE="$RETRO68/$TOOLCHAIN"
    make
else
    docker run --rm -v "$PWD":/src -w /src/build -u "$(id -u):$(id -g)" -e HOME=/tmp \
        ghcr.io/autc04/retro68 \
        sh -c "cmake .. -DCMAKE_TOOLCHAIN_FILE=/Retro68-build/$TOOLCHAIN && make"
fi
ls -l "$(pwd)"/*.bin "$(pwd)"/*.dsk 2>/dev/null || ls -l build/*.bin build/*.dsk
