import type { ActiveProfileInfo, DaemonEvent, DaemonState, Device, GameProfile, LayerSpec, ProfileSummary, RpcResponse, StateEntry } from './types';

export const DAEMON_WS_URL = 'ws://127.0.0.1:7474';
const RECONNECT_MS = 3000;
const REQUEST_TIMEOUT_MS = 5000;

/** Anything that quacks like a WebSocket (real socket or the dev mock). */
export interface DaemonSocket {
  send(data: string): void;
  close(): void;
  onopen: (() => void) | null;
  onclose: (() => void) | null;
  onmessage: ((ev: { data: string }) => void) | null;
}

export type SocketFactory = () => DaemonSocket;

/** Real WebSocket transport wrapped to the DaemonSocket shape. */
export function wsFactory(): SocketFactory {
  return () => {
    const ws = new WebSocket(DAEMON_WS_URL);
    const s: DaemonSocket = {
      onopen: null,
      onclose: null,
      onmessage: null,
      send: (d) => ws.send(d),
      close: () => ws.close(),
    };
    ws.onopen = () => s.onopen?.();
    ws.onclose = () => s.onclose?.();
    ws.onmessage = (ev) => s.onmessage?.(ev);
    return s;
  };
}

/**
 * Typed client for the daemon's JSON WebSocket contract:
 *   → {id, method, params}   ← {id, ok:true, data} | {id, ok:false, error}
 * Reconnects every 3s while offline; per-request id counter + timeout.
 */
export type Status = 'connecting' | 'online' | 'offline';
export type Listener = (status: Status, state: DaemonState | null) => void;
export type EventListener = (ev: DaemonEvent) => void;

export class ChromaClient {
  private factory: SocketFactory;
  private sock: DaemonSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, { resolve: (d: unknown) => void; reject: (e: Error) => void; timer: number }>();
  private listeners = new Set<Listener>();
  private eventListeners = new Set<EventListener>();
  private closed = false;
  private reconnectTimer = 0;
  status: Status = 'connecting';
  state: DaemonState | null = null;

  constructor(factory?: SocketFactory) {
    this.factory = factory ?? wsFactory();
    this.connect();
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    fn(this.status, this.state);
    return () => this.listeners.delete(fn);
  }

  /** Subscribe to unsolicited daemon push frames ({event:'key'|'audio'}). */
  onEvent(fn: EventListener): () => void {
    this.eventListeners.add(fn);
    return () => this.eventListeners.delete(fn);
  }

  private emit(): void {
    this.listeners.forEach((l) => l(this.status, this.state));
  }

  private setStatus(s: Status): void {
    this.status = s;
    if (s === 'offline') this.state = null;
    this.emit();
  }

