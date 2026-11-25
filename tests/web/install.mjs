#!/usr/bin/env node
// The browser installer's tests (WEB-3, 09 §5). Node 22 or later; no
// packages.
//
//   node tests/web/install.mjs                 mock-device suite (Node)
//   node tests/web/install.mjs --page          the page in demo mode, headless Chromium
//   node tests/web/install.mjs --device [--ofw FILE.ofw] [--backup]
//                                              the real player, Chromium (HIL; the lead runs it)
//
// Needs `cargo xtask site` and `cargo xtask wasm` (target/orpheus/site).
//
// * Default: the WebAssembly module's real flows (orpheus-host) through
//   the page's WebUSB transport (site/install/usb.js) against the
//   simulated player in WebUSB's shape (site/install/mockusb.js): update
//   through the developer channel, a damaged file refused, the backup read
//   twice (and an unreliable read caught), first install (refused without
//   the confirmation; interrupted and resumed), restore, recovery slot
//   switch; no forbidden write at any point.
// * --page: install.html?demo&hil in headless Chromium: the page's own
//   update and recovery buttons' code paths against the simulated player.
// * --device: install.html?hil with the real player in a visible Chromium
//   with its own persistent profile (target/orpheus/install-hil-profile).
//   WebUSB permission: granted once in that profile (the first run asks:
//   press "Choose the player" and pick the YM03; later runs find it). Only
//   safe steps: INFO; with --ofw an update of the running Orpheus and a
//   check that the new build starts; with --backup (player in recovery
//   mode) a read-only backup. Never a first install or restore.

import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { existsSync, readFileSync, statSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname, extname, normalize } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const args = process.argv.slice(2);
const opt = (name, def) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : def;
};
const has = (name) => args.includes(`--${name}`);
const site = resolve(opt('site', join(ROOT, 'target/orpheus/site')));
const pkg = join(site, 'install/pkg');
if (!existsSync(join(pkg, 'orpheus_install_web_bg.wasm'))) {
  console.error(`${pkg} is missing: run \`cargo xtask site\` and \`cargo xtask wasm\` first`);
  process.exit(2);
}

// ------------------------------------------------------------- Node suite

