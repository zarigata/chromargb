import type { EffectKind, EffectParams, LayerSpec, RGB } from './types';

// Client-side approximation of the daemon's effect engine, used for the
// studio's live canvas + strip previews. Pure in t: every frame is a function
// of (params, t, runtime-events) so React can re-render on a clock.

function hsl(h: number, s: number, l: number): [number, number, number] {
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => {
    const k = (n + h * 12) % 12;
    return l - a * Math.max(-1, Math.min(k - 3, Math.min(9 - k, 1)));
  };
  return [Math.round(f(0) * 255), Math.round(f(8) * 255), Math.round(f(4) * 255)];
}

function mix(c1: [number, number, number], c2: [number, number, number], t: number): [number, number, number] {
  return [
    Math.round(c1[0] + (c2[0] - c1[0]) * t),
    Math.round(c1[1] + (c2[1] - c1[1]) * t),
    Math.round(c1[2] + (c2[2] - c1[2]) * t),
  ];
}

/** Deterministic per-cell hash noise in [0,1) — starlight twinkle, fire flicker. */
function hash01(n: number): number {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}

/** External runtime inputs for event-driven effects (ripple origins, reactive
 *  presses, audio level). Kept out of params so layers stay serializable. */
export interface PreviewRuntime {
  /** Ripple/reactive origins: [x(0-1), y(0-1), ageSec] — oldest first. */
  events: Array<{ x: number; y: number; t0: number }>;
  /** Audio level 0-1000 (0 when audio off). */
  level?: number;
}

export const EMPTY_RUNTIME: PreviewRuntime = { events: [], level: 0 };

/** Draw one LED-strip frame (n LEDs, index 0 = left). */
export function drawFrame(effect: EffectKind, params: EffectParams, n: number, t: number): Array<[number, number, number]> {
  const speed = params.speed ?? 50;
  const out: Array<[number, number, number]> = new Array(n);
  switch (effect) {
    case 'static': {
      const c = params.color ?? [0, 255, 0];
      for (let i = 0; i < n; i++) out[i] = c;
      break;
    }
    case 'wave': {
      // rainbow wave sweeping across the strip; sign flips direction
      const dir = params.direction === 'left' ? 1 : -1;
      const phase = t * (speed / 100) * 1.5;
      for (let i = 0; i < n; i++) out[i] = hsl((((dir * i) / n + phase) % 1 + 1) % 1, 1, 0.5);
      break;
    }
    case 'spectrum': {
      // whole strip cycles the hue wheel together, with a subtle spatial offset
      const phase = t * (speed / 100) * 0.5;
      for (let i = 0; i < n; i++) out[i] = hsl((phase + (i / n) * 0.15) % 1, 1, 0.5);
      break;
    }
    case 'breathe': {
      const c = params.color ?? [0, 120, 255];
      const period = Math.max(0.3, 4 - 3 * (speed / 100)); // 4s slow .. ~1s fast
      const v = 0.12 + 0.88 * (0.5 + 0.5 * Math.sin((2 * Math.PI * t) / period));
      for (let i = 0; i < n; i++) {
        out[i] = [Math.round(c[0] * v), Math.round(c[1] * v), Math.round(c[2] * v)];
      }
      break;
    }
    case 'gradient': {
      const stops: Array<[number, number, number]> =
        params.colors && params.colors.length > 0 ? params.colors : [[255, 0, 0], [0, 0, 255]];
      for (let i = 0; i < n; i++) {
        const x = n === 1 ? 0 : (i / (n - 1)) * (stops.length - 1);
        const i0 = Math.floor(x);
        const i1 = Math.min(stops.length - 1, i0 + 1);
        out[i] = mix(stops[i0], stops[i1], x - i0);
      }
      break;
    }
    // The 5 new kinds don't map to a 1-D strip; approximate them there so
    // device-card strips still hint at what's running.
    case 'ripple':
    case 'reactive': {
      const c: [number, number, number] = params.color ?? [124, 92, 255];
      const v = 0.3 + 0.7 * Math.abs(Math.sin(t * (speed / 100) * 3));
      for (let i = 0; i < n; i++) out[i] = [Math.round(c[0] * v), Math.round(c[1] * v), Math.round(c[2] * v)];
      break;
    }
    case 'starlight': {
      const colors: Array<[number, number, number]> =
        params.colors && params.colors.length > 0 ? params.colors : [[255, 255, 255], [124, 92, 255]];
      for (let i = 0; i < n; i++) {
        const ph = hash01(i * 7 + 3);
        const tw = Math.max(0, Math.sin((t * (0.4 + (speed / 100) * 2.2) + ph) * Math.PI * 2));
        out[i] = mix([4, 5, 10], colors[i % colors.length], tw * tw);
      }
      break;
    }
    case 'fire': {
      for (let i = 0; i < n; i++) {
        const flick = 0.5 + 0.5 * Math.sin(t * (2 + speed / 12) + hash01(i) * 6.28);
        out[i] = mix([25, 4, 0], [255, 140 + Math.round(40 * flick), 20], flick * (0.35 + 0.65 * (i / Math.max(1, n - 1))));
      }
      break;
    }
    case 'audio_meter': {
      const c: [number, number, number] = params.color ?? [0, 224, 124];
      const level = 0.5 + 0.5 * Math.sin(t * 5); // placeholder pulse when no meter
      const lit = Math.round(level * n);
      for (let i = 0; i < n; i++) out[i] = i < lit ? c : [10, 14, 20];
      break;
    }
    default:
      for (let i = 0; i < n; i++) out[i] = [40, 40, 40];
  }
  return out;
}

