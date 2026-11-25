//! UI-6a: the Bluetooth app over the fake module (`sim/src/bt.rs`): turn
//! on, pair new device with a live list, connect success / failure /
//! cancel, paired devices, forget, disconnect, auto-reconnect, and what
//! the screens ask of the module.

use orpheus_apps::services::{BluetoothService, BtAddr, BtList, BtPhase, BtRequest};
use orpheus_apps::settings::id;
use orpheus_sim::AppSim;
use orpheus_sim::bt::{CONNECT_MS, INQUIRY_MS, START_MS};
use orpheus_ui::event::BtState;

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

const APP: &str = "next*3 ok wait:100 expect-screen:Bluetooth";
const SPORT_BUDS: BtAddr = BtAddr([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
const CAR_KIT: BtAddr = BtAddr([0x40, 0x41, 0x42, 0x43, 0x44, 0x45]);

fn phase(s: &AppSim) -> BtPhase {
    s.sys.bt_phase()
}

#[test]
fn turn_on_from_the_app() {
    let mut s = sim(APP);
    assert_eq!(phase(&s), BtPhase::Off);
    assert_eq!(s.sys.bt(), BtState::Off);
    // Pair new device while off: a toast, no screen.
    run(&mut s, "down*2 ok expect-screen:Bluetooth");
    assert!(s.sys.bt.log.is_empty(), "nothing asked of a module that is off");
    run(&mut s, "up*2 ok");
    assert_eq!(s.sys.settings.get(id::BLUETOOTH), 1);
    assert_eq!(phase(&s), BtPhase::Starting);
    assert_eq!(s.sys.bt(), BtState::On, "status bar: on while starting");
    run(&mut s, &format!("wait:{START_MS}"));
    assert_eq!(phase(&s), BtPhase::Ready);
    // And off again from the app.
    run(&mut s, "ok");
    assert_eq!(phase(&s), BtPhase::Off);
}

#[test]
fn scan_lists_devices_as_they_appear_and_back_stops() {
    let mut s = sim(&format!("set:bt=on {APP} down*2 ok expect-screen:BtScan"));
    assert_eq!(s.sys.bt.log, [BtRequest::Scan]);
    assert_eq!(phase(&s), BtPhase::Scanning);
    run(&mut s, "wait:400");
    assert_eq!(s.sys.bt_count(BtList::Found), 1);
    run(&mut s, "wait:1200");
    assert_eq!(s.sys.bt_count(BtList::Found), 3);
    // The loudspeaker answered without a name.
    assert_eq!(s.sys.bt_entry(BtList::Found, 1).unwrap().name(), "");
    run(&mut s, "back expect-screen:Bluetooth");
    assert_eq!(s.sys.bt.log, [BtRequest::Scan, BtRequest::StopScan]);
    assert_eq!(phase(&s), BtPhase::Ready);
    // Names of unnamed devices resolve after the inquiry.
    run(&mut s, "wait:700");
    assert_eq!(s.sys.bt_entry(BtList::Found, 1).unwrap().name(), "Kitchen Speaker");
}

#[test]
fn inquiry_ends_by_itself_then_search_again() {
    let mut s = sim(&format!("set:bt=on {APP} down*2 ok wait:{}", INQUIRY_MS + 700));
    assert_eq!(phase(&s), BtPhase::Ready);
    assert_eq!(s.sys.bt_count(BtList::Found), 5);
    // Last row: Search again (Up from the first row rolls round to it).
    run(&mut s, "up ok wait:100");
    assert_eq!(s.sys.bt.log.last(), Some(&BtRequest::Scan));
    assert_eq!(phase(&s), BtPhase::Scanning);
    assert_eq!(s.sys.bt_count(BtList::Found), 0, "a new inquiry empties the list");
}

#[test]
fn connect_while_scanning_stops_and_connects() {
    let mut s = sim(&format!("set:bt=on {APP} down*2 ok wait:500 ok expect-screen:BtConnect"));
    assert_eq!(s.sys.bt.log, [BtRequest::Scan, BtRequest::Connect(SPORT_BUDS)]);
    run(&mut s, &format!("wait:{}", CONNECT_MS + 300));
    assert_eq!(phase(&s), BtPhase::Connected);
    assert_eq!(s.screen(), "BtScan", "back on the list after connecting");
    assert_eq!(s.sys.bt_device(), "Sport Buds");
    assert_eq!(s.sys.bt(), BtState::Connected);
    // It is paired now and marked connected.
    let paired: Vec<_> =
        (0..s.sys.bt_count(BtList::Paired)).filter_map(|i| s.sys.bt_entry(BtList::Paired, i)).collect();
    assert!(paired.iter().any(|d| d.addr == SPORT_BUDS && d.connected), "{paired:?}");
}

#[test]
fn connect_failure_dialog_retry_and_cancel() {
    let mut s = sim(&format!("set:bt=on {APP} down*2 ok wait:{} down*3 ok", INQUIRY_MS + 700));
    assert_eq!(s.sys.bt.log.last(), Some(&BtRequest::Connect(CAR_KIT)));
    run(&mut s, &format!("wait:{} expect-screen:BtConnect", CONNECT_MS + 200));
    assert!(s.shell.dialog_open(), "failure dialog");
    assert_eq!(phase(&s), BtPhase::Ready);
    // Retry (focused): a second attempt.
    run(&mut s, "ok");
    assert_eq!(s.sys.bt.log.iter().filter(|r| **r == BtRequest::Connect(CAR_KIT)).count(), 2);
    assert_eq!(phase(&s), BtPhase::Connecting);
    run(&mut s, &format!("wait:{}", CONNECT_MS + 200));
    assert!(s.shell.dialog_open());
    // Cancel: back to the list.
    run(&mut s, "up ok expect-screen:BtScan");
    assert!(!s.shell.dialog_open());
}

#[test]
fn back_cancels_a_connection() {
    let mut s = sim(&format!("set:bt=on {APP} down*2 ok wait:{} ok wait:300", INQUIRY_MS + 700));
    assert_eq!(phase(&s), BtPhase::Connecting);
    run(&mut s, "back expect-screen:BtScan");
    assert_eq!(s.sys.bt.log.last(), Some(&BtRequest::CancelConnect));
    assert_eq!(phase(&s), BtPhase::Ready);
    run(&mut s, &format!("wait:{CONNECT_MS}"));
    assert_eq!(phase(&s), BtPhase::Ready, "stays cancelled");
}

#[test]
fn paired_devices_connect_disconnect_forget() {
    let mut s = sim(&format!("set:bt=on {APP} down*3 ok expect-screen:BtPaired"));
    assert_eq!(s.sys.bt_count(BtList::Paired), 1);
    // OK connects (Studio Headphones, paired at start).
    run(&mut s, &format!("ok expect-screen:BtConnect wait:{} expect-screen:BtPaired", CONNECT_MS + 200));
    assert_eq!(phase(&s), BtPhase::Connected);
    assert!(s.sys.bt_entry(BtList::Paired, 0).unwrap().connected);
    // OK on the connected device asks to disconnect; OK confirms.
    run(&mut s, "ok");
    assert!(s.shell.dialog_open());
    run(&mut s, "ok");
    assert_eq!(s.sys.bt.log.last(), Some(&BtRequest::Disconnect));
    assert_eq!(phase(&s), BtPhase::Ready);
    // Menu › Forget…: the safe button is focused; the second one forgets.
    run(&mut s, "menu down ok");
    assert!(s.shell.dialog_open());
    run(&mut s, "ok");
    assert_eq!(s.sys.bt_count(BtList::Paired), 1, "Cancel forgets nothing");
    run(&mut s, "menu down ok down ok");
    assert!(matches!(s.sys.bt.log.last(), Some(BtRequest::Forget(_))));
    assert_eq!(s.sys.bt_count(BtList::Paired), 0);
}

#[test]
fn reconnects_at_power_on_when_enabled() {
    // Connect once, so there is a last device.
    let mut s = sim(&format!("set:bt=on {APP} down*3 ok ok wait:{} back", CONNECT_MS + 200));
    assert_eq!(phase(&s), BtPhase::Connected);
    // Off and on from the app (Prev pages to the top): reconnects by
    // itself.
    run(&mut s, "prev*3 ok ok");
    run(&mut s, &format!("wait:{}", START_MS + CONNECT_MS + 300));
    assert_eq!(phase(&s), BtPhase::Connected);
    // Reconnect switched off (last row): stays unconnected.
    run(&mut s, "down*5 ok");
    assert_eq!(s.sys.settings.get(id::BT_RECONNECT), 0);
    run(&mut s, "prev*3 ok ok");
    run(&mut s, &format!("wait:{}", START_MS + CONNECT_MS + 300));
    assert_eq!(phase(&s), BtPhase::Ready);
}

#[test]
fn not_responding_and_turn_off_while_connected() {
    let s = sim(&format!("set:bt=dead {APP} down*2 ok"));
    assert_eq!(s.screen(), "Bluetooth", "lists need a module that answers");
    assert_eq!(s.sys.bt(), BtState::Off);
    let mut s = sim(&format!("set:bt=streaming {APP}"));
    assert_eq!(s.sys.bt(), BtState::Streaming);
    run(&mut s, "ok");
    assert_eq!(phase(&s), BtPhase::Off);
    assert_eq!(s.sys.bt(), BtState::Off);
}
