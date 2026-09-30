#include "textwin.h"

#include <stdarg.h>
#include <stdio.h>
#include <string.h>

enum { MENU_APPLE = 128, MENU_FILE = 129 };
enum { ITEM_ABOUT = 1, ITEM_QUIT = 1 };

#define NLINES 128 /* transcript lines kept */
#define MAXCOLS 120

static WindowPtr win;
static MenuHandle appleMenu;
static const char *aboutText;
static char lines[NLINES][MAXCOLS + 1];
static long nlines; /* lines ever added; line k is in lines[k % NLINES] */
static Rect textRect, inputRect;
static short lineH, ascent, cols, rows;
static RgnHandle scratchRgn;

static char input[TW_MAX_INPUT + 1];
static short inputLen;
static char submitted[TW_MAX_INPUT + 1];

static void layout(void)
{
    FontInfo fi;
    Rect r = win->portRect;

    GetFontInfo(&fi);
    ascent = fi.ascent;
    lineH = fi.ascent + fi.descent + fi.leading;
    InsetRect(&r, 4, 4);
    inputRect = r;
    inputRect.top = r.bottom - lineH;
    textRect = r;
    textRect.bottom = inputRect.top - 6;
    rows = (textRect.bottom - textRect.top) / lineH;
    if (rows < 1)
        rows = 1;
    textRect.top = textRect.bottom - rows * lineH;
    cols = (r.right - r.left) / CharWidth('m');
    if (cols > MAXCOLS)
        cols = MAXCOLS;
    if (cols < 20) /* add_line() needs room for a few characters */
        cols = 20;
}

static void draw_line(long k)
{
    long row = rows - 1 - (nlines - 1 - k);
    char *s;
    if (row < 0 || k < 0)
        return;
    s = lines[k % NLINES];
    MoveTo(textRect.left, textRect.top + (short)row * lineH + ascent);
    DrawText(s, 0, (short)strlen(s));
}

static void draw_input(void)
{
    short fit = cols - 3, start = inputLen > fit ? inputLen - fit : 0;
    SetPort(win);
    EraseRect(&inputRect);
    MoveTo(inputRect.left, inputRect.top + ascent);
    DrawText("> ", 0, 2);
    DrawText(input, start, inputLen - start);
    DrawChar('_');
}

static void draw_all(void)
{
    long k;
    EraseRect(&win->portRect);
    for (k = nlines - rows; k < nlines; k++)
        draw_line(k);
    MoveTo(win->portRect.left, inputRect.top - 4);
    LineTo(win->portRect.right, inputRect.top - 4);
    draw_input();
}

/* Adds one screen line: scroll the transcript up and draw it at the bottom. */
static void push_line(const char *s, short len)
{
    char *dst = lines[nlines % NLINES];
    memcpy(dst, s, len);
    dst[len] = 0;
    nlines++;
    SetPort(win);
    ScrollRect(&textRect, 0, -lineH, scratchRgn);
    draw_line(nlines - 1);
}

/* One logical line, wrapped at word boundaries; continuation lines are
 * indented. */
static void add_line(const char *s, short len)
{
    char buf[MAXCOLS + 1];
    short indent = 0;

    do {
        short room = cols - indent, n = len;
        if (n > room) {
            n = room;
            while (n > room / 2 && s[n] != ' ')
                n--;
            if (s[n] != ' ')
                n = room;
        }
        memset(buf, ' ', indent);
        memcpy(buf + indent, s, n);
        push_line(buf, indent + n);
        s += n;
        len -= n;
        while (len > 0 && *s == ' ')
            s++, len--;
        indent = 2;
    } while (len > 0);
}

void tw_print(const char *s)
{
    for (;;) {
        const char *nl = strchr(s, '\n');
        if (!nl) {
            if (*s)
                add_line(s, (short)strlen(s));
            return;
        }
        add_line(s, (short)(nl - s));
        s = nl + 1;
    }
}

void tw_printf(const char *fmt, ...)
{
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    tw_print(buf);
}

static Boolean menu(long choice)
{
    short item = choice & 0xFFFF;
    Boolean quit = false;

    if ((choice >> 16) == MENU_APPLE) {
        if (item == ITEM_ABOUT) {
            tw_print(aboutText);
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

static const char *key(char c)
{
    if (c == '\r' || c == 3) {
        if (inputLen == 0)
            return NULL;
        memcpy(submitted, input, inputLen);
        submitted[inputLen] = 0;
        inputLen = 0;
        draw_input();
        return submitted;
    }
    if (c == 8) {
        if (inputLen > 0)
            inputLen--;
    } else if ((unsigned char)c >= ' ' && c != 0x7F && inputLen < TW_MAX_INPUT) {
        input[inputLen++] = c;
    } else {
        return NULL;
    }
    draw_input();
    return NULL;
}

const char *tw_poll(Boolean *quit)
{
    EventRecord ev;
    WindowPtr w;

    SystemTask();
    if (!GetNextEvent(everyEvent, &ev))
        return NULL;
    switch (ev.what) {
    case keyDown:
    case autoKey:
        if (ev.modifiers & cmdKey) {
            *quit = menu(MenuKey((char)(ev.message & charCodeMask)));
            return NULL;
        }
        return key((char)(ev.message & charCodeMask));
    case mouseDown:
        switch (FindWindow(ev.where, &w)) {
        case inMenuBar:
            *quit = menu(MenuSelect(ev.where));
            break;
        case inSysWindow:
            SystemClick(&ev, w);
            break;
        case inDrag:
            DragWindow(w, ev.where, &qd.screenBits.bounds);
            break;
        case inGoAway:
            *quit = TrackGoAway(w, ev.where);
            break;
        case inContent:
            if (w != FrontWindow())
                SelectWindow(w);
            break;
        }
        break;
    case updateEvt:
        if ((WindowPtr)ev.message == win) {
            SetPort(win);
            BeginUpdate(win);
            draw_all();
            EndUpdate(win);
        }
        break;
    }
    return NULL;
}

static void pstr(unsigned char *dst, const char *s)
{
    size_t n = strlen(s);
    if (n > 255)
        n = 255;
    dst[0] = (unsigned char)n;
    memcpy(dst + 1, s, n);
}

void tw_init(const char *title, const char *about)
{
    Str255 s;
    MenuHandle fileMenu;
    Rect r;

    InitGraf(&qd.thePort);
    InitFonts();
    InitWindows();
    InitMenus();
    TEInit();
    InitDialogs(NULL);
    InitCursor();
    FlushEvents(everyEvent, 0);

    aboutText = about;
    s[0] = 1;
    s[1] = 0x14; /* the Apple logo */
    appleMenu = NewMenu(MENU_APPLE, s);
    snprintf((char *)s + 1, 254, "About %s...", title);
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

    /* Only valid after InitGraf. */
    r = qd.screenBits.bounds;
    r.top += 40;
    InsetRect(&r, 6, 0);
    r.bottom -= 6;
    pstr(s, title);
    win = NewWindow(NULL, &r, s, true, documentProc, (WindowPtr)-1, true, 0);
    SetPort(win);
    TextFont(4); /* Monaco */
    TextSize(9);
    layout();
    scratchRgn = NewRgn();
    draw_all();
}
