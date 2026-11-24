#!/usr/bin/env node
// The Try-it page's self-test in a headless browser (WEB-2, 09 §4).
//
//   node tests/web/try.mjs [--browser chromium|firefox] [--bin PATH] [--site DIR]
//
// Serves the built site (`cargo xtask site` + `cargo xtask wasm`, default
// target/orpheus/site) and sim/golden/ on 127.0.0.1, opens
// try.html?selftest in headless Chromium (DevTools protocol) or Firefox
// (WebDriver BiDi), waits for the page's verdict (site/try/app.js,
// `selftest`): two screens equal to their golden images after real key
// events, a demo track found by keys and played through the AudioWorklet
// with no dropouts, the first screen within 2 s and the frame-time budget.
// In Chromium also the normal page: a setting, the volume and a visitor's
// file survive a reload (IndexedDB), the developer panel, Factory reset.
// Saves a screenshot to target/orpheus/try-selftest-<browser>.png.
// Node 22 or later (or 20 with --experimental-websocket); no packages.

import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, readFileSync, readdirSync, writeFileSync, existsSync, statSync, rmSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, extname, dirname, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const args = process.argv.slice(2);
const opt = (name, def) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : def;
};
const browser = opt('browser', 'chromium');
const site = resolve(opt('site', join(ROOT, 'target/orpheus/site')));
const golden = join(ROOT, 'sim/golden');
const TIMEOUT_MS = 90_000;

if (typeof WebSocket === 'undefined') {
  console.error('Node has no WebSocket: use Node 22 or later, or run with --experimental-websocket');
  process.exit(2);
}
if (!existsSync(join(site, 'try/pkg/orpheus_web_bg.wasm'))) {
  console.error(`${site}/try/pkg is missing: run \`cargo xtask site\` and \`cargo xtask wasm\` first`);
  process.exit(2);
}

// ------------------------------------------------------------- server

const TYPES = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.mjs': 'text/javascript',
  '.wasm': 'application/wasm', '.css': 'text/css', '.png': 'image/png', '.svg': 'image/svg+xml',
  '.json': 'application/json', '.ico': 'image/x-icon', '.webmanifest': 'application/manifest+json',
};

const server = createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  let path = decodeURIComponent(url.pathname);
  let base = site;
  if (path.startsWith('/golden/')) {
    base = golden;
    path = path.slice('/golden'.length);
  }
  let file = normalize(join(base, path));
  if (!file.startsWith(base)) {
    res.writeHead(403).end();
    return;
  }
  if (existsSync(file) && statSync(file).isDirectory()) file = join(file, 'index.html');
  if (!existsSync(file)) {
    res.writeHead(404).end('not found');
    return;
  }
  res.writeHead(200, { 'content-type': TYPES[extname(file)] || 'application/octet-stream', 'cache-control': 'no-store' });
  res.end(readFileSync(file));
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pageUrl = `http://127.0.0.1:${server.address().port}/try.html?selftest`;

// ------------------------------------------------------------- browsers

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const profile = mkdtempSync(join(tmpdir(), 'orpheus-try-'));

function which(names) {
  for (const n of names) {
    for (const d of (process.env.PATH || '').split(':')) {
      const p = join(d, n);
      if (existsSync(p)) return p;
    }
  }
  return null;
}

async function freePort() {
  const s = createServer();
  await new Promise((r) => s.listen(0, '127.0.0.1', r));
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
}

function rpc(ws) {
  let id = 0;
  const pending = new Map();
  const handlers = [];
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id !== undefined && pending.has(m.id)) {
      const { resolve: ok, reject } = pending.get(m.id);
      pending.delete(m.id);
      if (m.error) reject(new Error(typeof m.error === 'string' ? `${m.error}: ${m.message}` : JSON.stringify(m.error)));
      else ok(m.result);
    } else {
      for (const h of handlers) h(m);
    }
  });
  return {
    call(method, params = {}) {
      const n = ++id;
      ws.send(JSON.stringify({ id: n, method, params }));
      return new Promise((ok, reject) => pending.set(n, { resolve: ok, reject }));
    },
    on(h) { handlers.push(h); },
  };
}

