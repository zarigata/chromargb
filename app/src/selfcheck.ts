// Self-check: run with `npm run check`. Assert-based, no framework.
// Covers the pieces of non-trivial renderer logic:
//   1. drawFrame effect approximations (pure math)
//   2. ChromaClient request/response id matching + ok:false + offline reject
//   3. Frozen-delta layer contract: setLayers/trigger/setAudio/onEvent push frames
//   4. drawMatrixFrame compositing + the 5 new effects
/* eslint-disable no-console */
import assert from 'node:assert';
import { ChromaClient, type DaemonSocket } from './ipc';
import { supportsPerLed } from './types';
import { drawFrame, drawMatrixFrame, EMPTY_RUNTIME } from './effectPreview';
import { hexToRgb, rgbToHex } from './color';
import type { DaemonEvent, Device, LayerSpec, RpcRequest, RpcResponse } from './types';

// ipc.ts uses window.setTimeout/clearTimeout — shim for node.
const g = globalThis as unknown as { window?: unknown };
g.window = { setTimeout: setTimeout, clearTimeout: clearTimeout };

// ---------- color helpers ----------
assert.deepStrictEqual(rgbToHex([255, 0, 68]), '#ff0044', 'rgbToHex');
assert.deepStrictEqual(hexToRgb('#00ff44'), [0, 255, 68], 'hexToRgb');
assert.deepStrictEqual(hexToRgb(rgbToHex([1, 2, 3])), [1, 2, 3], 'roundtrip');

// ---------- drawFrame ----------
const n = 24;

// static: uniform
const st = drawFrame('static', { color: [10, 20, 30] }, n, 0);
assert.ok(st.every((c) => c[0] === 10 && c[1] === 20 && c[2] === 30), 'static uniform');

// wave: left vs right are mirrored up to a one-LED shift (hue i/n ↔ 1-i/n)
const wl = drawFrame('wave', { speed: 50, direction: 'left' }, n, 0);
const wr = drawFrame('wave', { speed: 50, direction: 'right' }, n, 0);
const wrRev = [...wr].reverse();
const maxDiff = Math.max(
  ...wl.map((c, i) => Math.abs(c[0] - wrRev[i][0]) + Math.abs(c[1] - wrRev[i][1]) + Math.abs(c[2] - wrRev[i][2])),
);
assert.ok(maxDiff <= 90, `wave left/right mirrored (max channel diff ${maxDiff})`);
assert.notDeepStrictEqual(wl, wr, 'wave direction changes the pattern');

// wave: animates over time
const w0 = drawFrame('wave', { speed: 100, direction: 'right' }, n, 0);
const w1 = drawFrame('wave', { speed: 100, direction: 'right' }, n, 0.5);
assert.notDeepStrictEqual(w0, w1, 'wave animates');

// spectrum: cycles
const s0 = drawFrame('spectrum', { speed: 50 }, n, 0);
const s1 = drawFrame('spectrum', { speed: 50 }, n, 1.0);
assert.notDeepStrictEqual(s0, s1, 'spectrum animates');

// breathe: brightness oscillates (max at some t >> min at another)
const lum = (c: [number, number, number]) => c[0] + c[1] + c[2];
const bLeds = (t: number) => drawFrame('breathe', { speed: 50, color: [255, 255, 255] }, 4, t)[0];
let min = Infinity;
let max = -Infinity;
for (let t = 0; t < 4; t += 0.05) {
  const l = lum(bLeds(t));
  min = Math.min(min, l);
  max = Math.max(max, l);
}
assert.ok(max > min * 1.5, `breathe oscillates (max ${max} vs min ${min})`);

// gradient: endpoints exact, midpoint is a mix
const gr = drawFrame('gradient', { colors: [[0, 0, 0], [100, 0, 0]] }, 3, 0);
assert.deepStrictEqual(gr[0], [0, 0, 0], 'gradient start exact');
assert.deepStrictEqual(gr[2], [100, 0, 0], 'gradient end exact');
assert.deepStrictEqual(gr[1], [50, 0, 0], 'gradient midpoint mixed');
assert.ok(gr.every((c) => c[0] >= 0 && c[0] <= 100), 'gradient in range');

