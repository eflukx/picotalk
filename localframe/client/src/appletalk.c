#include "appletalk.h"

#include <stddef.h>
#include <string.h>

/* Driver reference numbers are fixed. */
#define MPP_REFNUM (-10)
#define ATP_REFNUM (-11)

/* _Control csCodes (Inside Macintosh II). */
#define CS_LOOKUP_NAME 251 /* .MPP */
#define CS_SEND_REQUEST 255 /* .ATP */
#define CS_KILL_SEND_REQ 258 /* .ATP */

#define ATP_XO 0x20

typedef struct {
    ATHeader h;
    char unused[16];
    Ptr aKillQEl; /* 44: the request to cancel */
} ATPKillPB;

/* The drivers read these at fixed offsets; catch a compiler that aligns
 * 32-bit fields to 4 bytes. */
_Static_assert(offsetof(ATHeader, userData) == 18, "ATHeader layout");
_Static_assert(offsetof(ATHeader, csCode) == 26, "ATHeader layout");
_Static_assert(offsetof(ATPRequest, reqPointer) == 36, "ATPRequest layout");
_Static_assert(offsetof(ATPRequest, numOfBuffs) == 44, "ATPRequest layout");
_Static_assert(offsetof(ATPRequest, TRelTime) == 50, "ATPRequest layout");
_Static_assert(offsetof(NBPLookupPB, retBuffSize) == 38, "NBPLookupPB layout");
_Static_assert(offsetof(NBPLookupPB, numGotten) == 42, "NBPLookupPB layout");
_Static_assert(sizeof(BDSElement) == 12, "BDSElement layout");
_Static_assert(offsetof(ATPKillPB, aKillQEl) == 44, "ATPKillPB layout");

OSErr at_open(void)
{
    static const unsigned char mpp[] = "\x04.MPP";
    static const unsigned char atp[] = "\x04.ATP";
    short ref;
    OSErr err = OpenDriver(mpp, &ref);
    if (err == noErr)
        err = OpenDriver(atp, &ref);
    return err;
}

static unsigned char *put_pstring(unsigned char *p, const char *s)
{
    size_t n = strlen(s);
    if (n > 32)
        n = 32;
    *p++ = (unsigned char)n;
    memcpy(p, s, n);
    return p + n;
}

OSErr at_lookup_start(NBPLookup *l, const char *object, const char *type, short max, Boolean async)
{
    unsigned char *p;

    /* The entity to look up is three packed Pascal strings. */
    p = put_pstring(l->entity, object);
    p = put_pstring(p, type);
    put_pstring(p, "*");

    memset(&l->pb, 0, sizeof l->pb);
    l->pb.h.ioRefNum = MPP_REFNUM;
    l->pb.h.csCode = CS_LOOKUP_NAME;
    l->pb.interval = 4; /* 4 × 8 ticks ≈ 0.5 s */
    l->pb.count = 3;
    l->pb.entityPtr = (Ptr)l->entity;
    l->pb.retBuffPtr = (Ptr)l->buf;
    l->pb.retBuffSize = sizeof l->buf;
    l->pb.maxToGet = max;
    return async ? PBControlAsync((ParmBlkPtr)&l->pb) : PBControlSync((ParmBlkPtr)&l->pb);
}

short at_lookup_results(const NBPLookup *l, NBPResult *results, short max)
{
    /* Reply tuples: net(2) node socket enumerator object type zone. */
    const unsigned char *p = l->buf;
    short i, n = 0;

    if (l->pb.h.ioResult != noErr)
        return 0;
    for (i = 0; i < l->pb.numGotten && n < max; i++) {
        NBPResult *r = &results[n];
        short k, dup = 0;
        r->addr.net = (unsigned short)((p[0] << 8) | p[1]);
        r->addr.node = p[2];
        r->addr.socket = p[3];
        p += 5;
        memcpy(r->object, p, p[0] + 1);
        p += p[0] + 1;
        memcpy(r->type, p, p[0] + 1);
        p += p[0] + 1;
        p += p[0] + 1; /* zone */
        for (k = 0; k < n; k++)
            if (memcmp(&results[k].addr, &r->addr, sizeof r->addr) == 0)
                dup = 1;
        if (!dup)
            n++;
    }
    return n;
}

OSErr at_request(ATPRequest *r, ATAddr to, const void *data, short len, long user_bytes, void *buf,
                 short npackets, short timeout, Boolean async)
{
    short i;

    memset(r, 0, sizeof *r);
    r->h.ioRefNum = ATP_REFNUM;
    r->h.csCode = CS_SEND_REQUEST;
    r->h.userData = user_bytes;
    r->atpFlags = ATP_XO;
    r->addr = to;
    r->reqLength = len;
    r->reqPointer = (Ptr)data;
    r->bdsPointer = (Ptr)r->bds;
    r->numOfBuffs = (unsigned char)npackets;
    r->timeOutVal = (unsigned char)timeout;
    r->retryCount = 4;
    for (i = 0; i < npackets; i++) {
        r->bds[i].buffSize = ATP_MAX_DATA;
        r->bds[i].buffPtr = (Ptr)buf + (long)i * ATP_MAX_DATA;
    }
    return async ? PBControlAsync((ParmBlkPtr)r) : PBControlSync((ParmBlkPtr)r);
}

long at_response_length(const ATPRequest *r)
{
    if (r->numOfResps == 0)
        return 0;
    return (long)(r->numOfResps - 1) * ATP_MAX_DATA + r->bds[r->numOfResps - 1].dataSize;
}

OSErr at_cancel(ATPRequest *r)
{
    ATPKillPB pb;

    if (at_done(r))
        return noErr;
    memset(&pb, 0, sizeof pb);
    pb.h.ioRefNum = ATP_REFNUM;
    pb.h.csCode = CS_KILL_SEND_REQ;
    pb.aKillQEl = (Ptr)r;
    return PBControlSync((ParmBlkPtr)&pb);
}