async function open(url) {
  const ws = new WebSocket(url);
  await new Promise((ok, err) => { ws.onopen = ok; ws.onerror = () => err(new Error(`cannot connect to ${url}`)); });
  return ws;
}

async function chromium() {
  const bin = opt('bin', null) || process.env.CHROME || which(['chromium-browser', 'chromium', 'google-chrome', 'google-chrome-stable']);
  if (!bin) throw new Error('no Chromium found (--bin PATH or $CHROME)');
  const proc = spawn(bin, [
    '--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`,
    '--autoplay-policy=no-user-gesture-required', '--no-first-run', '--no-default-browser-check',
    '--window-size=1280,1000',
    // CI runners: no user namespaces for the sandbox, no sound device
    // (the fake sink still runs the audio graph in real time).
    ...(process.env.CI ? ['--no-sandbox', '--disable-audio-output'] : []),
    'about:blank',
  ], { stdio: ['ignore', 'ignore', 'pipe'] });
  const wsUrl = await new Promise((ok, err) => {
    let buf = '';
    proc.stderr.on('data', (d) => {
      buf += d;
      const m = buf.match(/DevTools listening on (ws:\/\/\S+)/);
      if (m) ok(m[1]);
    });
    proc.on('exit', (c) => err(new Error(`${bin} exited (${c}): ${buf.slice(-500)}`)));
  });
  const port = new URL(wsUrl).port;
  const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const page = list.find((t) => t.type === 'page');
  const ws = await open(page.webSocketDebuggerUrl);
  const c = rpc(ws);
  c.on((m) => {
    if (m.method === 'Runtime.consoleAPICalled') console.log(`  [console.${m.params.type}]`, m.params.args.map((a) => a.value ?? a.description).join(' '));
    if (m.method === 'Runtime.exceptionThrown') console.log('  [exception]', m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
  });
  await c.call('Runtime.enable');
  await c.call('Page.enable');
  const version = (await c.call('Browser.getVersion')).product;
  await c.call('Page.navigate', { url: pageUrl });
  return {
    version,
    cdp: c,
    eval: async (expr) => (await c.call('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result.value,
    shot: async () => (await c.call('Page.captureScreenshot', { format: 'png' })).data,
    close: () => { ws.close(); proc.kill(); },
  };
}

/// A one-second 440 Hz WAV (16-bit mono, 8 kHz).
function toneWav(path) {
  const n = 8000;
  const b = Buffer.alloc(44 + n * 2);
  b.write('RIFF', 0);
  b.writeUInt32LE(36 + n * 2, 4);
  b.write('WAVEfmt ', 8);
  b.writeUInt32LE(16, 16);
  b.writeUInt16LE(1, 20);
  b.writeUInt16LE(1, 22);
  b.writeUInt32LE(8000, 24);
  b.writeUInt32LE(16000, 28);
  b.writeUInt16LE(2, 32);
  b.writeUInt16LE(16, 34);
  b.write('data', 36);
  b.writeUInt32LE(n * 2, 40);
  for (let i = 0; i < n; i++) b.writeInt16LE(Math.round(8000 * Math.sin((2 * Math.PI * 440 * i) / 8000)), 44 + 2 * i);
  writeFileSync(path, b);
}

/// The normal page (Chromium): a setting, the volume and a file of the
/// visitor's survive a reload; the developer panel shows; Factory reset
/// clears them.
async function persistence(b) {
  const c = b.cdp;
  const url = pageUrl.replace('?selftest', '');
  const running = async () => {
    for (let i = 0; i < 60; i++) {
      await sleep(250);
      if (await b.eval("document.getElementById('try')?.dataset.state === 'running' && !!orpheus.web").catch(() => false)) return;
    }
    throw new Error('the page did not start');
  };
  const checks = {};
  await c.call('Page.navigate', { url });
  await running();
  await b.eval("orpheus.web.run_script('theme:light')");
  for (let i = 0; i < 3; i++) {
    await b.eval("dispatchEvent(new KeyboardEvent('keydown', { key: '+' })), new Promise((r) => setTimeout(r, 80))");
    await b.eval("dispatchEvent(new KeyboardEvent('keyup', { key: '+' })), new Promise((r) => setTimeout(r, 80))");
  }
  const wav = join(profile, 'my tone.wav');
  toneWav(wav);
  const doc = await c.call('DOM.getDocument', {});
  const input = await c.call('DOM.querySelector', { nodeId: doc.root.nodeId, selector: '#try-files' });
  await c.call('DOM.setFileInputFiles', { nodeId: input.nodeId, files: [wav] });
  for (let i = 0; i < 40 && !(await b.eval("document.querySelectorAll('#try-yours li').length === 1 && document.getElementById('try').dataset.state === 'running'")); i++) await sleep(250);
  const before = await b.eval('orpheus.web.state()');
  await sleep(3500); // saved every 3 s
  await c.call('Page.reload', {});
  await running();
  const after = await b.eval('orpheus.web.state()');
  checks.stateKept = before === after && /\ns [0-9a-f]{4} 1\n/.test(after) && /\nvol 15\n/.test(after);
  checks.fileKept = (await b.eval('orpheus.web.user_paths()')) === '/Your files/my tone.wav';
  checks.fileListed = await b.eval("document.querySelector('#try-yours li')?.textContent.startsWith('my tone.wav')");
  checks.libraryKept = await b.eval("new Promise((r) => { const q = indexedDB.open('orpheus-try'); q.onsuccess = () => { const g = q.result.transaction('kv').objectStore('kv').get('library'); g.onsuccess = () => r(!!g.result && g.result.fp === orpheus.web.fingerprint() && g.result.files.length > 0); }; })");
  await b.eval("document.getElementById('try-dev-toggle').click()");
  await sleep(1200);
  checks.devPanel = await b.eval("document.querySelectorAll('#try-stats dt').length >= 8");
  await b.eval("document.getElementById('try-dev').scrollIntoView({ block: 'end' })");
  await sleep(300);
  const shot = join(ROOT, 'target/orpheus/try-dev-chromium.png');
  writeFileSync(shot, Buffer.from(await b.shot(), 'base64'));
  // Record two taps, then replay a committed seed of the fuzz corpus.
  await b.eval("document.getElementById('try-rec').click()");
  for (const k of ['ArrowRight', 'Enter']) {
    await b.eval(`dispatchEvent(new KeyboardEvent('keydown', { key: '${k}' })), new Promise((r) => setTimeout(r, 80))`);
    await b.eval(`dispatchEvent(new KeyboardEvent('keyup', { key: '${k}' })), new Promise((r) => setTimeout(r, 300))`);
  }
  await b.eval("document.getElementById('try-rec').click()");
  checks.recorded = await b.eval("document.getElementById('try-rec-note').textContent") === 'Recorded 3 operations.';
  const corpus = join(ROOT, 'tests/fuzz-corpus/ui_keys');
  const first = existsSync(corpus) ? readdirSync(corpus).sort()[0] : null;
  if (first) {
    const seed = join(corpus, first);
    const now = await c.call('DOM.getDocument', {});
    const replay = await c.call('DOM.querySelector', { nodeId: now.root.nodeId, selector: '#try-replay' });
    await c.call('DOM.setFileInputFiles', { nodeId: replay.nodeId, files: [seed] });
    let note = '';
    for (let i = 0; i < 80 && !note.startsWith('Replayed'); i++) {
      await sleep(250);
      note = await b.eval("document.getElementById('try-rec-note').textContent");
    }
    checks.replayed = note.startsWith('Replayed') && !note.includes('refused');
    console.log(`replay: ${note}`);
  }
  // The confirmation is answered here (headless dialogs are not shown).
  await b.eval("window.confirm = () => true, document.getElementById('try-reset').click()");
  for (let i = 0; i < 40 && (await b.eval('orpheus.web.user_paths()')) !== ''; i++) await sleep(250);
  await sleep(500);
  checks.resetFiles = (await b.eval('orpheus.web.user_paths()')) === '';
  checks.resetState = !/\ns /.test(await b.eval('orpheus.web.state()'));
  console.log('persistence:', JSON.stringify({ before, after, checks }, null, 2));
  console.log(`screenshot: ${shot}`);
  return Object.values(checks).every(Boolean);
}

async function firefox() {
  const bin = opt('bin', null) || process.env.FIREFOX || which(['firefox', 'firefox-esr']);
  if (!bin) throw new Error('no Firefox found (--bin PATH or $FIREFOX)');
  writeFileSync(join(profile, 'user.js'), [
    'user_pref("media.autoplay.default", 0);',
    'user_pref("media.autoplay.blocking_policy", 0);',
    'user_pref("browser.shell.checkDefaultBrowser", false);',
    'user_pref("datareporting.policy.dataSubmissionEnabled", false);',
    'user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);',
  ].join('\n'));
  const port = await freePort();
  const proc = spawn(bin, ['--headless', '--no-remote', '--profile', profile, '--remote-debugging-port', String(port), '--width=1280', '--height=1000'], { stdio: 'ignore' });
  let ws = null;
  for (let i = 0; i < 100 && !ws; i++) {
    await sleep(200);
    ws = await open(`ws://127.0.0.1:${port}/session`).catch(() => null);
  }
  if (!ws) throw new Error('Firefox did not open its WebDriver BiDi port');
  const c = rpc(ws);
  c.on((m) => {
    if (m.method === 'log.entryAdded') console.log(`  [${m.params.type}.${m.params.level}]`, m.params.text);
  });
  const s = await c.call('session.new', { capabilities: {} });
  await c.call('session.subscribe', { events: ['log.entryAdded'] });
  const tree = await c.call('browsingContext.getTree', {});
  const context = tree.contexts[0].context;
  await c.call('browsingContext.navigate', { context, url: pageUrl, wait: 'complete' });
  return {
    version: `Firefox ${s.capabilities.browserVersion}`,
    eval: async (expr) => {
      const r = await c.call('script.evaluate', { expression: expr, target: { context }, awaitPromise: false });
      return r.result?.value;
    },
    shot: async () => (await c.call('browsingContext.captureScreenshot', { context })).data,
    close: () => { ws.close(); proc.kill(); },
  };
}

// ------------------------------------------------------------- run

let b = null;
let code = 1;
try {
  b = browser === 'firefox' ? await firefox() : await chromium();
  console.log(`${b.version}: ${pageUrl}`);
  const t0 = Date.now();
  let r = null;
  while (Date.now() - t0 < TIMEOUT_MS) {
    await sleep(500);
    const v = await b.eval('JSON.stringify(window.orpheusSelftest || null)').catch(() => null);
    if (v && v !== 'null') {
      r = JSON.parse(v);
      break;
    }
  }
  const out = join(ROOT, 'target/orpheus');
  mkdirSync(out, { recursive: true });
  const shot = join(out, `try-selftest-${browser}.png`);
  writeFileSync(shot, Buffer.from(await b.shot(), 'base64'));
  if (!r) throw new Error(`no verdict within ${TIMEOUT_MS / 1000} s`);
  console.log(JSON.stringify(r, null, 2));
  console.log(`screenshot: ${shot}`);
  let pass = r.pass;
  console.log(r.pass ? 'self-test passed' : 'self-test FAILED');
  if (b.cdp) {
    const p = await persistence(b);
    console.log(p ? 'persistence passed' : 'persistence FAILED');
    pass = pass && p;
  }
  code = pass ? 0 : 1;
} catch (e) {
  console.error(`self-test could not run: ${e.message}`);
} finally {
  if (b) b.close();
  server.close();
  // The browser may still be writing its profile after exit: retry, and
  // never fail the run on cleanup.
  try {
    rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  } catch (e) {
    console.warn(`could not remove ${profile}: ${e.message}`);
  }
}
process.exit(code);
