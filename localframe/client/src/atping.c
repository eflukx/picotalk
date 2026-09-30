/*
 * ATPing: tests the AppleTalk chain from a classic Mac.
 *
 * Finds LFEcho servers (`lftest echo` on a PC) with NBP, then on command:
 *   p  ping: 10 exactly-once ATP transactions with 32 and 566 byte
 *      payloads, checking the echo and showing the round-trip time
 *   b  bulk: 20 transactions of 8 × 578-byte response packets, checking
 *      every byte and showing the throughput
 * The protocol is described in localframe/server/lftest/src/echo.rs.
 */
#include <string.h>

#include "appletalk.h"
#include "textwin.h"

#define ECHO_TYPE "LFEcho"
#define CMD_PING 1
#define CMD_BULK 2
#define PING_HEADER 12
#define MAX_SERVERS 8

static unsigned char response[ATP_MAX_PACKETS * ATP_MAX_DATA];
static unsigned char request[ATP_MAX_DATA];
static ATPRequest r;

static NBPResult servers[MAX_SERVERS];
static short nservers, which;

static long ticks_to_ms(long ticks)
{
    return ticks * 1000L / 60;
}

static void ping(ATAddr server)
{
    short seq, ok = 0;
    long total = 0;

    for (seq = 0; seq < 10; seq++) {
        short len = (seq % 3 == 2) ? 566 : 32, i;
        long t;
        OSErr err;

        request[0] = CMD_PING;
        request[1] = 0;
        request[2] = (unsigned char)(seq >> 8);
        request[3] = (unsigned char)seq;
        for (i = 0; i < len; i++)
            request[4 + i] = (unsigned char)(i ^ seq);

        t = TickCount();
        err = at_request(&r, server, request, 4 + len, 0x5EEDL, response, 1, 2, false);
        t = TickCount() - t;
        if (err != noErr) {
            tw_printf("ping %d: error %d", seq, err);
            continue;
        }
        if (r.bds[0].dataSize != PING_HEADER + len || response[2] != request[2] || response[3] != request[3] ||
            memcmp(response + PING_HEADER, request + 4, len) != 0) {
            tw_printf("ping %d: WRONG REPLY (%d bytes)", seq, r.bds[0].dataSize);
            continue;
        }
        ok++;
        total += t;
        tw_printf("ping %d: %d bytes, %ld ms, the server sees us as node %u", seq, len, ticks_to_ms(t), response[6]);
    }
    if (ok)
        tw_printf("%d/10 ok, average %ld ms (the clock ticks every 17 ms)", ok, ticks_to_ms(total) / ok);
    else
        tw_print("0/10 ok");
}

static void bulk(ATAddr server)
{
    unsigned short seq;
    long bytes = 0, bad = 0, failed = 0, t;

    tw_print("bulk: 20 transactions of 8 x 578 bytes...");
    t = TickCount();
    for (seq = 0; seq < 20; seq++) {
        short i, k;

        request[0] = CMD_BULK;
        request[1] = 0;
        request[2] = (unsigned char)(seq >> 8);
        request[3] = (unsigned char)seq;
        if (at_request(&r, server, request, 4, 0, response, ATP_MAX_PACKETS, 2, false) != noErr) {
            failed++;
            continue;
        }
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
    }
    t = TickCount() - t;
    if (t == 0)
        t = 1;
    tw_printf("%ld bytes in %ld ms = %ld bytes/s; %ld bad packets, %ld failed transactions", bytes, ticks_to_ms(t),
              bytes * 60 / t, bad, failed);
}

static void find(void)
{
    short i;
    OSErr err;

    tw_print("Looking up =:" ECHO_TYPE "@* ...");
    which = 0;
    nservers = 0;
    err = at_lookup("=", ECHO_TYPE, servers, MAX_SERVERS, &nservers);
    if (err != noErr) {
        tw_printf("NBP lookup failed: error %d", err);
        return;
    }
    for (i = 0; i < nservers; i++)
        tw_printf("  %d: %.*s at %u.%u:%u", i + 1, servers[i].object[0], (const char *)servers[i].object + 1,
                  servers[i].addr.net, servers[i].addr.node, servers[i].addr.socket);
    if (nservers == 0)
        tw_print("  None found. Is `lftest echo` running on the PC?");
}

static void help(void)
{
    tw_print("Type a command and press Return:  p = ping,  b = bulk,  l = look up servers again,"
             "  1-8 = use that server.  Cmd-Q quits.");
}

int main(void)
{
    Boolean quit = false;
    OSErr err;

    tw_init("ATPing", "ATPing 0.1: tests NBP and ATP against `lftest echo`. Part of LocalFrame (picotalk).");
    tw_print("ATPing - LocalFrame AppleTalk test");
    err = at_open();
    if (err != noErr) {
        tw_printf("Opening AppleTalk failed: error %d.", err);
        tw_print("Turn AppleTalk on in the Chooser (-97: port in use, -98: port not configured).");
    } else {
        find();
        help();
    }

    while (!quit) {
        const char *line = tw_poll(&quit);
        if (!line)
            continue;
        tw_printf("> %s", line);
        if (err != noErr)
            tw_print("AppleTalk is not open; quit, turn it on in the Chooser and start again.");
        else if (line[0] >= '1' && line[0] <= '8' && line[0] - '1' < nservers)
            which = line[0] - '1', tw_printf("Using server %d.", which + 1);
        else if (line[0] == 'l')
            find();
        else if (line[0] != 'p' && line[0] != 'b')
            help();
        else if (nservers == 0)
            tw_print("No server; look up again with l.");
        else if (line[0] == 'p')
            ping(servers[which].addr);
        else
            bulk(servers[which].addr);
    }
    return 0;
}
