// Orpheus in the browser (WEB-2, docs/plan/09-docs-website.md §4).
//
// Loads the WebAssembly build (`cargo xtask wasm` → try/pkg/), shows the
// player's screen in the canvas, maps the drawn keys and the keyboard to the
// player's keys, plays the decoded PCM through an AudioWorklet, keeps the
// visitor's files and the player's state in IndexedDB (never uploaded), and
// offers a developer panel (frame time, memory, key recorder/replayer in
// the `ui_keys` fuzz corpus format, screenshot). The demo card's files
// (real, freely licensed works; credits in CREDITS.txt on the card and
// sim/web/demo/CREDITS.md) come as one download, pkg/demo.bin, fetched
// while the module compiles.
//
// `try.html?selftest` runs a self-test instead (tests/web/try.mjs drives it
// in a headless browser): no stored state, compare two screens with their
// golden images, play a track, check the budgets; the result is put in
// `window.orpheusSelftest` and the document title.

const $ = (id) => document.getElementById(id);
const root = $('try');
const player = $('try-player');
const canvas = $('try-screen');
const ctx = canvas.getContext('2d', { alpha: false });
const image = ctx.createImageData(128, 128);
const status = (text) => { $('try-status').textContent = text; };
const SELFTEST = new URLSearchParams(location.search).has('selftest');

let web = null; // the wasm session
let wasm = null; // the module's exports (memory)
let firstScreenMs = null;
// For the browser console and tests/web/try.mjs: `orpheus.web.run_script('down ok')`.
window.orpheus = { get web() { return web; }, get wasm() { return wasm; } };

// ------------------------------------------------------------- scaling

function fit() {
  const main = document.querySelector('main');
  const avail = Math.min(document.documentElement.clientWidth, (main ? main.clientWidth : 800));
  const byWidth = Math.floor(avail / 176);
  const byHeight = Math.max(2, Math.floor((window.innerHeight - 60) / 340));
  const k = Math.max(1, Math.min(byWidth, byHeight, 3));
  player.style.setProperty('--k', String(k));
  const over = 176 * k - (main ? main.clientWidth - 32 : avail);
  player.style.marginLeft = over > 0 ? `${-Math.ceil(over / 2)}px` : '';
}
window.addEventListener('resize', fit);
fit();

// ------------------------------------------------------------- storage

const DB = SELFTEST ? null : 'orpheus-try';

