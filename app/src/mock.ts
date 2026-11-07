import type { DaemonSocket } from './ipc';
import type { GameProfile, LayerSpec, RGB, RpcRequest, RpcResponse, StateEntry } from './types';

// Dev mock daemon: implements the IPC contract in memory, exposed as
// window.ChromaMock (installed by main.tsx when VITE_DEV_MOCK=1). Mirrors the
// real daemon: color XOR effect XOR matrix XOR layer-stack per device, refresh
// flips the permission-denied device to available, profiles stored in memory,
// get_active_profile cycles names so the UI can be demoed without hardware.
// Also implements the frozen push-event delta: synthetic key events every 3s
// for the first matrix device and a cycling audio level while audio is on.

interface MockDevice {
  id: string;
  name: string;
  vendor: string;
  vid: number;
  pid: number;
  kind: 'Razer' | 'Logitech' | 'Bridge';
  zones: Array<{ name: string; leds: number; matrix?: [number, number] }>;
  available: boolean;
  openError?: string;
  errorKind?: 'permission' | 'busy' | 'unknown';
  note?: string;
}

/** Effect kinds the mock validates (mirrors the daemon's enum). */
const EFFECT_NAMES = [
  'static', 'wave', 'spectrum', 'breathe', 'gradient',
  'ripple', 'reactive', 'starlight', 'fire', 'audio_meter',
];

const MOCK_DEVICES: MockDevice[] = [
  {
    id: '1532:0228:MOCK12',
    name: 'BlackWidow Elite (mock)',
    vendor: 'Razer',
    vid: 0x1532,
    pid: 0x0228,
    kind: 'Razer',
    zones: [{ name: 'matrix', leds: 132, matrix: [6, 22] }],
    available: true,
  },
  {
    id: '046d:c08b:MOCK34',
    name: 'G502 HERO (mock)',
    vendor: 'Logitech',
    vid: 0x046d,
    pid: 0xc08b,
    kind: 'Logitech',
    zones: [
      { name: 'logo', leds: 2 },
      { name: 'wheel', leds: 1 },
    ],
    available: false, // simulates macOS HID permission denied; refresh fixes
    openError: 'hidapi: hid_open_path: failed to open IOHIDDevice: privilege violation',
    errorKind: 'permission',
  },
  {
    id: 'bridge:openrgb:0',
    name: 'Corsair Vengeance RGB (bridge)',
    vendor: 'OpenRGB',
    vid: 0,
    pid: 0,
    kind: 'Bridge',
    zones: [{ name: 'dram', leds: 12 }],
    available: true,
  },
];

const FRONTMOST_CYCLE = ['Cursor', 'Firefox', 'VALORANT', 'Finder'];

