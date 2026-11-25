//! `cargo xtask site [--check]` and `cargo xtask site shots [--check]`
//! (WEB-1, DOC-2; 09 §2–3).
//!
//! The website is built from `site/` (template, stylesheet, script, the
//! pages that exist only on the site) and the Markdown in `docs/user/`,
//! `docs/dev/` and `CHANGELOG.md`, into `target/orpheus/site/`. Links between
//! Markdown files are rewritten to the generated pages; links to other files
//! in the repository go to GitHub. `--check` fails on a broken internal link
//! or fragment, a screenshot without a golden image or out of date in
//! `docs/shots/`, a manual page over 600 words, or a Home page over 500 KB.
//!
//! Screenshots: every `shots/NAME.png` referenced by the README, the
//! documentation or the site's pages is `sim/golden/NAME.png` scaled ×2
//! (nearest neighbour). `site shots` writes them to `docs/shots/` (the
//! README and GitHub show those); the site build makes its own copies.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

use crate::args::Args;
use crate::util::{log, out_dir, root};

/// Where the site is published (decision D-11).
pub const BASE: &str = "https://timschmidt.github.io/orpheus/";
const REPO: &str = "https://github.com/timschmidt/orpheus";
const SHOTS: &str = "docs/shots";
const GOLDEN: &str = "sim/golden";
/// 09 §2: one page per topic, at most 600 words.
const MANUAL_WORDS: usize = 600;
/// 09 §3: Home at most 500 KB before the simulator is opened.
const HOME_BYTES: u64 = 500 * 1000;
/// Never linked from the site (decision D-7; local state).
const UNPUBLISHED: [&str; 3] = ["docs/hw", "docs/vendor", "local"];

const NAV: [(&str, &str, &str); 8] = [
    ("index.html", "Home", "home"),
    ("manual/index.html", "Manual", "manual"),
    ("dev/index.html", "Developers", "dev"),
    ("hardware.html", "Hardware", "hardware"),
    ("changelog.html", "Changelog", "changelog"),
    ("manual/faq.html", "FAQ", "faq"),
    ("try.html", "Try it", "try"),
    ("install.html", "Install", "install"),
];

/// One page of the site.
#[derive(Clone, Debug)]
struct Page {
    /// Source, relative to the repository root.
    src: PathBuf,
    /// Output, relative to the site root, `/`-separated.
    out: String,
    /// Navigation entry it belongs to.
    section: &'static str,
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root().join(dir))
        .map(|rd| {
            rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "md")).collect()
        })
        .unwrap_or_default();
    v.sort();
    v.into_iter().map(|p| dir.join(p.file_name().unwrap_or_default())).collect()
}

fn html_name(p: &Path) -> String {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    if stem == "README" { "index.html".into() } else { format!("{stem}.html") }
}

fn pages() -> Vec<Page> {
    let mut v = vec![Page { src: "site/pages/index.html".into(), out: "index.html".into(), section: "home" }];
    for p in md_files(Path::new("site/pages")) {
        let name = html_name(&p);
        let section = match name.as_str() {
            "try.html" => "try",
            "install.html" => "install",
            "hardware.html" => "hardware",
            _ => "",
        };
        v.push(Page { src: p, out: name, section });
    }
    v.push(Page { src: "CHANGELOG.md".into(), out: "changelog.html".into(), section: "changelog" });
    for p in md_files(Path::new("docs/user")) {
        let name = html_name(&p);
        let section = if name == "faq.html" { "faq" } else { "manual" };
        v.push(Page { out: format!("manual/{name}"), src: p, section });
    }
    for p in md_files(Path::new("docs/dev")) {
        v.push(Page { out: format!("dev/{}", html_name(&p)), src: p, section: "dev" });
    }
    for p in md_files(Path::new("docs/dev/how-to")) {
        v.push(Page { out: format!("dev/how-to/{}", html_name(&p)), src: p, section: "dev" });
    }
    v
}

/// `from` and `to` are site-relative; the result is relative to `from`'s
/// directory.
fn rel(from: &str, to: &str) -> String {
    format!("{}{to}", "../".repeat(from.matches('/').count()))
}

