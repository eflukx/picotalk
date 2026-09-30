/*
 * Minimal AppleTalk glue for the classic Mac OS: NBP lookup and ATP
 * requests through the .MPP and .ATP drivers.
 *
 * Retro68's default (Multiversal) interfaces have no AppleTalk.h, so the
 * parameter blocks are declared here, laid out as in Inside Macintosh II
 * ("The AppleTalk Manager"), and passed to _Control directly. This is what
 * Apple's "preferred interface" glue (PLookupName, PSendRequest) does.
 */
#ifndef APPLETALK_H
#define APPLETALK_H

#include "mac.h"

#define ATP_MAX_DATA 578 /* data bytes per ATP packet */
#define ATP_MAX_PACKETS 8

typedef struct {
    unsigned short net;
    unsigned char node;
    unsigned char socket;
} ATAddr;

/* Fields every .MPP/.ATP call shares (an I/O parameter block header). */
typedef struct {
    void *qLink;           /*  0 */
    short qType;           /*  4 */
    short ioTrap;          /*  6 */
    Ptr ioCmdAddr;         /*  8 */
    void *ioCompletion;    /* 12 */
    volatile short ioResult; /* 16: > 0 while an async call is running */
    long userData;         /* 18: ATP request user bytes */
    short reqTID;          /* 22 */
    short ioRefNum;        /* 24 */
    short csCode;          /* 26 */
} ATHeader;

typedef struct {
    short buffSize;  /*  0 */
    Ptr buffPtr;     /*  2 */
    short dataSize;  /*  6: bytes received in this packet */
    long userBytes;  /*  8 */
} BDSElement;

/* An ATP request and its response buffers. The buffers for packets
 * 0..n-1 are consecutive ATP_MAX_DATA slices of one block, so a response
 * whose packets are all full except the last arrives as one contiguous
 * byte stream. */
typedef struct {
    ATHeader h;
    unsigned char atpSocket;  /* 28 */
    unsigned char atpFlags;   /* 29 */
    ATAddr addr;              /* 30 */
    short reqLength;          /* 34 */
    Ptr reqPointer;           /* 36 */
    Ptr bdsPointer;           /* 40 */
    unsigned char numOfBuffs; /* 44 */
    unsigned char timeOutVal; /* 45: seconds */
    unsigned char numOfResps; /* 46: out */
    unsigned char retryCount; /* 47 */
    short intBuff;            /* 48 */
    unsigned char TRelTime;   /* 50 */
    unsigned char filler;     /* 51 */
    BDSElement bds[ATP_MAX_PACKETS];
} ATPRequest;

typedef struct {
    ATAddr addr;
    unsigned char object[33]; /* Pascal strings */
    unsigned char type[33];
} NBPResult;

/* Opens .MPP and .ATP. Fails with portInUse (-97) or portNotCf (-98) if
 * AppleTalk is not active on the printer port. */
OSErr at_open(void);

/* Looks up object:type@* (C strings, "=" as wildcard). Returns the number
 * of distinct entities found in *count. */
OSErr at_lookup(const char *object, const char *type, NBPResult *results, short max, short *count);

/* Starts an exactly-once request to `to`. `buf` receives up to `npackets`
 * response packets and must hold npackets * ATP_MAX_DATA bytes at an even
 * address. The request is sent again every `timeout` seconds, up to 4
 * times. `r`, `data` and `buf` must stay valid until the request is done.
 * With async set, poll at_done(); otherwise the call returns when the
 * transaction is over. Fails with reqFailed (-1096) if nobody answered. */
OSErr at_request(ATPRequest *r, ATAddr to, const void *data, short len, long user_bytes, void *buf,
                 short npackets, short timeout, Boolean async);

/* Cancels an async request (KillSendReq, AppleTalk version 48 and later).
 * If this fails the request is still running: wait for at_done(). */
OSErr at_cancel(ATPRequest *r);

/* True once an async request has finished; the result is r->h.ioResult. */
#define at_done(r) ((r)->h.ioResult <= 0)

/* Total response bytes, assuming every packet but the last is full. */
long at_response_length(const ATPRequest *r);

#endif
