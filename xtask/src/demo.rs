//! The demo card (WEB-2): real, freely licensed works pinned by reference
//! in `sim/web/demo/sources.toml`.
//!
//! * `cargo xtask wasm --demo` ([`regenerate`]): download each source into
//!   `target/orpheus/demo-cache/` (curl, a generic User-Agent, one file at
//!   a time), check its SHA-256, derive the browser's excerpts into
//!   `sim/web/demo/card/` (committed) with `ffmpeg`, and write
//!   `card/CREDITS.txt` and `sim/web/demo/CREDITS.md` from the manifest.
//! * every `cargo xtask wasm` ([`bundle`]): pack the committed card into
//!   `try/pkg/demo.bin`, the page's one download of demo data.
//! * `cargo xtask demo-card` ([`card_tree`]): the player's card tree in
//!   `target/orpheus/demo-card/`: the browser's files plus the complete
//!   recordings and full-size pictures (`web = false` outputs), and a
//!   `CREDITS.txt` for all of them. `cargo xtask dev demo-card
//!   --outside-orpheus` uploads it; copying it in USB file-transfer mode
//!   works as well (`docs/dev/getting-started.md`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::util::{log, out_dir, root, run};

/// The User-Agent of the downloads: no names, no contact details.
const USER_AGENT: &str = "orpheus-demo-fetch/1.0";
/// The card's credits file.
pub const CREDITS: &str = "CREDITS.txt";
/// Licences a demo work may have: public domain, CC0, CC BY, CC BY-SA
/// (no NC, no ND: the excerpts are derivatives).
const LICENCES: [&str; 6] = ["PD", "CC0-1.0", "CC-BY-2.0", "CC-BY-SA-3.0", "CC-BY-SA-4.0", "CC-BY-4.0"];

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub work: Vec<Work>,
    pub own: Own,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Own {
    pub files: Vec<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Work {
    pub id: String,
    pub title: String,
    pub creator: String,
    pub performer: String,
    pub licence: String,
    pub licence_url: String,
    pub rights: String,
    pub source_page: String,
    pub url: String,
    pub sha256: String,
    pub credit: String,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    pub out: Vec<Out>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Out {
    /// Path on the card (no leading `/`).
    pub card: String,
    /// In the browser's set (committed); else only on the player's card.
    #[serde(default)]
    pub web: bool,
    #[serde(default)]
    pub copy: bool,
    #[serde(default)]
    pub ffmpeg: Vec<String>,
    pub lrc: Option<Lrc>,
    pub changes: String,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Lrc {
    /// Lines starting before this many seconds.
    pub until: Option<f64>,
    pub title: String,
    pub artist: String,
}

pub fn manifest_path() -> PathBuf {
    root().join("sim/web/demo/sources.toml")
}

/// The committed browser card.
pub fn web_dir() -> PathBuf {
    root().join("sim/web/demo/card")
}

pub fn load() -> Result<Manifest> {
    let p = manifest_path();
    let m: Manifest =
        toml::from_str(&std::fs::read_to_string(&p)?).with_context(|| p.display().to_string())?;
    check(&m)?;
    Ok(m)
}

/// The manifest's own rules: pinned, licensed as allowed, one recipe per
/// output, paths that stay inside the card's top-level folders.
pub fn check(m: &Manifest) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for w in &m.work {
        if !LICENCES.contains(&w.licence.as_str()) {
            bail!("{}: licence {} is not one the demo may use", w.id, w.licence);
        }
        if w.sha256.len() != 64 || !w.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("{}: no SHA-256", w.id);
        }
        for u in [&w.url, &w.source_page, &w.licence_url] {
            if !u.starts_with("https://") {
                bail!("{}: {u} is not https", w.id);
            }
        }
        for o in &w.out {
            let recipes =
                usize::from(o.copy) + usize::from(!o.ffmpeg.is_empty()) + usize::from(o.lrc.is_some());
            if recipes != 1 {
                bail!("{}: {} needs exactly one of copy, ffmpeg, lrc", w.id, o.card);
            }
            // The upload's rules (`dev demo-card`): inside the demo tree,
            // short enough for the firmware's command buffer.
            crate::dev::create_command(&format!("/{}", o.card), crate::dev::Scope::DemoTree)
                .with_context(|| w.id.clone())?;
            if !seen.insert(o.card.to_lowercase()) {
                bail!("{} twice", o.card);
            }
        }
        if !w.out.iter().any(|o| o.web) {
            bail!("{}: nothing for the browser", w.id);
        }
    }
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The cached source of `w`, downloaded and checked if missing.
fn fetch(w: &Work) -> Result<PathBuf> {
    let dir = out_dir().join("demo-cache");
    std::fs::create_dir_all(&dir)?;
    let ext =
        w.url.rsplit('.').next().filter(|e| e.len() <= 4 && e.bytes().all(|b| b.is_ascii_alphanumeric()));
    let p = dir.join(format!("{}-{}.{}", w.id, &w.sha256[..12], ext.unwrap_or("srt")));
    if p.exists() && sha256_hex(&std::fs::read(&p)?) == w.sha256 {
        return Ok(p);
    }
    log(&format!("downloading {} ({})", w.id, w.url));
    let part = p.with_extension("part");
    run(Command::new("curl")
        .args(["-sSfL", "--retry", "5", "--retry-delay", "30", "-A", USER_AGENT, "-o"])
        .arg(&part)
        .arg(&w.url))
    .context("curl (needed to fetch the demo sources)")?;
    // Wikimedia asks automated clients not to hurry.
    std::thread::sleep(std::time::Duration::from_secs(3));
    let got = sha256_hex(&std::fs::read(&part)?);
    if got != w.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("{}: SHA-256 {got}, the manifest pins {}", w.url, w.sha256);
    }
    std::fs::rename(&part, &p)?;
    Ok(p)
}

fn is_audio(card: &str) -> bool {
    let ext = card.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    ["mp3", "flac", "ogg", "opus", "spx", "wav", "aiff", "au"].contains(&ext.as_str())
}

/// `HH:MM:SS,mmm` to seconds.
fn srt_time(t: &str) -> Option<f64> {
    let (hms, ms) = t.trim().split_once(',')?;
    let mut s = 0.0;
    for part in hms.split(':') {
        s = s * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(s + ms.parse::<f64>().ok()? / 1000.0)
}

/// SubRip to LRC: each cue's start time and its text on one line.
pub fn srt_to_lrc(srt: &str, l: &Lrc) -> Result<String> {
    let mut out = format!("[ti:{}]\n[ar:{}]\n", l.title, l.artist);
    for cue in srt.replace("\r\n", "\n").split("\n\n") {
        let mut lines = cue.lines().filter(|s| !s.trim().is_empty());
        let (Some(_), Some(times)) = (lines.next(), lines.next()) else { continue };
        let start = times.split("-->").next().and_then(srt_time).with_context(|| format!("cue {times:?}"))?;
        if l.until.is_some_and(|u| start >= u) {
            break;
        }
        let text: Vec<&str> = lines.map(str::trim).collect();
        let cs = (start * 100.0).round() as u64;
        let _ = writeln!(out, "[{:02}:{:02}.{:02}]{}", cs / 6000, cs / 100 % 60, cs % 100, text.join(" "));
    }
    Ok(out)
}

/// Make one output of `w` into `dest`/`o.card`.
fn derive(w: &Work, src: &Path, o: &Out, dest: &Path) -> Result<()> {
    let out = dest.join(&o.card);
    std::fs::create_dir_all(out.parent().context("path")?)?;
    if o.copy {
        std::fs::copy(src, &out)?;
    } else if let Some(l) = &o.lrc {
        std::fs::write(&out, srt_to_lrc(&std::fs::read_to_string(src)?, l)?)?;
    } else {
        let mut c = Command::new("ffmpeg");
        c.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(src);
        c.args(["-map_metadata", "-1", "-fflags", "+bitexact", "-flags", "+bitexact"]);
        if is_audio(&o.card) {
            c.args(["-map", "0:a", "-flags:a", "+bitexact"]);
            for (k, v) in &w.tags {
                c.arg("-metadata").arg(format!("{k}={v}"));
            }
            c.arg("-metadata").arg(format!("copyright={} ({})", w.licence, w.credit));
        } else {
            c.args(["-frames:v", "1", "-update", "1"]);
        }
        c.args(&o.ffmpeg).arg(&out);
        run(&mut c).context("ffmpeg (needed for the demo card only)")?;
    }
    Ok(())
}

fn credits_header(all: bool) -> String {
    let mut s = String::from(
        "Credits for the Orpheus demo card\n\
         =================================\n\n\
         Every picture, recording and book on this card is a real work by the\n\
         people named below, in the public domain or under a free licence.\n\
         None of it was generated. Where a file was trimmed, resized or\n\
         converted, that is said under \"Changes\".\n\n\
         Licences: PD = public domain; CC0 = https://creativecommons.org/publicdomain/zero/1.0/;\n\
         CC BY / BY-SA: the licence link is given with the work. Files made\n\
         from a CC BY-SA work are under the same licence.\n\n",
    );
    if !all {
        s.push_str(
            "This is the browser's card: short excerpts. The demo card for the\n\
             player (`cargo xtask demo-card`) adds the complete recordings and\n\
             full-size pictures.\n\n",
        );
    }
    s
}

/// `CREDITS.txt` for the outputs selected by `pick`.
pub fn credits_txt(m: &Manifest, pick: impl Fn(&Out) -> bool) -> String {
    let all = m.work.iter().all(|w| w.out.iter().all(&pick));
    let mut s = credits_header(all);
    for w in &m.work {
        let outs: Vec<&Out> = w.out.iter().filter(|o| pick(o)).collect();
        if outs.is_empty() {
            continue;
        }
        let _ = writeln!(s, "{}\n{}", w.title, "-".repeat(w.title.chars().count()));
        let _ = writeln!(s, "By:       {}", w.creator);
        if !w.performer.is_empty() {
            let _ = writeln!(s, "Made by:  {}", w.performer);
        }
        let _ = writeln!(s, "Licence:  {} <{}>", w.licence, w.licence_url);
        let _ = writeln!(s, "Rights:   {}", w.rights);
        let _ = writeln!(s, "Credit:   {}", w.credit);
        let _ = writeln!(s, "Source:   {}", w.source_page);
        for o in outs {
            let _ = writeln!(s, "File:     {}\n  Changes: {}", o.card, o.changes);
        }
        s.push('\n');
    }
    s.push_str("Written for the demo by the Orpheus project (MIT OR Apache-2.0)\n");
    s.push_str("--------------------------------------------------------------\n");
    for f in &m.own.files {
        let _ = writeln!(s, "File:     {f}");
    }
    s
}

/// `sim/web/demo/CREDITS.md`: the same, for the repository.
pub fn credits_md(m: &Manifest) -> String {
    let mut s = String::from(
        "# Demo card credits\n\n\
         Generated from [`sources.toml`](sources.toml) by `cargo xtask wasm --demo`;\n\
         the card carries the same text as `CREDITS.txt`. Browser files are in\n\
         [`card/`](card/); the player's demo card (`cargo xtask demo-card`) adds the\n\
         complete versions marked *card only*. Nothing here is generated or\n\
         synthesised; licences were checked on the source pages on 2026-11-27.\n\n\
         | Work | By | Made by | Licence | Source | Files on the card (changes) |\n\
         |---|---|---|---|---|---|\n",
    );
    let cell = |t: &str| t.replace('|', "\\|");
    for w in &m.work {
        let files: Vec<String> = w
            .out
            .iter()
            .map(|o| {
                format!("`{}`{} ({})", o.card, if o.web { "" } else { " *card only*" }, cell(&o.changes))
            })
            .collect();
        let _ = writeln!(
            s,
            "| {} | {} | {} | [{}]({}) | [source]({}) | {} |",
            cell(&w.title),
            cell(&w.creator),
            cell(&w.performer),
            w.licence,
            w.licence_url,
            w.source_page,
            files.join("<br>")
        );
    }
    s.push_str("\nRights in detail:\n\n");
    for w in &m.work {
        let _ = writeln!(s, "* **{}**: {} Credit: {}.", cell(&w.title), w.rights, w.credit);
    }
    s
}

/// Files of a directory tree, paths relative to it with `/`.
fn tree(dir: &Path) -> Result<Vec<String>> {
    Ok(orpheus_web::demo::read_tree(dir)?.into_iter().map(|f| f.path[1..].to_string()).collect())
}

/// `cargo xtask wasm --demo`: download, check, derive the browser's set.
pub fn regenerate() -> Result<String> {
    let m = load()?;
    let dest = web_dir();
    std::fs::create_dir_all(&dest)?;
    // Generated files are replaced; the project's own are kept.
    for f in tree(&dest)? {
        if !m.own.files.contains(&f) {
            std::fs::remove_file(dest.join(&f))?;
        }
    }
    let mut made = Vec::new();
    for w in &m.work {
        let src = fetch(w)?;
        for o in w.out.iter().filter(|o| o.web) {
            derive(w, &src, o, &dest)?;
            made.push(format!("{} {} B", o.card, std::fs::metadata(dest.join(&o.card))?.len()));
        }
    }
    std::fs::write(dest.join(CREDITS), credits_txt(&m, |o| o.web))?;
    std::fs::write(root().join("sim/web/demo/CREDITS.md"), credits_md(&m))?;
    remove_empty_dirs(&dest);
    for f in &m.own.files {
        if !dest.join(f).exists() {
            bail!("{f}: the project's own file is missing from {}", dest.display());
        }
    }
    Ok(made.join(", "))
}

fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                remove_empty_dirs(&e.path());
                let _ = std::fs::remove_dir(e.path());
            }
        }
    }
}

