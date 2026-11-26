//! The 128 × 128 panel (ST7735S class) on the 8080 bus, and its backlight.
//!
//! Geometry [HW]: RAM offset x + 2, y + 3 with MADCTL `0xC8`.

use atj2157_hal::dma::Channel;
use atj2157_hal::gpio::Output;
use atj2157_hal::lcm::{Lcm, Pins};
use atj2157_hal::pac::gpio as pgpio;
use atj2157_hal::pwm::Pwm;

use crate::pins;

pub const WIDTH: u32 = 128;
pub const HEIGHT: u32 = 128;
const X_OFS: u8 = 2;
const Y_OFS: u8 = 3;

/// Panel initialisation: (command, parameters, delay ms after).
/// VERBATIM(vendor DRV_LCD.DRV panel table, as measured working by the
/// Rockbox port): frame rate, power and gamma settings of this panel.
const INIT: &[(u8, &[u8], u16)] = &[
    (0x11, &[], 180), // sleep out
    (0xF0, &[0x11], 0),
    (0xF4, &[0xCA, 0x78, 0x64], 0),
    (0xB1, &[0x01, 0x08, 0x05], 0),                   // frame rate, normal
    (0xB2, &[0x01, 0x08, 0x05], 0),                   // idle
    (0xB3, &[0x01, 0x08, 0x05, 0x01, 0x08, 0x05], 0), // partial
    (0xB4, &[0x03], 0),                               // inversion control
    (0xC0, &[0x28, 0x08, 0x04], 0),                   // power 1..5
    (0xC1, &[0xC0], 0),
    (0xC2, &[0x0D, 0x00], 0),
    (0xC3, &[0x8D, 0x2A], 0),
    (0xC4, &[0x8D, 0xEE], 0),
    (0xC5, &[0x07], 0), // VCOM
    (0x36, &[0xC8], 0), // MADCTL
    (
        0xE0,
        &[0x04, 0x22, 0x07, 0x0A, 0x2E, 0x30, 0x25, 0x2A, 0x28, 0x26, 0x2E, 0x3A, 0x00, 0x01, 0x03, 0x13],
        0,
    ),
    (
        0xE1,
        &[0x04, 0x16, 0x06, 0x0D, 0x2D, 0x26, 0x23, 0x27, 0x27, 0x25, 0x2D, 0x3B, 0x00, 0x01, 0x04, 0x13],
        0,
    ),
    (0x3A, &[0x05], 0), // 16 bit/pixel
    (0x29, &[], 0),     // display on
];

/// The frame buffer: 128 × 128 RGB565 in SRAM1 (`0x158000`, read by the
/// host for screenshots).
#[unsafe(link_section = ".framebuffer")]
static mut FRAMEBUFFER: [u16; (WIDTH * HEIGHT) as usize] = [0; (WIDTH * HEIGHT) as usize];
static FB_TAKEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Take the frame buffer (once).
pub fn take_framebuffer() -> Option<&'static mut [u16; (WIDTH * HEIGHT) as usize]> {
    if FB_TAKEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return None;
    }
    // SAFETY: the swap above guarantees this runs once, so the returned
    // reference is the only one.
    unsafe { (&raw mut FRAMEBUFFER).as_mut() }
}

pub struct Display {
    lcm: Lcm,
    on: bool,
}

fn delay_ms(ms: u32) {
    atj2157_hal::delay::spin_us(ms * 1000);
}

impl Display {
    /// Bring up the controller and the panel (blocking: start-up only).
    pub fn new(dma: Channel) -> Self {
        let lcm =
            Lcm::new(&Pins { bus: &pins::LCD_BUS, cs: pins::LCD_CS, cs_drive: pins::LCD_CS_DRIVE }, dma, 0);
        let rst = Output::new(pins::LCD_RESET, true);
        delay_ms(10);
        rst.set(false);
        delay_ms(20);
        rst.set(true);
        delay_ms(60);
        let mut d = Self { lcm, on: false };
        for &(c, p, ms) in INIT {
            d.lcm.cmd_with(c, p);
            if ms > 0 {
                delay_ms(u32::from(ms));
            }
        }
        d.on = true;
        d
    }

    fn window(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        self.lcm.cmd_with(0x2A, &[0, x0 as u8 + X_OFS, 0, x1 as u8 + X_OFS]);
        self.lcm.cmd_with(0x2B, &[0, y0 as u8 + Y_OFS, 0, y1 as u8 + Y_OFS]);
        self.lcm.cmd(0x2C);
    }

