// A simulated player in WebUSB's shape (WEB-3): `MockUsb` stands in for
// `navigator.usb`, `MockUSBDevice` for a `USBDevice`. Behind them is the
// WebAssembly module's `MockDevice` (`orpheus_host::mock`): the boot ROM's
// ADFU mode, ymon, the NOR payloads and Orpheus's developer channel over a
// simulated flash chip. The Install page's demo mode (`install.html?demo`)
// and the Node test suite (tests/web/install.mjs) use it; the transport
// (usb.js) and the flows are the real ones.

const ADFU_CONFIG = {
  configurationValue: 1,
  interfaces: [
    {
      interfaceNumber: 0,
      alternate: {
        interfaceClass: 0xff,
        endpoints: [
          { endpointNumber: 1, direction: 'in', type: 'bulk', packetSize: 512 },
          { endpointNumber: 2, direction: 'out', type: 'bulk', packetSize: 512 },
        ],
      },
    },
  ],
};
const DEV_CONFIG = {
  configurationValue: 1,
  interfaces: [{ interfaceNumber: 0, alternate: { interfaceClass: 0xff, endpoints: [] } }],
};

const dv = (u8) => new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
const bytes = (d) => (d instanceof Uint8Array ? d : new Uint8Array(d.buffer ?? d, d.byteOffset ?? 0, d.byteLength));

export class MockUSBDevice {
  constructor(mock) {
    this.mock = mock;
    this.opened = false;
    this.configuration = null;
  }
  get firmware() {
    return this.mock.mode() === 'firmware';
  }
  get vendorId() {
    return this.firmware ? 0x1209 : 0x10d6;
  }
  get productId() {
    return this.firmware ? 0x0001 : 0x10d6;
  }
  get productName() {
    return this.firmware ? 'YM03 (simulated)' : '';
  }
  get serialNumber() {
    return this.firmware ? '0001' : '';
  }
  async open() {
    this.opened = true;
  }
  async close() {
    this.opened = false;
    this.configuration = null;
  }
  async selectConfiguration() {
    this.configuration = this.firmware ? DEV_CONFIG : ADFU_CONFIG;
  }
  async claimInterface() {}
  async releaseInterface() {}
  async clearHalt() {}
  async transferOut(_ep, data) {
    try {
      const b = bytes(data);
      this.mock.bulkOut(b);
      return { status: 'ok', bytesWritten: b.length };
    } catch {
      return { status: 'stall', bytesWritten: 0 };
    }
  }
  async transferIn(_ep, len) {
    try {
      return { status: 'ok', data: dv(this.mock.bulkIn(len)) };
    } catch {
      return { status: 'stall' };
    }
  }
  async controlTransferIn(s, len) {
    try {
      this.mock.tick(5);
      return { status: 'ok', data: dv(this.mock.controlIn(s.request, s.value, s.index, len)) };
    } catch {
      return { status: 'stall' };
    }
  }
  async controlTransferOut(s, data) {
    try {
      const b = data ? bytes(data) : new Uint8Array(0);
      this.mock.controlOut(s.request, s.value, s.index, b);
      // A reboot re-enumerates: the page opens the device again.
      this.configuration = null;
      this.opened = this.opened && s.request !== 0x07;
      return { status: 'ok', bytesWritten: b.length };
    } catch {
      return { status: 'stall' };
    }
  }
}

/// `navigator.usb` over one simulated player.
export class MockUsb {
  constructor(mock) {
    this.device = new MockUSBDevice(mock);
    this.mock = mock;
  }
  async getDevices() {
    return this.mock.mode() === 'dead' ? [] : [this.device];
  }
  async requestDevice() {
    return this.device;
  }
  addEventListener() {}
}