  private connect(): void {
    if (this.closed) return;
    this.setStatus('connecting');
    let sock: DaemonSocket;
    try {
      sock = this.factory();
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.sock = sock;
    sock.onopen = () => {
      this.setStatus('online');
      void this.refresh().catch(() => {});
    };
    sock.onclose = () => {
      this.failPending('daemon disconnected');
      this.sock = null;
      this.setStatus('offline');
      this.scheduleReconnect();
    };
    sock.onmessage = (ev) => this.handleMessage(String(ev.data));
  }

  private scheduleReconnect(): void {
    if (this.closed || this.reconnectTimer) return;
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = 0;
      if (!this.closed) this.connect();
    }, RECONNECT_MS);
  }

  private handleMessage(raw: string): void {
    let msg: RpcResponse | DaemonEvent;
    try {
      msg = JSON.parse(raw) as RpcResponse | DaemonEvent;
    } catch {
      return;
    }
    // Unsolicited push frame (no id, has event key) → dispatch to listeners.
    if ('event' in msg) {
      this.eventListeners.forEach((l) => l(msg as DaemonEvent));
      return;
    }
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    window.clearTimeout(p.timer);
    if (msg.ok) p.resolve(msg.data);
    else p.reject(new Error(msg.error));
  }

  private failPending(reason: string): void {
    this.pending.forEach((p) => {
      window.clearTimeout(p.timer);
      p.reject(new Error(reason));
    });
    this.pending.clear();
  }

  /** Low-level request; rejects on ok:false / timeout / offline. */
  request<T = unknown>(method: string, params: Record<string, unknown> = {}, timeoutMs = REQUEST_TIMEOUT_MS): Promise<T> {
    if (!this.sock || this.status !== 'online') return Promise.reject(new Error('daemon offline'));
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`timeout: ${method}`));
      }, timeoutMs);
      this.pending.set(id, { resolve: resolve as (d: unknown) => void, reject, timer });
      try {
        this.sock!.send(JSON.stringify({ id, method, params }));
      } catch (e) {
        this.pending.delete(id);
        window.clearTimeout(timer);
        reject(e as Error);
      }
    });
  }

  // ---- typed contract methods ----
  listDevices(): Promise<Device[]> {
    return this.request<Device[]>('list_devices');
  }

  /**
   * Re-fetch devices + per-device state and notify subscribers.
   * get_state returns [{id, color, effect|null}]: effect is a bare name (no
   * params) — client-side params from the last applied action are kept for
   * preview continuity; a color with no effect name is a direct set_color
   * (static). null/null = idle or frozen frame.
   */
  refresh(): Promise<DaemonState> {
    return Promise.all([this.listDevices(), this.request<StateEntry[]>('get_state')]).then(([devices, entries]) => {
      const prev = this.state?.active ?? {};
      const active: DaemonState['active'] = {};
      for (const e of entries) {
        if (e.layers != null && e.layers.length > 0) {
          // layer stack active: effect name = top layer for back-compat
          active[e.id] = { effect: e.layers[0].effect, params: e.layers[0].params };
        } else if (e.effect != null) active[e.id] = { effect: e.effect, params: prev[e.id]?.params ?? {} };
        else if (e.color != null) active[e.id] = { effect: 'static', params: { color: e.color } };
      }
      this.state = { devices, active, entries };
      this.emit();
      return this.state;
    });
  }

  setColor(device: string, color: [number, number, number]): Promise<{ device: string; color: [number, number, number] }> {
    return this.request('set_color', { device, color });
  }

  setEffect(device: string, effect: string, params: Record<string, unknown>): Promise<{ device: string; effect: string }> {
    return this.request('set_effect', { device, effect, params });
  }

  /** Replace a device's whole layer stack (top-first). set_effect is the single-layer shorthand for this. */
  setLayers(device: string, layers: LayerSpec[]): Promise<{ device: string; layers: number }> {
    return this.request('set_layers', { device, layers });
  }

  /** Interactive trigger (ripple/reactive origin) at a matrix cell (row, col);
   *  omit both = center. Converts to the daemon's normalized x/y contract. */
  trigger(device: string, row?: number, col?: number, dims?: { rows: number; cols: number }): Promise<{ queued: true }> {
    const rows = dims?.rows ?? 6;
    const cols = dims?.cols ?? 22;
    const x = col != null ? (col + 0.5) / cols : 0.5;
    const y = row != null ? (row + 0.5) / rows : 0.5;
    return this.request('trigger', { device, x, y });
  }

  /** Start/stop audio reactivity (daemon RMS meter). Omit device = system default input. */
  setAudio(on: boolean, device?: string): Promise<{ on: boolean; device: string }> {
    const params: Record<string, unknown> = device ? { on, device } : { on };
    return this.request('set_audio', params);
  }

  getAudioLevel(): Promise<{ level: number; on: boolean }> {
    return this.request('get_audio_level');
  }

  stopEffect(device: string): Promise<{ device: string; stopped: true }> {
    return this.request('stop_effect', { device });
  }

  setMatrix(device: string, zone: string, colors: Array<[number, number, number]>): Promise<{ device: string; zone: string; leds: number }> {
    return this.request('set_matrix', { device, zone, colors });
  }

  refreshDevices(): Promise<{ devices: number; ready: number; unavailable: number }> {
    return this.request<{ devices: number; ready: number; unavailable: number }>('refresh_devices').then((r) => {
      void this.refresh().catch(() => {});
      return r;
    });
  }

  // ---- daemon profiles ----
  profileSave(profile: GameProfile): Promise<{ saved: string; warnings: string[] }> {
    return this.request('profile_save', { profile });
  }

  profileList(): Promise<ProfileSummary[]> {
    return this.request('profile_list');
  }

  profileGet(name: string): Promise<GameProfile> {
    return this.request('profile_get', { name });
  }

  profileDelete(name: string): Promise<{ deleted: string }> {
    return this.request('profile_delete', { name });
  }

  profileApply(name: string): Promise<{ results: Record<string, string>; warnings: string[] }> {
    return this.request<{ results: Record<string, string>; warnings: string[] }>('profile_apply', { name }).then(async (r) => {
      await this.refresh().catch(() => {});
      return r;
    });
  }

  getActiveProfile(): Promise<ActiveProfileInfo> {
    return this.request('get_active_profile');
  }

  dispose(): void {
    this.closed = true;
    if (this.reconnectTimer) window.clearTimeout(this.reconnectTimer);
    this.failPending('client disposed');
    this.sock?.close();
    this.sock = null;
  }
}
