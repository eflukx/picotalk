/*
 * RChat: chat with a PC (and other Macs) over AppleTalk.
 *
 * Finds an RChat hub (`rchat` on a PC) with NBP and speaks the protocol in
 * localframe/server/rchat/src/proto.rs:
 *   JOIN once, then always one POLL outstanding (the hub holds it until a
 *   message arrives), and a SAY for every line typed.
 * The ATP requests are asynchronous, so typing never waits for the
 * network. The nickname is the Chooser's user name.
 */
#include <string.h>

#include "appletalk.h"
#include "textwin.h"

#define HUB_TYPE "RChat"
enum { CMD_JOIN = 1, CMD_SAY = 2, CMD_POLL = 3, CMD_LEAVE = 4 };
#define ST_OK 0
#define MAX_TEXT 200
#define MAX_NICK 31
#define POLL_TIMEOUT 4 /* s; the hub holds a POLL for 2 s */

enum { NET_OFF = -1, NET_LOOKUP, NET_JOINING, NET_JOINED };

static short state = NET_LOOKUP;
static long nextLookup;
static Boolean warnedNoHub;
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
        tw_printf("* %.*s", text[0], (const char *)text + 1);
    else
        tw_printf("<%.*s> %.*s", nk[0], (const char *)nk + 1, text[0], (const char *)text + 1);
}

static void lost(const char *why)
{
    if (state == NET_LOOKUP)
        return;
    tw_printf("* %s", why);
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

static void lookup(void)
{
    NBPResult found[1];
    short n = 0;

    if (!warnedNoHub)
        tw_print("* Looking for an RChat hub...");
    if (at_lookup("=", HUB_TYPE, found, 1, &n) != noErr || n == 0) {
        if (!warnedNoHub)
            tw_print("* No hub found yet; still looking. Is `rchat` running on the PC?");
        warnedNoHub = true;
        nextLookup = TickCount() + 5 * 60;
        return;
    }
    warnedNoHub = false;
    hub = found[0].addr;
    tw_printf("* Found hub %.*s at node %u; joining as %.*s", found[0].object[0], (const char *)found[0].object + 1,
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

static void net_idle(void)
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
    if (state == NET_LOOKUP && !joinBusy && !pollBusy && !sayBusy && TickCount() >= nextLookup)
        lookup();
    if (state == NET_JOINED && !pollBusy)
        start_poll();
    if (state == NET_JOINED && !sayBusy && outCount > 0)
        start_say();
}

/* Say goodbye, and make sure the driver no longer writes into our buffers
 * once we are gone. */
static void net_close(void)
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
    while ((joinBusy && !at_done(&joinR)) || (pollBusy && !at_done(&pollR)) || (sayBusy && !at_done(&sayR))) {
        if (TickCount() > give_up)
            break;
        SystemTask();
    }
}

static void send_line(const char *line)
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
        tw_print("* (not connected; will send when connected)");
}

int main(void)
{
    Boolean quit = false;
    StringHandle chooser;
    OSErr err;

    tw_init("RChat", "RChat 0.1: chat over AppleTalk with the `rchat` hub. Part of LocalFrame (picotalk).");

    /* The Chooser's user name. */
    chooser = GetString(-16096);
    if (chooser && (*chooser)[0] > 0) {
        short n = (*chooser)[0] > MAX_NICK ? MAX_NICK : (*chooser)[0];
        memcpy(nick + 1, *chooser + 1, n);
        nick[0] = (unsigned char)n;
    } else {
        memcpy(nick, "\x03Mac", 4);
    }

    err = at_open();
    if (err != noErr) {
        tw_printf(err == -97 || err == -98
                      ? "AppleTalk is not active (error %d). Turn it on in the Chooser, then start RChat again."
                      : "Could not open AppleTalk (error %d).",
                  err);
        state = NET_OFF;
    }

    while (!quit) {
        const char *line;
        if (state != NET_OFF)
            net_idle();
        line = tw_poll(&quit);
        if (line && state != NET_OFF)
            send_line(line);
    }
    if (state != NET_OFF)
        net_close();
    return 0;
}