/// Resolve `.` and `..` without touching the file system. `None` if the
/// path leaves the repository.
fn normalize(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(s) => out.push(s),
            _ => return None,
        }
    }
    Some(out)
}

fn is_external(dest: &str) -> bool {
    dest.contains("://") || dest.starts_with("mailto:") || dest.starts_with("data:")
}

/// GitHub's heading anchors: lower case; letters, digits, spaces, `-`
/// and `_` kept; spaces become `-`.
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Everything the renderer needs to map links.
struct Ctx {
    by_src: BTreeMap<PathBuf, String>,
}

impl Ctx {
    /// Rewrite one link or image destination found in `page`'s source.
    fn link(&self, page: &Page, dest: &str, problems: &mut Vec<String>) -> String {
        if dest.starts_with('#') || is_external(dest) || page.src.starts_with("site/pages") {
            // Site pages link in site terms; the built site is checked.
            return dest.to_string();
        }
        let (path, frag) = dest.split_once('#').map_or((dest, ""), |(p, f)| (p, f));
        let frag = if frag.is_empty() { String::new() } else { format!("#{frag}") };
        let base = page.src.parent().unwrap_or(Path::new(""));
        let Some(target) = normalize(&base.join(path)) else {
            problems.push(format!("{}: link `{dest}` leaves the repository", page.src.display()));
            return dest.to_string();
        };
        if let Some(out) = self.by_src.get(&target) {
            return format!("{}{frag}", rel(&page.out, out));
        }
        if let Some(out) = self.by_src.get(&target.join("README.md")) {
            return format!("{}{frag}", rel(&page.out, out));
        }
        let t = target.to_string_lossy().replace('\\', "/");
        let mapped = if let Some(n) = t.strip_prefix(&format!("{SHOTS}/")) {
            Some(format!("shots/{n}"))
        } else if let Some(n) = t.strip_prefix("docs/dev/img/") {
            Some(format!("dev/img/{n}"))
        } else if let Some(n) = t.strip_prefix("docs/brand/generated/") {
            Some(format!("brand/{n}"))
        } else if t == "docs/logo/orpheus-lyre.svg" {
            Some("brand/orpheus-lyre.svg".into())
        } else {
            None
        };
        if let Some(m) = mapped {
            return format!("{}{frag}", rel(&page.out, &m));
        }
        let full = root().join(&target);
        if UNPUBLISHED.iter().any(|u| target.starts_with(u)) {
            problems.push(format!("{}: link `{dest}` points into unpublished {}", page.src.display(), t));
        } else if !full.exists() {
            problems.push(format!("{}: broken link `{dest}` ({t} does not exist)", page.src.display()));
        }
        let kind = if full.is_dir() { "tree" } else { "blob" };
        format!("{REPO}/{kind}/main/{t}{frag}")
    }
}

/// A rendered Markdown page.
struct Rendered {
    html: String,
    title: String,
    description: String,
}

