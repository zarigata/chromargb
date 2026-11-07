import { useEffect, useMemo, useRef } from 'react';
import { drawMatrixFrame, type PreviewRuntime } from '../effectPreview';
import { ChromaClient } from '../ipc';
import { PHYSICAL_LAYOUT } from '../keyboardLayout';
import type { LayerSpec, RGB } from '../types';

interface Props {
  client: ChromaClient;
  device: string;
  layers: LayerSpec[];
  rows: number;
  cols: number;
  disabled?: boolean;
  /** Fires when a cell is clicked (row, col). */
  onCellClick?(row: number, col: number): void;
  onKeyHeatmap?(count: number): void;
}

/** Layout key for a device: cell -> physical rect. Devices other than the
 *  full ANSI 6x22 fall back to a uniform grid. */
const CELL_RECTS: Map<string, { x: number; y: number; w: number; h: number; label: string }> = (() => {
  const m = new Map<string, { x: number; y: number; w: number; h: number; label: string }>();
  for (const [row, col, x, w, label] of PHYSICAL_LAYOUT) {
    m.set(`${row}:${col}`, { x, y: row, w, h: 1, label });
  }
  // numpad + and Enter span two rows (rendered tall; single matrix cell each)
  const plus = m.get('2:21');
  if (plus) plus.h = 2;
  return m;
})();
const LAYOUT_UNITS_W = 22.75; // widest x + w in the table
const LAYOUT_UNITS_H = 6;

/**
 * The studio's live canvas: renders the layer stack onto the device's real
 * matrix grid client-side (approximation of the daemon engine) via rAF.
 * 6x22 matrix devices draw a physical ANSI key layout (staggered, real key
 * widths); other grids fall back to uniform cells. Key events from the
 * daemon (ws push) spawn ripple origins locally; clicking a key sends a
 * trigger{} to the daemon and ripples locally too.
 */
