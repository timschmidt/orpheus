// The browser installer's USB transport (WEB-3, 09 §5): the object the
// WebAssembly module (`orpheus-install-web`) calls for every transfer. It
// wraps a WebUSB `USBDevice` — the player, or the simulated one in
// mockusb.js — and implements `orpheus_host::transport::Transport`:
// bulk transfers on interface 0 for the boot ROM's ADFU mode and ymon,
// vendor control transfers for Orpheus's developer channel.

export const ADFU = { vendorId: 0x10d6, productId: 0x10d6 };
export const ORPHEUS = { vendorId: 0x1209, productId: 0x0001 };
export const FILTERS = [ADFU, ORPHEUS];

export function kindOf(dev) {
  if (dev.vendorId === ADFU.vendorId && dev.productId === ADFU.productId) return 'adfu';
  if (dev.vendorId === ORPHEUS.vendorId && dev.productId === ORPHEUS.productId) return 'orpheus';
  return 'other';
}

function timeout(p, ms, what) {
  if (!ms) return p;
  let t;
  return Promise.race([
    p,
    new Promise((_, reject) => {
      t = setTimeout(() => reject(new Error(`${what}: no answer within ${ms} ms`)), ms);
    }),
  ]).finally(() => clearTimeout(t));
}

const view = (dv) => new Uint8Array(dv.buffer, dv.byteOffset, dv.byteLength).slice();

export class WebUsbTransport {
  constructor(dev) {
    this.dev = dev;
    this.epIn = 1;
    this.epOut = 2;
    this.mps = 512;
  }

  /// Open the device and claim interface 0 (ADFU: its bulk endpoints;
  /// Orpheus: the developer interface, no endpoints).
  static async open(dev) {
    const t = new WebUsbTransport(dev);
    if (!dev.opened) await dev.open();
    if (!dev.configuration) await dev.selectConfiguration(1);
    await dev.claimInterface(0);
    const alt = dev.configuration.interfaces[0]?.alternate;
    for (const ep of alt?.endpoints || []) {
      if (ep.type !== 'bulk') continue;
      if (ep.direction === 'in') {
        t.epIn = ep.endpointNumber;
        t.mps = ep.packetSize || 512;
      } else {
        t.epOut = ep.endpointNumber;
      }
    }
    return t;
  }

  async close() {
    try {
      await this.dev.close();
    } catch {
      // Already gone (the player restarted).
    }
  }

  get packetSize() {
    return this.mps;
  }

  async bulkOut(data, ms) {
    const r = await timeout(this.dev.transferOut(this.epOut, data), ms, 'bulk OUT');
    if (r.status !== 'ok' || r.bytesWritten !== data.length) throw new Error(`bulk OUT: ${r.status}`);
  }

  async bulkIn(len, ms) {
    const r = await timeout(this.dev.transferIn(this.epIn, len), ms, 'bulk IN');
    if (r.status === 'stall') {
      await this.clearHaltIn();
      throw new Error('bulk IN: stall');
    }
    if (r.status !== 'ok' && r.status !== 'babble') throw new Error(`bulk IN: ${r.status}`);
    return view(r.data);
  }

  async clearHaltIn() {
    try {
      await this.dev.clearHalt('in', this.epIn);
    } catch {
      // Nothing to clear.
    }
  }

  async controlIn(request, value, index, length, ms) {
    const r = await timeout(
      this.dev.controlTransferIn({ requestType: 'vendor', recipient: 'device', request, value, index }, length),
      ms,
      `control IN ${request}`,
    );
    if (r.status !== 'ok') throw new Error(`control IN ${request}: ${r.status}`);
    return view(r.data);
  }

  async controlOut(request, value, index, data, ms) {
    const r = await timeout(
      this.dev.controlTransferOut({ requestType: 'vendor', recipient: 'device', request, value, index }, data),
      ms,
      `control OUT ${request}`,
    );
    if (r.status !== 'ok') throw new Error(`control OUT ${request}: ${r.status}`);
  }

  sleep(ms) {
    return new Promise((r) => setTimeout(r, ms));
  }

  now() {
    return performance.now();
  }
}

/// Wait until a device of `kind` ('adfu' or 'orpheus') is back (after a
/// restart). `usb`: navigator.usb or the simulated one.
export async function waitFor(usb, kind, ms = 60000) {
  const t0 = performance.now();
  while (performance.now() - t0 < ms) {
    const devs = await usb.getDevices();
    const d = devs.find((x) => kindOf(x) === kind);
    if (d) return d;
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(kind === 'adfu' ? 'the player did not enter recovery mode' : 'Orpheus did not start');
}
