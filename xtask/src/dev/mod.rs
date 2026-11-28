//! `xtask dev …`: commands that talk to the player (07 §2, §5).
//!
//! Safety rules (CLAUDE.md, 07 §7) enforced here:
//! * NOR writes only inside the dual-boot free region, and only after
//!   `backup-verify` passed in this process;
//! * no command powers the player off;
//! * one process at a time: a lock file in `target/orpheus/`.

mod fuzz;
mod hil;
mod mask;
mod mscbench;
mod player;
pub mod prof;
mod ram;
mod script;
mod soak;
mod stage;
mod suite;
mod usbimage;
mod usbtest;

use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use orpheus_host::adfu::{Adfu, yoton_dir};
use orpheus_host::native::nor;
use orpheus_host::{device, image};
use orpheus_proto::layout::dual;
use orpheus_proto::mem::{FB_BASE, FB_H, FB_SIZE, FB_W};
use orpheus_proto::{fault, mailbox, result};
use sha2::{Digest, Sha256};

use crate::args::{Args, num};
use crate::util::{log, out_dir};

const HELP: &str = "\
cargo xtask dev <subcommand>

  state                         what runs on the player
  adfu                          bring it to ADFU + ymon
  ram IMAGE.bin|NAME [--test T] [--wait S] [--arg N]...
                                load a RAM image at 0x100000 and start it;
                                NAME builds fw/bringup/NAME first; with --test,
                                arm test mode and collect the results
  hil SUITE                     run an on-device suite (smoke)
  usbtest [--cycles N]          USB-1 acceptance: enumerate, control sizes, re-enumeration
  usb-cancel [--rounds N]       USB-W1: provoke abandoned control transfers (firmware held
                                2.5 s, host cuts after 1 ms); the player must answer again
  log                           the firmware's log ring (developer channel), oldest first
  msc-bench [--mib N] [--file PATH] [--check]
                                PERF-SD: card reader throughput through files (direct I/O):
                                read a card file, write N MiB (default 64) to
                                /.orpheus/test/perf/, read it back and compare, delete;
                                --check against budgets.toml [msc]
  usbimage-test [--img F] [--iso F] [--rw] [--qemu]
                                USB-6: put images into /.orpheus/images/, present each,
                                read the whole block device, compare hashes, detach
  stage IMAGE|NAME|FILE.yfw      write a YFW1 slot to slot A (0x762000)
  update FILE.ofw [--no-reboot] [--stop-after BYTES]
                                firmware update through the running Orpheus
                                (developer channel, inactive slot, no ADFU; UPD-1);
                                --stop-after: write that much, then abort (timing the
                                write path; with `prof --during`)
  slots                          ADFU: what full slots A and B hold, and which slot
                                stage 2 started last (and after which failed, T-6)
  switch-slot                    ADFU: start the other slot next (recovery after an
                                update that keeps failing to start)
  install-stage2                 replace stage 2 at 0x7DF400 (keeps the vendor record copy)
  keys 'ok wait:300 hold:menu'  inject keys into running orpheus
  info                          developer-channel INFO of running orpheus
  put LOCAL /.orpheus/PATH [--outside-orpheus]
                                upload a file to the card (through the firmware's FS);
                                --outside-orpheus also allows the demo tree's folders
                                (/Music, /MIDI, /Pictures, /Books, root CREDITS.txt; test card)
  demo-card --outside-orpheus [--dir DIR]
                                copy the demo card tree (`cargo xtask demo-card`,
                                target/orpheus/demo-card/) onto the card, skipping files
                                already there with the same size (owner's test card only)
  suite [NAME…] [--list] [--update-expected] [--setup] [--sim] [--no-reboot]
                                on-device functional suite (VER-7): tests/suite/journeys.toml,
                                shots vs the simulator under tests/suite/masks.toml;
                                --setup uploads the fixtures to /.orpheus/test/suite/;
                                --sim runs it against the simulator; ends in a reboot
  fuzz-ui --seed N --minutes M [--check-every K] [--play] [--settings] [--no-minimise]
          [--max-replays R] | --replay FILE
                                random key sequences (ui_keys grammar, no Power) with the
                                07 §5.7 oracles; failures minimised into tests/regress/ui-device/
  soak SCENARIO [--hours H | --minutes M] [--seed N] [--sim] [--report DIR] [--setup]
       [--battery] [--music DIR]... [--no-minimise] [--max-replays R] | --replay FILE | --list
                                long soaks (VER-12, 07 §5.10): music, radio, browse-while-playing,
                                settings-churn, mixed, stress; oracles every 10 s, time series in
                                metrics/soak/, findings minimised into tests/regress/soak/;
                                starts and ends with a reboot
  prof --pc S | --faults S [--burst] [--period-us N] [--during 'SUBCOMMAND …'] [--only TEXT]
       [--update-hot NAME [--lines N] [--ro-lines N]] [--window-ms N] [--top N] [--any-build]
                                profilers (T-8, 07 §5.6): PC sampler or fault log while
                                S seconds pass or the subcommand runs (same process; a
                                soak captures from after its prologue, or only during the
                                first action containing TEXT); symbolised reports in
                                target/orpheus/prof/STAMP/; --update-hot writes a
                                hot.toml cluster from the hottest faulting sections
                                (`cargo xtask prof DIR` reports a saved capture again)
  stats [--irq]                 executor idle time, pager and UI counters (--irq: per line)
  time [--set]                  read (or set from the host, UTC) the player's clock
  otp-add 'otpauth://…'         PROVISION one authenticator account (the
                                Authenticator must be unlocked; confirm on the player)
  cmd 'play /x.mp3' [--wait S]  firmware test command, then print the reply
  results                       read result block, fault record and log ring
  peek ADDR [N]                 read words (switches the player to ymon)
  mem ADDR [N]                  read words from the running firmware
                                (developer channel; stays in the firmware)
  poke ADDR VALUE               write one word (ADFU/ymon)
  dump ADDR LEN OUT             read memory to a file
  shot OUT.png                  screenshot of the frame buffer
  nordump ADDR LEN OUT          read NOR (raw)
  backup-verify                 check the vendor NOR backups
  save-rockbox                  dump the installed Rockbox region of NOR to target/orpheus/
  restore rockbox [--verify]    write the saved Rockbox region back (--verify: compare only)
  restore vendor [--verify | --i-have-a-backup]
                                compare with / write back the stock NOR backup (whole chip)
  install-full --i-have-a-backup [IMAGE.yfw]
                                switch to the full layout (D-2): erases the stock firmware
  reset                         reset through ymon (the installed firmware boots)
  reboot                        normal reboot through the developer channel (no recovery mode)
";

/// Where an upload may write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    /// Under `/.orpheus/` only (CLAUDE.md rule 5): every command but the
    /// two below.
    Orpheus,
    /// Also the demo card tree (`orpheus_proto::devchan::DEMO_ROOTS`):
    /// `put --outside-orpheus` and `demo-card --outside-orpheus` only, on
    /// the owner's test card (owner, 2026-11-27).
    DemoTree,
}

/// The firmware's test command buffer (`audio::CMD_LEN`).
const CMD_LEN: usize = 120;

/// The test command that opens `remote` for an upload, or why the path is
/// refused: `fcreate` under `/.orpheus/`, `fcreate-demo` for a file of the
/// demo tree when `scope` allows it, nothing else.
pub(crate) fn create_command(remote: &str, scope: Scope) -> Result<String> {
    let cmd = if remote.starts_with("/.orpheus/") && !remote.split('/').any(|c| c == "..") {
        format!("fcreate {remote}")
    } else if scope == Scope::DemoTree && orpheus_proto::devchan::demo_write_root(remote).is_some() {
        format!("fcreate-demo {remote}")
    } else if scope == Scope::DemoTree {
        bail!(
            "{remote}: uploads go under /.orpheus/ or, with --outside-orpheus, into the demo tree ({})",
            orpheus_proto::devchan::DEMO_ROOTS.join(", ")
        );
    } else {
        bail!("{remote}: uploads go under /.orpheus/ only (CLAUDE.md rule 5)");
    };
    if cmd.len() > CMD_LEN {
        bail!("{remote}: path too long for the test command ({} bytes, at most {CMD_LEN})", cmd.len());
    }
    Ok(cmd)
}

/// Upload `data` to `remote` (under `/.orpheus/` only) through the
/// firmware's file system (`fcreate`, `PUT` chunks, `fclose`).
pub(super) fn put(data: &[u8], remote: &str) -> Result<String> {
    put_in(data, remote, Scope::Orpheus)
}

fn put_in(data: &[u8], remote: &str, scope: Scope) -> Result<String> {
    let cmd = create_command(remote, scope)?;
    let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
    let t0 = std::time::Instant::now();
    upload_with(&d, &cmd, data)?;
    Ok(format!("{} bytes -> {remote} in {:.1} s", data.len(), t0.elapsed().as_secs_f32()))
}

/// The size of `remote` on the card (`fsize`), `None` if it is missing.
fn remote_size(d: &orpheus_host::devchan::DevChannel, remote: &str) -> Result<Option<u64>> {
    use orpheus_proto::devchan::REQ_TEST;
    d.control_out(REQ_TEST, 0, 0, format!("fsize {remote}").as_bytes())?;
    let t0 = std::time::Instant::now();
    loop {
        let r = String::from_utf8_lossy(&d.control_in(REQ_TEST, 0, 0, 1024)?).into_owned();
        if let Some(size) = parse_fsize(&r, remote) {
            return Ok(size);
        }
        if t0.elapsed() > Duration::from_secs(10) {
            bail!("fsize {remote}: no answer (last reply `{r}`)");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The reply to `fsize PATH` for `remote`: `Some(Some(n))` for a file,
/// `Some(None)` when it is missing, `None` for any other (stale) reply.
fn parse_fsize(reply: &str, remote: &str) -> Option<Option<u64>> {
    if let Some(rest) = reply.strip_prefix("size ") {
        let (n, path) = rest.split_once(' ')?;
        return if path == remote { n.parse().ok().map(Some) } else { None };
    }
    (reply.strip_prefix("fsize: no such file ")? == remote).then_some(None)
}

/// Sent before the demo card's first file command: a reboot reopens the
/// last track paused, and the card task serves file commands only between
/// tracks.
const DEMO_CARD_PROLOGUE: &[&[u8]] = &[b"stop"];

/// `dev demo-card --outside-orpheus [--dir DIR]`: copy the demo card tree
/// (`cargo xtask demo-card`) onto the card, skipping files that are there
/// with the same size.
fn demo_card(args: &mut Args) -> Result<String> {
    if !args.flag("outside-orpheus") {
        bail!(
            "demo-card writes outside /.orpheus/ (the demo tree's folders only; allowed by the owner on the \
             test card, 2026-11-27): pass --outside-orpheus to confirm"
        );
    }
    let dir = args.value("dir").map_or_else(|| out_dir().join("demo-card"), PathBuf::from);
    let files = orpheus_web::demo::read_tree(&dir)
        .with_context(|| format!("{} (build it with `cargo xtask demo-card`)", dir.display()))?;
    if files.is_empty() {
        bail!("{} is empty (cargo xtask demo-card)", dir.display());
    }
    // Every path is checked before the first write.
    for f in &files {
        create_command(&f.path, Scope::DemoTree)?;
    }
    let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
    // A reboot reopens the last track paused, and the card task serves
    // file commands only between tracks: stop it first, as `suite --setup`
    // does (the first upload waited for `fsize` and failed, 2026-11-27).
    for c in DEMO_CARD_PROLOGUE {
        d.control_out(orpheus_proto::devchan::REQ_TEST, 0, 0, c)?;
    }
    std::thread::sleep(Duration::from_millis(500));
    let (mut sent, mut skipped, mut bytes) = (0, 0, 0u64);
    let t0 = std::time::Instant::now();
    for f in &files {
        if remote_size(&d, &f.path)? == Some(f.data.len() as u64) {
            skipped += 1;
            continue;
        }
        log(&format!("  {} ({} KB)", f.path, f.data.len().div_ceil(1024)));
        upload_with(&d, &create_command(&f.path, Scope::DemoTree)?, &f.data)?;
        sent += 1;
        bytes += f.data.len() as u64;
    }
    Ok(format!(
        "demo card: {sent} files uploaded ({} MB), {skipped} already there, in {:.0} s",
        bytes.div_ceil(1 << 20),
        t0.elapsed().as_secs_f32()
    ))
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let sub = args.next_positional().unwrap_or_else(|| "help".into());
    if sub == "help" {
        print!("{HELP}");
        return Ok("help".into());
    }
    let _lock = DeviceLock::take()?;
    dispatch(&sub, args)
}

/// One subcommand, with the device lock held by the caller (`cmd`, or
/// `prof --during`).
fn dispatch(sub: &str, args: &mut Args) -> Result<String> {
    match sub {
        "state" => Ok(device::state().name().to_string()),
        "adfu" => {
            to_ymon()?;
            Ok("ymon running".into())
        }
        "ram" => ram::cmd(args),
        "hil" => {
            let name = args.positional("suite")?;
            let cases = hil::suite(&name).ok_or_else(|| anyhow::anyhow!("unknown suite `{name}`"))?;
            let mut failed = Vec::new();
            for c in cases {
                log(&format!("== {name}/{}", c.test));
                match hil::run_case(c) {
                    Ok(s) => log(&format!("   ok: {s}")),
                    Err(e) => {
                        log(&format!("   FAILED: {e:#}"));
                        failed.push(c.test);
                    }
                }
            }
            if failed.is_empty() {
                Ok(format!("suite {name}: {} tests passed", cases.len()))
            } else {
                bail!("suite {name}: failed {failed:?}")
            }
        }
        "usbtest" => usbtest::cmd(args),
        "usb-cancel" => usbtest::cancel_cmd(args),
        "msc-bench" => mscbench::cmd(args),
        "log" => {
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            for l in prof::log_lines(&d)? {
                println!("{l}");
            }
            Ok("log printed".into())
        }
        "keys" => {
            let script = key_script(args)?;
            keys(&script)
        }
        "cmd" => {
            let text = args.positional("command text")?;
            let wait = args.value("wait").map(|s| num(&s)).transpose()?.unwrap_or(2);
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            d.control_out(orpheus_proto::devchan::REQ_TEST, 0, 0, text.as_bytes())?;
            std::thread::sleep(Duration::from_secs(u64::from(wait)));
            let r = d.control_in(orpheus_proto::devchan::REQ_TEST, 0, 0, 1024)?;
            println!("{}", String::from_utf8_lossy(&r));
            Ok("reply printed".into())
        }
        // A normal reboot through the developer channel (the firmware
        // restarts itself; `reset` goes through the recovery monitor and
        // can leave the player in recovery mode).
        "reboot" => {
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            d.reboot(orpheus_proto::devchan::REBOOT_NORMAL);
            Ok("reboot sent".into())
        }
        "stats" => {
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            if args.flag("irq") {
                let r = d.control_in(orpheus_proto::devchan::REQ_STATS, 1, 0, 256)?;
                let mut out = String::from("interrupts per line:");
                for (i, c) in r.chunks_exact(4).enumerate() {
                    let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    if v != 0 {
                        out.push_str(&format!(" {i}:{v}"));
                    }
                }
                return Ok(out);
            }
            let r = d.control_in(orpheus_proto::devchan::REQ_STATS, 0, 0, 128)?;
            let w: Vec<u32> =
                r.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            let g = |i: usize| w.get(i).copied().unwrap_or(0);
            Ok(format!(
                "uptime {} ms, idle {} us, pager loads {} (code faults {}, ro faults {}, evictions {}), usb resets {} setups {}, ui loops {} busy {} us rows {}, settings writes {} (last key {:#06x}, {} us), ui draw {} us flush {} us, rt decode {} us fill {} us serve {} us, clock level {} ({} switches){}",
                g(8),
                g(3),
                g(4),
                g(5),
                g(6),
                g(7),
                g(0),
                g(1),
                g(9),
                g(10),
                g(11),
                g(12),
                g(13) & 0xFFFF,
                g(13) >> 16,
                g(14),
                g(15),
                g(16),
                g(17),
                g(18),
                clock_level_name(g(19)),
                g(20),
                // VER-12 words (21 is reset by this read).
                if w.len() > 23 {
                    let h = g(21) as i32;
                    format!(
                        ", DAC headroom low-water {}, card errors {}, pager load errors {}",
                        if h == i32::MAX { "-".to_string() } else { format!("{h} frames") },
                        g(22),
                        g(23)
                    )
                } else {
                    String::new()
                },
            ) + &orpheus_proto::devchan::usb::Counters::from_words(&w)
                .map(|u| format!("\n{}", soak::usb_line(&u)))
                .unwrap_or_default())
        }
        "put" => {
            let local = args.positional("LOCAL file")?;
            let remote = args.positional("REMOTE path under /.orpheus/")?;
            let scope = if args.flag("outside-orpheus") { Scope::DemoTree } else { Scope::Orpheus };
            put_in(&std::fs::read(&local)?, &remote, scope)
        }
        "demo-card" => demo_card(args),
        "suite" => suite::cmd(args),
        "fuzz-ui" => fuzz::cmd(args),
        "soak" => soak::cmd(args),
        "prof" => prof::cmd(args),
        "usbimage-test" => usbimage::cmd(args),
        "otp-add" => {
            let uri = args.next_positional().context("otp-add needs an otpauth:// URI")?;
            if !uri.starts_with("otpauth://") || uri.len() > 1024 {
                bail!("expected one otpauth:// URI of at most 1024 bytes");
            }
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            d.control_out(orpheus_proto::devchan::REQ_PROVISION, 0, 0, uri.as_bytes())
                .context("refused: is the Authenticator unlocked on the player?")?;
            Ok("sent; confirm it on the player".into())
        }
        "time" => {
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            if args.flag("set") {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
                let (y, mo, da, h, mi, se) = civil(now);
                let yb = (y as u16).to_le_bytes();
                d.control_out(orpheus_proto::devchan::REQ_TIME, 0, 0, &[yb[0], yb[1], mo, da, h, mi, se])?;
            }
            let t = d.control_in(orpheus_proto::devchan::REQ_TIME, 0, 0, 7)?;
            if t.len() < 7 {
                bail!("clock not set");
            }
            Ok(format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                u16::from_le_bytes([t[0], t[1]]),
                t[2],
                t[3],
                t[4],
                t[5],
                t[6]
            ))
        }
        "info" => {
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            let i = d.info()?;
            Ok(format!(
                "uptime {} ms, battery {} mV, flags {:#x}, build {}, stack free {} of {} B",
                i.uptime_ms,
                i.battery_mv,
                i.flags,
                if i.build_id.iter().all(|&c| c == 0 || c.is_ascii_graphic()) {
                    String::from_utf8_lossy(&i.build_id).trim_end_matches('\0').to_string()
                } else {
                    i.build_id.iter().map(|x| format!("{x:02x}")).collect()
                },
                i.reserved & 0xFFFF,
                i.reserved >> 16
            ))
        }
        "stage" => stage::cmd_stage(args),
        "update" => stage::cmd_update(args),
        "slots" => stage::cmd_slots(),
        "switch-slot" => stage::cmd_switch_slot(),
        "install-stage2" => stage::cmd_install_stage2(args),
        "install-full" => stage::cmd_install_full(args),
        "stage2-ramtest" => stage::cmd_stage2_ramtest(args),
        "results" => {
            let mut d = to_ymon()?;
            hil::print_results(&mut d, None)
        }
        "mem" => {
            let addr = num(&args.positional("ADDR")?)?;
            let n = args.next_positional().map(|s| num(&s)).transpose()?.unwrap_or(1);
            let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            let data = d.peek(addr, n as usize * 4)?;
            for (i, w) in data.as_chunks::<4>().0.iter().enumerate() {
                println!(
                    "{:#010x}: {:#010x}",
                    addr + 4 * i as u32,
                    u32::from_le_bytes([w[0], w[1], w[2], w[3]])
                );
            }
            Ok(format!("{n} words"))
        }
        "peek" => {
            let addr = num(&args.positional("ADDR")?)?;
            let n = args.next_positional().map(|s| num(&s)).transpose()?.unwrap_or(1);
            let mut d = to_ymon()?;
            let data = d.read(addr, n as usize * 4)?;
            for (i, w) in data.as_chunks::<4>().0.iter().enumerate() {
                println!(
                    "{:#010x}: {:#010x}",
                    addr + 4 * i as u32,
                    u32::from_le_bytes([w[0], w[1], w[2], w[3]])
                );
            }
            Ok(format!("{n} words"))
        }
        "poke" => {
            let addr = num(&args.positional("ADDR")?)?;
            let v = num(&args.positional("VALUE")?)?;
            guard_poke(addr)?;
            let mut d = to_ymon()?;
            d.write_u32(addr, v)?;
            Ok(format!("{addr:#010x} <- {v:#010x}"))
        }
        "dump" => {
            let addr = num(&args.positional("ADDR")?)?;
            let len = num(&args.positional("LEN")?)?;
            let out = PathBuf::from(args.positional("OUT")?);
            let mut d = to_ymon()?;
            std::fs::write(&out, d.read(addr, len as usize)?)?;
            Ok(format!("{len} bytes -> {}", out.display()))
        }
        "shot" => {
            let out = PathBuf::from(args.positional("OUT.png")?);
            if device::state() == device::State::Yotonrs {
                let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
                let fb = d.frame()?;
                image::write_rgb565_png(&fb, FB_W, FB_H, &out)?;
                return Ok(format!("-> {} (developer channel)", out.display()));
            }
            let mut d = to_ymon()?;
            let fb = d.read(FB_BASE, FB_SIZE as usize)?;
            image::write_rgb565_png(&fb, FB_W, FB_H, &out)?;
            Ok(format!("-> {}", out.display()))
        }
        "nordump" => {
            let addr = num(&args.positional("ADDR")?)?;
            let len = num(&args.positional("LEN")?)?;
            let out = PathBuf::from(args.positional("OUT")?);
            let mut d = to_ymon()?;
            let (data, jedec) = nor::read(&mut d, addr, len as usize)?;
            std::fs::write(&out, data)?;
            Ok(format!("{len} bytes -> {} (JEDEC {jedec:06x})", out.display()))
        }
        "backup-verify" => backup_verify(),
        "save-rockbox" => save_rockbox(),
        "restore" => {
            let what = args.positional("rockbox | vendor")?;
            match what.as_str() {
                "rockbox" => restore_rockbox(args.flag("verify")),
                "vendor" => stage::cmd_restore_vendor(args),
                _ => bail!("restore what? `rockbox` or `vendor`"),
            }
        }
        "reset" => {
            let d = to_ymon()?;
            d.reset();
            Ok("reset sent".into())
        }
        other => bail!("unknown dev subcommand `{other}`\n{HELP}"),
    }
}

/// Exclusive use of the player by this process (07 §7 rule 8).
struct DeviceLock(#[allow(dead_code)] File);

/// The lock file: one for the whole machine, not per checkout, so workers
/// in separate git worktrees exclude each other too (2026-11-28: a worker's
/// `xtask gate` ran `dev hil smoke` on the player during the lead's soak,
/// each worktree holding its own `target/orpheus/device.lock`).
/// `ORPHEUS_DEVICE_LOCK` overrides it.
fn device_lock_path() -> std::path::PathBuf {
    std::env::var_os("ORPHEUS_DEVICE_LOCK")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("orpheus-device.lock"))
}

impl DeviceLock {
    fn take() -> Result<Self> {
        let p = device_lock_path();
        let f = File::create(&p)?;
        if f.try_lock().is_err() {
            bail!("another process is using the player ({} is locked)", p.display());
        }
        Ok(Self(f))
    }
}

pub fn to_ymon() -> Result<Adfu> {
    device::to_ymon(&mut |s| log(s), Duration::from_secs(90))
}

fn guard_poke(addr: u32) -> Result<()> {
    // The eFuse controller is read-only (CLAUDE.md rule 3).
    if (0xC002_0000..0xC003_0000).contains(&addr) {
        bail!("refusing to write the eFuse block");
    }
    // PMU regulator and charger fields need provenance: not from a poke.
    if (0xC001_0000..0xC001_0100).contains(&addr) {
        bail!("refusing to poke the PMU block; use a reviewed driver");
    }
    Ok(())
}

/// The reference repository's NOR dumps (read-only).
pub(crate) static DUMPS: std::sync::LazyLock<std::path::PathBuf> =
    std::sync::LazyLock::new(|| yoton_dir().join("dumps"));

pub(crate) fn backup_verify() -> Result<String> {
    let dumps = yoton_dir().join("dumps");
    let raw = dumps.join("nor_backup_8m.bin");
    let desc = dumps.join("nor_descrambled_8m.bin");
    let data = std::fs::read(&raw).with_context(|| format!("NOR backup missing: {}", raw.display()))?;
    if data.len() != 8 * 1024 * 1024 {
        bail!("{} has {} bytes, expected 8 MiB", raw.display(), data.len());
    }
    let h = Sha256::digest(&data);
    let hex: String = h.iter().take(4).map(|b| format!("{b:02x}")).collect();
    if hex != "ad45687e" {
        bail!("{} sha256 starts {hex}, expected ad45687e", raw.display());
    }
    let dl = std::fs::metadata(&desc).with_context(|| format!("missing {}", desc.display()))?.len();
    if dl != 8 * 1024 * 1024 {
        bail!("{} has {dl} bytes", desc.display());
    }
    Ok("vendor NOR backups present and intact".into())
}

fn rockbox_save_path() -> PathBuf {
    crate::util::root().join("local/rockbox-nor-0x762000.bin")
}

/// The whole free region as it is now (Rockbox pages, resident image,
/// vendor record copy and the C stage 2), so it can be put back exactly.
fn save_rockbox() -> Result<String> {
    let mut d = to_ymon()?;
    let len = (dual::FREE_END - dual::FREE_START) as usize;
    let (data, jedec) = nor::read(&mut d, dual::FREE_START, len)?;
    if jedec & 0xFF_FFFF != 0x17_405E {
        bail!("unexpected JEDEC id {jedec:06x}");
    }
    if data.get(0x7DF400 - 0x762000..0x7DF404 - 0x762000) != Some(b"YST2") {
        bail!("stage 2 magic not found at 0x7DF400; not the expected installation");
    }
    let p = rockbox_save_path();
    std::fs::write(&p, &data)?;
    let h = Sha256::digest(&data);
    let hex: String = h.iter().take(8).map(|b| format!("{b:02x}")).collect();
    std::fs::write(p.with_extension("sha256"), format!("{hex}\n"))?;
    Ok(format!("{len} bytes -> {} (sha256 {hex}…)", p.display()))
}

fn restore_rockbox(verify_only: bool) -> Result<String> {
    let p = rockbox_save_path();
    let saved = std::fs::read(&p).with_context(|| format!("no saved Rockbox region at {}", p.display()))?;
    let mut d = to_ymon()?;
    let (cur, _) = nor::read(&mut d, dual::FREE_START, saved.len())?;
    let diff = cur.chunks(4096).zip(saved.chunks(4096)).filter(|(a, b)| a != b).count();
    if verify_only {
        return Ok(if diff == 0 {
            "NOR free region matches the saved Rockbox installation".into()
        } else {
            format!("{diff} sectors differ from the saved Rockbox installation (dry run, nothing written)")
        });
    }
    backup_verify()?;
    if diff == 0 {
        return Ok("already identical".into());
    }
    nor::write(&mut d, dual::FREE_START, &saved, &mut |s| log(s))?;
    Ok(format!("restored {diff} sectors of the Rockbox installation"))
}

/// Read the persistent area after a test.
pub fn read_persistent(
    d: &mut Adfu,
) -> Result<(Option<result::ResultBlock>, Option<fault::FaultRecord>, mailbox::Mailbox)> {
    let r = d.read(result::BASE, result::SIZE)?;
    let f = d.read(fault::BASE, fault::SIZE)?;
    let m = d.read(mailbox::BASE, mailbox::SIZE)?;
    Ok((
        result::ResultBlock::from_bytes(&r),
        fault::FaultRecord::from_bytes(&f),
        mailbox::Mailbox::from_bytes(&m).unwrap_or_default(),
    ))
}

/// Upload `data` to `remote` (under `/.orpheus/`) through the firmware's
/// file system: `fcreate`, 1 KiB PUT chunks, `fclose`.
pub(crate) fn upload(d: &orpheus_host::devchan::DevChannel, remote: &str, data: &[u8]) -> Result<()> {
    upload_with(d, &create_command(remote, Scope::Orpheus)?, data)
}

/// The upload itself, after `cmd` (from [`create_command`]).
fn upload_with(d: &orpheus_host::devchan::DevChannel, cmd: &str, data: &[u8]) -> Result<()> {
    use orpheus_proto::devchan::{REQ_PUT, REQ_TEST};
    let wait_for = |want: &str| -> Result<()> {
        let t0 = std::time::Instant::now();
        loop {
            let r = String::from_utf8_lossy(&d.control_in(REQ_TEST, 0, 0, 256)?).into_owned();
            if r == want {
                return Ok(());
            }
            if r.contains("failed") {
                bail!("player: {r}");
            }
            if t0.elapsed() > Duration::from_secs(10) {
                bail!("timed out waiting for `{want}` (last reply `{r}`)");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    d.control_out(REQ_TEST, 0, 0, cmd.as_bytes())?;
    wait_for("put 0")?;
    // PERF-SD round 2: the developer interface's bulk endpoint when the
    // firmware has it (`orpheus_proto::devchan::upload::BULK_CMD`).
    if d.has_bulk_upload() && !data.is_empty() {
        let n = data.len();
        d.control_out(REQ_TEST, 0, 0, format!("{}{n}", orpheus_proto::devchan::upload::BULK_CMD).as_bytes())?;
        wait_for(&format!("fput {n}"))?;
        d.bulk_upload(data, Duration::from_secs(10))?;
        wait_for(&format!("put {n}"))?;
        d.control_out(REQ_TEST, 0, 0, b"fclose")?;
        return wait_for(&format!("closed {n}"));
    }
    // Chunks numbered in `wValue`, sent back to back: the player stages
    // them (`orpheus_proto::devchan::upload`) and refuses one while both
    // halves wait for the card; a refused chunk is sent again (PERF-SD:
    // ≈ 24 KB/s with a reply poll per chunk before).
    let mut sent = 0usize;
    for (seq, chunk) in data.chunks(1024).enumerate() {
        let t0 = std::time::Instant::now();
        let mut tries = 0u32;
        while d.control_out(REQ_PUT, seq as u16, 0, chunk).is_err() {
            tries += 1;
            if tries % 64 == 0 {
                let r = String::from_utf8_lossy(&d.control_in(REQ_TEST, 0, 0, 256)?).into_owned();
                if r.contains("failed") || r.contains("aborted") || r.contains("timed out") {
                    bail!("player: {r}");
                }
            }
            if t0.elapsed() > Duration::from_secs(10) {
                bail!("chunk {seq} refused for 10 s");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        sent += chunk.len();
    }
    d.control_out(REQ_TEST, 0, 0, b"fclose")?;
    wait_for(&format!("closed {sent}"))
}

/// Key names in scripts (orpheus_input::Key order).
const KEY_NAMES: [&str; 10] =
    ["power", "back", "menu", "ok", "prev", "next", "up", "down", "volup", "voldown"];

/// `ok`, `down*3`, `hold:menu` (800 ms), `long:power` (4.5 s), `wait:300`.
/// The key script: every remaining word (a shell passes one quoted
/// argument; `prof --during "keys …"` splits on whitespace).
fn key_script(args: &mut Args) -> Result<String> {
    let mut script = args.positional("key script")?;
    while let Some(w) = args.next_positional() {
        script.push(' ');
        script.push_str(&w);
    }
    Ok(script)
}

fn keys(script: &str) -> Result<String> {
    let d = orpheus_host::devchan::DevChannel::open(Duration::from_secs(3))?;
    let send =
        |k: u8, down: bool| d.control_out(orpheus_proto::devchan::REQ_KEYS, 0, 0, &[k, u8::from(down)]);
    let idx = |n: &str| KEY_NAMES.iter().position(|x| *x == n).map(|i| i as u8);
    let mut n = 0;
    for tok in script.split_whitespace() {
        if let Some(ms) = tok.strip_prefix("wait:") {
            std::thread::sleep(Duration::from_millis(num(ms)?.into()));
            continue;
        }
        let (hold_ms, name) = if let Some(k) = tok.strip_prefix("hold:") {
            (800, k)
        } else if let Some(k) = tok.strip_prefix("long:") {
            (4500, k)
        } else {
            (60, tok)
        };
        let (name, times) = match name.split_once('*') {
            Some((k, t)) => (k, num(t)?),
            None => (name, 1),
        };
        let k = idx(name).ok_or_else(|| anyhow::anyhow!("unknown key `{name}` (one of {KEY_NAMES:?})"))?;
        for _ in 0..times {
            send(k, true)?;
            std::thread::sleep(Duration::from_millis(hold_ms));
            send(k, false)?;
            std::thread::sleep(Duration::from_millis(120));
            n += 1;
        }
    }
    Ok(format!("{n} key presses sent"))
}

/// Unix seconds to (year, month, day, hour, minute, second), UTC.
fn civil(secs: u64) -> (i64, u8, u8, u8, u8, u8) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's days-from-civil inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u8, d as u8, (rem / 3600) as u8, (rem / 60 % 60) as u8, (rem % 60) as u8)
}

#[cfg(test)]
mod key_script_tests {
    use super::*;

    /// 2026-11-25: `dev prof --during "keys up*3 wait:200 ok …"` sent only
    /// `up*3` (the subcommand's words arrive as separate arguments and
    /// `keys` took the first): the whole script must arrive.
    #[test]
    fn regress_keys_takes_every_word_of_the_script() {
        let words: Vec<String> = ["up*3", "wait:200", "ok"].iter().map(|s| (*s).to_string()).collect();
        assert_eq!(key_script(&mut Args::from_vec(words)).unwrap(), "up*3 wait:200 ok");
        let one = vec!["next*2 ok".to_string()];
        assert_eq!(key_script(&mut Args::from_vec(one)).unwrap(), "next*2 ok");
    }
}

/// `dev stats` "clock level": the firmware's `clock::current() as u32`
/// (`orpheus_power::clock::Level` order).
fn clock_level_name(i: u32) -> String {
    use orpheus_power::clock::Level;
    match Level::ALL.get(i as usize) {
        Some(l) => format!("{l:?} {}", l.mhz()),
        None => "?".into(),
    }
}

#[cfg(test)]
mod stats_tests {
    /// PG-2b, 2026-11-27: `dev stats` printed "clock level ?" at 288 MHz
    /// (its own name table stopped at Turbo 192), so the real clock could
    /// not be read while MIDI played at Max.
    #[test]
    fn regress_stats_clock_level_max_unnamed() {
        assert_eq!(super::clock_level_name(1), "Base 48");
        assert_eq!(super::clock_level_name(4), "Turbo 192");
        assert_eq!(super::clock_level_name(5), "Max 288");
        assert_eq!(super::clock_level_name(6), "?");
    }
}

#[cfg(test)]
mod device_lock_tests {
    /// 2026-11-28: the device lock lived in each checkout's
    /// `target/orpheus/`, so a worker's gate in another worktree used the
    /// player during the lead's soak. The lock must be outside any checkout.
    #[test]
    fn regress_device_lock_was_per_worktree() {
        if std::env::var_os("ORPHEUS_DEVICE_LOCK").is_some() {
            return;
        }
        let p = super::device_lock_path();
        assert!(!p.starts_with(crate::util::root()), "{} is inside the checkout", p.display());
        assert!(!p.starts_with(crate::util::out_dir()), "{}", p.display());
    }
}

#[cfg(test)]
mod upload_guard_tests {
    use super::*;

    /// 2026-11-27, on the player: `dev demo-card` failed at once with
    /// "fsize /About this card.txt: no answer": a reboot had reopened the
    /// last track paused, and the card task serves file commands only
    /// between tracks. The upload stops the track first, as `suite
    /// --setup` does.
    #[test]
    fn regress_demo_card_waited_on_a_paused_track() {
        assert_eq!(DEMO_CARD_PROLOGUE.first(), Some(&&b"stop"[..]));
        // `stop` is not a file command the paused loop would answer.
        assert!(!orpheus_proto::devchan::served_while_paused(b"stop"));
        assert!(!orpheus_proto::devchan::served_while_paused(b"fsize /CREDITS.txt"));
    }

    /// The developer-channel upload guard after the demo tree was allowed
    /// (owner, 2026-11-27): the default scope still refuses everything
    /// outside `/.orpheus/`, and `--outside-orpheus` adds exactly the demo
    /// tree's folders and root files — never the root, other folders,
    /// the firmware files or traversal.
    #[test]
    fn regress_uploads_outside_orpheus_and_the_demo_tree_are_refused() {
        assert_eq!(
            create_command("/.orpheus/test/a.bin", Scope::Orpheus).unwrap(),
            "fcreate /.orpheus/test/a.bin"
        );
        assert_eq!(
            create_command("/.orpheus/test/a.bin", Scope::DemoTree).unwrap(),
            "fcreate /.orpheus/test/a.bin"
        );
        let demo = "/Music/Bach/Goldberg Variations/01 Aria.mp3";
        assert!(create_command(demo, Scope::Orpheus).is_err(), "the default scope refuses the demo tree");
        assert_eq!(create_command(demo, Scope::DemoTree).unwrap(), format!("fcreate-demo {demo}"));
        assert!(create_command("/CREDITS.txt", Scope::DemoTree).is_ok());
        for bad in [
            "/update.yfw",
            "/otpauth.txt",
            "/Recordings/x.wav",
            "/Music",
            "/Music/../update.yfw",
            "/.orpheus/../update.yfw",
            "/Pictures/.hidden",
            "Music/x.mp3",
            "/",
        ] {
            for scope in [Scope::Orpheus, Scope::DemoTree] {
                assert!(create_command(bad, scope).is_err(), "{bad} ({scope:?})");
            }
        }
        let long = format!("/Music/{}.mp3", "x".repeat(120));
        assert!(create_command(&long, Scope::DemoTree).is_err(), "longer than the firmware's buffer");
    }

    #[test]
    fn fsize_replies_are_matched_to_their_path() {
        assert_eq!(parse_fsize("size 1234 /Music/a b.mp3", "/Music/a b.mp3"), Some(Some(1234)));
        assert_eq!(parse_fsize("fsize: no such file /Music/a.mp3", "/Music/a.mp3"), Some(None));
        // A stale reply about the previous file, or another command's.
        assert_eq!(parse_fsize("size 99 /Music/old.mp3", "/Music/a.mp3"), None);
        assert_eq!(parse_fsize("closed 1024", "/Music/a.mp3"), None);
        assert_eq!(parse_fsize("size x /Music/a.mp3", "/Music/a.mp3"), None);
    }
}
