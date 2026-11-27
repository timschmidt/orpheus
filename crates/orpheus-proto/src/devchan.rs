//! Developer channel: vendor requests on endpoint 0
//! (docs/plan/06-connectivity.md §2.5).

/// Protocol version reported by `INFO`.
pub const VERSION: u16 = 1;

/// USB identity in development builds (decision D-3: pid.codes test id).
pub const DEV_VID: u16 = 0x1209;
pub const DEV_PID: u16 = 0x0001;

/// Boot ROM recovery mode.
pub const ADFU_VID: u16 = 0x10D6;
pub const ADFU_PID: u16 = 0x10D6;
/// Rockbox port in mass-storage mode; stock firmware.
pub const ROCKBOX_PID: u16 = 0x1157;
pub const STOCK_PID: u16 = 0x1101;

pub const REQ_INFO: u8 = 0x01;
pub const REQ_LOG: u8 = 0x02;
pub const REQ_FRAME: u8 = 0x03;
pub const REQ_KEYS: u8 = 0x04;
pub const REQ_STATS: u8 = 0x05;
pub const REQ_TEST: u8 = 0x06;
pub const REQ_REBOOT: u8 = 0x07;
pub const REQ_TIME: u8 = 0x08;
pub const REQ_SET: u8 = 0x09;
pub const REQ_PEEK: u8 = 0x0A;
pub const REQ_PROVISION: u8 = 0x0B;
/// OUT: append the data stage (≤ 4 KiB) to the file opened with the
/// `fcreate` test command (only under `/.orpheus/`; development builds).
pub const REQ_PUT: u8 = 0x0C;
/// OUT/IN: firmware update through the running firmware (UPD-1; the
/// sub-operations and the status are `orpheus_update::wire`).
pub const REQ_UPDATE: u8 = 0x0D;

/// Whether the firmware stops the playing track before it queues the test
/// command `cmd` for the card task: the card task reads its command queue
/// only between tracks, so `playdir` sent while a track played waited for
/// the whole queue to end (AU-5 fix, 2026-11-24). The new queue starts
/// when the command is served.
#[inline(always)]
#[must_use]
pub fn stops_track_first(cmd: &[u8]) -> bool {
    cmd.starts_with(b"playdir ")
}

/// The firmware's test reply buffer (`audio::REPLY`).
pub const REPLY_LEN: usize = 1024;
/// Room `ls` keeps in [`REPLY_LEN`] for its `(N entries)` footer.
pub const LS_FOOTER: usize = 24;

/// Whether `ls` lists an entry of `len` bytes (its name, `/` for a folder,
/// the newline) after `used` bytes: whole names only, and the footer always
/// fits, so a long folder's reply still ends in `entries)` (the soak waited
/// for that and timed out on every big music folder, 2026-11-27).
#[inline(always)]
#[must_use]
pub fn ls_fits(used: usize, len: usize) -> bool {
    used + len <= REPLY_LEN - LS_FOOTER
}

/// Whether the card task serves test command `cmd` while a track is
/// paused, without ending it: `ls` (read only, needs no scratch). Others
/// wait for the idle loop. Since a reboot reopens the last track paused,
/// `ls` after a reboot otherwise waited until the track was stopped
/// (the soak's prologue and its state check, 2026-11-27).
#[inline(always)]
#[must_use]
pub fn served_while_paused(cmd: &[u8]) -> bool {
    cmd.starts_with(b"ls ")
}

/// What the card task's upload loop (`fcreate`, then `REQ_PUT` chunks)
/// does with a test command that arrives while the file is open: `fclose`
/// closes it; any other command ends the upload too (the file closed with
/// what it has, the reply `upload aborted N`) and is then served as if no
/// upload had been open. An upload nobody finishes can no longer hold the
/// card task: before (2026-11-27) the loop took every later command
/// without an answer until an `fclose` came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadCmd {
    Close,
    Abort,
}

#[inline(always)]
#[must_use]
pub fn upload_cmd(cmd: &[u8]) -> UploadCmd {
    if cmd == b"fclose" { UploadCmd::Close } else { UploadCmd::Abort }
}

/// An open upload with neither a chunk nor a command for this long is
/// closed (reply `upload timed out N`). The host's uploader sends each
/// chunk as soon as the last one is answered (and gives up after 10 s).
pub const UPLOAD_IDLE_MS: u64 = 10_000;

