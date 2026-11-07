import { hexToRgb, rgbToHex } from '../color';
import { EFFECT_KINDS, type EffectKind, type EffectParams, type LayerSpec, type RGB } from '../types';

interface Props {
  layer: LayerSpec;
  /** Patch the selected layer (effect/params/opacity/blend). */
  onChange(patch: Partial<LayerSpec>): void;
}

/** Right-hand params panel for the selected layer: effect kind + typed params.
 *  Opacity/blend live here too, synced with the layer-list row. */
export function Inspector({ layer, onChange }: Props) {
  const setParams = (patch: Partial<EffectParams>) => onChange({ params: { ...layer.params, ...patch } });
  const setColorAt = (i: number, rgb: RGB) => {
    const colors = [...(layer.params.colors ?? [])];
    colors[i] = rgb;
    setParams({ colors });
  };

  return (
    <div className="inspector">
      <div className="param-row">
        <label>Effect</label>
        <select value={layer.effect} onChange={(e) => onChange({ effect: e.target.value as EffectKind })} aria-label="Effect kind">
          {EFFECT_KINDS.map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
      </div>

      {layer.effect === 'static' && (
        <ParamRow label="Color">
          <ColorSwatch value={layer.params.color ?? [0, 255, 68]} onChange={(c) => setParams({ color: c })} />
        </ParamRow>
      )}

      {(layer.effect === 'wave' || layer.effect === 'spectrum' || layer.effect === 'breathe' || layer.effect === 'ripple' || layer.effect === 'starlight' || layer.effect === 'fire') && (
        <ParamRow label={`Speed — ${layer.params.speed ?? 50}`}>
          <RangeInput value={layer.params.speed ?? 50} onChange={(v) => setParams({ speed: v })} />
        </ParamRow>
      )}

      {layer.effect === 'wave' && (
        <ParamRow label="Direction">
          <ToggleGroup
            options={['left', 'right'] as const}
            value={layer.params.direction ?? 'right'}
            onChange={(d) => setParams({ direction: d })}
          />
        </ParamRow>
      )}

      {layer.effect === 'breathe' && (
        <ParamRow label="Color (optional)">
          <div className="row">
            <ColorSwatch value={layer.params.color ?? [0, 120, 255]} onChange={(c) => setParams({ color: c })} />
            <button className="ghost small" onClick={() => setParams({ color: undefined })}>
              rainbow
            </button>
          </div>
        </ParamRow>
      )}

      {layer.effect === 'gradient' && (
        <ParamRow label={`Stops — ${(layer.params.colors ?? []).length}`}>
          <div className="stops">
            {(layer.params.colors ?? [[255, 0, 68], [0, 86, 255]]).map((c, i) => (
              <div className="row" key={i}>
                <ColorSwatch value={c} onChange={(rgb) => setColorAt(i, rgb)} />
                <span className="dim hex">{rgbToHex(c)}</span>
                <button
                  className="ghost small"
                  onClick={() => {
                    const colors = (layer.params.colors ?? []).filter((_, j) => j !== i);
                    if (colors.length >= 2) setParams({ colors });
                  }}
                  aria-label={`Remove stop ${i + 1}`}
                  disabled={(layer.params.colors ?? []).length <= 2}
                >
                  ✕
                </button>
              </div>
            ))}
            <button
              className="ghost small"
              onClick={() => layer.params.colors && layer.params.colors.length < 7 && setParams({ colors: [...layer.params.colors, [255, 255, 255]] })}
              disabled={(layer.params.colors ?? []).length >= 7}
            >
              + add stop
            </button>
          </div>
        </ParamRow>
      )}

      {(layer.effect === 'ripple' || layer.effect === 'reactive') && (
        <ParamRow label="Color">
          <ColorSwatch value={layer.params.color ?? [124, 92, 255]} onChange={(c) => setParams({ color: c })} />
        </ParamRow>
      )}

      {layer.effect === 'reactive' && (
        <ParamRow label={`Fade — ${layer.params.fade ?? 50}`}>
          <RangeInput value={layer.params.fade ?? 50} onChange={(v) => setParams({ fade: v })} />
        </ParamRow>
      )}

      {layer.effect === 'starlight' && (
        <ParamRow label={`Twinkle colors — ${(layer.params.colors ?? []).length}`}>
          <div className="stops">
            {(layer.params.colors ?? [[255, 255, 255], [124, 92, 255]]).map((c, i) => (
              <div className="row" key={i}>
                <ColorSwatch value={c} onChange={(rgb) => setColorAt(i, rgb)} />
                <span className="dim hex">{rgbToHex(c)}</span>
                <button
                  className="ghost small"
                  onClick={() => {
                    const colors = (layer.params.colors ?? []).filter((_, j) => j !== i);
                    if (colors.length >= 2) setParams({ colors });
                  }}
                  aria-label={`Remove color ${i + 1}`}
                  disabled={(layer.params.colors ?? []).length <= 2}
                >
                  ✕
                </button>
              </div>
            ))}
            <button
              className="ghost small"
              onClick={() => layer.params.colors && layer.params.colors.length < 7 && setParams({ colors: [...layer.params.colors, [255, 255, 255]] })}
              disabled={(layer.params.colors ?? []).length >= 7}
            >
              + add color
            </button>
          </div>
        </ParamRow>
      )}

      {layer.effect === 'audio_meter' && (
        <>
          <ParamRow label="Color (optional)">
            <div className="row">
              <ColorSwatch value={layer.params.color ?? [0, 224, 124]} onChange={(c) => setParams({ color: c })} />
              <button className="ghost small" onClick={() => setParams({ color: undefined })}>
                auto
              </button>
            </div>
          </ParamRow>
          <ParamRow label={`Bands — ${layer.params.bands ?? 12}`}>
            <RangeInput min={1} max={22} value={layer.params.bands ?? 12} onChange={(v) => setParams({ bands: v })} />
          </ParamRow>
        </>
      )}

      <div className="insp-divider" />
      <ParamRow label={`Opacity — ${layer.opacity}%`}>
        <RangeInput value={layer.opacity} onChange={(v) => onChange({ opacity: v })} />
      </ParamRow>
      <ParamRow label="Blend">
        <ToggleGroup
          options={['normal', 'add'] as const}
          value={layer.blend}
          onChange={(b) => onChange({ blend: b })}
        />
      </ParamRow>
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

function RangeInput({ value, onChange, min = 0, max = 100 }: { value: number; onChange: (v: number) => void; min?: number; max?: number }) {
  return <input type="range" min={min} max={max} value={value} onChange={(e) => onChange(Number(e.target.value))} />;
}

function ToggleGroup<T extends string>({ options, value, onChange }: { options: readonly T[]; value: T; onChange: (v: T) => void }) {
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
