//! The wasm-bindgen surface the Install page uses (`site/install/app.js`).
//!
//! Every function that talks to the player takes the page's transport
//! object (`Usb`, `site/install/usb.js`) and, for ADFU work, the payload
//! images (the release's, checked against its manifest by the page).
//! Errors come back as `Error` strings.

use anyhow::{Result, anyhow};
use js_sys::{Function, Uint8Array};
use orpheus_host::adfu::Link;
use orpheus_host::devchan::{self, Link as DevLink};
use orpheus_host::flows::{self, Release};
use orpheus_host::mock::{self, MockPlayer};
use orpheus_host::nor::{Payloads, Sector0Permit};
use orpheus_host::transport::{Control, Transport, block_on};
use orpheus_update::ofw;
use wasm_bindgen::prelude::*;

use crate::json;

#[wasm_bindgen]
extern "C" {
    /// The page's transport (`site/install/usb.js`, `WebUsbTransport`).
    pub type Usb;
    #[wasm_bindgen(method, catch, js_name = bulkOut)]
    async fn bulk_out(this: &Usb, data: Vec<u8>, timeout: u32) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch, js_name = bulkIn)]
    async fn bulk_in(this: &Usb, len: u32, timeout: u32) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch, js_name = clearHaltIn)]
    async fn clear_halt_in(this: &Usb) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, getter, js_name = packetSize)]
    fn packet_size(this: &Usb) -> u32;
    #[wasm_bindgen(method, catch, js_name = controlIn)]
    async fn control_in(
        this: &Usb,
        request: u8,
        value: u16,
        index: u16,
        len: u16,
        timeout: u32,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch, js_name = controlOut)]
    async fn control_out(
        this: &Usb,
        request: u8,
        value: u16,
        index: u16,
        data: Vec<u8>,
        timeout: u32,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch)]
    async fn sleep(this: &Usb, ms: u32) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method)]
    fn now(this: &Usb) -> f64;
}

fn js_err(e: JsValue) -> anyhow::Error {
    let msg = e
        .as_string()
        .or_else(|| js_sys::Reflect::get(&e, &JsValue::from_str("message")).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| "USB error".into());
    anyhow!(msg)
}

fn to_js(e: anyhow::Error) -> JsValue {
    js_sys::Error::new(&format!("{e:#}")).into()
}

fn bytes(v: &JsValue) -> Vec<u8> {
    Uint8Array::new(v).to_vec()
}

/// `orpheus_host`'s transport over the page's `Usb`.
struct Js(Usb);

impl Transport for Js {
    async fn bulk_out(&mut self, data: &[u8], timeout_ms: u32) -> Result<()> {
        self.0.bulk_out(data.to_vec(), timeout_ms).await.map(|_| ()).map_err(js_err)
    }
    async fn bulk_in(&mut self, len: usize, timeout_ms: u32) -> Result<Vec<u8>> {
        let v = self.0.bulk_in(u32::try_from(len)?, timeout_ms).await.map_err(js_err)?;
        Ok(bytes(&v))
    }
    async fn clear_halt_in(&mut self) {
        let _ = self.0.clear_halt_in().await;
    }
    fn packet_size(&self) -> usize {
        self.0.packet_size() as usize
    }
    async fn control_in(&mut self, c: Control, len: u16, timeout_ms: u32) -> Result<Vec<u8>> {
        let v = self.0.control_in(c.request, c.value, c.index, len, timeout_ms).await.map_err(js_err)?;
        Ok(bytes(&v))
    }
    async fn control_out(&mut self, c: Control, data: &[u8], timeout_ms: u32) -> Result<()> {
        self.0
            .control_out(c.request, c.value, c.index, data.to_vec(), timeout_ms)
            .await
            .map(|_| ())
            .map_err(js_err)
    }
    async fn sleep_ms(&mut self, ms: u32) {
        let _ = self.0.sleep(ms).await;
    }
    fn now_ms(&self) -> u64 {
        self.0.now() as u64
    }
}

fn reporter(f: &Function) -> impl FnMut(&str, u64, u64) + '_ {
    move |what, done, total| {
        let _ = f.call3(
            &JsValue::NULL,
            &JsValue::from_str(what),
            &JsValue::from_f64(done as f64),
            &JsValue::from_f64(total as f64),
        );
    }
}

