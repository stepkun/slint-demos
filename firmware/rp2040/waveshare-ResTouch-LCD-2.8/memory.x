/* Copyright © 2026 Stephan Kunz */
/* SPDX-License-Identifier: GPL-3.0-only */
/* Memory layout of the Raspberry Pi Pico W (RP2040 with 2 MB flash) */

MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    /* SRAM0-3 (256K) followed directly by SRAM4 and SRAM5 (4K each) */
    RAM   : ORIGIN = 0x20000000, LENGTH = 264K
}