export function LiveCanvas({ client, device, layers, rows, cols, disabled, onCellClick, onKeyHeatmap }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stackRef = useRef(layers);
  stackRef.current = layers; // live stack without restarting the clock
  const runtime = useRef<PreviewRuntime>({ events: [], level: 0 });
  const keyCount = useRef(0);
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;

  const dims = useMemo(() => ({ rows, cols }), [rows, cols]);
  const useLayout = rows === 6 && cols === 22;

  // push events: key → local ripple origin + heatmap; audio → VU level
  useEffect(() => {
    const off = client.onEvent((ev) => {
      if (ev.event === 'audio') {
        runtime.current.level = ev.level;
        return;
      }
      if (ev.event === 'key' && ev.device === device) {
        runtime.current.events.push({ x: (ev.col + 0.5) / dims.cols, y: (ev.row + 0.5) / dims.rows, t0: performance.now() / 1000 });
        keyCount.current += 1;
        onKeyHeatmap?.(keyCount.current);
      }
    });
    return off;
  }, [client, device, dims, onKeyHeatmap]);

  // animation clock
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    let raf = 0;

    const render = () => {
      const t = performance.now() / 1000;
      // prune old events (ripple/reactive decay out by ~3s)
      const ev = runtime.current.events;
      if (ev.length > 24) runtime.current.events = ev.slice(ev.length - 24);
      runtime.current.events = runtime.current.events.filter((e) => t - e.t0 < 3.2);

      const frame: RGB[][] = drawMatrixFrame(stackRef.current, dims.rows, dims.cols, t, runtime.current);

      // size canvas to element (dpr-aware)
      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(h * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);

      // geometry: physical layout (u units) or uniform grid
      const pad = 10;
      const gap = Math.max(2, Math.min(4, w / 200));
      let cellAt: (r: number, c: number) => { x: number; y: number; w: number; h: number } | null;
      if (useLayout) {
        const availW = w - pad * 2;
        const availH = h - pad * 2;
        const u = Math.min(availW / LAYOUT_UNITS_W, availH / LAYOUT_UNITS_H);
        const ox = (w - LAYOUT_UNITS_W * u) / 2;
        const oy = (h - LAYOUT_UNITS_H * u) / 2;
        cellAt = (r, c) => {
          const rect = CELL_RECTS.get(`${r}:${c}`);
          if (!rect) return null;
          return { x: ox + rect.x * u, y: oy + rect.y * u, w: rect.w * u - gap, h: rect.h * u - gap };
        };
      } else {
        const cellW = (w - pad * 2 - gap * (dims.cols - 1)) / dims.cols;
        const cellH = Math.min((h - pad * 2 - gap * (dims.rows - 1)) / dims.rows, cellW * 0.82);
        const gridW = dims.cols * cellW + (dims.cols - 1) * gap;
        const gridH = dims.rows * cellH + (dims.rows - 1) * gap;
        const ox = (w - gridW) / 2;
        const oy = (h - gridH) / 2;
        cellAt = (r, c) => ({ x: ox + c * (cellW + gap), y: oy + r * (cellH + gap), w: cellW, h: cellH });
      }

      for (let r = 0; r < dims.rows; r++) {
        for (let c = 0; c < dims.cols; c++) {
          const rect = cellAt(r, c);
          if (!rect) continue;
          const [cr, cg, cb] = frame[r][c];
          const glow = (cr + cg + cb) / 765; // 0-1 brightness → glow strength
          if (glow > 0.04) {
            ctx.shadowColor = `rgb(${cr},${cg},${cb})`;
            ctx.shadowBlur = 3 + glow * 10;
          } else {
            ctx.shadowBlur = 0;
          }
          ctx.fillStyle = glow > 0.02 ? `rgb(${cr},${cg},${cb})` : '#141826';
          roundRect(ctx, rect.x, rect.y, rect.w, rect.h, Math.max(2.5, Math.min(7, rect.w * 0.18)));
          ctx.fill();
          // legend on wide keys when there's room (physical layout only)
          if (useLayout && rect.w > 26) {
            const label = CELL_RECTS.get(`${r}:${c}`)!.label;
            if (label.length <= 5) {
              ctx.shadowBlur = 0;
              ctx.fillStyle = glow > 0.45 ? 'rgba(0,0,0,0.72)' : 'rgba(255,255,255,0.38)';
              ctx.font = `${Math.max(8, Math.min(11, rect.w * 0.26))}px ui-sans-serif, system-ui`;
              ctx.textAlign = 'left';
              ctx.textBaseline = 'top';
              ctx.fillText(label, rect.x + 4, rect.y + 3);
            }
          }
        }
      }
      ctx.shadowBlur = 0;
      raf = requestAnimationFrame(render);
    };
    raf = requestAnimationFrame(render);
    return () => cancelAnimationFrame(raf);
  }, [dims, useLayout]);

  // hit-test: which matrix cell is under the pointer
  const cellFromPoint = (x: number, y: number, w: number, h: number): [number, number] | null => {
    if (useLayout) {
      const pad = 10;
      const u = Math.min((w - pad * 2) / LAYOUT_UNITS_W, (h - pad * 2) / LAYOUT_UNITS_H);
      const ox = (w - LAYOUT_UNITS_W * u) / 2;
      const oy = (h - LAYOUT_UNITS_H * u) / 2;
      for (const [key, rect] of CELL_RECTS) {
        if (x >= ox + rect.x * u && x <= ox + (rect.x + rect.w) * u && y >= oy + rect.y * u && y <= oy + (rect.y + rect.h) * u) {
          const [r, c] = key.split(':').map(Number);
          return [r, c];
        }
      }
      return null;
    }
    const pad = 10;
    const gap = Math.max(2, Math.min(4, w / 200));
    const cellW = (w - pad * 2 - gap * (dims.cols - 1)) / dims.cols;
    const cellH = Math.min((h - pad * 2 - gap * (dims.rows - 1)) / dims.rows, cellW * 0.82);
    const gridW = dims.cols * cellW + (dims.cols - 1) * gap;
    const gridH = dims.rows * cellH + (dims.rows - 1) * gap;
    const ox = (w - gridW) / 2;
    const oy = (h - gridH) / 2;
    const c = Math.floor((x - ox) / (cellW + gap));
    const r = Math.floor((y - oy) / (cellH + gap));
    if (r < 0 || r >= dims.rows || c < 0 || c >= dims.cols) return null;
    return [r, c];
  };

  const handleClick = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (disabledRef.current) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const hit = cellFromPoint(e.clientX - rect.left, e.clientY - rect.top, rect.width, rect.height);
    if (!hit) return;
    const [r, c] = hit;
    // local ripple + daemon trigger
    runtime.current.events.push({ x: (c + 0.5) / dims.cols, y: (r + 0.5) / dims.rows, t0: performance.now() / 1000 });
    keyCount.current += 1;
    onKeyHeatmap?.(keyCount.current);
    onCellClick?.(r, c);
  };

  return (
    <canvas
      ref={canvasRef}
      className="live-canvas"
      onClick={handleClick}
      role="img"
      aria-label={`Live ${dims.rows}×${dims.cols} keyboard preview — click a key to trigger a ripple`}
      title={disabled ? undefined : 'Click a key to trigger'}
    />
  );
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}