struct Blobs {
    ymon: Vec<u8>,
    nordump: Vec<u8>,
    norprog: Vec<u8>,
    norprog0: Option<Vec<u8>>,
}

impl Blobs {
    fn payloads(&self) -> Payloads<'_> {
        Payloads {
            ymon: &self.ymon,
            nordump: &self.nordump,
            norprog: &self.norprog,
            norprog0: self.norprog0.as_deref(),
        }
    }
}

// ------------------------------------------------------------ files

/// Check a whole `.ofw` file (header, sizes, SHA-256); its header as JSON.
#[wasm_bindgen(js_name = checkOfw)]
pub fn check_ofw(file: &[u8]) -> Result<String, JsValue> {
    let (h, _) = devchan::check_file(file).map_err(to_js)?;
    Ok(json::ofw(&h, &file[..ofw::HEADER_SIZE]))
}

/// The slot image inside a checked `.ofw` file.
#[wasm_bindgen(js_name = ofwImage)]
pub fn ofw_image(file: &[u8]) -> Result<Vec<u8>, JsValue> {
    devchan::check_file(file).map_err(to_js)?;
    Ok(file[ofw::HEADER_SIZE..].to_vec())
}

/// Lowercase hex SHA-256 (the release manifest's).
#[wasm_bindgen(js_name = sha256Hex)]
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    flows::sha256_hex(data)
}

/// "stock", "orpheus" or "other" for a whole-chip image.
#[wasm_bindgen(js_name = classifyChip)]
#[must_use]
pub fn classify_chip(chip: &[u8]) -> String {
    json::chip_kind(flows::classify(chip)).into()
}

// ------------------------------------------------------------ Orpheus

/// The developer channel's INFO as JSON.
#[wasm_bindgen(js_name = devInfo)]
pub async fn dev_info(usb: Usb) -> Result<String, JsValue> {
    let mut d = DevLink::new(Js(usb));
    d.info().await.map(|i| json::info(&i)).map_err(to_js)
}

/// Update through the running firmware (UPD-1): returns the target slot
/// (0 A, 1 B). The caller reboots ([`reboot`]).
#[wasm_bindgen]
pub async fn update(usb: Usb, file: Vec<u8>, progress: Function) -> Result<u8, JsValue> {
    let mut d = DevLink::new(Js(usb));
    d.update(&file, &mut reporter(&progress)).await.map_err(to_js)
}

/// `REBOOT` (0 normal, 1 USB recovery).
#[wasm_bindgen]
pub async fn reboot(usb: Usb, target: u16) {
    let mut d = DevLink::new(Js(usb));
    d.reboot(target).await;
}

// ------------------------------------------------------------ ADFU

/// Read the whole flash twice; the backup (8 MiB).
#[wasm_bindgen]
pub async fn backup(
    usb: Usb,
    ymon: Vec<u8>,
    nordump: Vec<u8>,
    progress: Function,
) -> Result<Vec<u8>, JsValue> {
    let b = Blobs { ymon, nordump, norprog: Vec::new(), norprog0: None };
    let mut d = Link::new(Js(usb));
    flows::backup(&mut d, &b.payloads(), &mut reporter(&progress)).await.map_err(to_js)
}

/// First install from the original firmware. `confirmed`: the person
/// confirmed the boot-record step.
#[wasm_bindgen(js_name = firstInstall)]
#[allow(clippy::too_many_arguments)]
pub async fn first_install(
    usb: Usb,
    ymon: Vec<u8>,
    nordump: Vec<u8>,
    norprog: Vec<u8>,
    norprog0: Vec<u8>,
    backup: Vec<u8>,
    ofw_file: Vec<u8>,
    stage2: Vec<u8>,
    stage1: Vec<u8>,
    confirmed: bool,
    progress: Function,
) -> Result<(), JsValue> {
    let b = Blobs { ymon, nordump, norprog, norprog0: Some(norprog0) };
    let permit = Sector0Permit::new(&backup, confirmed)
        .ok_or_else(|| to_js(anyhow!("the boot-record step needs the backup and your confirmation")))?;
    devchan::check_file(&ofw_file).map_err(to_js)?;
    let rel = Release { firmware: &ofw_file[ofw::HEADER_SIZE..], stage2: &stage2, stage1: &stage1 };
    let mut d = Link::new(Js(usb));
    flows::first_install(&mut d, &b.payloads(), &backup, &rel, &permit, &mut reporter(&progress))
        .await
        .map_err(to_js)
}

