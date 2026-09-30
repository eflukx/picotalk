/*
 * LFTest: tests the AppleTalk chain from a classic Mac against
 * `lftest serve` on a PC. Two windows that run at the same time:
 *   Echo Test  buttons for ping and bulk tests (see echo.c)
 *   Chat       a chat with the PC and other Macs (see chat.c)
 */
#include "appletalk.h"
#include "chat.h"
#include "echo.h"
#include "textwin.h"

int main(void)
{
    Rect sb, top, bottom;
    TextWin *testWin, *chatWin;
    Boolean quit = false;
    OSErr err;

    tw_init("LFTest", "LFTest 0.2: AppleTalk tests and chat against `lftest serve`. "
                      "Part of LocalFrame (picotalk).");

    /* Echo Test above, Chat below; each content area leaves room for the
     * window title bar (about 20 pixels) of the one below it. */
    sb = qd.screenBits.bounds;
    SetRect(&top, sb.left + 4, sb.top + 40, sb.right - 4, sb.top + 40 + (sb.bottom - sb.top - 66) * 2 / 5);
    SetRect(&bottom, top.left, top.bottom + 22, top.right, sb.bottom - 4);
    /* Created last, so the Echo Test window is in front and its buttons
     * are active. Typing always goes to the chat's input line. */
    chatWin = tw_new("Chat", &bottom, true);
    testWin = tw_new("Echo Test", &top, false);

    err = at_open();
    if (err != noErr) {
        const char *why = err == -97 || err == -98
                              ? "AppleTalk is not active (error %d). Turn it on in the Chooser, then start LFTest again."
                              : "Could not open AppleTalk (error %d).";
        tw_printf(testWin, why, err);
        tw_printf(chatWin, why, err);
    } else {
        echo_init(testWin);
        chat_init(chatWin);
    }

    while (!quit) {
        TWEvent e;
        if (err == noErr) {
            echo_idle();
            chat_idle();
        }
        if (!tw_poll(&e))
            continue;
        switch (e.kind) {
        case TW_QUIT:
            quit = true;
            break;
        case TW_LINE:
            if (err == noErr)
                chat_line(e.line);
            break;
        case TW_BUTTON:
            if (err == noErr)
                echo_button(e.button);
            break;
        case TW_CANCEL:
            if (err == noErr)
                echo_cancel();
            break;
        default:
            break;
        }
    }
    if (err == noErr) {
        echo_close();
        chat_close();
    }
    return 0;
}
