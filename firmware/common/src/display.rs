// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Transfer of changed rows of the frame buffer to the display. mipidsi only initializes the
// display, the frames are sent with the address commands of the MIPI DCS, which all controllers
// of the boards understand (ST7789, GC9A01, ILI9341).

use core::cell::RefCell;

use embedded_hal::digital::{ErrorType, OutputPin};
use embedded_hal::spi::SpiBus;

/// Chip select shared between the display init by mipidsi and the frame transfers
pub struct ChipSelect<'a, P>(pub &'a RefCell<P>);

impl<P> ErrorType for ChipSelect<'_, P> {
    type Error = core::convert::Infallible;
}

impl<P: OutputPin> OutputPin for ChipSelect<'_, P> {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        let _ = self.0.borrow_mut().set_low();
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        let _ = self.0.borrow_mut().set_high();
        Ok(())
    }
}

/// Sends a command with its parameters
fn command<S: SpiBus, D: OutputPin>(spi: &mut S, dc: &mut D, command: u8, parameters: &[u8]) {
    // errors are ignored, a broken frame is overwritten with the next one
    let _ = dc.set_low();
    let _ = spi.write(&[command]);
    let _ = dc.set_high();
    if !parameters.is_empty() {
        let _ = spi.write(parameters);
    }
}

/// Selects the full rows `first_row..=last_row` of a display `width` pixels wide and starts the
/// memory write, their pixel data follows
fn start_rows<S: SpiBus, D: OutputPin>(
    spi: &mut S,
    dc: &mut D,
    width: usize,
    first_row: u16,
    last_row: u16,
) {
    let last_column = (width - 1) as u16;
    // column address set
    command(
        spi,
        dc,
        0x2a,
        &[0, 0, (last_column >> 8) as u8, last_column as u8],
    );
    // row (page) address set
    command(
        spi,
        dc,
        0x2b,
        &[
            (first_row >> 8) as u8,
            first_row as u8,
            (last_row >> 8) as u8,
            last_row as u8,
        ],
    );
    // memory write, followed by the pixel data
    command(spi, dc, 0x2c, &[]);
}

/// Sends full rows of the frame buffer to the display by DMA, `rows` must contain exactly the
/// pixel data of the rows `first_row..=last_row` (embassy: the commands blocking, the pixels
/// asynchronously)
#[cfg(feature = "rp")]
pub async fn send_rows<S, D, C>(
    spi: &mut S,
    dc: &mut D,
    cs: &RefCell<C>,
    width: usize,
    (first_row, last_row): (u16, u16),
    rows: &[u8],
) where
    S: SpiBus + embedded_hal_async::spi::SpiBus,
    D: OutputPin,
    C: OutputPin,
{
    use embassy_time::{Duration, with_timeout};
    let _ = cs.borrow_mut().set_low();
    start_rows(spi, dc, width, first_row, last_row);
    // a full frame takes 12 to 16 ms on the boards, a much longer time means the DMA hangs
    if with_timeout(
        Duration::from_millis(500),
        embedded_hal_async::spi::SpiBus::write(spi, rows),
    )
    .await
    .is_err()
    {
        panic!("DMA transfer to the display timed out");
    }
    let _ = cs.borrow_mut().set_high();
}

/// Sends full rows of the frame buffer to the display, `rows` must contain exactly the pixel
/// data of the rows `first_row..=last_row` (esp-hal: by DMA through its copy buffer, blocking)
#[cfg(not(feature = "rp"))]
pub fn send_rows<S: SpiBus, D: OutputPin, C: OutputPin>(
    spi: &mut S,
    dc: &mut D,
    cs: &RefCell<C>,
    width: usize,
    (first_row, last_row): (u16, u16),
    rows: &[u8],
) {
    let _ = cs.borrow_mut().set_low();
    start_rows(spi, dc, width, first_row, last_row);
    let _ = spi.write(rows);
    let _ = cs.borrow_mut().set_high();
}
