# Mac-side program, built with Retro68 (see ../README.md, or just run
# ./build.sh):
#
#   mkdir build && cd build
#   cmake .. -DCMAKE_TOOLCHAIN_FILE=<Retro68-build>/toolchain/m68k-apple-macos/cmake/retro68.toolchain.cmake
#   make
#
# Each application comes out as NAME.bin (MacBinary), NAME.dsk (an 800K
# disk image holding it) and NAME.APPL.

cmake_minimum_required(VERSION 3.13)
# Retro68 links applications with the C++ driver, so CXX must be enabled.
project(LocalFrameClient C CXX)

set(CMAKE_C_STANDARD 11)
add_compile_options(-Wall -O2)

# Echo tests and chat against `lftest serve`.
add_application(LFTest
    src/lftest.c
    src/echo.c
    src/chat.c
    src/textwin.c
    src/appletalk.c
    CREATOR "LFts"
)

# NOS Teletekst via `lftest serve`, which bridges to `ssh teletekst.nl`.
add_application(Teletekst
    src/teletekst.c
    src/textwin.c
    src/appletalk.c
    CREATOR "LFtt"
)
