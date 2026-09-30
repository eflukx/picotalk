/*
 * A small application shell for the test programs: the Toolbox set up, an
 * Apple and a File menu, and text windows. Each window has a scrolling
 * transcript and, optionally, a row of push buttons above it and a
 * one-line input field below it.
 *
 * Deliberately not Retro68's console library: that is C++ and pulls in
 * over a megabyte of libstdc++, too much for a Mac Plus.
 */
#ifndef TEXTWIN_H
#define TEXTWIN_H

#include "mac.h"

#define TW_MAX_INPUT 200

typedef struct TextWin TextWin;

/* What tw_poll() saw. */
typedef enum {
    TW_NONE,
    TW_LINE,   /* the user entered `line` in `win` */
    TW_BUTTON, /* the user clicked `button` in `win` */
    TW_CANCEL, /* Esc or Cmd-. */
    TW_QUIT,   /* Quit from the File menu */
} TWKind;

typedef struct {
    TWKind kind;
    TextWin *win;
    const char *line;
    ControlHandle button;
} TWEvent;

/* Initialises the Toolbox and the menus. `about` is printed in the first
 * window for About... in the Apple menu. */
void tw_init(const char *app_name, const char *about);

/* Opens a window with content area `bounds` (global coordinates). */
TextWin *tw_new(const char *title, const Rect *bounds, Boolean with_input);

/* Adds a push button to the row at the top of the window. Call before
 * printing to the window. */
ControlHandle tw_add_button(TextWin *w, const char *title);
void tw_enable_button(TextWin *w, ControlHandle b, Boolean enabled);

/* Appends text to a window's transcript; '\n' separates lines, long lines
 * wrap. */
void tw_print(TextWin *w, const char *s);
void tw_printf(TextWin *w, const char *fmt, ...) __attribute__((format(printf, 2, 3)));

/* Handles at most one event; returns false if there was nothing to report. */
Boolean tw_poll(TWEvent *e);

#endif
