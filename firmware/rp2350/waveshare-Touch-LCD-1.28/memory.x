/* Copyright © 2026 Stephan Kunz */
/* SPDX-License-Identifier: GPL-3.0-only */
/* Memory layout of the Waveshare RP2350-Touch-LCD-1.28 (RP2350A with 16 MB flash) */

MEMORY {
    /* no boot2 like on the RP2040: the boot ROM finds the IMAGE_DEF block of embassy-rp */
    FLASH : ORIGIN = 0x10000000, LENGTH = 16384K
    /* SRAM0-7 (512K), SRAM8 and SRAM9 (4K each) are not used */
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
}

SECTIONS {
    /* IMAGE_DEF block of embassy-rp, it must be in the first 4 KB of the flash */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH
} INSERT AFTER .vector_table;

/* the code starts after the block */
_stext = ADDR(.start_block) + SIZEOF(.start_block);

SECTIONS {
    /* picotool binary info entries */
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

SECTIONS {
    /* the end block of the image */
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);