fn heading_text(events: &[Event<'_>], from: usize) -> String {
    let mut s = String::new();
    for e in &events[from + 1..] {
        match e {
            Event::End(TagEnd::Heading(_)) => break,
            Event::Text(t) | Event::Code(t) => s.push_str(t),
            _ => {}
        }
    }
    s
}

fn render(page: &Page, text: &str, ctx: &Ctx, problems: &mut Vec<String>) -> Rendered {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events: Vec<Event<'_>> = Parser::new_ext(text, opts).collect();
    let mut out: Vec<Event<'_>> = Vec::with_capacity(events.len());
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    let mut title = String::new();
    let mut description = String::new();
    let mut in_para = false;
    let mut para = String::new();
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::Heading { level, classes, attrs, .. }) => {
                let t = heading_text(&events, i);
                if *level == HeadingLevel::H1 && title.is_empty() {
                    title.clone_from(&t);
                }
                let base = slug(&t);
                let n = used.entry(base.clone()).or_insert(0);
                let id = if *n == 0 { base.clone() } else { format!("{base}-{n}") };
                *n += 1;
                out.push(Event::Start(Tag::Heading {
                    level: *level,
                    id: Some(CowStr::from(id)),
                    classes: classes.clone(),
                    attrs: attrs.clone(),
                }));
            }
            Event::Start(Tag::Link { link_type, dest_url, title: t, id }) => {
                let dest = ctx.link(page, dest_url, problems);
                out.push(Event::Start(Tag::Link {
                    link_type: *link_type,
                    dest_url: CowStr::from(dest),
                    title: t.clone(),
                    id: id.clone(),
                }));
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let dest = ctx.link(page, dest_url, problems);
                let mut alt = String::new();
                while i + 1 < events.len() {
                    i += 1;
                    match &events[i] {
                        Event::End(TagEnd::Image) => break,
                        Event::Text(t) | Event::Code(t) => alt.push_str(t),
                        _ => {}
                    }
                }
                let class = if dest.contains("shots/") {
                    r#" class="shot" width="256" height="256""#
                } else if dest.ends_with(".svg") && dest.contains("dev/img/") {
                    r#" class="diagram""#
                } else {
                    ""
                };
                out.push(Event::InlineHtml(CowStr::from(format!(
                    r#"<img src="{}" alt="{}"{class} loading="lazy" decoding="async">"#,
                    esc(&dest),
                    esc(&alt)
                ))));
            }
            Event::Start(Tag::Table(a)) => {
                out.push(Event::Html(CowStr::from("<div class=\"table\">")));
                out.push(Event::Start(Tag::Table(a.clone())));
            }
            Event::End(TagEnd::Table) => {
                out.push(Event::End(TagEnd::Table));
                out.push(Event::Html(CowStr::from("</div>")));
            }
            Event::Start(Tag::Paragraph) => {
                in_para = description.is_empty();
                out.push(events[i].clone());
            }
            Event::End(TagEnd::Paragraph) => {
                if in_para && !para.trim().is_empty() {
                    description = para.split_whitespace().collect::<Vec<_>>().join(" ");
                }
                in_para = false;
                para.clear();
                out.push(events[i].clone());
            }
            e => {
                if in_para && let Event::Text(t) | Event::Code(t) = e {
                    para.push_str(t);
                }
                out.push(e.clone());
            }
        }
        i += 1;
    }
    let mut html_out = String::new();
    html::push_html(&mut html_out, out.into_iter());
    if description.chars().count() > 160 {
        let cut: String = description.chars().take(157).collect();
        let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a).to_string();
        description = format!("{cut}…");
    }
    Rendered { html: html_out, title, description }
}

/// `<!-- key: value -->` at the top of an HTML page.
fn html_meta(text: &str, key: &str) -> Option<String> {
    let pat = format!("<!-- {key}: ");
    let s = text.find(&pat)? + pat.len();
    let e = text[s..].find(" -->")? + s;
    Some(text[s..e].to_string())
}

// ------------------------------------------------------------ screenshots

/// Names of the screenshots (`shots/NAME.png`) referenced by the README, the
/// documentation and the site's pages, with the files that reference them.
fn shot_refs() -> Result<BTreeMap<String, Vec<String>>> {
    let r = root();
    let mut files = vec![PathBuf::from("README.md")];
    for d in ["docs/user", "docs/dev", "docs/dev/how-to", "site/pages"] {
        files.extend(md_files(Path::new(d)));
    }
    files.push("site/pages/index.html".into());
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let text = std::fs::read_to_string(r.join(&f)).with_context(|| f.display().to_string())?;
        let mut rest = text.as_str();
        while let Some(at) = rest.find("shots/") {
            // `target/orpheus/shots/` is the simulator's output, not ours.
            let sim_output = rest[..at].ends_with("orpheus/");
            rest = &rest[at + 6..];
            let name: String =
                rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            if !sim_output && !name.is_empty() && rest[name.len()..].starts_with(".png") {
                out.entry(name).or_default().push(f.display().to_string());
            }
        }
    }
    Ok(out)
}

