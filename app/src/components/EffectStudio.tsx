import { useEffect, useState } from 'react';
import { hexToRgb, rgbToHex } from '../color';
import { cloneParams, defaultParams } from '../effectParams';
import { EFFECT_KINDS, type ActiveEffect, type Device, type EffectKind, type EffectParams, type RGB } from '../types';
import { StripPreview } from './StripPreview';

interface Props {
  device: Device;
  initial?: ActiveEffect;
  onClose(): void;
  onApply(effect: EffectKind, params: EffectParams): void;
}

const isKind = (s: string): s is EffectKind => (EFFECT_KINDS as string[]).includes(s);

/** The drawer stays the legacy quick-picker: the 5 original kinds. The full
 *  10-kind editor (incl. ripple/fire/starlight/audio) is the Studio page. */
const DRAWER_KINDS: EffectKind[] = ['static', 'wave', 'spectrum', 'breathe', 'gradient'];

export function EffectStudio({ device, initial, onClose, onApply }: Props) {
  const [effect, setEffect] = useState<EffectKind>(initial && isKind(initial.effect) ? initial.effect : 'static');
  const [params, setParams] = useState<EffectParams>(
    initial && isKind(initial.effect) ? cloneParams(initial.params) : defaultParams('static'),
  );

  // esc closes
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [onClose]);

  const pick = (k: EffectKind) => {
    setEffect(k);
    setParams(defaultParams(k));
  };

  const set = (patch: Partial<EffectParams>) => setParams((p) => ({ ...p, ...patch }));
  const setColorAt = (i: number, rgb: RGB) => {
    const colors = [...(params.colors ?? [])];
    colors[i] = rgb;
    set({ colors });
  };

  // gradient stop management
  const addStop = () => params.colors && params.colors.length < 7 && set({ colors: [...params.colors, [255, 255, 255]] });
  const delStop = (i: number) => {
    const colors = (params.colors ?? []).filter((_, j) => j !== i);
    if (colors.length >= 2) set({ colors });
  };

  const apply = () => onApply(effect, params);

  return (
    <div className="drawer-backdrop" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <aside className="drawer" role="dialog" aria-label={`Effect studio — ${device.name}`}>
        <div className="drawer-head">
          <h2>Effect studio</h2>
          <span className="card-sub">{device.name}</span>
          <button className="ghost" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>

        <div className="effects-row" role="tablist">
          {DRAWER_KINDS.map((k) => (
            <button key={k} role="tab" aria-selected={k === effect} className={k === effect ? 'tab on' : 'tab'} onClick={() => pick(k)}>
              {k}
            </button>
          ))}
        </div>

        <div className="preview-wrap">
          <StripPreview effect={effect} params={params} leds={32} height={44} />
          <span className="dim preview-label">client-side approximation</span>
        </div>

        <div className="params">
          {effect === 'static' && (
            <ParamRow label="Color">
              <ColorSwatch value={params.color ?? [0, 255, 0]} onChange={(c) => set({ color: c })} />
            </ParamRow>
          )}

          {effect === 'wave' && (
            <>
              <ParamRow label={`Speed — ${params.speed ?? 60}`}>
                <RangeInput value={params.speed ?? 60} onChange={(v) => set({ speed: v })} />
              </ParamRow>
              <ParamRow label="Direction">
                <ToggleGroup
                  options={['left', 'right'] as const}
                  value={params.direction ?? 'right'}
                  onChange={(d) => set({ direction: d })}
                />
              </ParamRow>
            </>
          )}

          {effect === 'spectrum' && (
            <ParamRow label={`Speed — ${params.speed ?? 50}`}>
              <RangeInput value={params.speed ?? 50} onChange={(v) => set({ speed: v })} />
            </ParamRow>
          )}

          {effect === 'breathe' && (
            <>
              <ParamRow label={`Speed — ${params.speed ?? 40}`}>
                <RangeInput value={params.speed ?? 40} onChange={(v) => set({ speed: v })} />
              </ParamRow>
              <ParamRow label="Color (optional)">
                <div className="row">
                  <ColorSwatch value={params.color ?? [0, 120, 255]} onChange={(c) => set({ color: c })} />
                  <button className="ghost small" onClick={() => set({ color: undefined })}>
                    rainbow
                  </button>
                </div>
              </ParamRow>
            </>
          )}

          {effect === 'gradient' && (
            <ParamRow label={`Stops — ${(params.colors ?? []).length}`}>
              <div className="stops">
                {(params.colors ?? []).map((c, i) => (
                  <div className="row" key={i}>
                    <ColorSwatch value={c} onChange={(rgb) => setColorAt(i, rgb)} />
                    <span className="dim hex">{rgbToHex(c)}</span>
                    <button className="ghost small" onClick={() => delStop(i)} aria-label={`Remove stop ${i + 1}`} disabled={(params.colors ?? []).length <= 2}>
                      ✕
                    </button>
                  </div>
                ))}
                <button className="ghost small" onClick={addStop} disabled={(params.colors ?? []).length >= 7}>
                  + add stop
                </button>
              </div>
            </ParamRow>
          )}
        </div>

        <div className="drawer-actions">
          <span className="dim">
            {device.zones.map((z) => z.name).join(', ')}
          </span>
          <button className="primary" onClick={apply}>
            Apply
          </button>
        </div>
      </aside>
    </div>
  );
}

function ParamRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="param-row">
      <label>{label}</label>
      {children}
    </div>
  );
}

function RangeInput({ value, onChange }: { value: number; onChange: (v: number) => void }) {
  return <input type="range" min={0} max={100} value={value} onChange={(e) => onChange(Number(e.target.value))} />;
}

function ToggleGroup({ options, value, onChange }: { options: readonly ('left' | 'right')[]; value: 'left' | 'right'; onChange: (v: 'left' | 'right') => void }) {
  return (
    <div className="toggle-group">
      {options.map((o) => (
        <button key={o} className={o === value ? 'tab on' : 'tab'} onClick={() => onChange(o)} aria-pressed={o === value}>
          {o}
        </button>
      ))}
    </div>
  );
}

function ColorSwatch({ value, onChange }: { value: RGB; onChange: (c: RGB) => void }) {
  const hex = rgbToHex(value);
  return (
    <span className="swatch" style={{ background: hex }}>
      <input type="color" value={hex} onChange={(e) => onChange(hexToRgb(e.target.value))} aria-label="Pick color" />
    </span>
  );
}
