# Mac-side programs, built with Retro68 (see ../README.md):
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

add_library(common STATIC src/appletalk.c src/textwin.c)

# Test program: NBP lookup, ATP ping and bulk transfer.
add_application(ATPing src/atping.c CREATOR "LFtp")
target_link_libraries(ATPing common)

# Chat client for the `rchat` hub.
add_application(RChat src/rchat.c CREATOR "LFch")
target_link_libraries(RChat common)
