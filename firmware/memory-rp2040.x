MEMORY {
    /*
     * The RP2040 boots from external flash: the first 256 bytes hold the
     * second-stage bootloader (boot2) that sets up the flash chip; embassy-rp
     * provides it. The Pico and Pico W have 2 MiB.
     */
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    /*
     * 264 KiB of SRAM: banks 0-3 striped (256K) and banks 4 and 5 (4K each).
     * The striped banks are used as one region.
     */
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

SECTIONS {
    /* ### Boot loader
     *
     * The boot ROM loads the first 256 bytes of flash and runs them: boot2
     * must be exactly there.
     */
    .boot2 ORIGIN(BOOT2) :
    {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;
