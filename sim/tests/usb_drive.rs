//! USB-6 / UI-6: the USB Drive app over the simulator's export, which uses
//! the firmware's checks and extent mapping on the simulated card: what
//! the computer reads is the file, byte for byte; a writable export is
//! only possible for a test image in test mode and changes only that
//! file; a scattered file is refused with the app saying why; the card is
//! the computer's while presented and the player's again after Back.

use orpheus_apps::services::{
    CardState, DriveMedium, DrivePhase, DriveRefusal, DriveRequest, StorageService, UsbDriveService,
};
use orpheus_sim::AppSim;
use orpheus_sim::card::{SCATTERED_RUNS, TEST_IMG, image_bytes};
use orpheus_usb::msc::extent::POOL_EXTENTS;

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

/// Home grid: USB Drive is the tenth app.
const APP: &str = "down*2 next ok wait:100 expect-screen:UsbDrive";
/// Images › rescue.iso (the second row), its options.
const ISO: &str = "ok wait:100 expect-screen:DriveFiles down ok expect-screen:DriveOptions";
/// Test images › test.img (the second row).
const TEST: &str = "down ok wait:100 expect-screen:DriveFiles down ok expect-screen:DriveOptions";

fn hash(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3))
}

#[test]
fn iso_is_a_read_only_cd_and_reads_back_whole() {
    let mut s = sim(&format!("set:drivehost=1 {APP} {ISO}"));
    // Defaults for .iso: CD-ROM, read-only (forced).
    run(&mut s, "ok wait:400 expect-screen:DriveStatus");
    let i = s.sys.drive();
    assert_eq!((i.phase, i.medium, i.read_only), (DrivePhase::Attached, DriveMedium::Cd, true));
    assert_eq!(s.sys.card(), CardState::UsbOwned, "the card is the computer's");
    run(&mut s, "wait:500");
    let file = s.sys.media.read_file("/Images/rescue.iso");
    assert_eq!(s.sys.drive.read_back, file, "the computer read the file");
    assert_eq!(s.sys.drive().bytes_read, 2048);
    // Back: detached, the card is back, the file list again.
    run(&mut s, "back wait:100 expect-screen:DriveFiles");
    assert_eq!(s.sys.drive().phase, DrivePhase::Off);
    assert_eq!(s.sys.card(), CardState::Ready);
}

#[test]
fn test_image_hashes_like_the_file() {
    let mut s = sim(&format!("set:drivehost=1 {APP} {TEST} ok wait:3000"));
    let i = s.sys.drive();
    assert_eq!((i.medium, i.read_only, i.host), (DriveMedium::RemovableDisk, true, true));
    assert_eq!(i.bytes_read, TEST_IMG as u64);
    assert_eq!(hash(&s.sys.drive.read_back), hash(&image_bytes(TEST_IMG, 0x5A)));
    assert_eq!(s.sys.drive.runs().len(), 1, "written in one piece: one run");
    run(&mut s, "back");
}

#[test]
fn read_only_unless_test_image_in_test_mode() {
    // Not in test mode: the toggle does not move, a toast says so.
    let mut s = sim(&format!("{APP} {TEST} up ok wait:50"));
    run(&mut s, "down ok wait:400 expect-screen:DriveStatus");
    assert!(s.sys.drive().read_only);
    run(&mut s, "back");
    // A user image in test mode: still forced.
    let mut s = sim(&format!(
        "set:developer=1 {APP} {ISO} up*2 ok down ok down ok wait:400 expect-screen:DriveStatus"
    ));
    let i = s.sys.drive();
    assert_eq!((i.medium, i.read_only), (DriveMedium::RemovableDisk, true));
    run(&mut s, "back");
}

#[test]
fn service_refuses_writable_user_images() {
    // Asked directly (not through the app): the service's own checks.
    let mut s = sim(&format!("set:developer=1 {APP} {ISO}"));
    let f = s.sys.media.file_ref("/Images/boot.img").expect("listed");
    s.sys.drive_start(DriveRequest { file: f, medium: DriveMedium::RemovableDisk, write: true }, "boot.img");
    assert_eq!(s.sys.drive().phase, DrivePhase::Failed);
    assert_eq!(s.sys.drive().refusal, Some(DriveRefusal::WriteOutsideOrpheus));
    assert_eq!(s.sys.card(), CardState::Ready, "the card never left");
    s.sys.drive_stop();
    s.sys.drive_start(DriveRequest { file: f, medium: DriveMedium::Cd, write: true }, "boot.img");
    assert_eq!(s.sys.drive().refusal, Some(DriveRefusal::CdReadOnly));
    s.sys.drive_stop();
    // 512 bytes are less than one CD block.
    s.sys.drive_start(DriveRequest { file: f, medium: DriveMedium::Cd, write: false }, "boot.img");
    assert_eq!(s.sys.drive().refusal, Some(DriveRefusal::TooSmall));
    s.sys.drive_stop();
    // Out of test mode a test image is not writable either.
    run(&mut s, "set:developer=0");
    let t = s.sys.media.file_ref("/.orpheus/images/test.img");
    let t = t.or_else(|| {
        s.sys.media.learn("/.orpheus/images");
        s.sys.media.file_ref("/.orpheus/images/test.img")
    });
    let t = t.expect("test image");
    s.sys.drive_start(DriveRequest { file: t, medium: DriveMedium::RemovableDisk, write: true }, "test.img");
    assert_eq!(s.sys.drive().refusal, Some(DriveRefusal::WriteNeedsTestMode));
    s.sys.drive_stop();
}