/// Restore the original firmware from `backup`; sectors written.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub async fn restore(
    usb: Usb,
    ymon: Vec<u8>,
    nordump: Vec<u8>,
    norprog: Vec<u8>,
    norprog0: Vec<u8>,
    backup: Vec<u8>,
    confirmed: bool,
    progress: Function,
) -> Result<u32, JsValue> {
    let b = Blobs { ymon, nordump, norprog, norprog0: Some(norprog0) };
    let permit = Sector0Permit::new(&backup, confirmed)
        .ok_or_else(|| to_js(anyhow!("restoring the boot record needs the backup and your confirmation")))?;
    let mut d = Link::new(Js(usb));
    flows::restore(&mut d, &b.payloads(), &backup, &permit, &mut reporter(&progress)).await.map_err(to_js)
}

/// Recovery: the two slots as JSON.
#[wasm_bindgen]
pub async fn slots(usb: Usb, ymon: Vec<u8>, nordump: Vec<u8>) -> Result<String, JsValue> {
    let b = Blobs { ymon, nordump, norprog: Vec::new(), norprog0: None };
    let mut d = Link::new(Js(usb));
    flows::slots(&mut d, &b.payloads()).await.map(|s| json::slots(&s)).map_err(to_js)
}

/// Recovery: start the other slot next.
#[wasm_bindgen(js_name = switchSlot)]
pub async fn switch_slot(usb: Usb, ymon: Vec<u8>, nordump: Vec<u8>, norprog: Vec<u8>) -> Result<(), JsValue> {
    let b = Blobs { ymon, nordump, norprog, norprog0: None };
    let mut d = Link::new(Js(usb));
    flows::switch_slot(&mut d, &b.payloads()).await.map_err(to_js)
}

/// Recovery: the release firmware into slot A again.
#[wasm_bindgen]
pub async fn reinstall(
    usb: Usb,
    ymon: Vec<u8>,
    nordump: Vec<u8>,
    norprog: Vec<u8>,
    ofw_file: Vec<u8>,
    progress: Function,
) -> Result<(), JsValue> {
    devchan::check_file(&ofw_file).map_err(to_js)?;
    let b = Blobs { ymon, nordump, norprog, norprog0: None };
    let mut d = Link::new(Js(usb));
    flows::reinstall(&mut d, &b.payloads(), &ofw_file[ofw::HEADER_SIZE..], &mut reporter(&progress))
        .await
        .map_err(to_js)
}

/// Leave USB recovery: reset through ymon or the ROM.
#[wasm_bindgen(js_name = resetPlayer)]
pub async fn reset_player(usb: Usb) {
    let mut d = Link::new(Js(usb));
    d.reset().await;
}

// ------------------------------------------------------------ the simulated player

/// A simulated player for the demo mode and the tests (`MockPlayer`).
#[wasm_bindgen]
pub struct MockDevice {
    m: MockPlayer,
}

#[wasm_bindgen]
impl MockDevice {
    /// A player with the original firmware.
    #[must_use]
    pub fn stock(seed: u8) -> MockDevice {
        MockDevice { m: MockPlayer::stock(seed) }
    }

    /// A player running Orpheus from slot A (`image`: a slot image).
    #[wasm_bindgen(js_name = withOrpheus)]
    #[must_use]
    pub fn with_orpheus(image: &[u8]) -> MockDevice {
        MockDevice { m: MockPlayer::stock(1).with_orpheus(image) }
    }

    /// "adfu", "firmware" or "dead".
    #[must_use]
    pub fn mode(&self) -> String {
        match self.m.mode {
            mock::Mode::Adfu => "adfu",
            mock::Mode::Firmware => "firmware",
            mock::Mode::Dead => "dead",
        }
        .into()
    }