// ---------------------------------------------------------------------------
// Matrix frames: the studio canvas renders a rows×cols grid per layer, then
// composites the stack like the daemon engine does.
// ---------------------------------------------------------------------------

/** Fire heat palette: black → deep red → orange → yellow. */
function heat(v: number): [number, number, number] {
  const r = Math.min(255, Math.round(v * 3 * 255));
  const g = Math.min(255, Math.round(Math.max(0, v * 3 - 1) * 255));
  const b = Math.min(255, Math.round(Math.max(0, v * 3 - 2) * 255 * 0.7));
  return [r, g, b];
}

/** Draw one layer into a rows×cols grid. x = col fraction, y = row fraction. */
function drawMatrixLayer(
  effect: EffectKind,
  params: EffectParams,
  rows: number,
  cols: number,
  t: number,
  rt: PreviewRuntime,
): RGB[][] {
  const grid: RGB[][] = Array.from({ length: rows }, () => Array.from({ length: cols }, () => [0, 0, 0] as RGB));
  const speed = params.speed ?? 50;
  switch (effect) {
    case 'static': {
      const col = params.color ?? [0, 255, 0];
      for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) grid[r][c] = col;
      break;
    }
    case 'wave': {
      const dir = params.direction === 'left' ? 1 : -1;
      const phase = t * (speed / 100) * 1.5;
      for (let r = 0; r < rows; r++)
        for (let c = 0; c < cols; c++)
          grid[r][c] = hsl((((dir * c) / cols + phase) % 1 + 1) % 1, 1, 0.5);
      break;
    }
    case 'spectrum': {
      const phase = t * (speed / 100) * 0.5;
      for (let r = 0; r < rows; r++)
        for (let c = 0; c < cols; c++)
          grid[r][c] = hsl((phase + (c / cols) * 0.15 + (r / rows) * 0.1) % 1, 1, 0.5);
      break;
    }
    case 'breathe': {
      const col = params.color ?? [0, 120, 255];
      const period = Math.max(0.3, 4 - 3 * (speed / 100));
      const v = 0.12 + 0.88 * (0.5 + 0.5 * Math.sin((2 * Math.PI * t) / period));
      for (let r = 0; r < rows; r++)
        for (let c = 0; c < cols; c++)
          grid[r][c] = [Math.round(col[0] * v), Math.round(col[1] * v), Math.round(col[2] * v)];
      break;
    }
    case 'gradient': {
      const stops: Array<[number, number, number]> =
        params.colors && params.colors.length > 0 ? params.colors : [[255, 0, 0], [0, 0, 255]];
      for (let r = 0; r < rows; r++)
        for (let c = 0; c < cols; c++) {
          const x = cols === 1 ? 0 : (c / (cols - 1)) * (stops.length - 1);
          const i0 = Math.floor(x);
          const i1 = Math.min(stops.length - 1, i0 + 1);
          grid[r][c] = mix(stops[i0], stops[i1], x - i0);
        }
      break;
    }
    case 'ripple': {
      const color: [number, number, number] = params.color ?? [124, 92, 255];
      const maxR = Math.hypot(1, (rows / Math.max(1, cols)) || 1); // grid diag in col units
      for (const ev of rt.events) {
        const age = t - ev.t0;
        if (age < 0 || age > 3) continue;
        const radius = age * (0.4 + (speed / 100) * 1.6); // cols/sec
        const fade = Math.max(0, 1 - age / 3);
        const ring = 0.16; // ring thickness in col units
        for (let r = 0; r < rows; r++)
          for (let c = 0; c < cols; c++) {
            const x = (c + 0.5) / cols - ev.x;
            const y = ((r + 0.5) / rows - ev.y) * (rows / Math.max(1, cols));
            const d = Math.hypot(x, y);
            const band = Math.max(0, 1 - Math.abs(d - radius) / ring);
            if (band <= 0) continue;
            const a = band * fade;
            grid[r][c] = [
              Math.min(255, grid[r][c][0] + Math.round(color[0] * a)),
              Math.min(255, grid[r][c][1] + Math.round(color[1] * a)),
              Math.min(255, grid[r][c][2] + Math.round(color[2] * a)),
            ];
          }
      }
      break;
    }
    case 'reactive': {
      const color: [number, number, number] = params.color ?? [0, 224, 124];
      const fadeSec = Math.max(0.3, (100 - (params.fade ?? 50)) / 100 * 2.4 + 0.3); // fade param: 0=fast .. 100=slow-ish
      for (const ev of rt.events) {
        const age = t - ev.t0;
        if (age < 0 || age > fadeSec) continue;
        const decay = 1 - age / fadeSec;
        for (let r = 0; r < rows; r++)
          for (let c = 0; c < cols; c++) {
            const x = (c + 0.5) / cols - ev.x;
            const y = ((r + 0.5) / rows - ev.y) * (rows / Math.max(1, cols));
            const d = Math.hypot(x, y);
            const a = Math.max(0, 1 - d * 6) * decay; // small halo around the cell
            if (a <= 0) continue;
            grid[r][c] = [
              Math.min(255, grid[r][c][0] + Math.round(color[0] * a)),
              Math.min(255, grid[r][c][1] + Math.round(color[1] * a)),
              Math.min(255, grid[r][c][2] + Math.round(color[2] * a)),
            ];
          }
      }
      break;
    }
    case 'starlight': {
      const colors: Array<[number, number, number]> =
        params.colors && params.colors.length > 0 ? params.colors : [[255, 255, 255], [124, 92, 255], [0, 224, 124]];
      const rate = 0.25 + (speed / 100) * 1.6;
      for (let r = 0; r < rows; r++)
        for (let c = 0; c < cols; c++) {
          const seed = hash01(r * 31 + c * 7 + 13);
          const phase = hash01(c * 11 + r * 3 + 101);
          const tw = Math.max(0, Math.sin((t * rate + phase) * Math.PI * (1 + seed))); // sparse twinkle
          const a = tw * tw * (0.4 + 0.6 * seed);
          if (a < 0.02) continue;
          const col = colors[Math.floor(seed * colors.length) % colors.length];
          grid[r][c] = mix([0, 0, 0], col, a);
        }
      break;
    }
    case 'fire': {
      const scroll = t * (0.3 + (speed / 100) * 1.2); // rows/sec
      for (let r = 0; r < rows; r++) {
        const height = (r + 0.5) / rows; // 1 at bottom, 0 at top
        for (let c = 0; c < cols; c++) {
          const noise = hash01(c * 17 + Math.floor(r * 3 - scroll * 3) * 5 + Math.floor(t * 6) * 29);
          const flick = Math.max(0, height - 0.12 * noise * (1 - height));
          grid[r][c] = heat(Math.min(1, flick * 1.3));
        }
      }
      break;
    }
    case 'audio_meter': {
      const base: [number, number, number] = params.color ?? [0, 224, 124];
      const bands = Math.max(1, Math.min(cols, params.bands ?? 12));
      const level = (rt.level ?? 0) / 1000; // 0-1, 0 when audio off
      const bandW = cols / bands;
      for (let b = 0; b < bands; b++) {
        // per-band jitter so bars don't move in lockstep (approximation)
        const jitter = 0.75 + 0.25 * Math.sin(t * (2 + b * 0.7) + b * 2.4);
        const h = level * jitter;
        const litRows = Math.round(h * rows);
        for (let r = 0; r < rows; r++) {
          const fromBottom = rows - 1 - r;
          const c0 = Math.floor(b * bandW);
          const c1 = Math.floor((b + 1) * bandW);
          for (let c = c0; c < Math.max(c0 + 1, c1); c++) {
            if (fromBottom < litRows) {
              // hue shift red at top like a real VU meter
              const hot = fromBottom / Math.max(1, rows - 1);
              grid[r][c] = hot > 0.8 ? mix(base, [255, 60, 60], (hot - 0.8) * 5) : base;
            }
          }
        }
      }
      break;
    }
    default:
      break;
  }
  return grid;
}

