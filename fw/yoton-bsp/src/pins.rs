//! Pin assignments [HW] unless noted (01-hardware §5).

/// LCD 8080 bus: D0–D7, RS (8), RD (10), WR (11) — function 2.
pub const LCD_BUS: [u8; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11];
/// LCD chip select (function 2, drive 2).
pub const LCD_CS: u8 = 43;
pub const LCD_CS_DRIVE: u32 = 2;
/// LCD reset (GPIO out, active low).
pub const LCD_RESET: u8 = 21;
/// Backlight: PWM4 (function 0x10) or GPIO low = off.
pub const BACKLIGHT: u8 = 22;
pub const BACKLIGHT_PWM: usize = 4;
pub const BACKLIGHT_PWM_FUNC: u32 = 0x10;
/// Key ladder 2 input (function 0x3B with `0xC01C0304 = 0x3001`).
pub const KEY_LADDER2: u8 = 17;
/// SD card detect [code].
pub const SD_DETECT: u8 = 18;
/// Bluetooth module enable (high = on) [code].
pub const BT_ENABLE: u8 = 20;
/// I2C bus to tuner and accelerometer (TWI1 pads) [code].
pub const I2C_SCL: u8 = 24;
pub const I2C_SDA: u8 = 25;
/// Bluetooth host-busy output [code].
pub const BT_HOST_BUSY: u8 = 31;

/// SD0 pads: (pin, drive) for CLK, CMD, D0, D1, D2, D3; function word
/// `0x804` [HW].
pub const SD_PADS: [(u8, u32); 6] = [(30, 5), (32, 3), (33, 3), (29, 3), (28, 3), (27, 3)];
pub const SD_FUNC: u32 = 0x804;
