#include "textwin.h"

#include <stdarg.h>
#include <stdio.h>
#include <string.h>

enum { MENU_APPLE = 128, MENU_FILE = 129 };
enum { ITEM_ABOUT = 1, ITEM_QUIT = 1 };

#define MAX_WINDOWS 4
#define MAX_BUTTONS 6
#define NLINES 128 /* transcript lines kept per window */
#define MAXCOLS 120
#define BUTTON_H 20
#define STRIP_H (BUTTON_H + 10) /* button row, with margins */

struct TextWin {
    WindowPtr win;
    Boolean hasInput;
    char lines[NLINES][MAXCOLS + 1];
    long nlines; /* lines ever added; line k is in lines[k % NLINES] */
    Rect textRect, inputRect;
    short cols, rows;
    char input[TW_MAX_INPUT + 1];
    short inputLen;
    ControlHandle buttons[MAX_BUTTONS];
    Boolean enabled[MAX_BUTTONS];
    short nbuttons;
    short buttonsRight; /* where the next button goes */
};

static TextWin wins[MAX_WINDOWS];
static short nwins;
static MenuHandle appleMenu;
static const char *aboutText;
static short lineH, ascent;
static RgnHandle scratchRgn;
static char submitted[TW_MAX_INPUT + 1];

static TextWin *find(WindowPtr w)
{
    short i;
    for (i = 0; i < nwins; i++)
        if (wins[i].win == w)
            return &wins[i];
    return NULL;
}

/* The window that gets typed text: the front one if it has an input
 * field, else the first that has one. */
static TextWin *input_win(void)
{
    TextWin *t = find(FrontWindow());
    short i;
    if (t && t->hasInput)
        return t;
    for (i = 0; i < nwins; i++)
        if (wins[i].hasInput)
            return &wins[i];
    return NULL;
}

static void layout(TextWin *t)
{
    Rect r = t->win->portRect;

    InsetRect(&r, 4, 4);
    if (t->nbuttons > 0)
        r.top = t->win->portRect.top + STRIP_H + 2;
    t->textRect = r;
    if (t->hasInput) {
        t->inputRect = r;
        t->inputRect.top = r.bottom - lineH;
        t->textRect.bottom = t->inputRect.top - 6;
    }
    t->rows = (t->textRect.bottom - t->textRect.top) / lineH;
    if (t->rows < 1)
        t->rows = 1;
    t->textRect.top = t->textRect.bottom - t->rows * lineH;
    t->cols = (r.right - r.left) / CharWidth('m');
    if (t->cols > MAXCOLS)
        t->cols = MAXCOLS;
    if (t->cols < 20) /* add_line() needs room for a few characters */
        t->cols = 20;
}

static void draw_line(TextWin *t, long k)
{
    long row = t->rows - 1 - (t->nlines - 1 - k);
    char *s;
    if (row < 0 || k < 0)
        return;
    s = t->lines[k % NLINES];
    MoveTo(t->textRect.left, t->textRect.top + (short)row * lineH + ascent);
    DrawText(s, 0, (short)strlen(s));
}

static void draw_input(TextWin *t)
{
    short fit = t->cols - 3, start = t->inputLen > fit ? t->inputLen - fit : 0;
    EraseRect(&t->inputRect);
    MoveTo(t->inputRect.left, t->inputRect.top + ascent);
    DrawText("> ", 0, 2);
    DrawText(t->input, start, t->inputLen - start);
    DrawChar('_');
}

static void draw_all(TextWin *t)
{
    Rect r = t->win->portRect;
    long k;

    EraseRect(&r);
    for (k = t->nlines - t->rows; k < t->nlines; k++)
        draw_line(t, k);
    if (t->nbuttons > 0) {
        MoveTo(r.left, r.top + STRIP_H);
        LineTo(r.right, r.top + STRIP_H);
        DrawControls(t->win);
    }
    if (t->hasInput) {
        MoveTo(r.left, t->inputRect.top - 4);
        LineTo(r.right, t->inputRect.top - 4);
        draw_input(t);
    }
}

/* Adds one screen line: scroll the transcript up and draw it at the bottom. */
static void push_line(TextWin *t, const char *s, short len)
{
    char *dst = t->lines[t->nlines % NLINES];
    GrafPtr old;

    memcpy(dst, s, len);
    dst[len] = 0;
    t->nlines++;
    GetPort(&old);
    SetPort(t->win);
    ScrollRect(&t->textRect, 0, -lineH, scratchRgn);
    draw_line(t, t->nlines - 1);
    SetPort(old);
}