/// WebUSB `bVendorCode`: the browser asks for the landing page with
/// `GET_URL` (`bRequest` = this, `wIndex` = 2). Outside the request numbers
/// above.
pub const WEBUSB_VENDOR_CODE: u8 = 0x57;
/// Microsoft OS 2.0 descriptor set (`bRequest` = this, `wIndex` = 7): makes
/// Windows bind WinUSB to the developer interface without a driver.
pub const MSOS_VENDOR_CODE: u8 = 0x4D;
/// The WebUSB landing page (09 §5): the site's Install page.
pub const LANDING_URL: &str = "https://timschmidt.github.io/orpheus/install.html";
/// `DeviceInterfaceGUIDs` registry value the MS OS 2.0 descriptor sets.
pub const DEVICE_INTERFACE_GUID: &str = "{6E9F3E35-2C1B-4F5A-9F0B-4F5250484555}";

/// `REBOOT` targets (`wValue`).
pub const REBOOT_NORMAL: u16 = 0;
pub const REBOOT_ADFU: u16 = 1;
pub const REBOOT_STOCK: u16 = 2;
pub const REBOOT_OTHER_SLOT: u16 = 3;

/// Largest data stage the host asks for until U-USB-2 settles multi-packet
/// control transfers.
pub const SAFE_CHUNK: u16 = 64;

/// `INFO` response (little-endian, 32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub version: u16,
    pub layout: u8,
    pub personality: u8,
    pub uptime_ms: u32,
    pub battery_mv: u16,
    pub flags: u16,
    pub build_id: [u8; 16],
    pub reserved: u32,
}

impl Info {
    pub const SIZE: usize = 32;

    #[must_use]
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0..2].copy_from_slice(&self.version.to_le_bytes());
        b[2] = self.layout;
        b[3] = self.personality;
        b[4..8].copy_from_slice(&self.uptime_ms.to_le_bytes());
        b[8..10].copy_from_slice(&self.battery_mv.to_le_bytes());
        b[10..12].copy_from_slice(&self.flags.to_le_bytes());
        b[12..28].copy_from_slice(&self.build_id);
        b[28..32].copy_from_slice(&self.reserved.to_le_bytes());
        b
    }

    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let b: &[u8; Self::SIZE] = b.get(..Self::SIZE)?.try_into().ok()?;
        let mut build_id = [0u8; 16];
        build_id.copy_from_slice(&b[12..28]);
        Some(Self {
            version: u16::from_le_bytes([b[0], b[1]]),
            layout: b[2],
            personality: b[3],
            uptime_ms: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            battery_mv: u16::from_le_bytes([b[8], b[9]]),
            flags: u16::from_le_bytes([b[10], b[11]]),
            build_id,
            reserved: u32::from_le_bytes([b[28], b[29], b[30], b[31]]),
        })
    }
}

/// USB controller counters in `STATS` group 0 (USB-W1, 06-connectivity
/// §2.6). Words 0–2 have always been bus resets, SETUP packets read and
/// USB DMA errors; words 24–30 follow the soak words.
pub mod usb {
    /// Bus resets seen.
    pub const W_RESETS: usize = 0;
    /// SETUP packets read.
    pub const W_SETUPS: usize = 1;
    /// USB DMA errors and timeouts (EP0 and bulk).
    pub const W_DMA_ERRORS: usize = 2;
    /// EP0 stalls sent (rejected requests).
    pub const W_STALLS: usize = 24;
    /// Control transfers given up: a data or status stage overtaken by the
    /// next SETUP (the host cancelled), timed out, or cut by a bus reset.
    pub const W_ABORTS: usize = 25;
    /// Recoveries: controller re-initialisations after a liveness failure
    /// or a VBUS cycle.
    pub const W_REINITS: usize = 26;
    /// The last error: code (low byte, [`error_name`]) | uptime in seconds
    /// << 8.
    pub const W_LAST_ERROR: usize = 27;
    /// Controller initialisations (every personality switch, every
    /// recovery).
    pub const W_INITS: usize = 28;
    /// `CX_COMABT` (control transfer aborted by a new SETUP, FOTG210) found
    /// set when a SETUP was read.
    pub const W_COMABT: usize = 29;
    /// Liveness state: bit 0 VBUS, bit 1 a SETUP waits unread, bit 2 wedged
    /// (the watchdog is no longer fed), bits 8–15 consecutive recoveries
    /// without a SETUP answered in between.
    pub const W_STATE: usize = 30;
    /// Words in group 0 with these counters.
    pub const WORDS: usize = 31;

