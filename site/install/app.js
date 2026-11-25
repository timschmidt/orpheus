// The Install page (WEB-3, 09 §5): update, first install, restore and
// recovery from a Chromium browser over WebUSB. The protocols and flows
// are the WebAssembly module's (`orpheus-install-web` = orpheus-host);
// this file is the page: device choice, release files and their SHA-256
// against the release manifest, the backup (download + IndexedDB), the
// confirmations, progress, resuming an interrupted job.
//
// install.html?demo runs everything against a simulated player
// (mockusb.js). install.html?hil exposes `window.orpheusInstall` for
// tests/web/install.mjs --device.

import init, * as w from './pkg/orpheus_install_web.js';
import { FILTERS, WebUsbTransport, kindOf, waitFor } from './usb.js';
import { MockUsb } from './mockusb.js';

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);
const DEMO = params.has('demo');
const HIL = params.has('hil');

let usb = null;
let dev = null;
let mock = null;
let backupCache = null;

// ------------------------------------------------------------- storage

const store = await (async () => {
  const mem = new Map();
  const fallback = {
    get: async (k) => mem.get(k),
    put: async (k, v) => mem.set(k, v),
    del: async (k) => mem.delete(k),
  };
  if (DEMO || typeof indexedDB === 'undefined') return fallback;
  try {
    const db = await new Promise((ok, bad) => {
      const r = indexedDB.open('orpheus-install', 1);
      r.onupgradeneeded = () => r.result.createObjectStore('kv');
      r.onsuccess = () => ok(r.result);
      r.onerror = () => bad(r.error);
    });
    const tx = (mode, f) =>
      new Promise((ok, bad) => {
        const t = db.transaction('kv', mode);
        const r = f(t.objectStore('kv'));
        t.oncomplete = () => ok(r?.result);
        t.onerror = () => bad(t.error);
      });
    return {
      get: (k) => tx('readonly', (s) => s.get(k)),
      put: (k, v) => tx('readwrite', (s) => s.put(v, k)),
      del: (k) => tx('readwrite', (s) => s.delete(k)),
    };
  } catch {
    return fallback;
  }
})();

// ------------------------------------------------------------- page

function say(text, kind = '') {
  const s = $('inst-status');
  s.textContent = text;
  s.dataset.kind = kind;
}

function progress(label) {
  const bar = $('inst-progress');
  bar.hidden = false;
  return (what, done, total) => {
    bar.max = Math.max(1, total);
    bar.value = done;
    $('inst-progress-text').textContent = `${label}: ${what} ${Math.floor((100 * done) / Math.max(1, total))} %`;
  };
}

function done() {
  $('inst-progress').hidden = true;
  $('inst-progress-text').textContent = '';
}

function show(id, on = true) {
  $(id).hidden = !on;
}

async function busy(label, f) {
  for (const b of document.querySelectorAll('#install button')) b.disabled = true;
  say(`${label}…`);
  try {
    const r = await f();
    return r;
  } catch (e) {
    say(`${label} failed: ${e.message || e}`, 'error');
    throw e;
  } finally {
    done();
    for (const b of document.querySelectorAll('#install button')) b.disabled = false;
    refreshButtons();
  }
}

// ------------------------------------------------------------- release files

let manifest = null;
const files = new Map();

async function loadManifest() {
  if (DEMO) return { version: 'demo', files: {} };
  try {
    const r = await fetch('release/manifest.json', { cache: 'no-store' });
    if (!r.ok) return null;
    return await r.json();
  } catch {
    return null;
  }
}

