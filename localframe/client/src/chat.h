/*
 * The chat with the hub in `lftest serve` (NBP type RChat), in its own
 * window. Finds the hub, joins it with the Chooser's user name, and keeps
 * one POLL outstanding that the hub holds until there is a message.
 * Everything is asynchronous and driven by chat_idle().
 */
#ifndef CHAT_H
#define CHAT_H

#include "textwin.h"

void chat_init(TextWin *w);
void chat_idle(void);
/* A line the user typed in the chat window. */
void chat_line(const char *line);
/* Before quitting: says goodbye to the hub and returns once no request is
 * using our buffers. */
void chat_close(void);

#endif