function idb() {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => {
      r.result.createObjectStore('files', { keyPath: 'name' });
      r.result.createObjectStore('kv');
    };
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

let dbp = null;
function db() {
  if (!DB) return Promise.reject(new Error('no storage in the self-test'));
  if (!dbp) dbp = idb();
  return dbp;
}

async function tx(store, mode, fn) {
  const d = await db();
  return new Promise((resolve, reject) => {
    const t = d.transaction(store, mode);
    const r = fn(t.objectStore(store));
    t.oncomplete = () => resolve(r && 'result' in r ? r.result : undefined);
    t.onerror = () => reject(t.error);
    t.onabort = () => reject(t.error);
  });
}

const kvGet = (k) => tx('kv', 'readonly', (s) => s.get(k)).catch(() => undefined);
const kvPut = (k, v) => tx('kv', 'readwrite', (s) => s.put(v, k)).catch(() => undefined);
const filesAll = () => tx('files', 'readonly', (s) => s.getAll()).catch(() => []);

// ------------------------------------------------------------- session

async function loadFiles() {
  const files = (await filesAll()).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  web.clear_files();
  for (const f of files) web.add_file(f.name, new Uint8Array(f.data));
  showFiles(files);
  return files;
}

async function boot(state) {
  const fp = web.fingerprint();
  const lib = await kvGet('library');
  if (lib && lib.fp === fp) {
    for (const [name, data] of lib.files) web.add_library_file(name, new Uint8Array(data));
  }
  let err = web.boot(state || '');
  if (err) {
    console.warn('Orpheus: boot with the saved state failed:', err);
    err = web.boot('');
  }
  if (err) throw new Error(err);
  if (!SELFTEST) {
    const d = new Date();
    web.set_clock(d.getFullYear(), d.getMonth() + 1, d.getDate(), d.getHours(), d.getMinutes(), d.getSeconds(), (d.getDay() + 6) % 7);
    if (!lib || lib.fp !== fp) {
      const files = web.library_names().split('\n').filter(Boolean).map((n) => [n, web.library_file(n).buffer]);
      kvPut('library', { fp, files });
    }
  }
  lastState = web.state();
}

let lastState = '';
// A replayed sequence runs on default settings: not saved over the visitor's.
let frozen = false;
function saveState() {
  if (!web || SELFTEST || frozen) return;
  const s = web.state();
  if (s !== lastState) {
    lastState = s;
    kvPut('state', s);
  }
}
setInterval(saveState, 3000);
document.addEventListener('visibilitychange', () => { if (document.hidden) saveState(); });
window.addEventListener('pagehide', saveState);

// ------------------------------------------------------------- audio

let actx = null;
let node = null;
let gainNode = null;
let sent = 0; // source frames sent in this epoch
let played = 0; // and played (worklet's report)
let underruns = 0;
let epoch = 0;
let wasActive = false;

async function startAudio() {
  if (actx) {
    if (actx.state === 'suspended') actx.resume();
    return;
  }
  try {
    actx = new AudioContext({ latencyHint: 'playback' });
    await actx.audioWorklet.addModule(new URL('worklet.js', import.meta.url));
    node = new AudioWorkletNode(actx, 'orpheus-out', { numberOfInputs: 0, outputChannelCount: [2] });
    gainNode = actx.createGain();
    gainNode.gain.value = web ? web.gain() : 0;
    node.connect(gainNode).connect(actx.destination);
    node.port.onmessage = (e) => {
      if (e.data.epoch === epoch) played = e.data.played;
      underruns = e.data.underruns;
    };
    if (actx.state === 'suspended') await actx.resume();
    $('try-sound').textContent = `Sound on (${actx.sampleRate} Hz output).`;
  } catch (e) {
    $('try-sound').textContent = `No sound: ${e.message}`;
    actx = null;
    node = null;
  }
}

function pumpAudio(dtMs) {
  if (web.take_flush() && node) {
    epoch++;
    sent = 0;
    played = 0;
    node.port.postMessage({ flush: true, epoch });
  }
  const active = web.audio_active();
  const rate = web.audio_rate();
  if (!node || !rate) {
    // No output yet: keep the decoder in step with the player.
    if (rate) web.pull_audio(Math.ceil((rate * dtMs) / 1000));
    return;
  }
  const target = (document.hidden ? 1.5 : 0.4) * rate;
  const need = Math.floor(target - (sent - played));
  if (need > rate / 50) {
    const pcm = web.pull_audio(need);
    if (pcm.length) {
      sent += pcm.length / 2;
      node.port.postMessage({ pcm, rate, active }, [pcm.buffer]);
    }
  }
  if (active !== wasActive) {
    wasActive = active;
    node.port.postMessage({ active });
  }
  gainNode.gain.setTargetAtTime(web.gain(), actx.currentTime, 0.02);
}

// ------------------------------------------------------------- loop

let last = 0;
const frameTimes = [];
let replaying = false;

function step(now) {
  const t0 = performance.now();
  const dt = last ? Math.min(t0 - last, 5000) : 0;
  last = t0;
  if (!replaying) web.advance(dt);
  pumpAudio(dt);
  const px = web.frame();
  if (px.length) {
    image.data.set(px);
    ctx.putImageData(image, 0, 0);
    if (firstScreenMs === null) firstScreenMs = performance.now();
  }
  frameTimes.push(performance.now() - t0);
  if (frameTimes.length > 240) frameTimes.shift();
  return now;
}

function raf(now) {
  step(now);
  requestAnimationFrame(raf);
}

// rAF stops in hidden tabs: keep time and sound going there.
setInterval(() => { if (web && document.hidden) step(performance.now()); }, 250);

// ------------------------------------------------------------- keys

const KEYMAP = {
  ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'prev', ArrowRight: 'next',
  Enter: 'ok', ' ': 'ok', Backspace: 'back', Escape: 'back',
  m: 'menu', M: 'menu', '+': 'vol+', '=': 'vol+', '-': 'vol-', '_': 'vol-', p: 'power', P: 'power',
};
const held = new Map(); // key -> source
const recordingKeys = () => recBtn.classList.contains('recording');

function keyEl(key) {
  return player.querySelector(`[data-key="${CSS.escape(key)}"]`);
}

function press(key, src) {
  if (!web || replaying || held.has(key)) return;
  startAudio();
  step(performance.now());
  held.set(key, src);
  web.press(key);
  keyEl(key)?.classList.add('down');
  if (recordingKeys()) $('try-rec-note').textContent = `Recording: ${web.rec_ops()} operations.`;
}

function release(key) {
  if (!web || !held.has(key)) return;
  step(performance.now());
  held.delete(key);
  web.release(key);
  keyEl(key)?.classList.remove('down');
}

function ours(e) {
  const a = document.activeElement;
  if (e.ctrlKey || e.altKey || e.metaKey) return false;
  if (a && a !== document.body && a !== document.documentElement && !player.contains(a)) return false;
  return true;
}

window.addEventListener('keydown', (e) => {
  const k = KEYMAP[e.key];
  if (!k || !ours(e)) return;
  e.preventDefault();
  if (!e.repeat) press(k, 'kbd');
});
window.addEventListener('keyup', (e) => {
  const k = KEYMAP[e.key];
  if (!k) return;
  if (held.get(k) === 'kbd') {
    e.preventDefault();
    release(k);
  }
});
window.addEventListener('blur', () => { for (const k of [...held.keys()]) release(k); });

player.addEventListener('pointerdown', (e) => {
  const el = e.target.closest('[data-key]');
  if (!el) return;
  e.preventDefault();
  player.focus({ preventScroll: true });
  el.setPointerCapture(e.pointerId);
  press(el.dataset.key, `p${e.pointerId}`);
});
for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) {
  player.addEventListener(type, (e) => {
    for (const [k, src] of held) if (src === `p${e.pointerId}`) release(k);
  });
}
player.addEventListener('contextmenu', (e) => e.preventDefault());