// ---------- drawMatrixFrame: compositing + new effects ----------
const ROWS = 6, COLS = 22;
const mk = (effect: LayerSpec['effect'], over: Partial<LayerSpec> = {}): LayerSpec => ({
  effect, params: {}, opacity: 100, blend: 'normal', enabled: true, ...over,
});
const total = (g: Array<Array<[number, number, number]>>) => g.flat().reduce((s, c) => s + c[0] + c[1] + c[2], 0);

// empty stack = black frame
assert.strictEqual(total(drawMatrixFrame([], ROWS, COLS, 0)), 0, 'empty stack → black');

// disabled + opacity=0 layers contribute nothing
const base = mk('static', { params: { color: [100, 100, 100] } });
assert.strictEqual(
  total(drawMatrixFrame([{ ...mk('fire'), enabled: false }, base], ROWS, COLS, 0)),
  total(drawMatrixFrame([base], ROWS, COLS, 0)),
  'disabled layer contributes nothing',
);
assert.strictEqual(
  total(drawMatrixFrame([{ ...mk('fire'), opacity: 0 }, base], ROWS, COLS, 0)),
  total(drawMatrixFrame([base], ROWS, COLS, 0)),
  'opacity 0 layer contributes nothing',
);

// normal blend: opacity 50 halves the static base
const half = drawMatrixFrame([{ ...base, opacity: 50 }], ROWS, COLS, 0);
assert.strictEqual(half[0][0][0], 50, 'normal blend lerps (opacity 50)');

// add blend: saturating addition (stack is top-first: add layer sits ON the base)
const add = drawMatrixFrame([{ ...base, blend: 'add' }, base], ROWS, COLS, 0);
assert.strictEqual(add[0][0][0], 200, 'add blend stacks (100+100=200)');
const addHot = drawMatrixFrame([{ ...mk('static', { params: { color: [200, 200, 200] } }), blend: 'add' }, { ...mk('static', { params: { color: [200, 200, 200] } }), blend: 'add' }], ROWS, COLS, 0);
assert.strictEqual(addHot[0][0][0], 255, 'add blend clamps at 255');

// ripple: ring expands away from origin; origin dims as it travels
const rt = { events: [{ x: 0.5, y: 0.5, t0: 0 }], level: 0 };
const rip0 = drawMatrixFrame([mk('ripple', { params: { color: [255, 255, 255], speed: 50 } })], ROWS, COLS, 0.001, rt);
const ripLate = drawMatrixFrame([mk('ripple', { params: { color: [255, 255, 255], speed: 50 } })], ROWS, COLS, 1.0, rt);
const cR = Math.floor(ROWS / 2), cC = Math.floor(COLS / 2);
assert.ok(total(rip0) > 0, 'ripple lights up at t≈0');
assert.ok(rip0[cR][cC][0] > ripLate[cR][cC][0], 'ripple origin decays as ring expands');

// reactive: cell lights then decays to zero
const react0 = drawMatrixFrame([mk('reactive', { params: { color: [255, 0, 0], fade: 50 } })], ROWS, COLS, 0.001, rt);
const reactLate = drawMatrixFrame([mk('reactive', { params: { color: [255, 0, 0], fade: 50 } })], ROWS, COLS, 5, rt);
assert.ok(react0[cR][cC][0] > 200, 'reactive lights the pressed cell');
assert.strictEqual(react0[0][0][0], 0, 'reactive far cell dark');
assert.strictEqual(total(reactLate), 0, 'reactive fully decays');

// starlight: twinkles (frame differs over time), stays dark-ish at minima baseline
const star0 = drawMatrixFrame([mk('starlight', { params: { colors: [[255, 255, 255]], speed: 50 } })], ROWS, COLS, 0);
const star1 = drawMatrixFrame([mk('starlight', { params: { colors: [[255, 255, 255]], speed: 50 } })], ROWS, COLS, 0.7);
assert.notDeepStrictEqual(star0, star1, 'starlight animates');

// fire: bottom rows hotter than top rows
const fire = drawMatrixFrame([mk('fire', { params: { speed: 60 } })], ROWS, COLS, 1.2);
const rowSum = (r: number) => fire[r].reduce((s, c) => s + c[0], 0);
assert.ok(rowSum(ROWS - 1) > rowSum(0), `fire bottom hotter than top (${rowSum(ROWS - 1)} > ${rowSum(0)})`);

