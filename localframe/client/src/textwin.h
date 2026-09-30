/*
 * A small text-window application shell shared by the test programs: the
 * Toolbox set up, an Apple and a File menu, and one window with a
 * scrolling transcript above a one-line input field.
 *
 * Deliberately not Retro68's console library: that is C++ and pulls in
 * over a megabyte of libstdc++, too much for a Mac Plus.
 */
#ifndef TEXTWIN_H
#define TEXTWIN_H

#include "mac.h"

#define TW_MAX_INPUT 200

/* Initialises the Toolbox, the menus and the window. `about` is shown in
 * the transcript for About... in the Apple menu. */
void tw_init(const char *title, const char *about);

/* Appends text to the transcript; '\n' separates lines, long lines wrap. */
void tw_print(const char *s);
void tw_printf(const char *fmt, ...) __attribute__((format(printf, 1, 2)));

/* Handles at most one event. Returns a line the user entered (without the
 * Return) or NULL, and sets *quit when the user chose Quit. */
const char *tw_poll(Boolean *quit);

#endif