// ------------------------------------------------------------- your files

const yours = $('try-yours');
function showFiles(files) {
  yours.replaceChildren();
  for (const f of files) {
    const li = document.createElement('li');
    li.textContent = `${f.name} (${Math.ceil(f.data.byteLength / 1024)} KB) `;
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = 'Remove';
    b.addEventListener('click', async () => {
      await tx('files', 'readwrite', (s) => s.delete(f.name)).catch(() => undefined);
      await reload();
    });
    li.append(b);
    yours.append(li);
  }
}

async function reload() {
  status('Inserting the card…');
  root.dataset.state = 'loading';
  saveState();
  await loadFiles();
  await boot(web.state());
  root.dataset.state = 'running';
}

const MAX_BYTES = 256 * 1024 * 1024;
async function addFiles(list) {
  if (!list.length || SELFTEST) return;
  const have = (await filesAll()).reduce((n, f) => n + f.data.byteLength, 0);
  let total = have;
  for (const file of list) {
    if (total + file.size > MAX_BYTES) {
      alert(`${file.name}: the virtual card holds ${MAX_BYTES >> 20} MB of your files.`);
      continue;
    }
    const data = await file.arrayBuffer();
    total += data.byteLength;
    await tx('files', 'readwrite', (s) => s.put({ name: file.name, data, added: Date.now() }));
  }
  await reload();
}

$('try-files').addEventListener('change', (e) => addFiles([...e.target.files]));
let dragDepth = 0;
for (const el of [player, $('try-drop')]) {
  el.addEventListener('dragenter', (e) => { e.preventDefault(); dragDepth++; player.classList.add('dragging'); });
  el.addEventListener('dragover', (e) => { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; });
  el.addEventListener('dragleave', () => { if (--dragDepth <= 0) { dragDepth = 0; player.classList.remove('dragging'); } });
  el.addEventListener('drop', (e) => {
    e.preventDefault();
    dragDepth = 0;
    player.classList.remove('dragging');
    addFiles([...e.dataTransfer.files]);
  });
}

$('try-reset').addEventListener('click', async () => {
  if (!confirm('Factory reset: delete your files, the settings, the resume point and the library kept in this browser?')) return;
  if (dbp) (await dbp).close();
  dbp = null;
  await new Promise((resolve) => {
    const r = indexedDB.deleteDatabase(DB);
    r.onsuccess = r.onerror = r.onblocked = () => resolve();
  });
  lastState = '';
  web.clear_files();
  showFiles([]);
  await boot('');
});