function blendCell(dst: RGB, src: RGB, opacity: number, blend: 'normal' | 'add'): RGB {
  const a = Math.max(0, Math.min(100, opacity)) / 100;
  if (a <= 0) return dst;
  if (blend === 'add') {
    return [
      Math.min(255, dst[0] + Math.round(src[0] * a)),
      Math.min(255, dst[1] + Math.round(src[1] * a)),
      Math.min(255, dst[2] + Math.round(src[2] * a)),
    ];
  }
  return [
    Math.round(dst[0] + (src[0] - dst[0]) * a),
    Math.round(dst[1] + (src[1] - dst[1]) * a),
    Math.round(dst[2] + (src[2] - dst[2]) * a),
  ];
}

/** Composite a whole layer stack onto a rows×cols frame. Stack is TOP-FIRST:
 *  bottom layer paints first, each higher layer blends over it. */
export function drawMatrixFrame(
  stack: LayerSpec[],
  rows: number,
  cols: number,
  t: number,
  rt: PreviewRuntime = EMPTY_RUNTIME,
): RGB[][] {
  let frame: RGB[][] = Array.from({ length: rows }, () => Array.from({ length: cols }, () => [0, 0, 0] as RGB));
  for (let i = stack.length - 1; i >= 0; i--) {
    const layer = stack[i];
    if (!layer.enabled || layer.opacity <= 0) continue;
    const layerGrid = drawMatrixLayer(layer.effect, layer.params, rows, cols, t, rt);
    for (let r = 0; r < rows; r++)
      for (let c = 0; c < cols; c++) frame[r][c] = blendCell(frame[r][c], layerGrid[r][c], layer.opacity, layer.blend);
  }
  return frame;
}