/// The page's demo data: the committed card packed.
pub fn bundle() -> Result<Vec<u8>> {
    let files = orpheus_web::demo::read_tree(&web_dir())?;
    if files.is_empty() {
        bail!("{} is empty (cargo xtask wasm --demo)", web_dir().display());
    }
    Ok(orpheus_web::bundle::encode(&files))
}

/// `cargo xtask demo-card`: the player's demo tree.
pub fn card_tree() -> Result<String> {
    let m = load()?;
    let dest = out_dir().join("demo-card");
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    let mut n = 0;
    for f in tree(&web_dir())? {
        if f == CREDITS {
            continue;
        }
        let to = dest.join(&f);
        std::fs::create_dir_all(to.parent().context("path")?)?;
        std::fs::copy(web_dir().join(&f), &to)?;
        n += 1;
    }
    for w in &m.work {
        let outs: Vec<&Out> = w.out.iter().filter(|o| !o.web).collect();
        if outs.is_empty() {
            continue;
        }
        let src = fetch(w)?;
        for o in outs {
            derive(w, &src, o, &dest)?;
            n += 1;
        }
    }
    std::fs::write(dest.join(CREDITS), credits_txt(&m, |_| true))?;
    let bytes: u64 =
        tree(&dest)?.iter().filter_map(|f| std::fs::metadata(dest.join(f)).ok()).map(|m| m.len()).sum();
    Ok(format!(
        "{} files ({} MB) in {}; upload with `cargo xtask dev demo-card --outside-orpheus` or copy in USB file-transfer mode",
        n + 1,
        bytes.div_ceil(1 << 20),
        dest.display()
    ))
}

