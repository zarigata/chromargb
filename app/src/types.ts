// Contract types — pinned to the daemon IPC schema (see core/README.md).
// If core/ changes the schema, this file is the single place to update.

export type RGB = [number, number, number];

export interface DeviceZone {
  name: string;
  leds: number;
  matrix?: [number, number]; // [rows, cols] — present ⇒ per-LED addressable
}

export type DeviceKind = 'Razer' | 'Logitech' | 'Bridge' | 'Generic';

export interface Device {
  id: string;
  name: string;
  vendor: string;
  vid: number;
  pid: number;
  kind: DeviceKind;
  zones: DeviceZone[];
  /** false ⇒ backend failed to open (e.g. macOS HID permission denied); writes reject */
  available: boolean;
  /** Raw open error when unavailable (e.g. hidapi privilege violation). */
  openError: string | null;
  /** Classified open failure — drives the Fix Permissions flow. */
  errorKind: ErrorKind | null;
  note?: string;
}

// ---- daemon-side profiles (per-game) ----
export type ProfileDeviceEntry =
  | { kind: 'static'; color: RGB }
  | { kind: 'effect'; effect: EffectKind; params: EffectParams }
  | { kind: 'matrix'; zone: string; colors: RGB[] }
  | { kind: 'layers'; layers: LayerSpec[] };

export interface GameProfile {
  name: string;
  /** App-name substrings that trigger auto-switch (case-insensitive). */
  bindings: string[];
  devices: Record<string, ProfileDeviceEntry>;
}

export interface ProfileSummary {
  name: string;
  bindings: string[];
  devices: string[];
}

export interface ActiveProfileInfo {
  name: string | null;
  watcher: 'on' | 'off';
  frontmost: string | null;
}

/** The wire has no supportsPerLed field — derive it client-side. */
export function supportsPerLed(d: Device): boolean {
  return d.zones.some((z) => z.matrix != null);
}

/** One get_state entry; color/effect null = nothing active (idle / frozen frame).
 *  `layers` present when a layer stack is active (mode stays 'effect';
 *  `effect` = top layer name for back-compat). */
export interface StateEntry {
  id: string;
  color: RGB | null;
  effect: string | null;
  mode: 'effect' | 'static' | 'matrix' | 'idle';
  matrix: Record<string, RGB[]> | null;
  layers?: LayerSpec[] | null;
}

export type ErrorKind = 'permission' | 'busy' | 'unknown';

export interface ActiveEffect {
  effect: string;
  params: EffectParams;
}

/** Effect params per the pinned schema; exactly one variant per effect kind. */
export interface EffectParams {
  color?: RGB; // static · breathe · ripple · reactive · audio_meter (optional)
  speed?: number; // wave · spectrum · breathe · ripple · starlight · fire (0-100)
  direction?: 'left' | 'right'; // wave
  colors?: RGB[]; // gradient · starlight
  fade?: number; // reactive (0-100)
  bands?: number; // audio_meter (column groups)
}

export type EffectKind =
  | 'static'
  | 'wave'
  | 'spectrum'
  | 'breathe'
  | 'gradient'
  | 'ripple'
  | 'reactive'
  | 'starlight'
  | 'fire'
  | 'audio_meter';

export const EFFECT_KINDS: EffectKind[] = [
  'static',
  'wave',
  'spectrum',
  'breathe',
  'gradient',
  'ripple',
  'reactive',
  'starlight',
  'fire',
  'audio_meter',
];

/** One composited layer of a device's lighting stack (set_layers / get_state.layers).
 *  Array order is top-first: index 0 composites last (front), like a layers panel. */
export interface LayerSpec {
  effect: EffectKind;
  params: EffectParams;
  /** 0-100 */
  opacity: number;
  blend: 'normal' | 'add';
  enabled: boolean;
}

// ---- unsolicited server push frames ----
export interface KeyEvent {
  event: 'key';
  device: string;
  row: number;
  col: number;
  code: number; // HID usage
}
export interface AudioEvent {
  event: 'audio';
  level: number; // 0-1000
}
export type DaemonEvent = KeyEvent | AudioEvent;

/** Client-side view: devices from list_devices merged with get_state entries. */
export interface DaemonState {
  devices: Device[];
  active: Record<string, ActiveEffect | undefined>;
  /** Raw get_state entries (mode + applied matrix) when available. */
  entries?: StateEntry[];
}

// ---- JSON-RPC-ish envelope ----
export interface RpcRequest {
  id: number;
  method: string;
  params: Record<string, unknown>;
}
export type RpcResponse =
  | { id: number; ok: true; data: unknown }
  | { id: number; ok: false; error: string };
