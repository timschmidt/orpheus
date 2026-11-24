// The player's output stage (WEB-2): plays the PCM the page sends (stereo,
// interleaved Float32Array chunks at the track's rate, decoded and processed
// by the firmware's codecs and DSP chain in WebAssembly), resampled linearly
// to the context's rate. Reports what it played and how often it ran dry
// while a track was being decoded (an underrun is an audible dropout).

class OrpheusOut extends AudioWorkletProcessor {
  constructor() {
    super();
    this.queue = [];
    this.cur = null; // { pcm, rate }
    this.pos = 0; // fractional frame index into cur
    this.played = 0; // source frames consumed
    this.underruns = 0;
    this.active = false; // the page is decoding a track
    this.dry = true;
    this.calls = 0;
    this.epoch = 0;
    this.port.onmessage = (e) => {
      const m = e.data;
      if (m.flush) {
        this.epoch = m.epoch;
        this.played = 0;
        this.queue = [];
        this.cur = null;
        this.pos = 0;
        this.dry = true;
      }
      if (m.pcm) this.queue.push({ pcm: m.pcm, rate: m.rate });
      if (m.active !== undefined) this.active = m.active;
    };
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const left = out[0];
    const right = out[1] || out[0];
    for (let i = 0; i < left.length; i++) {
      while (this.cur && this.pos >= this.cur.pcm.length / 2) {
        const whole = Math.floor(this.cur.pcm.length / 2);
        this.pos -= whole;
        this.cur = this.queue.shift() || null;
      }
      if (!this.cur) {
        this.cur = this.queue.shift() || null;
        this.pos = 0;
      }
      if (!this.cur) {
        if (!this.dry && this.active) this.underruns++;
        this.dry = true;
        left[i] = 0;
        right[i] = 0;
        continue;
      }
      this.dry = false;
      const pcm = this.cur.pcm;
      const n = pcm.length / 2;
      const k = Math.floor(this.pos);
      const f = this.pos - k;
      const k1 = Math.min(k + 1, n - 1);
      left[i] = pcm[2 * k] + (pcm[2 * k1] - pcm[2 * k]) * f;
      right[i] = pcm[2 * k + 1] + (pcm[2 * k1 + 1] - pcm[2 * k + 1]) * f;
      const step = this.cur.rate / sampleRate;
      this.pos += step;
      this.played += step;
    }
    if (++this.calls % 8 === 0) {
      this.port.postMessage({ epoch: this.epoch, played: this.played, underruns: this.underruns });
    }
    return true;
  }
}

registerProcessor('orpheus-out', OrpheusOut);