/* One logical line, wrapped at word boundaries; continuation lines are
 * indented. */
static void add_line(TextWin *t, const char *s, short len)
{
    char buf[MAXCOLS + 1];
    short indent = 0;

    do {
        short room = t->cols - indent, n = len;
        if (n > room) {
            n = room;
            while (n > room / 2 && s[n] != ' ')
                n--;
            if (s[n] != ' ')
                n = room;
        }
        memset(buf, ' ', indent);
        memcpy(buf + indent, s, n);
        push_line(t, buf, indent + n);
        s += n;
        len -= n;
        while (len > 0 && *s == ' ')
            s++, len--;
        indent = 2;
    } while (len > 0);
}

void tw_print(TextWin *t, const char *s)
{
    for (;;) {
        const char *nl = strchr(s, '\n');
        if (!nl) {
            if (*s)
                add_line(t, s, (short)strlen(s));
            return;
        }
        add_line(t, s, (short)(nl - s));
        s = nl + 1;
    }
}

void tw_printf(TextWin *t, const char *fmt, ...)
{
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    tw_print(t, buf);
}

static void pstr(unsigned char *dst, const char *s)
{
    size_t n = strlen(s);
    if (n > 255)
        n = 255;
    dst[0] = (unsigned char)n;
    memcpy(dst + 1, s, n);
}

ControlHandle tw_add_button(TextWin *t, const char *title)
{
    Str255 s;
    Rect r;
    short width;
    GrafPtr old;

    if (t->nbuttons == MAX_BUTTONS)
        return NULL;
    GetPort(&old);
    SetPort(t->win);
    pstr(s, title);
    /* Buttons are drawn in the system font. */
    TextFont(0);
    TextSize(12);
    width = StringWidth(s) + 24;
    TextFont(4);
    TextSize(9);
    if (t->nbuttons == 0)
        t->buttonsRight = t->win->portRect.left + 4;
    SetRect(&r, t->buttonsRight + 6, t->win->portRect.top + 5, t->buttonsRight + 6 + width,
            t->win->portRect.top + 5 + BUTTON_H);
    t->buttonsRight = r.right;
    t->buttons[t->nbuttons] = NewControl(t->win, &r, s, true, 0, 0, 1, pushButProc, 0);
    t->enabled[t->nbuttons] = true;
    t->nbuttons++;
    layout(t);
    InvalRect(&t->win->portRect);
    SetPort(old);
    return t->buttons[t->nbuttons - 1];
}

static void hilite_buttons(TextWin *t)
{
    Boolean active = t->win == FrontWindow();
    short i;
    for (i = 0; i < t->nbuttons; i++)
        HiliteControl(t->buttons[i], active && t->enabled[i] ? 0 : 255);
}

void tw_enable_button(TextWin *t, ControlHandle b, Boolean enabled)
{
    short i;
    for (i = 0; i < t->nbuttons; i++)
        if (t->buttons[i] == b && t->enabled[i] != enabled) {
            t->enabled[i] = enabled;
            hilite_buttons(t);
        }
}

TextWin *tw_new(const char *title, const Rect *bounds, Boolean with_input)
{
    TextWin *t = &wins[nwins++];
    Str255 s;

    memset(t, 0, sizeof *t);
    t->hasInput = with_input;
    pstr(s, title);
    t->win = NewWindow(NULL, bounds, s, true, noGrowDocProc, (WindowPtr)-1, false, 0);
    SetPort(t->win);
    TextFont(4); /* Monaco */
    TextSize(9);
    if (lineH == 0) {
        FontInfo fi;
        GetFontInfo(&fi);
        ascent = fi.ascent;
        lineH = fi.ascent + fi.descent + fi.leading;
    }
    layout(t);
    return t;
}

static Boolean menu(long choice)
{
    short item = choice & 0xFFFF;
    Boolean quit = false;

    if ((choice >> 16) == MENU_APPLE) {
        if (item == ITEM_ABOUT) {
            if (nwins > 0)
                tw_print(&wins[0], aboutText);
        } else {
            Str255 name;
            GetMenuItemText(appleMenu, item, name);
            OpenDeskAcc(name);
        }
    } else if ((choice >> 16) == MENU_FILE && item == ITEM_QUIT) {
        quit = true;
    }
    HiliteMenu(0);
    return quit;
}