async function suite() {
  const w = await import(pathToFileURL(join(pkg, 'orpheus_install_web.js')));
  await w.default({ module_or_path: readFileSync(join(pkg, 'orpheus_install_web_bg.wasm')) });
  const { WebUsbTransport } = await import(pathToFileURL(join(ROOT, 'site/install/usb.js')));
  const { MockUsb } = await import(pathToFileURL(join(ROOT, 'site/install/mockusb.js')));
  const P = { ymon: w.mockPayload('ymon'), nordump: w.mockPayload('nordump'), norprog: w.mockPayload('norprog'), norprog0: w.mockPayload('norprog0') };
  const quiet = () => {};
  const open = async (mock) => WebUsbTransport.open((await new MockUsb(mock).getDevices())[0]);
  const TOP = 0x7e0000;
  let n = 0;
  const test = async (name, f) => {
    await f();
    n++;
    console.log(`  ok  ${name}`);
  };

  await test('update through the developer channel, then recovery switches back', async () => {
    const mock = w.MockDevice.withOrpheus(w.ofwImage(w.mockOfw(1, '0.1.0')));
    let t = await open(mock);
    assert.ok(JSON.parse(await w.devInfo(t)).build.startsWith('01'));
    const file = w.mockOfw(2, '0.2.0');
    assert.equal(JSON.parse(w.checkOfw(file)).version, '0.2.0');
    let last = 0;
    const slot = await w.update(t, file, (_what, done) => { last = done; });
    assert.equal(slot, 1);
    assert.ok(last > 0, 'progress reported');
    await w.reboot(t, 0);
    t = await open(mock);
    assert.ok(JSON.parse(await w.devInfo(t)).build.startsWith('02'));
    assert.equal(mock.booted(), 1);
    // The new firmware keeps failing to start: recovery.
    mock.toRecovery();
    t = await open(mock);
    const s = JSON.parse(await w.slots(t, P.ymon, P.nordump));
    assert.equal(s.a.state, 'superseded');
    assert.equal(s.b.state, 'valid');
    await w.switchSlot(t, P.ymon, P.nordump, P.norprog);
    assert.equal(mock.booted(), 0);
    assert.equal(mock.violations(), 0);
  });

  await test('a damaged update file never reaches the player', async () => {
    const mock = w.MockDevice.withOrpheus(w.ofwImage(w.mockOfw(1, '0.1.0')));
    const t = await open(mock);
    const bad = w.mockOfw(2, '0.2.0');
    bad[bad.length - 50] ^= 1;
    assert.throws(() => w.checkOfw(bad), /SHA-256/);
    await assert.rejects(w.update(t, bad, quiet), /SHA-256/);
    assert.equal(mock.booted(), 0);
  });

  await test('backup reads the flash twice and catches a bad read', async () => {
    const mock = w.MockDevice.stock(3);
    const t = await open(mock);
    const b = await w.backup(t, P.ymon, P.nordump, quiet);
    assert.equal(b.length, 8 << 20);
    assert.deepEqual(b, mock.chip());
    assert.equal(w.classifyChip(b), 'stock');
    mock.flakyRead();
    await assert.rejects(w.backup(t, P.ymon, P.nordump, quiet), /differ/);
  });

  const release = () => ({ ofw: w.mockOfw(4, '0.4.0'), s2: w.mockStage(2), s1: w.mockStage(1) });
  const install = (t, b, r, confirmed) =>
    w.firstInstall(t, P.ymon, P.nordump, P.norprog, P.norprog0, b, r.ofw, r.s2, r.s1, confirmed, quiet);

  await test('first install: refused unconfirmed, then installed; sector 0 only in its step', async () => {
    const mock = w.MockDevice.stock(5);
    const t = await open(mock);
    const b = await w.backup(t, P.ymon, P.nordump, quiet);
    const r = release();
    await assert.rejects(install(t, b, r, false), /confirmation/);
    assert.deepEqual(mock.chip(), b, 'nothing written');
    await install(t, b, r, true);
    const chip = mock.chip();
    assert.deepEqual(chip.subarray(TOP), b.subarray(TOP), 'vendor data untouched');
    assert.deepEqual(chip.subarray(0, 512), r.s1);
    assert.ok(mock.sector0Writes() > 0);
    assert.equal(mock.violations(), 0);
    mock.restart();
    assert.equal(mock.mode(), 'firmware');
    assert.equal(mock.booted(), 0);
    // A second first install on what is now Orpheus is refused.
    mock.toRecovery();
    const t2 = await open(mock);
    const b2 = await w.backup(t2, P.ymon, P.nordump, quiet);
    assert.equal(w.classifyChip(b2), 'orpheus');
    await assert.rejects(install(t2, b2, r, true), /original firmware/);
  });

  await test('an interrupted first install resumes where it stopped', async () => {
    const ref = w.MockDevice.stock(6);
    const tr = await open(ref);
    const br = await w.backup(tr, P.ymon, P.nordump, quiet);
    await install(tr, br, release(), true);
    for (const cut of [10, 90, 300]) {
      const mock = w.MockDevice.stock(6);
      const t = await open(mock);
      const b = await w.backup(t, P.ymon, P.nordump, quiet);
      mock.failCall(cut);
      await assert.rejects(install(t, b, release(), true));
      mock.failCall(0);
      await install(t, b, release(), true);
      assert.deepEqual(mock.chip(), ref.chip(), `cut ${cut}`);
      assert.equal(mock.violations(), 0);
    }
  });

  await test('restore puts the original firmware back below 0x7E0000', async () => {
    const mock = w.MockDevice.stock(8);
    const t = await open(mock);
    const b = await w.backup(t, P.ymon, P.nordump, quiet);
    await install(t, b, release(), true);
    const n = await w.restore(t, P.ymon, P.nordump, P.norprog, P.norprog0, b, true, quiet);
    assert.ok(n > 2);
    assert.deepEqual(mock.chip().subarray(0, TOP), b.subarray(0, TOP));
    assert.equal(mock.violations(), 0);
    await assert.rejects(w.restore(t, P.ymon, P.nordump, P.norprog, P.norprog0, b, false, quiet), /confirmation/);
    mock.restart();
    assert.equal(mock.mode(), 'adfu', 'the original firmware is not Orpheus (no slot): the mock stays in recovery');
  });

  await test('the transport reports a stalled transfer', async () => {
    const mock = w.MockDevice.withOrpheus(w.ofwImage(w.mockOfw(1, '0.1.0')));
    const t = await open(mock);
    await assert.rejects(t.controlIn(0x55, 0, 0, 4, 1000), /stall/);
    await assert.rejects(t.bulkOut(new Uint8Array(31), 1000), /stall/);
  });

  console.log(`installer mock-device suite: ${n} tests passed`);
}

// ------------------------------------------------------------- Chromium

const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png' };

function serve(extra = {}) {
  const server = createServer((req, res) => {
    const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    if (extra[path]) {
      res.writeHead(200, { 'content-type': 'application/octet-stream' }).end(extra[path]);
      return;
    }
    let file = normalize(join(site, path));
    if (!file.startsWith(site)) return res.writeHead(403).end();
    if (existsSync(file) && statSync(file).isDirectory()) file = join(file, 'index.html');
    if (!existsSync(file)) return res.writeHead(404).end('not found');
    res.writeHead(200, { 'content-type': TYPES[extname(file)] || 'application/octet-stream', 'cache-control': 'no-store' });
    res.end(readFileSync(file));
  });
  return new Promise((ok) => server.listen(0, '127.0.0.1', () => ok(server)));
}

function which(names) {
  for (const n of names) for (const d of (process.env.PATH || '').split(':')) if (existsSync(join(d, n))) return join(d, n);
  return null;
}

