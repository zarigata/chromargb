import { useEffect, useRef, useState } from 'react';
import { hexToRgb } from '../color';
import type { Device, DeviceZone, RGB } from '../types';

interface Props {
  device: Device;
  zone: DeviceZone;
  /** Currently applied matrix from get_state (row-major), if any. */
  initial?: RGB[];
  onClose(): void;
  onApply(colors: Array<[number, number, number]>): void;
}

const OFF: RGB = [12, 14, 22];

function toGrid(rows: number, cols: number, flat?: RGB[] | null): RGB[][] {
  if (!flat || flat.length !== rows * cols) {
    return Array.from({ length: rows }, () => Array.from({ length: cols }, () => OFF));
  }
  return Array.from({ length: rows }, (_, r) => flat.slice(r * cols, (r + 1) * cols));
}

/**
 * Per-key matrix painter. Click/drag paints cells with the selected color;
 * eraser mode resets cells; Apply flattens row-major into the per-LED array
 * for set_matrix. Opens with the device's currently applied matrix.
 */
export function MatrixEditor({ device, zone, initial, onClose, onApply }: Props) {
  const rows = zone.matrix?.[0] ?? 1;
  const cols = zone.matrix?.[1] ?? zone.leds;
  const [brush, setBrush] = useState('#ff0044');
  const [erase, setErase] = useState(false);
  const [painted, setPainted] = useState<RGB[][]>(() => toGrid(rows, cols, initial));
  const painting = useRef(false);
  const lastCell = useRef<string>('');

  // esc closes
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [onClose]);

  const paint = (r: number, c: number) => {
    const key = `${r}:${c}`;
    if (painting.current && key === lastCell.current) return;
    lastCell.current = key;
    setPainted((prev) => {
      const next = prev.map((row) => [...row]);
      next[r][c] = erase ? OFF : hexToRgb(brush);
      return next;
    });
  };

  const apply = () => onApply(painted.flat());

  const fillAll = () => setPainted((prev) => prev.map((row) => row.map(() => (erase ? OFF : hexToRgb(brush)))));

  const cellStyle = (r: number, c: number): React.CSSProperties => {
    const [cr, cg, cb] = painted[r][c];
    return { background: `rgb(${cr},${cg},${cb})`, boxShadow: `0 0 6px rgba(${cr},${cg},${cb},0.5)` };
  };

  return (
    <div className="drawer-backdrop" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="drawer matrix" role="dialog" aria-label={`Matrix editor — ${device.name} ${zone.name}`}>
        <div className="drawer-head">
          <h2>Matrix studio</h2>
          <span className="card-sub">
            {device.name} · {zone.name} ({rows}×{cols}){initial ? ' · loaded current' : ''}
          </span>
          <button className="ghost" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>

        <div className="row tools">
          <label className="btn color-btn" style={{ ['--swatch' as string]: brush } as React.CSSProperties}>
            <input type="color" value={brush} onChange={(e) => setBrush(e.target.value)} aria-label="Brush color" />
            Brush
          </label>
          <button className={erase ? 'small on-tool' : 'ghost small'} onClick={() => setErase((e) => !e)} title="Erase mode: resets cells to off">
            eraser{erase ? ' (on)' : ''}
          </button>
          <button className="ghost small" onClick={fillAll}>
            fill all
          </button>
          <button
            className="ghost small"
            onClick={() => setPainted(toGrid(rows, cols, initial))}
          >
            reset
          </button>
          <span className="dim">drag to paint · {rows * cols} LEDs total</span>
        </div>

        <div
          className="matrix-grid"
          style={{ gridTemplateColumns: `repeat(${cols}, minmax(14px, 1fr))` }}
          onPointerDown={() => (painting.current = true)}
          onPointerUp={() => (painting.current = false)}
          onPointerLeave={() => (painting.current = false)}
        >
          {painted.map((row, r) =>
            row.map((_, c) => (
              <div
                key={`${r}-${c}`}
                className="key"
                style={cellStyle(r, c)}
                onPointerDown={() => paint(r, c)}
                onPointerEnter={() => paint(r, c)}
                role="button"
                aria-label={`LED row ${r + 1} col ${c + 1}`}
              />
            )),
          )}
        </div>

        <div className="drawer-actions">
          <span className="dim">Apply sends set_matrix. Use Game profiles {'>'} Capture current to save this layout per game.</span>
          <button className="primary" onClick={apply}>
            Apply
          </button>
        </div>
      </div>
    </div>
  );
}