/// A release file, checked against the manifest's SHA-256 and size.
async function releaseFile(name) {
  if (files.has(name)) return files.get(name);
  let data;
  if (DEMO) {
    const demo = {
      'ymon.bin': () => w.mockPayload('ymon'),
      'nordump.bin': () => w.mockPayload('nordump'),
      'norprog.bin': () => w.mockPayload('norprog'),
      'norprog0.bin': () => w.mockPayload('norprog0'),
      'firmware.ofw': () => w.mockOfw(2, '0.2.0-demo'),
      'stage1.bin': () => w.mockStage(1),
      'stage2.bin': () => w.mockStage(2),
    };
    data = demo[name]();
  } else {
    const entry = manifest?.files?.[name];
    if (!entry) throw new Error(`the release has no ${name}`);
    const r = await fetch(`release/${entry.path || name}`, { cache: 'no-store' });
    if (!r.ok) throw new Error(`download of ${name} failed (${r.status})`);
    data = new Uint8Array(await r.arrayBuffer());
    if (data.length !== entry.size || w.sha256Hex(data) !== entry.sha256) {
      throw new Error(`${name} does not match the release manifest (SHA-256): not used`);
    }
  }
  files.set(name, data);
  return data;
}

async function payloads(withProg = true, with0 = false) {
  return {
    ymon: await releaseFile('ymon.bin'),
    nordump: await releaseFile('nordump.bin'),
    norprog: withProg ? await releaseFile('norprog.bin') : new Uint8Array(0),
    norprog0: with0 ? await releaseFile('norprog0.bin') : new Uint8Array(0),
  };
}

// ------------------------------------------------------------- the player

async function open() {
  if (!dev) throw new Error('no player chosen');
  return WebUsbTransport.open(dev);
}

async function connect() {
  dev = DEMO ? (await usb.getDevices())[0] : await usb.requestDevice({ filters: FILTERS });
  await describe();
}

async function reconnect(kind) {
  say(kind === 'adfu' ? 'Waiting for recovery mode…' : 'Waiting for Orpheus to start…');
  dev = await waitFor(usb, kind);
  await describe();
}

async function describe() {
  const kind = dev ? kindOf(dev) : 'none';
  $('install').dataset.mode = kind;
  if (kind === 'orpheus') {
    const t = await open();
    const info = JSON.parse(await w.devInfo(t));
    $('inst-device').textContent = `Orpheus is running (build ${info.build.slice(0, 12)}, battery ${info.battery_mv} mV).`;
  } else if (kind === 'adfu') {
    $('inst-device').textContent = 'The player is in recovery mode (USB recovery of its boot ROM).';
  } else {
    $('inst-device').textContent = 'No player chosen.';
  }
  refreshButtons();
}

function refreshButtons() {
  const mode = $('install').dataset.mode;
  show('inst-orpheus', mode === 'orpheus');
  show('inst-adfu', mode === 'adfu');
  const b = backupCache;
  $('inst-backup-info').textContent = b
    ? `Backup of ${new Date(b.date).toLocaleString()}: ${b.kind === 'stock' ? 'original firmware' : b.kind}, SHA-256 ${b.sha.slice(0, 16)}…`
    : 'No backup yet.';
  $('inst-first').disabled = !(b && b.kind === 'stock' && $('inst-saved').checked);
  $('inst-restore').disabled = !(b && b.kind === 'stock' && $('inst-saved').checked);
}

function download(bytes, name) {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([bytes], { type: 'application/octet-stream' }));
  a.download = name;
  a.textContent = `Download ${name} again`;
  const p = $('inst-download');
  p.replaceChildren(a);
  a.click();
}

// ------------------------------------------------------------- flows

/// Update a running Orpheus through the developer channel (UPD-1).
async function doUpdate(file) {
  const meta = JSON.parse(w.checkOfw(file));
  return busy(`Updating to ${meta.version}`, async () => {
    let t = await open();
    const slot = await w.update(t, file, progress('Update'));
    say(`Written to slot ${slot === 0 ? 'A' : 'B'} and verified. Restarting…`);
    await w.reboot(t, 0);
    await t.close();
    await reconnect('orpheus');
    t = await open();
    const info = JSON.parse(await w.devInfo(t));
    if (info.build !== meta.build) {
      throw new Error('the player restarted with the previous firmware: the new one did not start (see Recovery)');
    }
    say(`Done: Orpheus ${meta.version} is running.`, 'ok');
    return meta.version;
  });
}