    /// Send rows `y0..y1` (full width) of a 128-pixel-wide RGB565 frame
    /// buffer by DMA.
    pub async fn flush_rows(&mut self, fb: &[u16], y0: u32, y1: u32) {
        if !self.on || y1 <= y0 || y1 > HEIGHT || fb.len() < (WIDTH * HEIGHT) as usize {
            return;
        }
        self.window(0, y0, WIDTH - 1, y1 - 1);
        let start = fb[(y0 * WIDTH) as usize..].as_ptr().cast::<u8>();
        self.lcm.blit(start, (y1 - y0) * WIDTH * 2, WIDTH, y1 - y0).await;
    }

    /// Send rows `y0..y1` of the frame buffer to panel rows `py0..` (the
    /// display turned by 180°: the caller has turned the band,
    /// `orpheus_ui::gfx::turn_band`).
    pub async fn flush_band(&mut self, fb: &[u16], y0: u32, y1: u32, py0: u32) {
        let h = y1.saturating_sub(y0);
        if !self.on || h == 0 || y1 > HEIGHT || py0 + h > HEIGHT || fb.len() < (WIDTH * HEIGHT) as usize {
            return;
        }
        self.window(0, py0, WIDTH - 1, py0 + h - 1);
        let start = fb[(y0 * WIDTH) as usize..].as_ptr().cast::<u8>();
        self.lcm.blit(start, h * WIDTH * 2, WIDTH, h).await;
    }

    /// Blocking full-screen flush (start-up and crash screens).
    pub fn flush_blocking(&mut self, fb: &[u16]) -> bool {
        if fb.len() < (WIDTH * HEIGHT) as usize {
            return false;
        }
        self.window(0, 0, WIDTH - 1, HEIGHT - 1);
        self.lcm.blit_blocking(fb.as_ptr().cast(), WIDTH * HEIGHT * 2, WIDTH, HEIGHT)
    }

    /// PIO fill of a rectangle with one colour (no frame buffer).
    pub fn fill_pio(&mut self, x0: u32, y0: u32, w: u32, h: u32, rgb565: u16) {
        self.window(x0, y0, x0 + w - 1, y0 + h - 1);
        for _ in 0..w * h {
            self.lcm.data((rgb565 >> 8) as u8);
            self.lcm.data(rgb565 as u8);
        }
    }

    /// Read the panel's display id (RDDID 0x04): dummy + 3 bytes.
    pub fn read_id(&mut self) -> [u8; 4] {
        let mut b = [0u8; 4];
        self.lcm.read_after(0x04, &mut b);
        b
    }

    /// Read back `n` pixels from (x, y) with RAMRD (0x2E); panel returns a
    /// dummy byte then 3 bytes (R, G, B, 6 bits each, left aligned) per pixel.
    pub fn read_pixels(&mut self, x: u32, y: u32, out: &mut [u8]) {
        self.lcm.cmd_with(0x2A, &[0, x as u8 + X_OFS, 0, (WIDTH - 1) as u8 + X_OFS]);
        self.lcm.cmd_with(0x2B, &[0, y as u8 + Y_OFS, 0, (HEIGHT - 1) as u8 + Y_OFS]);
        self.lcm.read_after(0x2E, out);
    }

    pub fn sleep(&mut self, on: bool) {
        if on {
            self.lcm.cmd(0x28);
            delay_ms(10);
            self.lcm.cmd(0x10);
            self.on = false;
        } else {
            self.lcm.cmd(0x11);
            delay_ms(120);
            self.lcm.cmd(0x29);
            self.on = true;
        }
    }

    #[must_use]
    pub fn lcm_ctl(&self) -> u32 {
        self.lcm.ctl()
    }
}

/// Backlight on PWM4.
pub struct Backlight {
    pwm: Pwm,
}

pub const BACKLIGHT_STEPS: u32 = 16;

impl Backlight {
    #[must_use]
    pub fn new(level: u32) -> Self {
        let pwm = Pwm::new(pins::BACKLIGHT_PWM, BACKLIGHT_STEPS);
        let b = Self { pwm };
        b.set(level);
        b
    }

    /// Level 0 (off) ..= 16.
    pub fn set(&self, level: u32) {
        let pin = usize::from(pins::BACKLIGHT);
        if level == 0 {
            pgpio::mfp(pin).modify(|v| (v & !0x1F) | pgpio::mfp_bits::OUT_EN);
            let _ = Output::new(pins::BACKLIGHT, false);
        } else {
            self.pwm.set_duty(level.min(BACKLIGHT_STEPS));
            self.pwm.enable();
            pgpio::mfp(pin).modify(|v| (v & !0xDF) | pins::BACKLIGHT_PWM_FUNC);
        }
    }
}