// ------------------------------------------------------------- developer panel

const dev = $('try-dev');
$('try-dev-toggle').addEventListener('click', (e) => {
  dev.hidden = !dev.hidden;
  e.currentTarget.setAttribute('aria-expanded', String(!dev.hidden));
});

function stats() {
  const ft = frameTimes.slice(10);
  const mean = ft.reduce((a, b) => a + b, 0) / Math.max(1, ft.length);
  const worst = ft.reduce((a, b) => Math.max(a, b), 0);
  return { mean, worst, n: ft.length };
}

function showStats() {
  if (dev.hidden || !web) return;
  const s = stats();
  const i = JSON.parse(web.info());
  const mem = wasm ? wasm.memory.buffer.byteLength : 0;
  const rate = web.audio_rate();
  const rows = [
    ['frame time', `${s.mean.toFixed(2)} ms mean, ${s.worst.toFixed(2)} ms worst (last ${s.n})`],
    ['wasm memory', `${(mem / 1048576).toFixed(1)} MiB`],
    ['card', `${(i.card_bytes / 1048576).toFixed(0)} MiB FAT32`],
    ['screen', `${i.screen}, ${i.frames} frames drawn, ${i.dirty_pixels} pixels`],
    ['clock', `${(i.now_ms / 1000).toFixed(1)} s simulated`],
    ['track', i.track || '—'],
    ['decoder', i.codec ? `${i.codec}, arena ${i.arena} B, at ${(i.audio_ms / 1000).toFixed(1)} s` : '—'],
    ['output', actx ? `${rate} Hz → ${actx.sampleRate} Hz, ${rate ? ((sent - played) / rate * 1000).toFixed(0) : 0} ms buffered, ${underruns} dropouts, volume ${i.volume}` : 'not started'],
  ];
  if (i.audio_error) rows.push(['audio error', i.audio_error]);
  const dl = $('try-stats');
  dl.replaceChildren(...rows.flatMap(([k, v]) => {
    const dt = document.createElement('dt');
    dt.textContent = k;
    const dd = document.createElement('dd');
    dd.textContent = v;
    return [dt, dd];
  }));
}
setInterval(showStats, 500);