    pub const ERR_NONE: u8 = 0;
    /// Reading a SETUP packet failed (DMA error or timeout).
    pub const ERR_SETUP_DMA: u8 = 1;
    /// A SETUP read was cut short (the read future was dropped on a bus
    /// event, or a bus reset came first).
    pub const ERR_SETUP_LOST: u8 = 2;
    /// An EP0 data stage timed out (the host stopped asking).
    pub const ERR_DATA_TIMEOUT: u8 = 3;
    /// An EP0 data DMA reported an error.
    pub const ERR_DATA_DMA: u8 = 4;
    /// A data or status stage was overtaken by the next SETUP.
    pub const ERR_OVERTAKEN: u8 = 5;
    /// A bus reset cut an EP0 transfer.
    pub const ERR_RESET: u8 = 6;
    /// A bulk DMA failed or timed out.
    pub const ERR_BULK: u8 = 7;
    /// Recovery: a bus reset went unanswered.
    pub const ERR_RECOVER_RESET: u8 = 8;
    /// Recovery: a SETUP stayed unread.
    pub const ERR_RECOVER_SETUP: u8 = 9;
    /// Recovery: the controller was re-initialised after VBUS came back.
    pub const ERR_VBUS_REINIT: u8 = 10;
    /// Wedged: a recovery was asked for and the USB task never did it.
    pub const ERR_WEDGED_TASK: u8 = 11;
    /// Wedged: SETUPs stay unread after two recoveries.
    pub const ERR_WEDGED_SETUP: u8 = 12;