async function browser(url, headed, profile) {
  const bin = opt('bin', null) || process.env.CHROME || which(['chromium-browser', 'chromium', 'google-chrome', 'google-chrome-stable']);
  if (!bin) throw new Error('no Chromium found (--bin PATH or $CHROME)');
  const proc = spawn(bin, [...(headed ? [] : ['--headless=new']), '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', ...(process.env.CI ? ['--no-sandbox'] : []), 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
  const wsUrl = await new Promise((ok, err) => {
    let buf = '';
    proc.stderr.on('data', (d) => {
      buf += d;
      const m = buf.match(/DevTools listening on (ws:\/\/\S+)/);
      if (m) ok(m[1]);
    });
    proc.on('exit', (c) => err(new Error(`${bin} exited (${c}): ${buf.slice(-400)}`)));
  });
  const port = new URL(wsUrl).port;
  const page = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, err) => { ws.onopen = ok; ws.onerror = err; });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.method === 'Runtime.exceptionThrown') console.log('  [exception]', m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      m.error ? p.bad(new Error(m.error.message)) : p.ok(m.result);
    }
  };
  const call = (method, params = {}) => new Promise((ok, bad) => { pending.set(++id, { ok, bad }); ws.send(JSON.stringify({ id, method, params })); });
  await call('Runtime.enable');
  await call('Page.navigate', { url });
  const evaluate = async (expr) => {
    const r = await call('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  const t0 = Date.now();
  while (!(await evaluate('window.orpheusInstallReady === true').catch(() => false))) {
    if (Date.now() - t0 > 30000) throw new Error(`the page did not start: ${await evaluate("document.getElementById('inst-status')?.textContent")}`);
    await new Promise((r) => setTimeout(r, 200));
  }
  const shot = async (path) => {
    await call('Emulation.setDeviceMetricsOverride', { width: 1000, height: 1400, deviceScaleFactor: 1, mobile: false });
    const r = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
    writeFileSync(path, Buffer.from(r.data, 'base64'));
  };
  return { evaluate, shot, close: () => { ws.close(); proc.kill(); } };
}

async function page() {
  const server = await serve();
  const b = await browser(`http://127.0.0.1:${server.address().port}/install.html?demo&hil`, false, mkdtempSync(join(tmpdir(), 'orpheus-install-')));
  try {
    assert.equal(await b.evaluate('orpheusInstall.connect()'), 'orpheus');
    const before = (await b.evaluate('orpheusInstall.info()')).build;
    await b.evaluate("document.getElementById('inst-update').click(); new Promise(r => setTimeout(r, 3000))");
    const after = (await b.evaluate('orpheusInstall.info()')).build;
    assert.notEqual(before, after, 'the demo update changed the running build');
    const status = await b.evaluate("document.getElementById('inst-status').textContent");
    assert.match(status, /Done/);
    const png = join(ROOT, 'target/orpheus/install-demo.png');
    await b.shot(png);
    console.log(`installer page (demo, Chromium): update ok: ${status}; screenshot ${png}`);
  } finally {
    b.close();
    server.close();
  }
}

async function device() {
  const extra = {};
  const ofw = opt('ofw', null);
  if (ofw) extra['/hil/update.ofw'] = readFileSync(ofw);
  const server = await serve(extra);
  const profile = join(ROOT, 'target/orpheus/install-hil-profile');
  mkdirSync(profile, { recursive: true });
  const b = await browser(`http://127.0.0.1:${server.address().port}/install.html?hil`, true, profile);
  try {
    let mode = await b.evaluate('orpheusInstall.connect()');
    if (mode === 'none') {
      console.log('No granted player: in the Chromium window press "Choose the player" and pick the YM03 (once per profile).');
      const t0 = Date.now();
      while (mode === 'none' && Date.now() - t0 < 120000) {
        await new Promise((r) => setTimeout(r, 1000));
        mode = await b.evaluate("document.getElementById('install').dataset.mode");
      }
    }
    console.log(`player: ${mode}`);
    if (mode === 'orpheus') {
      console.log('INFO', JSON.stringify(await b.evaluate('orpheusInstall.info()')));
      if (ofw) {
        console.log(`updating with ${ofw} …`);
        console.log(`  ${await b.evaluate("orpheusInstall.update('hil/update.ofw')")} is running`);
        console.log('INFO', JSON.stringify(await b.evaluate('orpheusInstall.info()')));
      }
    } else if (mode === 'adfu' && has('backup')) {
      console.log(`backup SHA-256 ${await b.evaluate('orpheusInstall.backup()')} (also in the Downloads folder of the profile)`);
    } else if (mode !== 'adfu' && mode !== 'orpheus') {
      throw new Error('no player');
    }
  } finally {
    b.close();
    server.close();
  }
}

try {
  if (has('device')) await device();
  else if (has('page')) await page();
  else await suite();
  process.exit(0);
} catch (e) {
  console.error(`FAILED: ${e.stack || e}`);
  process.exit(1);
}