static const char *key(TextWin *t, char c)
{
    GrafPtr old;
    const char *line = NULL;

    if (!t)
        return NULL;
    if (c == '\r' || c == 3) {
        if (t->inputLen == 0)
            return NULL;
        memcpy(submitted, t->input, t->inputLen);
        submitted[t->inputLen] = 0;
        t->inputLen = 0;
        line = submitted;
    } else if (c == 8) {
        if (t->inputLen > 0)
            t->inputLen--;
    } else if ((unsigned char)c >= ' ' && c != 0x7F && t->inputLen < TW_MAX_INPUT) {
        t->input[t->inputLen++] = c;
    } else {
        return NULL;
    }
    GetPort(&old);
    SetPort(t->win);
    draw_input(t);
    SetPort(old);
    return line;
}

Boolean tw_poll(TWEvent *e)
{
    EventRecord ev;
    WindowPtr w;
    TextWin *t;

    e->kind = TW_NONE;
    e->win = NULL;
    SystemTask();
    if (!GetNextEvent(everyEvent, &ev))
        return false;
    switch (ev.what) {
    case keyDown:
    case autoKey: {
        char c = (char)(ev.message & charCodeMask);
        if (c == 0x1B || (c == '.' && (ev.modifiers & cmdKey))) {
            e->kind = TW_CANCEL;
        } else if (ev.modifiers & cmdKey) {
            if (menu(MenuKey(c)))
                e->kind = TW_QUIT;
        } else {
            e->win = input_win();
            e->line = key(e->win, c);
            if (e->line)
                e->kind = TW_LINE;
        }
        break;
    }
    case mouseDown:
        switch (FindWindow(ev.where, &w)) {
        case inMenuBar:
            if (menu(MenuSelect(ev.where)))
                e->kind = TW_QUIT;
            break;
        case inSysWindow:
            SystemClick(&ev, w);
            break;
        case inDrag:
            DragWindow(w, ev.where, &qd.screenBits.bounds);
            break;
        case inContent:
            if (w != FrontWindow()) {
                SelectWindow(w);
            } else if ((t = find(w)) != NULL) {
                ControlHandle c;
                Point pt = ev.where;
                SetPort(w);
                GlobalToLocal(&pt);
                if (FindControl(pt, w, &c) && TrackControl(c, pt, NULL)) {
                    e->kind = TW_BUTTON;
                    e->win = t;
                    e->button = c;
                }
            }
            break;
        }
        break;
    case activateEvt:
        if ((t = find((WindowPtr)ev.message)) != NULL)
            hilite_buttons(t);
        break;
    case updateEvt:
        if ((t = find((WindowPtr)ev.message)) != NULL) {
            GrafPtr old;
            GetPort(&old);
            SetPort(t->win);
            BeginUpdate(t->win);
            draw_all(t);
            EndUpdate(t->win);
            SetPort(old);
        }
        break;
    }
    return e->kind != TW_NONE;
}

void tw_init(const char *app_name, const char *about)
{
    Str255 s;
    MenuHandle fileMenu;

    InitGraf(&qd.thePort);
    InitFonts();
    InitWindows();
    InitMenus();
    TEInit();
    InitDialogs(NULL);
    InitCursor();
    FlushEvents(everyEvent, 0);
    scratchRgn = NewRgn();

    aboutText = about;
    s[0] = 1;
    s[1] = 0x14; /* the Apple logo */
    appleMenu = NewMenu(MENU_APPLE, s);
    snprintf((char *)s + 1, 254, "About %s...", app_name);
    s[0] = (unsigned char)strlen((char *)s + 1);
    AppendMenu(appleMenu, s);
    pstr(s, "(-");
    AppendMenu(appleMenu, s);
    AppendResMenu(appleMenu, 'DRVR');
    InsertMenu(appleMenu, 0);
    pstr(s, "File");
    fileMenu = NewMenu(MENU_FILE, s);
    pstr(s, "Quit/Q");
    AppendMenu(fileMenu, s);
    InsertMenu(fileMenu, 0);
    DrawMenuBar();
}