/// A golden image scaled ×2 with nearest-neighbour sampling, as PNG.
fn shot_png(name: &str) -> Result<Vec<u8>> {
    let path = root().join(GOLDEN).join(format!("{name}.png"));
    let file = std::fs::File::open(&path).with_context(|| format!("no golden image {}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| anyhow!("{name}: image too large"))?];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => bail!("{name}: indexed PNG after expansion"),
    };
    let mut rgba = Vec::with_capacity(w * h * 16);
    for y in 0..h * 2 {
        for x in 0..w * 2 {
            let p = &buf[((y / 2) * info.line_size)..][(x / 2) * ch..(x / 2) * ch + ch];
            let px = match ch {
                1 => [p[0], p[0], p[0], 255],
                2 => [p[0], p[0], p[0], p[1]],
                3 => [p[0], p[1], p[2], 255],
                _ => [p[0], p[1], p[2], p[3]],
            };
            rgba.extend_from_slice(&px);
        }
    }
    crate::brand::encode_png((w * 2) as u32, (h * 2) as u32, &rgba)
}

/// Every referenced screenshot, generated. Fails if one has no golden.
fn shots_generate() -> Result<Vec<(String, Vec<u8>)>> {
    let refs = shot_refs()?;
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for (name, from) in &refs {
        if root().join(GOLDEN).join(format!("{name}.png")).exists() {
            out.push((name.clone(), shot_png(name)?));
        } else {
            missing.push(format!("{name} (referenced by {})", from.join(", ")));
        }
    }
    if !missing.is_empty() {
        bail!("screenshots without a golden image in {GOLDEN}/:\n  {}", missing.join("\n  "));
    }
    Ok(out)
}

/// Out-of-date or superfluous files in `docs/shots/`.
fn shots_stale(shots: &[(String, Vec<u8>)]) -> Vec<String> {
    let dir = root().join(SHOTS);
    let mut bad: Vec<String> = shots
        .iter()
        .filter(|(n, d)| std::fs::read(dir.join(format!("{n}.png"))).ok().as_deref() != Some(d.as_slice()))
        .map(|(n, _)| format!("{SHOTS}/{n}.png"))
        .collect();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if !shots.iter().any(|(n, _)| format!("{n}.png") == f) {
                bad.push(format!("{SHOTS}/{f} (not referenced)"));
            }
        }
    }
    bad
}

/// Rewrite `docs/shots/` from the golden images. Called by `site shots`
/// and after `test golden --bless`.
pub fn write_shots() -> Result<String> {
    let shots = shots_generate()?;
    let dir = root().join(SHOTS);
    std::fs::create_dir_all(&dir)?;
    let mut n = 0;
    for (name, data) in &shots {
        let p = dir.join(format!("{name}.png"));
        if std::fs::read(&p).ok().as_deref() != Some(data.as_slice()) {
            std::fs::write(&p, data)?;
            n += 1;
        }
    }
    for e in std::fs::read_dir(&dir)?.flatten() {
        let f = e.file_name().to_string_lossy().to_string();
        if !shots.iter().any(|(s, _)| format!("{s}.png") == f) {
            std::fs::remove_file(e.path())?;
            log(&format!("removed {SHOTS}/{f}"));
        }
    }
    Ok(format!("{} screenshots in {SHOTS}/, {n} rewritten", shots.len()))
}

// ------------------------------------------------------------------ build

fn nav_html(page: &Page) -> String {
    let mut s = String::new();
    for (href, label, section) in NAV {
        let cur = if section == page.section { r#" aria-current="page""# } else { "" };
        let _ = writeln!(s, r#"      <li><a href="{}"{cur}>{label}</a></li>"#, rel(&page.out, href));
    }
    s.trim_end().to_string()
}

struct Built {
    problems: Vec<String>,
    files: usize,
}

fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut s = template.to_string();
    for (k, v) in vars {
        s = s.replace(&format!("{{{{{k}}}}}"), v);
    }
    s
}

fn write(out: &Path, rel_path: &str, data: &[u8]) -> Result<()> {
    let p = out.join(rel_path);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, data).with_context(|| p.display().to_string())
}