#[test]
fn writable_test_image_changes_only_itself() {
    let iso_before = sim("").sys.media.read_file("/Images/rescue.iso");
    let mut s = sim(&format!(
        "set:developer=1 set:drivehost=1 set:drivewrite=8 {APP} {TEST} up ok down ok wait:3000"
    ));
    let i = s.sys.drive();
    assert!(!i.read_only, "read/write: {i:?}");
    assert_eq!(i.bytes_written, 8 * 512);
    // Back asks first (the computer wrote); Stop detaches.
    run(&mut s, "back wait:50");
    assert!(s.shell.dialog_open());
    run(&mut s, "ok wait:100 expect-screen:DriveFiles");
    assert_eq!(s.sys.card(), CardState::Ready);
    let mut want = image_bytes(TEST_IMG, 0x5A);
    want[..8 * 512].fill(0xA5);
    let got = s.sys.media.read_file("/.orpheus/images/test.img");
    assert_eq!(got.len(), TEST_IMG, "the size cannot change through the export");
    assert_eq!(hash(&got), hash(&want), "the file holds what the computer wrote");
    assert_eq!(s.sys.media.read_file("/Images/rescue.iso"), iso_before, "other files unchanged");
}

#[test]
fn read_only_export_refuses_the_computers_writes() {
    let mut s = sim(&format!("set:drivehost=1 set:drivewrite=8 {APP} {TEST} ok wait:3000"));
    assert_eq!(s.sys.drive.refused_writes, 1);
    assert_eq!(s.sys.drive().bytes_written, 0);
    run(&mut s, "back wait:100");
    assert!(!s.shell.dialog_open(), "nothing written: no question");
    assert_eq!(hash(&s.sys.media.read_file("/.orpheus/images/test.img")), hash(&image_bytes(TEST_IMG, 0x5A)));
}

#[test]
fn scattered_image_is_refused_with_a_reason() {
    let mut s = sim(&format!("{APP} down ok wait:100 ok ok wait:100 expect-screen:DriveStatus"));
    let i = s.sys.drive();
    assert_eq!((i.phase, i.refusal), (DrivePhase::Failed, Some(DriveRefusal::TooFragmented)));
    const { assert!(SCATTERED_RUNS > POOL_EXTENTS) };
    assert_eq!(s.sys.card(), CardState::Ready, "the card stayed with the player");
    run(&mut s, "back wait:100 expect-screen:DriveFiles");
    assert_eq!(s.sys.drive().phase, DrivePhase::Off);
}

#[test]
fn files_and_the_usb_dialog_open_the_app() {
    // Files › Images › boot.img: its options.
    let mut s = sim("next ok wait:100 down ok wait:100 ok expect-screen:DriveOptions");
    run(&mut s, "ok wait:400 expect-screen:DriveStatus back wait:100");
    // Computer connected › Present disk image…: the app.
    let mut s = sim("set:usb=host expect-screen:UsbConnect down*2 ok wait:100 expect-screen:UsbDrive");
    run(&mut s, "back expect-screen:Home");
    // Home › USB Drive with an export running: the "Presenting" row
    // leads to the status.
    let mut s = sim(&format!("{APP} {ISO} ok wait:400 hold:back wait:100 expect-screen:Home"));
    assert!(s.sys.drive().phase.is_active(), "leaving the app does not detach");
    run(&mut s, &format!("{APP} ok wait:100 expect-screen:DriveStatus back wait:100"));
    assert_eq!(s.sys.drive().phase, DrivePhase::Off);
}

#[test]
fn choose_file_lists_the_card_with_images_bright() {
    let s = sim(&format!("{APP} down*2 ok wait:100 expect-screen:DriveFiles"));
    assert!(s.sys.media.listing().count > 5, "the card root");
}
