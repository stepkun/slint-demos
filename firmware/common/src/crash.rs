// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// The crash signal of the Picos by the backlight, called by their panic handlers

use core::ptr::write_volatile;

use crate::heap;

/// Registers of the chip that switch the backlight pin
pub struct Chip {
    // IO_BANK0, the GPIOn_CTRL register of pin n is at io_bank0 + 8 * n + 4
    io_bank0: usize,
    // SIO GPIO_OUT_CLR, GPIO_OE_SET and GPIO_OUT_XOR
    out_clr: usize,
    oe_set: usize,
    out_xor: usize,
}

pub const RP2040: Chip = Chip {
    io_bank0: 0x4001_4000,
    out_clr: 0xd000_0018,
    oe_set: 0xd000_0024,
    out_xor: 0xd000_001c,
};

/// The addresses of the RP2350 differ from the RP2040's (checked against rp-pac). The pad
/// isolation of the RP2350 was already removed by the PWM setup of the backlight.
pub const RP2350: Chip = Chip {
    io_bank0: 0x4002_8000,
    out_clr: 0xd000_0020,
    oe_set: 0xd000_0038,
    out_xor: 0xd000_0028,
};

/// Shows a crash with the backlight on `pin`: blinking about 4 times per second when the heap
/// ran out, dark on any other panic. `system_clock` in Hz.
pub fn signal(chip: &Chip, pin: u32, system_clock: u32) -> ! {
    // The backlight is normally driven by PWM. It is switched back to a plain output with direct
    // register writes, because the HAL peripherals are owned by main: off, output enabled, then
    // function 5 (SIO) in its GPIOn_CTRL register.
    let mask = 1 << pin;
    unsafe {
        write_volatile(chip.out_clr as *mut u32, mask);
        write_volatile(chip.oe_set as *mut u32, mask);
        write_volatile((chip.io_bank0 + 8 * pin as usize + 4) as *mut u32, 5);
    }
    if heap::out_of_memory() {
        loop {
            cortex_m::asm::delay(system_clock / 8);
            unsafe { write_volatile(chip.out_xor as *mut u32, mask) };
        }
    }
    loop {
        cortex_m::asm::wfi();
    }
}