function download(name, blob) {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

$('try-shot').addEventListener('click', () => {
  canvas.toBlob((b) => download(`orpheus-${web.screen()}.png`, b), 'image/png');
});

const recBtn = $('try-rec');
recBtn.addEventListener('click', () => {
  if (recBtn.classList.toggle('recording')) {
    web.rec_start();
    recBtn.textContent = 'Stop recording';
    $('try-rec-note').textContent = 'Recording: 0 operations.';
  } else {
    const data = web.rec_stop();
    recBtn.textContent = 'Record keys';
    const n = (data.length - 1) / 3;
    $('try-rec-note').textContent = `Recorded ${n} operations${n > 48 ? ' (the fuzz target runs the first 48)' : ''}.`;
    const stamp = new Date().toISOString().replace(/[:.]/g, '-');
    download(`ui_keys-${stamp}.bin`, new Blob([data], { type: 'application/octet-stream' }));
  }
});

$('try-replay').addEventListener('change', async (e) => {
  const f = e.target.files[0];
  e.target.value = '';
  if (!f) return;
  const n = web.replay_load(new Uint8Array(await f.arrayBuffer()));
  const note = $('try-rec-note');
  note.textContent = `Replaying ${n} operations from the start…`;
  replaying = true;
  frozen = true;
  let i = 0;
  const tick = () => {
    const cmd = web.replay_step();
    if (!cmd) {
      replaying = false;
      last = performance.now();
      note.textContent = `Replayed ${i} operations; now on ${web.screen()}. Reload the page to get your own state back.`;
      return;
    }
    i++;
    note.textContent = `Replaying ${i}/${n}: ${cmd}`;
    setTimeout(tick, 150);
  };
  tick();
});

// ------------------------------------------------------------- self-test

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const key = (k, type) => window.dispatchEvent(new KeyboardEvent(type, { key: k, bubbles: true }));
async function tap(k) { key(k, 'keydown'); await sleep(80); key(k, 'keyup'); await sleep(120); }
async function hold(k, ms) { key(k, 'keydown'); await sleep(ms); key(k, 'keyup'); await sleep(150); }

async function golden(name) {
  const blob = await (await fetch(`/golden/${name}.png`)).blob();
  const bmp = await createImageBitmap(blob, { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
  const c = new OffscreenCanvas(128, 128).getContext('2d');
  c.drawImage(bmp, 0, 0);
  const want = c.getImageData(0, 0, 128, 128).data;
  const have = ctx.getImageData(0, 0, 128, 128).data;
  let diff = 0;
  for (let i = 0; i < want.length; i += 4) {
    if (want[i] !== have[i] || want[i + 1] !== have[i + 1] || want[i + 2] !== have[i + 2]) diff++;
  }
  return diff;
}

async function selftest() {
  const r = { firstScreenMs, goldens: {}, screens: [], errors: [] };
  try {
    await startAudio();
    r.audioState = actx ? actx.state : 'none';
    await sleep(400);
    r.goldens.app_home_dark_latin = await golden('app_home_dark_latin');
    await tap('ArrowLeft');
    await tap('Enter');
    await sleep(600);
    r.screens.push(web.screen());
    r.goldens.app_settings_dark_latin = await golden('app_settings_dark_latin');
    await hold('Backspace', 900);
    r.screens.push(web.screen());
    // Files › Music › Bach › Goldberg Variations › 01 Aria.mp3, by keys.
    for (const k of ['ArrowRight', 'Enter', 'ArrowDown', 'ArrowDown']) { await tap(k); await sleep(250); }
    for (const k of ['Enter', 'Enter', 'Enter', 'Enter']) { await tap(k); await sleep(250); }
    r.screens.push(web.screen());
    frameTimes.length = 0;
    const u0 = underruns;
    const t0 = performance.now();
    await sleep(4000);
    r.playedSeconds = played / Math.max(1, web.audio_rate());
    r.wallSeconds = (performance.now() - t0) / 1000;
    r.underruns = underruns - u0;
    r.audio = { sent, played, epoch, contextTime: actx ? actx.currentTime : null, rate: web.audio_rate() };
    r.track = JSON.parse(web.info()).track;
    r.audioError = JSON.parse(web.info()).audio_error;
    const s = stats();
    r.frameMeanMs = s.mean;
    r.frameWorstMs = s.worst;
    r.frames = s.n;
  } catch (e) {
    r.errors.push(String(e && e.stack || e));
  }
  r.pass = r.errors.length === 0
    && Object.values(r.goldens).every((d) => d === 0)
    && r.firstScreenMs !== null && r.firstScreenMs < 2000
    && r.track === '/Music/Bach/Goldberg Variations/01 Aria.mp3'
    && !r.audioError
    && r.playedSeconds > 2.5
    && r.underruns === 0
    && r.frameMeanMs < 4 && r.frameWorstMs < 16;
  window.orpheusSelftest = r;
  document.title = `selftest ${r.pass ? 'PASS' : 'FAIL'}`;
  console.log('orpheus selftest', JSON.stringify(r));
}

// ------------------------------------------------------------- start

async function demoBundle() {
  // Relative to this script (the page is one folder up).
  const r = await fetch(new URL('./pkg/demo.bin', import.meta.url));
  if (!r.ok) throw new Error(`demo card: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

async function main() {
  try {
    // The demo card downloads while the module compiles.
    const demo = demoBundle();
    const mod = await import('./pkg/orpheus_web.js');
    wasm = await mod.default();
    web = new mod.Web();
    status('Loading the demo card…');
    const derr = web.add_demo(await demo);
    if (derr) throw new Error(`demo card: ${derr}`);
    status('Building the card…');
    if (!SELFTEST) await loadFiles();
    const state = SELFTEST ? '' : await kvGet('state');
    await boot(state);
    root.dataset.state = 'running';
    requestAnimationFrame(raf);
    if (SELFTEST) {
      await sleep(100);
      await selftest();
    }
  } catch (e) {
    console.error(e);
    status(`Orpheus did not start: ${e.message}. It needs a browser with WebAssembly and IndexedDB.`);
    if (SELFTEST) {
      window.orpheusSelftest = { pass: false, errors: [String(e && e.stack || e)] };
      document.title = 'selftest FAIL';
    }
  }
}

main();