    /// Name of an error code.
    #[must_use]
    pub fn error_name(code: u8) -> &'static str {
        match code {
            ERR_NONE => "none",
            ERR_SETUP_DMA => "setup-dma",
            ERR_SETUP_LOST => "setup-lost",
            ERR_DATA_TIMEOUT => "data-timeout",
            ERR_DATA_DMA => "data-dma",
            ERR_OVERTAKEN => "overtaken",
            ERR_RESET => "reset",
            ERR_BULK => "bulk",
            ERR_RECOVER_RESET => "recover-reset",
            ERR_RECOVER_SETUP => "recover-setup",
            ERR_VBUS_REINIT => "vbus-reinit",
            ERR_WEDGED_TASK => "wedged-task",
            ERR_WEDGED_SETUP => "wedged-setup",
            _ => "?",
        }
    }

    /// The USB words of a group-0 reply.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Counters {
        pub resets: u32,
        pub setups: u32,
        pub dma_errors: u32,
        pub stalls: u32,
        pub aborts: u32,
        pub reinits: u32,
        /// Code of the last error.
        pub last_error: u8,
        /// Uptime (s) at the last error.
        pub last_error_s: u32,
        pub inits: u32,
        pub comabt: u32,
        pub vbus: bool,
        pub setup_waiting: bool,
        pub wedged: bool,
        pub failed_recoveries: u8,
    }

    impl Counters {
        /// `None` for firmware older than USB-W1 (no words 24–30).
        #[must_use]
        pub fn from_words(w: &[u32]) -> Option<Self> {
            if w.len() < WORDS {
                return None;
            }
            let st = w[W_STATE];
            Some(Self {
                resets: w[W_RESETS],
                setups: w[W_SETUPS],
                dma_errors: w[W_DMA_ERRORS],
                stalls: w[W_STALLS],
                aborts: w[W_ABORTS],
                reinits: w[W_REINITS],
                last_error: (w[W_LAST_ERROR] & 0xFF) as u8,
                last_error_s: w[W_LAST_ERROR] >> 8,
                inits: w[W_INITS],
                comabt: w[W_COMABT],
                vbus: st & 1 != 0,
                setup_waiting: st & 2 != 0,
                wedged: st & 4 != 0,
                failed_recoveries: (st >> 8) as u8,
            })
        }

        /// The state word as the firmware packs it.
        #[must_use]
        pub fn state_word(vbus: bool, setup_waiting: bool, wedged: bool, failed: u8) -> u32 {
            u32::from(vbus) | u32::from(setup_waiting) << 1 | u32::from(wedged) << 2 | u32::from(failed) << 8
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `ls` of a big folder lost its `(N entries)` footer
    /// (2026-11-27, `dev soak music` on the player: the names filled the
    /// whole 1 KiB reply, so the soak's discovery waited for `entries)` and
    /// reported "no answer after 18 s" for every music folder).
    #[test]
    fn regress_ls_of_a_big_folder_lost_its_footer() {
        let mut used = 0;
        let mut listed = 0;
        for i in 0..200 {
            // "Track 007 - a long title [abcdefghijk].mp3\n", longer every 10th.
            let len = 44 + i % 10;
            if ls_fits(used, len) {
                used += len;
                listed += 1;
            }
        }
        assert!(listed > 0 && listed < 200);
        let footer = "(4294967295 entries)".len();
        assert!(used + footer <= REPLY_LEN, "{used} + {footer}");
        // An entry that exactly fills the listing's room still fits.
        assert!(ls_fits(0, REPLY_LEN - LS_FOOTER));
        assert!(!ls_fits(1, REPLY_LEN - LS_FOOTER));
    }

    /// Regression: `ls` sent while a track was paused got no answer
    /// (2026-11-27, found by `dev soak music` on the player: a reboot
    /// reopens the last track paused, the card task read commands only
    /// between tracks, so the prologue's `ls` of each music folder timed
    /// out after 18 s and the state check after the reboot saw no files).
    #[test]
    fn regress_ls_waited_while_a_track_was_paused() {
        assert!(served_while_paused(b"ls /.orpheus"));
        assert!(served_while_paused(b"ls /"));
        // Nothing that needs the decoder's scratch or changes the track.
        for other in [&b"playdir /x"[..], b"play /a.mp3", b"put /.orpheus/x", b"synth bench 2", b"lsx", b"ls"]
        {
            assert!(!served_while_paused(other), "{:?}", core::str::from_utf8(other));
        }
        // And `ls` never stops the paused track.
        assert!(!stops_track_first(b"ls /.orpheus"));
    }

    /// Regression (BOOK-1/2b, 2026-11-27): an upload never closed (its
    /// `fcreate` served late, after the host had given up) held the card
    /// task: every later test command (`stop`, another `fcreate`, `ls`)
    /// was taken by the upload loop without an answer until `fclose`. Now
    /// only `fclose` closes it as asked; any other command aborts it and is
    /// served after; an idle upload times out.
    #[test]
    fn regress_abandoned_upload_took_every_command() {
        assert_eq!(upload_cmd(b"fclose"), UploadCmd::Close);
        for other in [&b"stop"[..], b"fcreate /.orpheus/test/x", b"ls /.orpheus", b"fclose ", b"fclos", b""] {
            assert_eq!(upload_cmd(other), UploadCmd::Abort, "{:?}", core::str::from_utf8(other));
        }
        const { assert!(UPLOAD_IDLE_MS >= 2_000 && UPLOAD_IDLE_MS <= 30_000) };
    }

    /// Regression: `dev playdir` while a track played was ignored (AU-5,
    /// 2026-11-24, found on the player: the card task reads commands only
    /// between tracks, so the new folder never started).
    #[test]
    fn regress_playdir_during_playback_stops_the_track() {
        assert!(stops_track_first(b"playdir /.orpheus/test/gapless-mp3"));
        for other in [&b"status"[..], b"playdir", b"play /a.mp3", b"stop", b"fm on"] {
            assert!(!stops_track_first(other), "{other:?}");
        }
    }

    #[test]
    fn info_roundtrip() {
        let i = Info { version: VERSION, layout: 1, uptime_ms: 1234, battery_mv: 3900, ..Default::default() };
        assert_eq!(Info::from_bytes(&i.to_bytes()), Some(i));
    }

    #[test]
    fn usb_words() {
        assert_eq!(usb::Counters::from_words(&[0; 24]), None);
        let mut w = [0u32; usb::WORDS];
        w[usb::W_SETUPS] = 7;
        w[usb::W_REINITS] = 2;
        w[usb::W_LAST_ERROR] = u32::from(usb::ERR_OVERTAKEN) | 125 << 8;
        w[usb::W_STATE] = usb::Counters::state_word(true, false, true, 3);
        let c = usb::Counters::from_words(&w).unwrap();
        assert_eq!((c.setups, c.reinits, c.last_error, c.last_error_s), (7, 2, usb::ERR_OVERTAKEN, 125));
        assert!(c.vbus && !c.setup_waiting && c.wedged);
        assert_eq!(c.failed_recoveries, 3);
        assert_eq!(usb::error_name(c.last_error), "overtaken");
        assert_eq!(usb::error_name(200), "?");
    }
}