fn copy_dir(out: &Path, from: &str, to: &str) -> Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(root().join(from)).with_context(|| from.to_string())?.flatten() {
        if e.path().is_file() {
            let name = e.file_name().to_string_lossy().to_string();
            write(out, &format!("{to}/{name}"), &std::fs::read(e.path())?)?;
            n += 1;
        }
    }
    Ok(n)
}

fn build(out: &Path) -> Result<Built> {
    let r = root();
    // The browser builds (`xtask wasm`: WEB-2's try/pkg, WEB-3's
    // install/pkg) and a release's files (install/release) survive a site
    // rebuild.
    let keep = ["try/pkg", "install/pkg", "install/release"];
    let kept: Vec<(PathBuf, PathBuf)> = keep
        .iter()
        .map(|k| (out.join(k), out.with_file_name(format!("site-keep-{}", k.replace('/', "-")))))
        .collect();
    for (from, to) in &kept {
        if from.is_dir() {
            let _ = std::fs::remove_dir_all(to);
            std::fs::rename(from, to)?;
        }
    }
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    std::fs::create_dir_all(out)?;
    for (back, saved) in &kept {
        if saved.is_dir() {
            if let Some(parent) = back.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(saved, back)?;
        }
    }
    let template = std::fs::read_to_string(r.join("site/template.html")).context("site/template.html")?;
    let wordmark = std::fs::read_to_string(r.join(crate::brand::GENERATED).join("wordmark.svg"))
        .context("wordmark.svg (run `cargo xtask brand`)")?
        .trim()
        .replace(r#"role="img" aria-label="Orpheus""#, r#"aria-hidden="true" focusable="false""#);
    let pages = pages();
    let ctx = Ctx { by_src: pages.iter().map(|p| (p.src.clone(), p.out.clone())).collect() };
    let mut problems = Vec::new();
    let mut files = 0;
    for page in &pages {
        let text =
            std::fs::read_to_string(r.join(&page.src)).with_context(|| page.src.display().to_string())?;
        let is_md = page.src.extension().is_some_and(|e| e == "md");
        let (content, title, description, class) = if is_md {
            let rd = render(page, &text, &ctx, &mut problems);
            if rd.title.is_empty() {
                problems.push(format!("{}: no `# title`", page.src.display()));
            }
            (rd.html, format!("{} · Orpheus", rd.title), rd.description, "doc")
        } else {
            let title = html_meta(&text, "title").unwrap_or_else(|| "Orpheus".into());
            let description = html_meta(&text, "description").unwrap_or_default();
            (text.clone(), title, description, "home")
        };
        let root_rel = rel(&page.out, "");
        let canonical =
            if page.out == "index.html" { BASE.to_string() } else { format!("{BASE}{}", page.out) };
        let source = format!("{REPO}/blob/main/{}", page.src.display());
        let html = fill(
            &template,
            &[
                ("title", &esc(&title)),
                ("description", &esc(&description)),
                ("canonical", &canonical),
                ("base", BASE),
                ("root", &root_rel),
                ("nav", &nav_html(page)),
                ("wordmark", &wordmark),
                ("source", &source),
                ("body_class", class),
                ("content", content.trim_end()),
            ],
        );
        write(out, &page.out, html.as_bytes())?;
        files += 1;
    }
    // Not found (served at any depth: absolute links).
    let nf = Page { src: "site/template.html".into(), out: "404.html".into(), section: "" };
    let html = fill(
        &template,
        &[
            ("title", "Not found · Orpheus"),
            ("description", "This page does not exist."),
            ("canonical", BASE),
            ("base", BASE),
            ("root", BASE),
            ("nav", &nav_html(&nf).replace("href=\"", &format!("href=\"{BASE}"))),
            ("wordmark", &wordmark),
            ("source", REPO),
            ("body_class", "doc"),
            (
                "content",
                &format!(
                    "<h1>Not found</h1>\n<p>This page does not exist. Try the <a href=\"{BASE}\">home page</a> or the <a href=\"{BASE}manual/index.html\">manual</a>.</p>"
                ),
            ),
        ],
    );
    write(out, "404.html", html.as_bytes())?;
    // Styles: the palette first, then the site's rules.
    let mut css = std::fs::read_to_string(r.join(crate::brand::GENERATED).join("palette.css"))?;
    css.push('\n');
    css.push_str(&std::fs::read_to_string(r.join("site/style.css"))?);
    write(out, "style.css", css.as_bytes())?;
    write(out, "theme.js", &std::fs::read(r.join("site/theme.js"))?)?;
    files += copy_dir(out, crate::brand::GENERATED, "brand")?;
    write(out, "brand/orpheus-lyre.svg", &std::fs::read(r.join("docs/logo/orpheus-lyre.svg"))?)?;
    files += copy_dir(out, "docs/dev/img", "dev/img")?;
    // The Try-it page's script, worklet and styles (the module: `xtask wasm`).
    files += copy_dir(out, "site/try", "try")?;
    // The Install page's scripts and styles (the module: `xtask wasm`).
    files += copy_dir(out, "site/install", "install")?;
    for (name, data) in shots_generate()? {
        write(out, &format!("shots/{name}.png"), &data)?;
        files += 1;
    }
    let palette = crate::brand::parse_palette(&std::fs::read_to_string(r.join("docs/brand/palette.toml"))?)?;
    let manifest = format!(
        "{{\n  \"name\": \"Orpheus\",\n  \"short_name\": \"Orpheus\",\n  \"start_url\": \"index.html\",\n  \"display\": \"browser\",\n  \"theme_color\": \"{}\",\n  \"background_color\": \"{}\",\n  \"icons\": [\n    {{ \"src\": \"brand/icon-192.png\", \"sizes\": \"192x192\", \"type\": \"image/png\" }},\n    {{ \"src\": \"brand/icon-512.png\", \"sizes\": \"512x512\", \"type\": \"image/png\" }}\n  ]\n}}\n",
        crate::brand::hex(palette.brand["lyre"]),
        crate::brand::hex(palette.dark["bg"]),
    );
    write(out, "site.webmanifest", manifest.as_bytes())?;
    write(out, ".nojekyll", b"")?;
    Ok(Built { problems, files: files + 5 })
}

// ----------------------------------------------------------------- checks

/// Values of `attr="…"` in an HTML text.
fn attr_values<'a>(html: &'a str, attr: &str) -> Vec<&'a str> {
    let pat = format!(" {attr}=\"");
    let mut v = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find(&pat) {
        rest = &rest[at + pat.len()..];
        if let Some(end) = rest.find('"') {
            v.push(&rest[..end]);
            rest = &rest[end..];
        }
    }
    v
}