async function doBackup() {
  return busy('Reading the whole flash twice', async () => {
    const p = await payloads(false);
    const t = await open();
    const bytes = await w.backup(t, p.ymon, p.nordump, progress('Backup'));
    const rec = { bytes, sha: w.sha256Hex(bytes), kind: w.classifyChip(bytes), date: Date.now() };
    // The first backup of the original firmware is never replaced by a
    // later one of a half-installed chip.
    const old = await store.get('backup');
    if (!(old && old.kind === 'stock' && rec.kind !== 'stock')) {
      await store.put('backup', rec);
      backupCache = rec;
    }
    download(bytes, `ym03-backup-${new Date(rec.date).toISOString().slice(0, 10)}-${rec.sha.slice(0, 8)}.bin`);
    say(`Backup read twice and identical (${rec.kind}). It is saved in this browser and downloaded: keep the file.`, 'ok');
    return rec;
  });
}

async function confirmSector0(what) {
  if (HIL || DEMO) return $('inst-confirm0').checked;
  return (
    $('inst-confirm0').checked &&
    window.confirm(`${what}\n\nThis rewrites the player's boot record (flash sector 0). The backup is saved. Continue?`)
  );
}

async function doFirstInstall() {
  const b = backupCache;
  if (!b || b.kind !== 'stock') throw new Error('make a backup of the original firmware first');
  if (!(await confirmSector0('First install'))) return say('Not started: the boot-record step was not confirmed.');
  return busy('Installing Orpheus', async () => {
    const p = await payloads(true, true);
    const ofw = await releaseFile('firmware.ofw');
    const s2 = await releaseFile('stage2.bin');
    const s1 = await releaseFile('stage1.bin');
    await store.put('job', { flow: 'first', date: Date.now() });
    const t = await open();
    await w.firstInstall(t, p.ymon, p.nordump, p.norprog, p.norprog0, b.bytes, ofw, s2, s1, true, progress('Install'));
    await store.del('job');
    await w.resetPlayer(t);
    await t.close();
    say('Installed. The player restarts into Orpheus.', 'ok');
    if (DEMO) mock.restart();
  });
}

async function doRestore() {
  const b = backupCache;
  if (!b || b.kind !== 'stock') throw new Error('choose the backup of the original firmware first');
  if (!(await confirmSector0('Restore the original firmware'))) return say('Not started: not confirmed.');
  return busy('Restoring the original firmware', async () => {
    const p = await payloads(true, true);
    await store.put('job', { flow: 'restore', date: Date.now() });
    const t = await open();
    const n = await w.restore(t, p.ymon, p.nordump, p.norprog, p.norprog0, b.bytes, true, progress('Restore'));
    await store.del('job');
    await w.resetPlayer(t);
    await t.close();
    say(`Restored (${n} sectors written). The player starts the original firmware.`, 'ok');
    if (DEMO) mock.restart();
  });
}

async function doSlots() {
  return busy('Reading the firmware slots', async () => {
    const p = await payloads(false);
    const t = await open();
    const s = JSON.parse(await w.slots(t, p.ymon, p.nordump));
    const d = (x) => (x.state === 'empty' ? 'empty' : `${x.state}, build ${x.build.slice(0, 12)}`);
    $('inst-slots').textContent = `Slot A: ${d(s.a)}. Slot B: ${d(s.b)}.`;
    $('inst-switch').disabled = !s.can_switch;
    say('Slots read.', 'ok');
    return s;
  });
}

async function doSwitch() {
  return busy('Switching to the other firmware', async () => {
    const p = await payloads(true);
    const t = await open();
    await w.switchSlot(t, p.ymon, p.nordump, p.norprog);
    await w.resetPlayer(t);
    await t.close();
    say('Switched. The player restarts into the other firmware.', 'ok');
    if (DEMO) mock.restart();
  });
}

async function doReinstall() {
  return busy('Reinstalling Orpheus', async () => {
    const p = await payloads(true);
    const ofw = await releaseFile('firmware.ofw');
    const t = await open();
    await w.reinstall(t, p.ymon, p.nordump, p.norprog, ofw, progress('Reinstall'));
    await w.resetPlayer(t);
    await t.close();
    say('Reinstalled. The player restarts into Orpheus.', 'ok');
    if (DEMO) mock.restart();
  });
}

