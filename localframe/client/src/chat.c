/*
 * The chat protocol, described in localframe/server/lftest/src/chat.rs:
 *   JOIN once, then always one POLL outstanding (the hub holds it until a
 *   message arrives), and a SAY for every line typed.
 */
#include "chat.h"

#include <string.h>

#include "appletalk.h"

#define HUB_TYPE "RChat"
enum { CMD_JOIN = 1, CMD_SAY = 2, CMD_POLL = 3, CMD_LEAVE = 4 };
#define ST_OK 0
#define MAX_TEXT 200
#define MAX_NICK 31
#define POLL_TIMEOUT 4 /* s; the hub holds a POLL for 2 s */

enum { NET_LOOKUP, NET_FINDING, NET_JOINING, NET_JOINED };

static TextWin *win;
static short state = NET_LOOKUP;
static long nextLookup;
static Boolean warnedNoHub;
static NBPLookup look;
static ATAddr hub;
static unsigned char nick[MAX_NICK + 1]; /* Pascal string */
static unsigned short after;

static ATPRequest joinR, pollR, sayR;
static Boolean joinBusy, pollBusy, sayBusy;
static unsigned char joinReq[2 + MAX_NICK + 1], pollReq[4], sayReq[2 + MAX_TEXT];
static unsigned char joinBuf[ATP_MAX_DATA], pollBuf[ATP_MAX_DATA], sayBuf[ATP_MAX_DATA];

#define OUTBOX 4
static char outbox[OUTBOX][MAX_TEXT + 1];
static short outHead, outCount;

/* A message from the hub: nick and text are Pascal strings (Mac Roman). */
static void show_message(const unsigned char *nk, const unsigned char *text)
{
    if (nk[0] == 1 && nk[1] == '*')
        tw_printf(win, "* %.*s", text[0], (const char *)text + 1);
    else
        tw_printf(win, "<%.*s> %.*s", nk[0], (const char *)nk + 1, text[0], (const char *)text + 1);
}

static void lost(const char *why)
{
    if (state == NET_LOOKUP)
        return;
    tw_printf(win, "* %s", why);
    state = NET_LOOKUP;
    nextLookup = TickCount() + 60;
}

static void start_join(void)
{
    joinReq[0] = CMD_JOIN;
    joinReq[1] = 0;
    memcpy(joinReq + 2, nick, nick[0] + 1);
    state = NET_JOINING;
    joinBusy = at_request(&joinR, hub, joinReq, 3 + nick[0], 0, joinBuf, 1, 2, true) == noErr;
    if (!joinBusy)
        lost("Could not send to the hub.");
}

static void start_poll(void)
{
    pollReq[0] = CMD_POLL;
    pollReq[1] = 0;
    pollReq[2] = (unsigned char)(after >> 8);
    pollReq[3] = (unsigned char)after;
    pollBusy = at_request(&pollR, hub, pollReq, 4, 0, pollBuf, 1, POLL_TIMEOUT, true) == noErr;
}

static void start_say(void)
{
    short n = (short)strlen(outbox[outHead]);
    sayReq[0] = CMD_SAY;
    sayReq[1] = 0;
    memcpy(sayReq + 2, outbox[outHead], n);
    sayBusy = at_request(&sayR, hub, sayReq, 2 + n, 0, sayBuf, 1, 2, true) == noErr;
}

static void start_lookup(void)
{
    if (!warnedNoHub)
        tw_print(win, "* Looking for a chat hub...");
    if (at_lookup_start(&look, "=", HUB_TYPE, 1, true) == noErr)
        state = NET_FINDING;
    else
        nextLookup = TickCount() + 5 * 60;
}

static void on_lookup(void)
{
    NBPResult found;

    if (at_lookup_results(&look, &found, 1) == 0) {
        if (!warnedNoHub)
            tw_print(win, "* No hub found yet; still looking. Is `lftest serve` running on the PC?");
        warnedNoHub = true;
        state = NET_LOOKUP;
        nextLookup = TickCount() + 5 * 60;
        return;
    }
    warnedNoHub = false;
    hub = found.addr;
    tw_printf(win, "* Found hub %.*s at node %u; joining as %.*s", found.object[0], (const char *)found.object + 1,
              hub.node, nick[0], (const char *)nick + 1);
    start_join();
}