fn html_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                html_files(&p, out);
            } else if p.extension().is_some_and(|e| e == "html") {
                out.push(p);
            }
        }
    }
}

/// Every internal link and fragment of the built site resolves.
fn check_links(site: &Path) -> Vec<String> {
    let mut files = Vec::new();
    html_files(site, &mut files);
    let mut ids: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut texts = BTreeMap::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        ids.insert(f.clone(), attr_values(&text, "id").into_iter().map(str::to_string).collect());
        texts.insert(f.clone(), text);
    }
    let mut bad = Vec::new();
    for (f, text) in &texts {
        let shown = f.strip_prefix(site).unwrap_or(f).display().to_string();
        for dest in attr_values(text, "href").into_iter().chain(attr_values(text, "src")) {
            if is_external(dest) || dest.is_empty() {
                continue;
            }
            let dest = dest.replace("&amp;", "&");
            let (path, frag) = dest.split_once('#').map_or((dest.as_str(), ""), |(p, f)| (p, f));
            // A query (`install.html?demo`) names the same file.
            let path = path.split_once('?').map_or(path, |(p, _)| p);
            let target = if path.is_empty() {
                f.clone()
            } else {
                let mut t = f.parent().unwrap_or(site).join(path);
                if path.ends_with('/') {
                    t = t.join("index.html");
                }
                match normalize(t.strip_prefix(site).unwrap_or(&t)) {
                    Some(n) => site.join(n),
                    None => {
                        bad.push(format!("{shown}: `{dest}` leaves the site"));
                        continue;
                    }
                }
            };
            if !target.is_file() {
                bad.push(format!("{shown}: `{dest}` does not exist"));
                continue;
            }
            if !frag.is_empty() && target.extension().is_some_and(|e| e == "html") {
                let has = ids.get(&target).is_some_and(|s| s.contains(frag));
                if !has {
                    bad.push(format!("{shown}: `{dest}`: no id `{frag}`"));
                }
            }
        }
    }
    bad
}

