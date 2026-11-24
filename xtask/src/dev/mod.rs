//! `xtask dev …`: commands that talk to the player (07 §2, §5).
//!
//! Safety rules (CLAUDE.md, 07 §7) enforced here:
//! * NOR writes only inside the dual-boot free region, and only after
//!   `backup-verify` passed in this process;
//! * no command powers the player off;
//! * one process at a time: a lock file in `target/yoton/`.

mod hil;
mod ram;
mod stage;
mod usbtest;

use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use yoton_host::adfu::{Adfu, yoton_dir};
use yoton_host::{device, image, nor};
use yoton_proto::layout::dual;
use yoton_proto::mem::{FB_BASE, FB_H, FB_SIZE, FB_W};
use yoton_proto::{fault, mailbox, result};

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
  stage IMAGE|NAME|FILE.yfw      write a YFW1 slot to slot A (0x762000)
  install-stage2                 replace stage 2 at 0x7DF400 (keeps the vendor record copy)
  keys 'ok wait:300 hold:menu'  inject keys into running yotonrs
  info                          developer-channel INFO of running yotonrs
  stats [--irq]                 executor idle time, pager and UI counters (--irq: per line)
  time [--set]                  read (or set from the host, UTC) the player's clock
  cmd 'play /x.mp3' [--wait S]  firmware test command, then print the reply
  results                       read result block, fault record and log ring
  peek ADDR [N]                 read words
  poke ADDR VALUE               write one word (ADFU/ymon)
  dump ADDR LEN OUT             read memory to a file
  shot OUT.png                  screenshot of the frame buffer
  nordump ADDR LEN OUT          read NOR (raw)
  backup-verify                 check the vendor NOR backups
  save-rockbox                  dump the installed Rockbox region of NOR to target/yoton/
  restore rockbox [--verify]    write the saved Rockbox region back (--verify: compare only)
  reset                         reset through ymon (the installed firmware boots)
";

pub fn cmd(args: &mut Args) -> Result<String> {
    let sub = args.next_positional().unwrap_or_else(|| "help".into());
    if sub == "help" {
        print!("{HELP}");
        return Ok("help".into());
    }
    let _lock = DeviceLock::take()?;
    match sub.as_str() {
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
        "keys" => {
            let script = args.positional("key script")?;
            keys(&script)
        }
        "cmd" => {
            let text = args.positional("command text")?;
            let wait = args.value("wait").map(|s| num(&s)).transpose()?.unwrap_or(2);
            let d = yoton_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            d.control_out(yoton_proto::devchan::REQ_TEST, 0, 0, text.as_bytes())?;
            std::thread::sleep(Duration::from_secs(u64::from(wait)));
            let r = d.control_in(yoton_proto::devchan::REQ_TEST, 0, 0, 1024)?;
            println!("{}", String::from_utf8_lossy(&r));
            Ok("reply printed".into())
        }
        "stats" => {
            let d = yoton_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            if args.flag("irq") {
                let r = d.control_in(yoton_proto::devchan::REQ_STATS, 1, 0, 256)?;
                let mut out = String::from("interrupts per line:");
                for (i, c) in r.chunks_exact(4).enumerate() {
                    let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    if v != 0 {
                        out.push_str(&format!(" {i}:{v}"));
                    }
                }
                return Ok(out);
            }
            let r = d.control_in(yoton_proto::devchan::REQ_STATS, 0, 0, 128)?;
            let w: Vec<u32> =
                r.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            let g = |i: usize| w.get(i).copied().unwrap_or(0);
            Ok(format!(
                "uptime {} ms, idle {} us, pager loads {} (code faults {}, ro faults {}, evictions {}), usb resets {} setups {}, ui loops {} busy {} us rows {}, settings writes {} (last key {:#06x}, {} us), ui draw {} us flush {} us, rt decode {} us fill {} us serve {} us, clock level {} ({} switches)",
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
                ["Low 24", "Base 48", "Mid 96", "High 144", "Turbo 192"]
                    .get(g(19) as usize)
                    .copied()
                    .unwrap_or("?"),
                g(20)
            ))
        }
        "time" => {
            let d = yoton_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            if args.flag("set") {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
                let (y, mo, da, h, mi, se) = civil(now);
                let yb = (y as u16).to_le_bytes();
                d.control_out(yoton_proto::devchan::REQ_TIME, 0, 0, &[yb[0], yb[1], mo, da, h, mi, se])?;
            }
            let t = d.control_in(yoton_proto::devchan::REQ_TIME, 0, 0, 7)?;
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
            let d = yoton_host::devchan::DevChannel::open(Duration::from_secs(3))?;
            let i = d.info()?;
            Ok(format!(
                "uptime {} ms, battery {} mV, flags {:#x}, build {}, stack free {} of {} B",
                i.uptime_ms,
                i.battery_mv,
                i.flags,
                String::from_utf8_lossy(&i.build_id).trim_end_matches('\0'),
                i.reserved & 0xFFFF,
                i.reserved >> 16
            ))
        }
        "stage" => stage::cmd_stage(args),
        "install-stage2" => stage::cmd_install_stage2(args),
        "stage2-ramtest" => stage::cmd_stage2_ramtest(args),
        "results" => {
            let mut d = to_ymon()?;
            hil::print_results(&mut d, None)
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
                let d = yoton_host::devchan::DevChannel::open(Duration::from_secs(3))?;
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
            let what = args.positional("rockbox")?;
            if what != "rockbox" {
                bail!("only `restore rockbox` exists so far (vendor-boot: WP-LAY-1)");
            }
            restore_rockbox(args.flag("verify"))
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

impl DeviceLock {
    fn take() -> Result<Self> {
        let p = out_dir().join("device.lock");
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

/// Key names in scripts (yoton_input::Key order).
const KEY_NAMES: [&str; 10] =
    ["power", "back", "menu", "ok", "prev", "next", "up", "down", "volup", "voldown"];

/// `ok`, `down*3`, `hold:menu` (800 ms), `long:power` (4.5 s), `wait:300`.
fn keys(script: &str) -> Result<String> {
    let d = yoton_host::devchan::DevChannel::open(Duration::from_secs(3))?;
    let send = |k: u8, down: bool| d.control_out(yoton_proto::devchan::REQ_KEYS, 0, 0, &[k, u8::from(down)]);
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