    #[wasm_bindgen(js_name = bulkOut)]
    pub fn bulk_out(&mut self, data: &[u8]) -> Result<(), JsValue> {
        block_on(self.m.bulk_out(data, 0)).map_err(to_js)
    }
    #[wasm_bindgen(js_name = bulkIn)]
    pub fn bulk_in(&mut self, len: u32) -> Result<Vec<u8>, JsValue> {
        block_on(self.m.bulk_in(len as usize, 0)).map_err(to_js)
    }
    #[wasm_bindgen(js_name = controlIn)]
    pub fn control_in(&mut self, request: u8, value: u16, index: u16, len: u16) -> Result<Vec<u8>, JsValue> {
        block_on(self.m.control_in(Control { request, value, index }, len, 0)).map_err(to_js)
    }
    #[wasm_bindgen(js_name = controlOut)]
    pub fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), JsValue> {
        block_on(self.m.control_out(Control { request, value, index }, data, 0)).map_err(to_js)
    }
    /// Time passes (the firmware's background read-back runs).
    pub fn tick(&mut self, ms: u32) {
        block_on(self.m.sleep_ms(ms));
    }
    /// The whole simulated chip.
    #[must_use]
    pub fn chip(&self) -> Vec<u8> {
        self.m.chip()
    }
    /// Forbidden writes attempted (sector 0 without the sector-0 payload,
    /// the vendor data).
    #[must_use]
    pub fn violations(&self) -> u32 {
        self.m.violations as u32
    }
    #[wasm_bindgen(js_name = sector0Writes)]
    #[must_use]
    pub fn sector0_writes(&self) -> u32 {
        self.m.sector0_writes as u32
    }
    /// The slot stage 2 starts (-1: USB recovery).
    #[must_use]
    pub fn booted(&self) -> i32 {
        self.m.booted().map_or(-1, |i| i as i32)
    }
    /// The next whole-chip read returns one wrong bit.
    #[wasm_bindgen(js_name = flakyRead)]
    pub fn flaky_read(&mut self) {
        self.m.flaky_read = true;
    }
    /// The `n`th payload call from now fails (an unplugged cable).
    #[wasm_bindgen(js_name = failCall)]
    pub fn fail_call(&mut self, n: u32) {
        self.m.fail_call = (n > 0).then_some(self.m.calls + n as usize);
    }
    /// Power cycle: stage 2 decides.
    pub fn restart(&mut self) {
        self.m.boot();
    }
    /// Into USB recovery (as the boot counter or the Next key would).
    #[wasm_bindgen(js_name = toRecovery)]
    pub fn to_recovery(&mut self) {
        self.m.mode = mock::Mode::Adfu;
        self.m.ymon = false;
    }
}

/// Stand-ins for the payloads the simulated player accepts:
/// "ymon", "nordump", "norprog", "norprog0".
#[wasm_bindgen(js_name = mockPayload)]
#[must_use]
pub fn mock_payload(kind: &str) -> Vec<u8> {
    mock::payload(match kind {
        "ymon" => mock::TAG_YMON,
        "nordump" => mock::TAG_NORDUMP,
        "norprog0" => mock::TAG_NORPROG0,
        _ => mock::TAG_NORPROG,
    })
}

/// A small but complete `.ofw` file for the simulated player.
#[wasm_bindgen(js_name = mockOfw)]
#[must_use]
pub fn mock_ofw(seed: u8, version: &str) -> Vec<u8> {
    let img = orpheus_update::sim::image(seed, 0x3000, 2);
    let mut f =
        orpheus_update::sim::ofw_header(&img, version, "Demo build\nFor the simulated player").to_vec();
    f.extend_from_slice(&img);
    f
}

/// Stage 1 / stage 2 stand-ins for the simulated player's first install.
#[wasm_bindgen(js_name = mockStage)]
#[must_use]
pub fn mock_stage(which: u8) -> Vec<u8> {
    if which == 1 {
        let mut s = vec![0x11u8; flows::STAGE1_LEN];
        s[2] = 0xA5;
        s
    } else {
        let mut s = orpheus_proto::layout::STAGE2_MAGIC.to_le_bytes().to_vec();
        s.resize(700, 0x5A);
        s
    }
}