/// Home plus what it loads (styles, script, icons, images).
fn home_bytes(site: &Path) -> Result<u64> {
    let text = std::fs::read_to_string(site.join("index.html"))?;
    let mut seen = BTreeSet::new();
    let mut total = text.len() as u64;
    for dest in attr_values(&text, "src").into_iter().chain(attr_values(&text, "href")) {
        let ext_ok =
            [".css", ".js", ".png", ".svg", ".ico", ".webmanifest"].iter().any(|e| dest.ends_with(e));
        if is_external(dest) || !ext_ok || !seen.insert(dest.to_string()) {
            continue;
        }
        total += std::fs::metadata(site.join(dest)).map(|m| m.len()).unwrap_or(0);
    }
    Ok(total)
}

/// Words of a Markdown page outside code blocks (09 §2 limit).
fn words(text: &str) -> usize {
    let mut n = 0;
    let mut code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        if !code {
            n += line.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count();
        }
    }
    n
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let check = args.flag("check");
    if let Some(sub) = args.next_positional() {
        if sub != "shots" {
            bail!("unknown `site {sub}` (site [--check] | site shots [--check])");
        }
        if check {
            let bad = shots_stale(&shots_generate()?);
            if !bad.is_empty() {
                bail!("screenshots out of date (run `cargo xtask site shots`):\n  {}", bad.join("\n  "));
            }
            return Ok("screenshots current".into());
        }
        return write_shots();
    }
    let site = out_dir().join("site");
    let built = build(&site)?;
    for p in &built.problems {
        log(&format!("problem: {p}"));
    }
    if !check {
        return Ok(format!(
            "{} files in {}{}",
            built.files,
            site.display(),
            if built.problems.is_empty() {
                String::new()
            } else {
                format!(", {} problems", built.problems.len())
            }
        ));
    }
    let mut bad = built.problems;
    bad.extend(check_links(&site));
    bad.extend(
        shots_stale(&shots_generate()?)
            .into_iter()
            .map(|s| format!("{s} is out of date (run `cargo xtask site shots`)")),
    );
    for p in md_files(Path::new("docs/user")) {
        let n = words(&std::fs::read_to_string(root().join(&p))?);
        if n > MANUAL_WORDS {
            bad.push(format!("{}: {n} words (at most {MANUAL_WORDS})", p.display()));
        }
    }
    let home = home_bytes(&site)?;
    if home > HOME_BYTES {
        bad.push(format!("Home loads {home} bytes (at most {HOME_BYTES})"));
    }
    if !bad.is_empty() {
        bail!("site check failed:\n  {}", bad.join("\n  "));
    }
    Ok(format!("{} files, links and screenshots current, Home {} KB", built.files, home.div_ceil(1000)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slug("8. Working with the player safely"), "8-working-with-the-player-safely");
        assert_eq!(slug("Install, update and restore"), "install-update-and-restore");
        assert_eq!(slug("Add a `hot.toml` cluster"), "add-a-hottoml-cluster");
        assert_eq!(slug("  Ünïcode — dash "), "ünïcode--dash");
    }

    #[test]
    fn relative_paths() {
        assert_eq!(rel("index.html", "manual/keys.html"), "manual/keys.html");
        assert_eq!(rel("manual/keys.html", "manual/faq.html"), "../manual/faq.html");
        assert_eq!(rel("dev/how-to/x.html", "style.css"), "../../style.css");
        assert_eq!(normalize(Path::new("docs/user/../dev/a.md")), Some(PathBuf::from("docs/dev/a.md")));
        assert_eq!(normalize(Path::new("../x")), None);
    }

    fn ctx() -> Ctx {
        Ctx { by_src: pages().into_iter().map(|p| (p.src, p.out)).collect() }
    }

    fn page(src: &str, out: &str) -> Page {
        Page { src: src.into(), out: out.into(), section: "manual" }
    }

    #[test]
    fn links_are_rewritten() {
        let c = ctx();
        let p = page("docs/user/keys.md", "manual/keys.html");
        let mut probs = Vec::new();
        assert_eq!(
            c.link(&p, "install.md#restore-the-original-firmware", &mut probs),
            "../manual/install.html#restore-the-original-firmware"
        );
        assert_eq!(c.link(&p, "../dev/how-to/", &mut probs), "../dev/how-to/index.html");
        assert_eq!(
            c.link(&p, "../shots/app_lock_dark_latin.png", &mut probs),
            "../shots/app_lock_dark_latin.png"
        );
        assert_eq!(c.link(&p, "../STATUS.md", &mut probs), format!("{REPO}/blob/main/docs/STATUS.md"));
        assert_eq!(c.link(&p, "../plan/", &mut probs), format!("{REPO}/tree/main/docs/plan"));
        assert_eq!(c.link(&p, "https://example.org/x", &mut probs), "https://example.org/x");
        assert!(probs.is_empty(), "{probs:?}");
        c.link(&p, "../nothing-here.md", &mut probs);
        c.link(&p, "../hw/system.md", &mut probs);
        c.link(&p, "../../../outside", &mut probs);
        assert_eq!(probs.len(), 3, "{probs:?}");
    }

    #[test]
    fn render_sets_ids_title_description_and_images() {
        let c = ctx();
        let p = page("docs/user/x.md", "manual/x.html");
        let mut probs = Vec::new();
        let md = "# Title here\n\nFirst *para* text.\n\n## Same\n\n## Same\n\n![Alt](../shots/app_lock_dark_latin.png)\n\n| a |\n|---|\n| b |\n";
        let r = render(&p, md, &c, &mut probs);
        assert_eq!(r.title, "Title here");
        assert_eq!(r.description, "First para text.");
        assert!(r.html.contains(r#"<h2 id="same">"#) && r.html.contains(r#"<h2 id="same-1">"#), "{}", r.html);
        assert!(r.html.contains(r#"class="shot""#) && r.html.contains(r#"alt="Alt""#), "{}", r.html);
        assert!(r.html.contains(r#"<div class="table">"#));
    }

    #[test]
    fn link_checker_finds_missing_files_and_ids() {
        let d = out_dir().join("site-test");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::write(d.join("index.html"), r##"<a href="a/b.html#here">x</a> <a href="#top">t</a> <img src="nope.png"> <a href="https://x.org/">e</a> <h1 id="top">T</h1>"##).unwrap();
        std::fs::write(
            d.join("a/b.html"),
            r#"<h2 id="there">x</h2><a href="../index.html">up</a> <a href="../../out.html">out</a>"#,
        )
        .unwrap();
        let bad = check_links(&d);
        assert_eq!(bad.len(), 3, "{bad:?}");
        assert!(bad.iter().any(|b| b.contains("no id `here`")));
        assert!(bad.iter().any(|b| b.contains("nope.png")));
        assert!(bad.iter().any(|b| b.contains("leaves the site")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn word_count_skips_code() {
        assert_eq!(words("one two\n```\nnot counted here\n```\nthree | - |"), 3);
    }

    #[test]
    fn shot_is_doubled() {
        let png = shot_png("app_lock_dark_latin").unwrap();
        let dec = png::Decoder::new(std::io::Cursor::new(png));
        let r = dec.read_info().unwrap();
        assert_eq!((r.info().width, r.info().height), (256, 256));
        assert!(shot_png("no_such_golden").is_err());
    }
}