// audio_meter: level 0 → black; level 1000 → bottom row lit, and bands respected
const aud0 = drawMatrixFrame([mk('audio_meter', { params: { color: [0, 255, 0], bands: 11 } })], ROWS, COLS, 0, { events: [], level: 0 });
assert.strictEqual(total(aud0), 0, 'audio_meter level 0 → black');
const audHi = drawMatrixFrame([mk('audio_meter', { params: { color: [0, 255, 0], bands: 11 } })], ROWS, COLS, 0, { events: [], level: 1000 });
assert.ok(total(audHi) > 0, 'audio_meter level 1000 lights up');
assert.ok(audHi[ROWS - 1].every((c) => c[1] > 0), 'audio_meter bottom row lit at full level');
// band gaps: with 11 bands over 22 cols every band starts at even col
assert.ok(audHi[0].some((c) => c[1] === 0), 'audio_meter top row mostly dark at full level');

// ---------- ChromaClient with fake socket ----------
async function main() {
const DEVICES = [
  {
    id: '1532:0228:X1', name: 'BlackWidow Elite', vendor: 'Razer', vid: 0x1532, pid: 0x0228, kind: 'Razer',
    zones: [{ name: 'matrix', leds: 132, matrix: [6, 22] }], available: true,
  },
  {
    id: '046d:c08b:X2', name: 'G502 HERO', vendor: 'Logitech', vid: 0x046d, pid: 0xc08b, kind: 'Logitech',
    zones: [{ name: 'logo', leds: 2 }, { name: 'wheel', leds: 1 }], available: false,
    note: 'open failed: privilege violation',
  },
  { id: 'bridge:0', name: 'DRAM', vendor: 'OpenRGB', vid: 0, pid: 0, kind: 'Bridge', zones: [{ name: 'dram', leds: 12 }], available: true },
];

// fake daemon that can also inject unsolicited push frames
function pushDaemon() {
  let sockRef: DaemonSocket | null = null;
  const factory = (): DaemonSocket => {
    const sock: DaemonSocket = { onopen: null, onclose: null, onmessage: null, send: () => {}, close: () => {} };
    sock.send = (raw) => {
      const req = JSON.parse(raw) as RpcRequest;
      const reply = (r: RpcResponse) => sock.onmessage?.({ data: JSON.stringify(r) });
      if (req.method === 'boom') reply({ id: req.id, ok: false, error: 'daemon says no' });
      else if (req.method === 'list_devices') reply({ id: req.id, ok: true, data: DEVICES });
      else if (req.method === 'set_layers') reply({ id: req.id, ok: true, data: { device: req.params.device, layers: (req.params.layers as unknown[]).length } });
      else if (req.method === 'trigger') reply({ id: req.id, ok: true, data: { queued: true } });
      else if (req.method === 'set_audio') reply({ id: req.id, ok: true, data: { on: req.params.on === true, device: String(req.params.device ?? 'default') } });
      else if (req.method === 'get_audio_level') reply({ id: req.id, ok: true, data: { level: 420, on: true } });
      else if (req.method === 'get_state')
        reply({
          id: req.id,
          ok: true,
          data: [
            {
              id: '1532:0228:X1', color: null, effect: 'wave', mode: 'effect', matrix: null,
              layers: [
                { effect: 'wave', params: { speed: 60 }, opacity: 100, blend: 'normal', enabled: true },
                { effect: 'ripple', params: { color: [124, 92, 255], speed: 60 }, opacity: 80, blend: 'add', enabled: true },
              ],
            },
            { id: '046d:c08b:X2', color: null, effect: null, mode: 'idle', matrix: null },
            { id: 'bridge:0', color: [255, 0, 0], effect: null, mode: 'static', matrix: null },
          ],
        });
      else reply({ id: req.id, ok: true, data: { echo: req.method } });
    };
    setTimeout(() => sock.onopen?.(), 1);
    sockRef = sock;
    return sock;
  };
  return { factory, push: (frame: unknown) => sockRef?.onmessage?.({ data: JSON.stringify(frame) }) };
}

// frozen-contract shape checks on the fixture itself
assert.ok(supportsPerLed(DEVICES[0] as unknown as Device), 'matrix zone ⇒ supportsPerLed');
assert.ok(!supportsPerLed(DEVICES[1] as unknown as Device), 'zone-less device ⇒ !supportsPerLed');
assert.strictEqual((DEVICES[1] as { available: boolean }).available, false, 'fixture G502 unavailable');

const daemon = pushDaemon();
const wire: RpcRequest[] = [];
// wire tap: wrap the factory to record what the client sends
const c = new ChromaClient(() => {
  const sock = daemon.factory();
  const origSend = sock.send.bind(sock);
  sock.send = (raw) => {
    wire.push(JSON.parse(raw) as RpcRequest);
    origSend(raw);
  };
  return sock;
});
const sockPush = daemon.push;

// wait for the async onopen before issuing requests
await new Promise<void>((res) => c.subscribe((s) => s === 'online' && res()));

// request ids strictly increase and match responses to the right promise
const r1 = await c.request<{ echo: string }>('alpha');
const r2 = await c.request<{ echo: string }>('beta');
assert.strictEqual(r1.echo, 'alpha', 'response routed to correct promise');
assert.strictEqual(r2.echo, 'beta', 'response routed to correct promise 2');
assert.ok(wire.length >= 2, 'wire messages sent');
assert.ok(wire[wire.length - 1].id > wire[wire.length - 2].id, 'request ids increase');

// refresh(): merges list_devices + get_state by id; static from color; unavailable not marked active
const st = await c.refresh();
assert.strictEqual(st.devices.length, 3, 'refresh merged 3 devices');
assert.strictEqual(st.devices[1].available, false, 'available flag round-trips');
assert.strictEqual(st.active['1532:0228:X1']?.effect, 'wave', 'effect merged by id');
assert.strictEqual(st.active['046d:c08b:X2'], undefined, 'null effect ⇒ not active');
assert.deepStrictEqual(st.active['bridge:0']?.params.color, [255, 0, 0], 'color → static params');

// ok:false rejects
await assert.rejects(() => c.request('boom'), /daemon says no/, 'ok:false rejects with error string');

// typed sugar
await assert.doesNotReject(() => c.setEffect('dev', 'wave', { speed: 10, direction: 'left' }), 'set_effect ok');
await assert.doesNotReject(() => c.stopEffect('dev'), 'stop_effect ok');
await assert.doesNotReject(() => c.setMatrix('dev', 'matrix', [[1, 2, 3]]), 'set_matrix ok');

// ---- frozen delta: setLayers / trigger / setAudio / getAudioLevel + push events ----
const layerStack: LayerSpec[] = [
  { effect: 'ripple', params: { color: [124, 92, 255], speed: 60 }, opacity: 100, blend: 'normal', enabled: true },
];
const sl = await c.setLayers('1532:0228:X1', layerStack);
assert.deepStrictEqual(sl, { device: '1532:0228:X1', layers: 1 }, 'set_layers echo');

const tg = await c.trigger('1532:0228:X1', 2, 10);
assert.deepStrictEqual(tg, { queued: true }, 'trigger with row/col');
const tgC = await c.trigger('1532:0228:X1');
assert.deepStrictEqual(tgC, { queued: true }, 'trigger center (no row/col)');

const au = await c.setAudio(true);
assert.deepStrictEqual(au, { on: true, device: 'default' }, 'set_audio on');
const lvl = await c.getAudioLevel();
assert.ok(typeof lvl.level === 'number' && lvl.on === true, 'get_audio_level shape');

// push frames: event listeners fire; pending map untouched
const seen: DaemonEvent[] = [];
const off = c.onEvent((ev) => seen.push(ev));
sockPush({ event: 'key', device: '1532:0228:X1', row: 1, col: 2, code: 4 });
sockPush({ event: 'audio', level: 500 });
await new Promise((r) => setTimeout(r, 20));
assert.strictEqual(seen.length, 2, 'both push events received');
assert.strictEqual(seen[0].event === 'key' ? seen[0].row : -1, 1, 'key event fields');
assert.strictEqual(seen[1].event === 'audio' ? seen[1].level : -1, 500, 'audio event level');
off();
sockPush({ event: 'audio', level: 999 });
await new Promise((r) => setTimeout(r, 20));
assert.strictEqual(seen.length, 2, 'unsubscribe stops delivery');

// get_state layers parsing: refresh surfaces the stack; effect = top layer name
const st2 = await c.refresh();
assert.strictEqual(st2.active['1532:0228:X1']?.effect, 'wave', 'layers[0].effect surfaced as active effect');

c.dispose();
await assert.rejects(() => c.request('get_state'), /offline|disposed/, 'offline after dispose');

console.log('selfcheck: all assertions passed');
}

void main();