pub fn cmd(_args: &mut crate::args::Args) -> Result<String> {
    card_tree()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_complete_and_the_committed_card_matches_it() {
        let m = load().unwrap();
        let on_disk: std::collections::BTreeSet<String> = tree(&web_dir()).unwrap().into_iter().collect();
        let mut want: std::collections::BTreeSet<String> =
            m.work.iter().flat_map(|w| w.out.iter().filter(|o| o.web).map(|o| o.card.clone())).collect();
        want.extend(m.own.files.iter().cloned());
        want.insert(CREDITS.into());
        assert_eq!(on_disk, want, "sim/web/demo/card/ against sources.toml (cargo xtask wasm --demo)");
        // The credits on the card and in the repository are current.
        let credits = std::fs::read_to_string(web_dir().join(CREDITS)).unwrap();
        assert_eq!(credits, credits_txt(&m, |o| o.web), "CREDITS.txt (cargo xtask wasm --demo)");
        let md = std::fs::read_to_string(root().join("sim/web/demo/CREDITS.md")).unwrap();
        assert_eq!(md, credits_md(&m), "CREDITS.md (cargo xtask wasm --demo)");
    }

    #[test]
    fn manifest_rules_refuse_unpinned_or_restricted_works() {
        let base = std::fs::read_to_string(manifest_path()).unwrap();
        let bad = [
            base.replacen("licence = \"CC0-1.0\"", "licence = \"CC-BY-NC-4.0\"", 1),
            base.replacen("sha256 = \"1b6c", "sha256 = \"zz6c", 1),
            base.replacen("card = \"Music/Bach/", "card = \"System/Bach/", 1),
            base.replacen("copy = true", "copy = true\nffmpeg = [\"-c\", \"copy\"]", 1),
        ];
        for t in bad {
            let m: Manifest = toml::from_str(&t).unwrap();
            assert!(check(&m).is_err());
        }
    }

    #[test]
    fn srt_becomes_lrc() {
        let srt = "1\r\n00:00:0,000 --> 00:00:12,000\r\nRecitar!\r\n\r\n2\r\n00:01:12,200 --> 00:01:22,000\r\nridi, Pagliaccio,\r\ne ognun\r\n\r\n3\r\n00:02:00,000 --> 00:02:01,000\r\nlater\r\n";
        let l = Lrc { until: Some(100.0), title: "T".into(), artist: "A".into() };
        assert_eq!(
            srt_to_lrc(srt, &l).unwrap(),
            "[ti:T]\n[ar:A]\n[00:00.00]Recitar!\n[01:12.20]ridi, Pagliaccio, e ognun\n"
        );
    }
}