function createMockSocket(): DaemonSocket {
  const effects = new Map<string, { name: string; params: Record<string, unknown> }>();
  const layers = new Map<string, LayerSpec[]>();
  const statics = new Map<string, RGB>();
  const matrices = new Map<string, Record<string, RGB[]>>();
  const profiles = new Map<string, GameProfile>();
  let audioOn = false;
  let audioLevel = 0;
  let activeProfile: string | null = null;
  let frontmostIdx = 0;
  const sock: DaemonSocket = { onopen: null, onclose: null, onmessage: null, send: () => {}, close: () => {} };

  const push = (frame: unknown) => sock.onmessage?.({ data: JSON.stringify(frame) });

  // synthetic key events: every 3s a random matrix cell on the first matrix device
  const firstMatrix = MOCK_DEVICES.find((d) => d.zones.some((z) => z.matrix));
  const keyTimer = window.setInterval(() => {
    if (!firstMatrix) return;
    const m = firstMatrix.zones.find((z) => z.matrix)!.matrix!;
    push({
      event: 'key',
      device: firstMatrix.id,
      row: Math.floor(Math.random() * m[0]),
      col: Math.floor(Math.random() * m[1]),
      code: 0x04 + Math.floor(Math.random() * 101),
    });
  }, 3000);

  // cycling audio level (~10/s) while audio is on
  const audioTimer = window.setInterval(() => {
    if (!audioOn) return;
    const t = performance.now() / 1000;
    audioLevel = Math.round(
      500 + 420 * Math.sin(t * 1.7) * Math.sin(t * 0.6 + 1.3) + 80 * Math.sin(t * 9),
    );
    push({ event: 'audio', level: Math.max(0, Math.min(1000, audioLevel)) });
  }, 100);

  const reply = (msg: RpcRequest, make: () => { ok: true; data: unknown } | { ok: false; error: string }) => {
    window.setTimeout(() => {
      const r = { id: msg.id, ...make() } as RpcResponse;
      sock.onmessage?.({ data: JSON.stringify(r) });
    }, 40);
  };

  const deviceJson = (d: MockDevice) => ({
    ...d,
    openError: d.available ? null : (d.openError ?? null),
    errorKind: d.available ? null : (d.errorKind ?? 'unknown'),
  });

  const stateEntries = (): StateEntry[] =>
    MOCK_DEVICES.map((d) => {
      const eff = effects.get(d.id);
      if (eff) return { id: d.id, color: null, effect: eff.name, mode: 'effect' as const, matrix: null };
      const stack = layers.get(d.id);
      if (stack)
        return {
          id: d.id,
          color: null,
          effect: stack[0]?.effect ?? null, // top layer name for back-compat
          mode: 'effect' as const,
          matrix: null,
          layers: stack,
        };
      const m = matrices.get(d.id);
      if (m) return { id: d.id, color: null, effect: null, mode: 'matrix' as const, matrix: m };
      const c = statics.get(d.id);
      return {
        id: d.id,
        color: c ?? null,
        effect: null,
        mode: c ? ('static' as const) : ('idle' as const),
        matrix: null,
      };
    });

  /** set_layers-style validation: same error shape as set_effect. */
  const validateLayers = (raw: unknown): { ok: true; layers: LayerSpec[] } | { ok: false; error: string } => {
    if (!Array.isArray(raw) || raw.length === 0) return { ok: false, error: 'layers must be a non-empty array' };
    if (raw.length > 8) return { ok: false, error: 'layers: too many layers (max 8)' };
    for (let i = 0; i < raw.length; i++) {
      const l = raw[i] as Partial<LayerSpec>;
      if (!l || typeof l !== 'object' || !EFFECT_NAMES.includes(String(l.effect)))
        return { ok: false, error: `layers[${i}]: unknown effect: ${String((l as { effect?: string })?.effect)}` };
      const op = l.opacity;
      if (typeof op !== 'number' || !Number.isFinite(op) || op < 0 || op > 100)
        return { ok: false, error: `layers[${i}].opacity must be a number 0-100` };
      if (l.blend !== 'normal' && l.blend !== 'add')
        return { ok: false, error: `layers[${i}].blend must be "normal" or "add"` };
      if (typeof l.enabled !== 'boolean') return { ok: false, error: `layers[${i}].enabled must be a boolean` };
      if (l.params != null && typeof l.params !== 'object')
        return { ok: false, error: `layers[${i}].params must be an object` };
    }
    return { ok: true, layers: raw as LayerSpec[] };
  };

  sock.send = (raw) => {
    let msg: RpcRequest;
    try {
      msg = JSON.parse(raw) as RpcRequest;
    } catch {
      return;
    }
    const dev = (n: string) => MOCK_DEVICES.find((d) => d.id === n || d.name === n);
    const need = (d: MockDevice | undefined): { ok: false; error: string } | null =>
      !d
        ? { ok: false, error: `unknown device: ${msg.params.device}` }
        : !d.available
          ? { ok: false, error: `device ${d.id} unavailable: ${d.openError ?? d.note ?? 'not open'}` }
          : null;

    switch (msg.method) {
      case 'list_devices':
        reply(msg, () => ({ ok: true, data: MOCK_DEVICES.map(deviceJson) }));
        break;
      case 'get_state':
        reply(msg, () => ({ ok: true, data: stateEntries() }));
        break;
      case 'refresh_devices': {
        // simulate the user granting permissions: everything becomes available
        for (const d of MOCK_DEVICES) {
          d.available = true;
          d.openError = undefined;
          d.errorKind = undefined;
        }
        reply(msg, () => ({
          ok: true,
          data: { devices: MOCK_DEVICES.length, ready: MOCK_DEVICES.length, unavailable: 0 },
        }));
        break;
      }
      case 'set_color': {
        const d = dev(String(msg.params.device));
        const c = msg.params.color as RGB;
        const err = need(d);
        if (err) reply(msg, () => err);
        else {
          effects.delete(d!.id);
          matrices.delete(d!.id);
          layers.delete(d!.id);
          statics.set(d!.id, c);
          reply(msg, () => ({ ok: true, data: { device: d!.id, color: c } }));
        }
        break;
      }
      case 'set_effect': {
        const d = dev(String(msg.params.device));
        const name = String(msg.params.effect);
        const err = need(d);
        if (!EFFECT_NAMES.includes(name)) reply(msg, () => ({ ok: false, error: `unknown effect: ${name}` }));
        else if (err) reply(msg, () => err);
        else {
          statics.delete(d!.id);
          matrices.delete(d!.id);
          // set_effect is single-layer shorthand: it REPLACES any stack
          layers.set(d!.id, [
            {
              effect: name as LayerSpec['effect'],
              params: (msg.params.params as LayerSpec['params']) ?? {},
              opacity: 100,
              blend: 'normal',
              enabled: true,
            },
          ]);
          effects.set(d!.id, { name, params: (msg.params.params as Record<string, unknown>) ?? {} });
          reply(msg, () => ({ ok: true, data: { device: d!.id, effect: name } }));
        }
        break;
      }
      case 'set_layers': {
        const d = dev(String(msg.params.device));
        const err = need(d);
        const v = validateLayers(msg.params.layers);
        if (err) reply(msg, () => err);
        else if (!v.ok) reply(msg, () => v);
        else {
          statics.delete(d!.id);
          matrices.delete(d!.id);
          effects.delete(d!.id);
          layers.set(d!.id, v.layers);
          reply(msg, () => ({ ok: true, data: { device: d!.id, layers: v.layers.length } }));
        }
        break;
      }
      case 'trigger': {
        const d = dev(String(msg.params.device));
        const err = need(d);
        if (err) reply(msg, () => err);
        else {
          // bounce a synthetic key event back so the studio canvas ripples (mock-only feedback loop)
          const z = d!.zones.find((zz) => zz.matrix);
          const row = msg.params.row != null ? Number(msg.params.row) : Math.floor(((z?.matrix?.[0] ?? 6) - 1) / 2);
          const col = msg.params.col != null ? Number(msg.params.col) : Math.floor(((z?.matrix?.[1] ?? 22) - 1) / 2);
          window.setTimeout(() => push({ event: 'key', device: d!.id, row, col, code: 0 }), 30);
          reply(msg, () => ({ ok: true, data: { queued: true } }));
        }
        break;
      }
      case 'set_audio': {
        const on = msg.params.on === true;
        audioOn = on;
        if (!on) audioLevel = 0;
        reply(msg, () => ({ ok: true, data: { on, device: String(msg.params.device ?? 'default') } }));
        break;
      }
      case 'get_audio_level':
        reply(msg, () => ({ ok: true, data: { level: audioOn ? audioLevel : 0, on: audioOn } }));
        break;
      case 'stop_effect': {
        const d = dev(String(msg.params.device));
        if (!d) reply(msg, () => ({ ok: false, error: `unknown device: ${msg.params.device}` }));
        else {
          effects.delete(d.id); // freezes the last rendered frame
          layers.delete(d.id);
          reply(msg, () => ({ ok: true, data: { device: d.id, stopped: true } }));
        }
        break;
      }
      case 'set_matrix': {
        const d = dev(String(msg.params.device));
        const zone = String(msg.params.zone);
        const colors = msg.params.colors as RGB[];
        const err = need(d);
        if (err) reply(msg, () => err);
        else {
          const z = d!.zones.find((zz) => zz.name === zone);
          if (!z) reply(msg, () => ({ ok: false, error: `unknown zone: ${zone}` }));
          else if (colors.length !== z.leds)
            reply(msg, () => ({ ok: false, error: `zone '${zone}' expects ${z.leds} colors, got ${colors.length}` }));
          else {
            effects.delete(d!.id);
            statics.delete(d!.id);
            layers.delete(d!.id);
            matrices.set(d!.id, { [zone]: colors });
            reply(msg, () => ({ ok: true, data: { device: d!.id, zone, leds: colors.length } }));
          }
        }
        break;
      }
      case 'profile_save': {
        const p = msg.params.profile as GameProfile;
        if (!p?.name?.trim()) reply(msg, () => ({ ok: false, error: 'profile.name must be non-empty' }));
        else {
          const warnings: string[] = [];
          let bad = false;
          for (const [id, entry] of Object.entries(p.devices)) {
            if (entry.kind === 'layers') {
              const v = validateLayers(entry.layers);
              if (!v.ok) {
                bad = true;
                warnings.push(v.error);
                break;
              }
            }
          }
          if (bad) reply(msg, () => ({ ok: false, error: warnings.join('; ') }));
          else {
            profiles.set(p.name, p);
            reply(msg, () => ({ ok: true, data: { saved: p.name, warnings: [] } }));
          }
        }
        break;
      }
      case 'profile_list':
        reply(msg, () => ({
          ok: true,
          data: [...profiles.values()].map((p) => ({ name: p.name, bindings: p.bindings, devices: Object.keys(p.devices) })),
        }));
        break;
      case 'profile_get': {
        const p = profiles.get(String(msg.params.name));
        reply(msg, () => (p ? { ok: true, data: p } : { ok: false, error: `no such profile: ${msg.params.name}` }));
        break;
      }
      case 'profile_delete': {
        const name = String(msg.params.name);
        if (profiles.delete(name)) reply(msg, () => ({ ok: true, data: { deleted: name } }));
        else reply(msg, () => ({ ok: false, error: `no such profile: ${name}` }));
        break;
      }
      case 'profile_apply': {
        const name = String(msg.params.name);
        const p = profiles.get(name);
        if (!p) {
          reply(msg, () => ({ ok: false, error: `no such profile: ${name}` }));
          break;
        }
        const results: Record<string, string> = {};
        const warnings: string[] = [];
        for (const [id, entry] of Object.entries(p.devices)) {
          const d = dev(id);
          if (!d || !d.available) {
            warnings.push(`device ${id} not available; skipped`);
            continue;
          }
          if (entry.kind === 'static') {
            effects.delete(id);
            matrices.delete(id);
            layers.delete(id);
            statics.set(id, entry.color);
          } else if (entry.kind === 'effect') {
            statics.delete(id);
            matrices.delete(id);
            layers.delete(id);
            effects.set(id, { name: entry.effect, params: entry.params as Record<string, unknown> });
          } else if (entry.kind === 'layers') {
            statics.delete(id);
            matrices.delete(id);
            effects.delete(id);
            layers.set(id, entry.layers);
          } else {
            effects.delete(id);
            statics.delete(id);
            layers.delete(id);
            matrices.set(id, { [entry.zone]: entry.colors });
          }
          results[id] = 'ok';
        }
        activeProfile = name;
        reply(msg, () => ({ ok: true, data: { results, warnings } }));
        break;
      }
      case 'get_active_profile': {
        // cycle the fake frontmost app so the watcher indicator animates
        frontmostIdx = (frontmostIdx + 1) % FRONTMOST_CYCLE.length;
        const frontmost = FRONTMOST_CYCLE[frontmostIdx];
        const hit = [...profiles.values()].find((p) =>
          p.bindings.some((b) => frontmost.toLowerCase().includes(b.toLowerCase())),
        );
        if (hit) activeProfile = hit.name; // simulate auto-switch
        reply(msg, () => ({ ok: true, data: { name: activeProfile, watcher: 'on', frontmost } }));
        break;
      }
      default:
        reply(msg, () => ({ ok: false, error: `unknown method: ${msg.method}` }));
    }
  };

  sock.close = () => {
    window.clearInterval(keyTimer);
    window.clearInterval(audioTimer);
  };

  // simulate connect latency so the UI's 'connecting' state is visible
  window.setTimeout(() => sock.onopen?.(), 250);
  return sock;
}

export function installChromaMock(): void {
  (window as unknown as { ChromaMock?: { createSocket: () => DaemonSocket } }).ChromaMock = { createSocket: createMockSocket };
}
