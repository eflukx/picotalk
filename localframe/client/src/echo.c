/*
 * The echo tests. The protocol is described in
 * localframe/server/lftest/src/echo.rs:
 *   PING: 01 00 seq(2) payload  → echo of it, and our address as the server sees it
 *   BULK: 02 00 seq(2)          → 8 × 578 bytes in a known pattern
 */
#include "echo.h"

#include <string.h>

#include "appletalk.h"

#define ECHO_TYPE "LFEcho"
#define CMD_PING 1
#define CMD_BULK 2
#define PING_HEADER 12
#define PINGS 10
#define BULKS 20
#define CANCELED (-128) /* userCanceledErr */

enum { IDLE, LOOKUP, PING, BULK, STOPPING };

static TextWin *win;
static ControlHandle bPing, bBulk, bFind, bStop;
static short state = IDLE;

static NBPLookup look;
static NBPResult server;
static Boolean haveServer;

static ATPRequest r;
static unsigned char request[ATP_MAX_DATA];
static unsigned char response[ATP_MAX_PACKETS * ATP_MAX_DATA];

/* The running test. */
static unsigned short seq;
static short len, ok;
static long sent, started, total, bytes, bad;

static long ticks_to_ms(long ticks)
{
    return ticks * 1000L / 60;
}

static void buttons(void)
{
    tw_enable_button(win, bPing, state == IDLE && haveServer);
    tw_enable_button(win, bBulk, state == IDLE && haveServer);
    tw_enable_button(win, bFind, state == IDLE);
    tw_enable_button(win, bStop, state == PING || state == BULK);
}

static void set_state(short s)
{
    state = s;
    buttons();
}

static void find(void)
{
    tw_print(win, "Looking for " ECHO_TYPE " servers...");
    if (at_lookup_start(&look, "=", ECHO_TYPE, 1, true) == noErr)
        set_state(LOOKUP);
    else
        tw_print(win, "  NBP lookup failed.");
}

static void found(void)
{
    haveServer = at_lookup_results(&look, &server, 1) > 0;
    if (haveServer)
        tw_printf(win, "  Server: %.*s at %u.%u:%u", server.object[0], (const char *)server.object + 1,
                  server.addr.net, server.addr.node, server.addr.socket);
    else
        tw_print(win, "  None found. Is `lftest serve` running on the PC?");
    set_state(IDLE);
}

/* A test stopped early: say why; after a timeout look the server up
 * again, as it may have restarted under another address. */
static void stopped(OSErr err)
{
    if (err == CANCELED) {
        tw_print(win, "Stopped.");
        set_state(IDLE);
    } else {
        tw_printf(win, "No answer (error %d). Looking for the server again.", err);
        set_state(IDLE);
        find();
    }
}

static Boolean send(short length, short packets)
{
    OSErr err;
    sent = TickCount();
    err = at_request(&r, server.addr, request, length, 0x5EEDL, response, packets, 2, true);
    if (err != noErr)
        stopped(err);
    return err == noErr;
}

static void send_ping(void)
{
    short i;
    len = (seq % 3 == 2) ? 566 : 32;
    request[0] = CMD_PING;
    request[1] = 0;
    request[2] = (unsigned char)(seq >> 8);
    request[3] = (unsigned char)seq;
    for (i = 0; i < len; i++)
        request[4 + i] = (unsigned char)(i ^ seq);
    send(4 + len, 1);
}

static void send_bulk(void)
{
    request[0] = CMD_BULK;
    request[1] = 0;
    request[2] = (unsigned char)(seq >> 8);
    request[3] = (unsigned char)seq;
    send(4, ATP_MAX_PACKETS);
}

static void ping_done(void)
{
    long t = TickCount() - sent;

    if (r.bds[0].dataSize != PING_HEADER + len || response[2] != request[2] || response[3] != request[3] ||
        memcmp(response + PING_HEADER, request + 4, len) != 0) {
        tw_printf(win, "ping %u: WRONG REPLY (%d bytes)", seq, r.bds[0].dataSize);
    } else {
        ok++;
        total += t;
        tw_printf(win, "ping %u: %d bytes, %ld ms, we are node %u", seq, len, ticks_to_ms(t), response[6]);
    }
    if (++seq < PINGS) {
        send_ping();
        return;
    }
    if (ok)
        tw_printf(win, "%d/%d ok, average %ld ms (the clock ticks every 17 ms)", ok, PINGS, ticks_to_ms(total) / ok);
    else
        tw_printf(win, "0/%d ok", PINGS);
    set_state(IDLE);
}

static void bulk_done(void)
{
    short i, k;
    long t;

    for (i = 0; i < r.numOfResps; i++) {
        const unsigned char *p = response + (long)i * ATP_MAX_DATA;
        short size = r.bds[i].dataSize;
        short wrong = size != ATP_MAX_DATA || p[0] != CMD_BULK || p[1] != i || p[2] != (seq >> 8) ||
                      p[3] != (unsigned char)seq;
        for (k = 4; k < size && !wrong; k++)
            wrong = p[k] != (unsigned char)(seq + 17 * i + k);
        bytes += size;
        bad += wrong;
    }
    if (++seq < BULKS) {
        send_bulk();
        return;
    }
    t = TickCount() - started;
    if (t == 0)
        t = 1;
    tw_printf(win, "%ld bytes in %ld ms = %ld bytes/s; %ld bad packets", bytes, ticks_to_ms(t), bytes * 60 / t, bad);
    set_state(IDLE);
}

void echo_idle(void)
{
    switch (state) {
    case LOOKUP:
        if (at_lookup_done(&look))
            found();
        break;
    case PING:
    case BULK:
        if (!at_done(&r))
            break;
        if (r.h.ioResult != noErr)
            stopped(r.h.ioResult);
        else if (state == PING)
            ping_done();
        else
            bulk_done();
        break;
    case STOPPING:
        if (at_done(&r))
            stopped(CANCELED);
        break;
    }
}

void echo_button(ControlHandle b)
{
    if (b == bStop) {
        echo_cancel();
    } else if (state != IDLE) {
        return;
    } else if (b == bFind) {
        find();
    } else if (b == bPing && haveServer) {
        seq = ok = 0;
        total = 0;
        set_state(PING);
        send_ping();
    } else if (b == bBulk && haveServer) {
        seq = 0;
        bytes = bad = 0;
        tw_printf(win, "bulk: %d transactions of 8 x 578 bytes...", BULKS);
        started = TickCount();
        set_state(BULK);
        send_bulk();
    }
}

void echo_cancel(void)
{
    if (state != PING && state != BULK)
        return;
    /* Without KillSendReq (old AppleTalk) the request runs on until it
     * times out; echo_idle() waits for that in STOPPING. */
    at_cancel(&r);
    set_state(STOPPING);
}

void echo_close(void)
{
    long give_up = TickCount() + 15 * 60;

    echo_cancel();
    while (state != IDLE && TickCount() < give_up) {
        SystemTask();
        echo_idle();
    }
}

void echo_init(TextWin *w)
{
    win = w;
    bPing = tw_add_button(w, "Ping");
    bBulk = tw_add_button(w, "Bulk");
    bFind = tw_add_button(w, "Find Server");
    bStop = tw_add_button(w, "Stop");
    buttons();
    find();
}
