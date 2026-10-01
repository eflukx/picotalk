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
    src/lftest.r
    src/echo.c
    src/chat.c
    src/textwin.c
    src/appletalk.c
    CREATOR "LFts"
)

# NOS Teletekst via `lftest serve`, which bridges to `ssh teletekst.nl`.
add_application(Teletekst
    src/teletekst.c
    src/teletekst.r
    src/textwin.c
    src/appletalk.c
    CREATOR "LFtt"
)

# A tank-level dashboard via `lftest serve` (with BLD_URL set).
add_application(Tanks
    src/tanks.c
    src/tanks.r
    src/textwin.c
    src/appletalk.c
    CREATOR "LFtk"
)

# The Finder shows an application's own icon (the ICN#, FREF and BNDL in
# src/*.r, made by tools/mkicons.py) only when the file's bundle bit is
# set, which Rez does not do: tools/setbundle.py sets it in NAME.bin, and
# NAME.dsk is made again from that copy.
get_filename_component(HFSUTILS "${CMAKE_C_COMPILER}" DIRECTORY)
find_package(Python3 REQUIRED COMPONENTS Interpreter)
set(MKDISK sh ${CMAKE_CURRENT_SOURCE_DIR}/tools/mkdisk.sh ${HFSUTILS})
foreach(app LFTest Teletekst Tanks)
    add_custom_command(TARGET ${app}_APPL POST_BUILD
        COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/tools/setbundle.py ${app}.bin
        COMMAND ${MKDISK} ${app}.dsk ${app} ${app}.bin
        WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}
        VERBATIM)
endforeach()

# "The One Disk": all three applications on one 800K HFS disk image,
# demos.dsk.
add_custom_command(
    OUTPUT demos.dsk
    COMMAND ${MKDISK} demos.dsk "The One Disk" LFTest.bin Teletekst.bin Tanks.bin
    DEPENDS LFTest_APPL Teletekst_APPL Tanks_APPL
    WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}
    VERBATIM
)
add_custom_target(OneDisk ALL DEPENDS demos.dsk)
