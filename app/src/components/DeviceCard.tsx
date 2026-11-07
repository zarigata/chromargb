import { useState } from 'react';
import { hexToRgb } from '../color';
import { supportsPerLed, type ActiveEffect, type Device, type EffectKind, type RGB } from '../types';
import { StripPreview } from './StripPreview';

interface Props {
  device: Device;
  active?: ActiveEffect;
  mode?: 'effect' | 'static' | 'matrix' | 'idle';
  onStatic(color: RGB): void;
  onStop(): void;
  onStudio(): void;
  onMatrix(zoneIndex: number): void;
  onRetry(): void;
}

const IDLE_PARAMS = { color: [28, 32, 44] as RGB };

function summarize(a: ActiveEffect | undefined, mode?: string): string {
  if (mode === 'matrix') return 'custom matrix';
  if (!a) return mode === 'static' ? 'static' : 'idle';
  switch (a.effect) {
    case 'static': return 'static';
    case 'wave': return `wave ${a.params.direction ?? ''} ${a.params.speed ?? ''}`.trim();
    case 'spectrum': return `spectrum ${a.params.speed ?? ''}`.trim();
    case 'breathe': return `breathe ${a.params.speed ?? ''}`.trim();
    case 'gradient': return `gradient (${a.params.colors?.length ?? 0} stops)`;
    case 'ripple': return 'ripple';
    case 'reactive': return 'reactive';
    case 'starlight': return 'starlight';
    case 'fire': return 'fire';
    case 'audio_meter': return 'audio meter';
    default: return a.effect; // unknown/future effect from daemon — show raw name
  }
}

function unavailHelp(device: Device): { title: string; body: string } {
  if (device.errorKind === 'permission') {
    return {
      title: 'macOS is blocking access',
      body: 'Grant Input Monitoring to ChromaRGB (or the terminal running the daemon): System Settings > Privacy & Security > Input Monitoring — then press Retry.',
    };
  }
  if (device.errorKind === 'busy') {
    return {
      title: 'Held by another app',
      body: 'Another RGB app (Synapse, G HUB, OpenRGB…) has the device. Close it, then press Retry.',
    };
  }
  return {
    title: 'Could not open device',
    body: device.openError ?? device.note ?? 'The daemon could not open this device.',
  };
}

export function DeviceCard({ device, active, mode, onStatic, onStop, onStudio, onMatrix, onRetry }: Props) {
  const [color, setColor] = useState('#00ff44');
  const matrixZone = device.zones.findIndex((z) => z.matrix != null);
  const previewEffect = (active?.effect as EffectKind) || 'static';
  const off = !device.available;
  const help = off ? unavailHelp(device) : null;
  return (
    <div className={off ? 'card off' : 'card'}>
      <div className="card-head">
        <div>
          <div className="card-name">
            {device.name} <span className="dim kind">{device.kind.toLowerCase()}</span>
          </div>
          <div className="card-sub">
            {device.vendor} · vid 0x{device.vid.toString(16)} pid 0x{device.pid.toString(16)}
          </div>
        </div>
        <span className={off ? 'badge unavail' : active || mode === 'matrix' ? 'badge live' : 'badge'} title={off ? help?.body : undefined}>
          {off ? 'unavailable' : summarize(active, mode)}
        </span>
      </div>

      {off && help && (
        <div className="card-note">
          <strong>{help.title}.</strong> {help.body}
          <button className="btn retry" onClick={onRetry}>Retry</button>
        </div>
      )}

      <StripPreview effect={previewEffect} params={active?.params ?? IDLE_PARAMS} />

      <div className="chips">
        {device.zones.map((z) => (
          <span key={z.name} className="chip" title={z.matrix ? `${z.matrix[0]}×${z.matrix[1]} matrix` : `${z.leds} LEDs`}>
            {z.name} · {z.leds}
          </span>
        ))}
        {supportsPerLed(device) && <span className="chip perled">per-LED</span>}
      </div>

      <div className="card-actions">
        <label className="btn color-btn" title="Set a static color" style={{ ['--swatch' as string]: color } as React.CSSProperties}>
          <input
            type="color"
            value={color}
            disabled={off}
            onChange={(e) => setColor(e.target.value)}
            onPointerUp={() => !off && onStatic(hexToRgb(color))}
            onBlur={() => !off && onStatic(hexToRgb(color))}
          />
          Static
        </label>
        <button onClick={onStudio} disabled={off}>Effect…</button>
        {matrixZone >= 0 && (
          <button onClick={() => onMatrix(matrixZone)} disabled={off}>Edit matrix</button>
        )}
        <button className="ghost" onClick={onStop} disabled={off || !active} title="Freeze the current frame (stop_effect)">
          Stop
        </button>
      </div>
    </div>
  );
}