async function toRecovery() {
  return busy('Restarting into recovery mode', async () => {
    const t = await open();
    await w.reboot(t, 1);
    await t.close();
    if (DEMO) mock.toRecovery();
    if (DEMO) return reconnect('adfu');
    // Recovery mode is another USB device: the browser asks again.
    say('The player restarted into recovery mode. Press "Choose the player" and pick the device without a name.');
    dev = null;
    $('install').dataset.mode = 'none';
  });
}

// ------------------------------------------------------------- wiring

async function main() {
  if (!DEMO && !('usb' in navigator)) {
    show('inst-unsupported');
    return;
  }
  await init();
  manifest = await loadManifest();
  if (DEMO) {
    mock = params.get('demo') === 'stock' ? w.MockDevice.stock(7) : w.MockDevice.withOrpheus(w.ofwImage(w.mockOfw(1, '0.1.0-demo')));
    usb = new MockUsb(mock);
    $('inst-demo').hidden = false;
  } else {
    usb = navigator.usb;
    usb.addEventListener('disconnect', (e) => {
      if (e.device === dev) {
        dev = null;
        $('install').dataset.mode = 'none';
        refreshButtons();
      }
    });
  }
  $('inst-release').textContent = manifest
    ? `Release ${manifest.version}.`
    : 'No release is published here yet: the Update can use an .ofw file you choose; first install and recovery need a release.';
  show('install');
  backupCache = (await store.get('backup')) || null;
  const job = await store.get('job');
  if (job) {
    show('inst-resume');
    $('inst-resume-text').textContent = `A ${job.flow === 'first' ? 'first install' : 'restore'} was interrupted on ${new Date(job.date).toLocaleString()}. Put the player in recovery mode, choose it and continue: it carries on where it stopped.`;
  }
  const on = (id, f) => $(id).addEventListener('click', () => f().catch(() => {}));
  on('inst-connect', connect);
  on('inst-update', async () => doUpdate($('inst-file').files[0] ? new Uint8Array(await $('inst-file').files[0].arrayBuffer()) : await releaseFile('firmware.ofw')));
  on('inst-recovery', toRecovery);
  on('inst-backup', doBackup);
  on('inst-first', doFirstInstall);
  on('inst-restore', doRestore);
  on('inst-slots-read', doSlots);
  on('inst-switch', doSwitch);
  on('inst-reinstall', doReinstall);
  on('inst-reset', async () => {
    const t = await open();
    await w.resetPlayer(t);
    if (DEMO) mock.restart();
  });
  on('inst-resume-go', async () => ((await store.get('job'))?.flow === 'first' ? doFirstInstall() : doRestore()));
  $('inst-saved').addEventListener('change', refreshButtons);
  $('inst-backup-file').addEventListener('change', async () => {
    const f = $('inst-backup-file').files[0];
    if (!f) return;
    const bytes = new Uint8Array(await f.arrayBuffer());
    backupCache = { bytes, sha: w.sha256Hex(bytes), kind: w.classifyChip(bytes), date: f.lastModified };
    refreshButtons();
  });
  if (DEMO) await connect();
  refreshButtons();
  say(DEMO ? 'Demo: a simulated player is connected.' : 'Choose the player to begin.');
  if (HIL) {
    window.orpheusInstall = {
      connect: async () => {
        const d = await usb.getDevices();
        dev = d.find((x) => kindOf(x) !== 'other') || null;
        await describe();
        return $('install').dataset.mode;
      },
      info: async () => JSON.parse(await w.devInfo(await open())),
      update: async (url) => doUpdate(new Uint8Array(await (await fetch(url)).arrayBuffer())),
      backup: async () => (await doBackup()).sha,
      slots: doSlots,
    };
    window.orpheusInstallReady = true;
  }
}

main().catch((e) => say(`The installer could not start: ${e.message || e}`, 'error'));
