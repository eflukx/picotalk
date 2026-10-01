* Merlin 32 link file: Teletekst as a GS/OS application (S16, $B3).
*
*   merlin32 link.s

         dsk   Teletekst
         typ   $B3
         asm   teletekst.s
         knd   $1000            ; static code segment, not in special memory
         lna   Teletekst
         sna   Main