static void on_join(void)
{
    if (joinR.h.ioResult != noErr || joinBuf[1] != ST_OK) {
        lost("Joining failed; looking for the hub again.");
        return;
    }
    after = (unsigned short)((joinBuf[2] << 8) | joinBuf[3]);
    state = NET_JOINED;
}

static void on_poll(void)
{
    const unsigned char *p = pollBuf + 4;
    short count, i;

    if (pollR.h.ioResult != noErr) {
        lost("Lost the hub; looking for it again.");
        return;
    }
    if (pollBuf[1] != ST_OK) {
        /* The hub forgot us (restart, timeout): join again. */
        if (state == NET_JOINED)
            start_join();
        return;
    }
    count = pollBuf[2];
    for (i = 0; i < count; i++) {
        const unsigned char *nk = p + 2, *text = nk + nk[0] + 1;
        if (text + text[0] + 1 > pollBuf + ATP_MAX_DATA)
            break;
        after = (unsigned short)((p[0] << 8) | p[1]);
        show_message(nk, text);
        p = text + text[0] + 1;
    }
}

static void on_say(void)
{
    if (sayR.h.ioResult != noErr) {
        lost("Lost the hub; looking for it again.");
    } else if (sayBuf[1] == ST_OK) {
        outHead = (outHead + 1) % OUTBOX;
        outCount--;
    }
}

void chat_idle(void)
{
    if (joinBusy && at_done(&joinR)) {
        joinBusy = false;
        on_join();
    }
    if (pollBusy && at_done(&pollR)) {
        pollBusy = false;
        on_poll();
    }
    if (sayBusy && at_done(&sayR)) {
        sayBusy = false;
        on_say();
    }
    if (state == NET_FINDING && at_lookup_done(&look))
        on_lookup();
    if (state == NET_LOOKUP && !joinBusy && !pollBusy && !sayBusy && TickCount() >= nextLookup)
        start_lookup();
    if (state == NET_JOINED && !pollBusy)
        start_poll();
    if (state == NET_JOINED && !sayBusy && outCount > 0)
        start_say();
}

void chat_line(const char *line)
{
    char *slot = outbox[(outHead + outCount) % OUTBOX];
    if (outCount == OUTBOX) {
        SysBeep(10);
        return;
    }
    strncpy(slot, line, MAX_TEXT);
    slot[MAX_TEXT] = 0;
    outCount++;
    if (state != NET_JOINED)
        tw_print(win, "* (not connected; will send when connected)");
}

void chat_close(void)
{
    static ATPRequest leaveR;
    static unsigned char leaveReq[2] = {CMD_LEAVE, 0};
    static unsigned char leaveBuf[ATP_MAX_DATA];
    long give_up = TickCount() + 10 * 60;

    if (state == NET_JOINED)
        at_request(&leaveR, hub, leaveReq, 2, 0, leaveBuf, 1, 1, false);
    if (joinBusy)
        at_cancel(&joinR);
    if (pollBusy)
        at_cancel(&pollR);
    if (sayBusy)
        at_cancel(&sayR);
    while (((joinBusy && !at_done(&joinR)) || (pollBusy && !at_done(&pollR)) || (sayBusy && !at_done(&sayR)) ||
            (state == NET_FINDING && !at_lookup_done(&look))) &&
           TickCount() < give_up)
        SystemTask();
}

void chat_init(TextWin *w)
{
    StringHandle chooser;

    win = w;
    /* The Chooser's user name. */
    chooser = GetString(-16096);
    if (chooser && (*chooser)[0] > 0) {
        short n = (*chooser)[0] > MAX_NICK ? MAX_NICK : (*chooser)[0];
        memcpy(nick + 1, *chooser + 1, n);
        nick[0] = (unsigned char)n;
    } else {
        memcpy(nick, "\x03Mac", 4);
    }
}
