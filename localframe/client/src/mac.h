/* Toolbox headers: Retro68's default Multiversal interfaces have one
 * umbrella header; Apple's Universal Interfaces split them up. */
#ifndef MAC_H
#define MAC_H

#if __has_include(<Multiverse.h>)
#include <Multiverse.h>
#else
#include <Desk.h>
#include <Devices.h>
#include <Dialogs.h>
#include <Events.h>
#include <Fonts.h>
#include <Memory.h>
#include <Menus.h>
#include <OSUtils.h>
#include <Quickdraw.h>
#include <Resources.h>
#include <Sound.h>
#include <TextEdit.h>
#include <TextUtils.h>
#include <ToolUtils.h>
#include <Windows.h>
#endif

#endif
