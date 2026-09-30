/*
 * The echo tests against `lftest serve` (NBP type LFEcho): ping (round-trip
 * time, payload check) and bulk (8-packet responses, throughput), run from
 * buttons in their own window. Everything is asynchronous and driven by
 * echo_idle(), so the chat keeps running during a test.
 */
#ifndef ECHO_H
#define ECHO_H

#include "textwin.h"

void echo_init(TextWin *w);
void echo_idle(void);
void echo_button(ControlHandle b);
/* Stops a running test (Esc, the Stop button). */
void echo_cancel(void);
/* Before quitting: returns once no request is using our buffers. */
void echo_close(void);

#endif
