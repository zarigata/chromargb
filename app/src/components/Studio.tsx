import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ChromaClient } from '../ipc';
import { cloneParams, makeLayer } from '../effectParams';
import { EFFECT_KINDS, type EffectKind, type LayerSpec, type ProfileDeviceEntry } from '../types';
import { Inspector } from './Inspector';
import { LiveCanvas } from './LiveCanvas';

interface Props {
  client: ChromaClient;
  devices: Array<{ id: string; name: string; available: boolean; zones: Array<{ name: string; matrix?: [number, number] }> }>;
  /** Currently active stack per device (from get_state layers) for seeding. */
  activeLayers: Record<string, LayerSpec[] | undefined>;
  /** Profile capture: push {kind:'layers'} for this device. */
  onSaveToProfile(deviceId: string, entry: ProfileDeviceEntry): void;
  onFlash(msg: string): void;
  status: 'connecting' | 'online' | 'offline';
}

/** Presets are data: one click = a full layer stack (top-first). */
const PRESETS: Array<{ name: string; stack: LayerSpec[] }> = [
  {
    name: 'Wave',
    stack: [makeLayer('wave')],
  },
  {
    name: 'Spectrum',
    stack: [makeLayer('spectrum')],
  },
  {
    name: 'Ripple Purple',
    stack: [makeLayer('ripple', { params: { color: [124, 92, 255], speed: 60 }, blend: 'normal' })],
  },
  {
    name: 'Fire',
    stack: [makeLayer('fire', { params: { speed: 60 }, opacity: 100, blend: 'normal' })],
  },
  {
    name: 'Starlight',
    stack: [makeLayer('starlight')],
  },
  {
    name: 'Audio Meter',
    stack: [makeLayer('audio_meter')],
  },
];

const APPLY_DEBOUNCE_MS = 500;

type ApplyState = 'idle' | 'pending' | 'applied' | 'error';

export function Studio({ client, devices, activeLayers, onSaveToProfile, onFlash, status }: Props) {
  const matrixDevices = useMemo(() => devices.filter((d) => d.zones.some((z) => z.matrix)), [devices]);
  const [deviceId, setDeviceId] = useState<string>('');
  const device = matrixDevices.find((d) => d.id === deviceId) ?? matrixDevices[0] ?? null;

  const [stack, setStack] = useState<LayerSpec[]>([]);
  const [selected, setSelected] = useState(0);
  const [applyLive, setApplyLive] = useState(false);
  const [applyState, setApplyState] = useState<ApplyState>('idle');
  const [audioOn, setAudioOn] = useState(false);
  const [keyHeat, setKeyHeat] = useState(0);
  const [savingName, setSavingName] = useState<string | null>(null);
  const [loadedFor, setLoadedFor] = useState<string>('');

  const deviceAvailable = !!device && device.available && status === 'online';

  // pick first matrix device on mount
  useEffect(() => {
    if (!deviceId && matrixDevices.length > 0) setDeviceId(matrixDevices[0].id);
  }, [matrixDevices, deviceId]);

  // seed the stack from get_state when switching devices (once per device)
  useEffect(() => {
    if (!device || loadedFor === device.id) return;
    setLoadedFor(device.id);
    const remote = activeLayers[device.id];
    setStack(remote && remote.length > 0 ? remote.map((l) => ({ ...l, params: cloneParams(l.params) })) : []);
    setSelected(0);
  }, [device, activeLayers, loadedFor]);

  const zone = device?.zones.find((z) => z.matrix);
  const rows = zone?.matrix?.[0] ?? 6;
  const cols = zone?.matrix?.[1] ?? 22;

  // ---- apply plumbing ----
  const timer = useRef(0);
  const stackRef = useRef(stack);
  stackRef.current = stack;
  const deviceRef = useRef(device?.id ?? '');
  deviceRef.current = device?.id ?? '';

  const doApply = useCallback(async () => {
    const id = deviceRef.current;
    if (!id || stackRef.current.length === 0) return;
    setApplyState('pending');
    try {
      await client.setLayers(id, stackRef.current);
      setApplyState('applied');
      await client.refresh().catch(() => {});
    } catch (e) {
      setApplyState('error');
      onFlash(`set_layers failed: ${(e as Error).message}`);
    }
  }, [client, onFlash]);

  // apply-live: debounce stack edits → set_layers
  useEffect(() => {
    if (!applyLive || !deviceAvailable) return;
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void doApply(), APPLY_DEBOUNCE_MS);
    return () => window.clearTimeout(timer.current);
  }, [stack, applyLive, deviceAvailable, doApply]);

  const applyNow = () => void doApply();

  // ---- stack edits ----
  const patchLayer = (i: number, patch: Partial<LayerSpec>) =>
    setStack((s) => s.map((l, j) => (j === i ? { ...l, ...patch } : l)));

  const addLayer = (effect: EffectKind) =>
    setStack((s) => {
      const next = [...s, makeLayer(effect)];
      setSelected(next.length - 1);
      return next;
    });

  const applyPreset = (p: (typeof PRESETS)[number]) => {
    setStack(p.stack.map((l) => ({ ...l, params: cloneParams(l.params) })));
    setSelected(0);
  };

  const moveLayer = (i: number, dir: -1 | 1) =>
    setStack((s) => {
      const j = i + dir;
      if (j < 0 || j >= s.length) return s;
      const next = [...s];
      [next[i], next[j]] = [next[j], next[i]];
      setSelected(j);
      return next;
    });

  const duplicateLayer = (i: number) =>
    setStack((s) => {
      const next = [...s];
      next.splice(i + 1, 0, { ...s[i], params: cloneParams(s[i].params) });
      setSelected(i + 1);
      return next;
    });

  const deleteLayer = (i: number) =>
    setStack((s) => {
      const next = s.filter((_, j) => j !== i);
      setSelected((sel) => Math.min(sel, next.length - 1));
      return next;
    });

  const triggerCell = (row: number, col: number) => {
    void client.trigger(deviceId, row, col, { rows, cols }).catch((e) => onFlash(`trigger failed: ${(e as Error).message}`));
  };

  const toggleAudio = async () => {
    try {
      const r = await client.setAudio(!audioOn);
      setAudioOn(r.on);
    } catch (e) {
      onFlash(`set_audio failed: ${(e as Error).message}`);
    }
  };

  const saveToProfile = () => {
    if (!device || stack.length === 0) return;
    setSavingName('');
  };

  const confirmSave = async () => {
    if (!device || savingName == null) return;
    const name = savingName.trim();
    if (!name) {
      onFlash('profile needs a name');
      return;
    }
    onSaveToProfile(device.id, { kind: 'layers', layers: stack });
    setSavingName(null);
    onFlash(`layer stack queued for profile "${name}" — finish in Profiles ▸ New profile`);
  };

  // key heatmap resets when switching devices
  useEffect(() => setKeyHeat(0), [deviceId]);

  const offline = status !== 'online';
  const sel = stack[selected] as LayerSpec | undefined;

  return (
    <section className="studio" aria-label="Studio">
      <div className="studio-top">
        <label className="studio-device">
          <span className="dim">Device</span>
          <select value={device?.id ?? ''} onChange={(e) => setDeviceId(e.target.value)} disabled={matrixDevices.length === 0}>
            {matrixDevices.length === 0 && <option value="">no matrix devices</option>}
            {matrixDevices.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name}
                {d.available ? '' : ' (unavailable)'}
              </option>
            ))}
          </select>
        </label>

        <div className="preset-row" role="group" aria-label="Presets">
          {PRESETS.map((p) => (
            <button key={p.name} className="preset" onClick={() => applyPreset(p)} disabled={!deviceAvailable} title={`Replace stack: ${p.stack.map((l) => l.effect).join(' + ')}`}>
              {p.name}
            </button>
          ))}
        </div>

        <label className="apply-live" title="Layer edits apply to hardware automatically (debounced 500ms)">
          <input type="checkbox" checked={applyLive} onChange={(e) => setApplyLive(e.target.checked)} disabled={offline} />
          Apply-live
        </label>
      </div>

      {(offline || (device && !device.available)) && (
        <div className="studio-note" role="status">
          {offline
            ? 'Daemon offline — start the daemon to drive hardware. Canvas keeps previewing locally.'
            : `${device?.name} is unavailable (permissions or busy). Controls are disabled; retry from the Devices page.`}
        </div>
      )}

      <div className="studio-body">
        {/* LEFT: layer list */}
        <div className="layer-list" aria-label="Layers">
          <div className="studio-col-head">
            <span>Layers</span>
            <label className="add-layer">
              <span className="dim">Add</span>
              <select value="" onChange={(e) => e.target.value && addLayer(e.target.value as EffectKind)}>
                <option value="">effect…</option>
                {EFFECT_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {stack.length === 0 && <div className="dim empty-layers">No layers — pick a preset or add one above.</div>}
          {stack.map((l, i) => (
            <div
              key={i}
              className={i === selected ? 'layer-row on' : 'layer-row'}
              onClick={() => setSelected(i)}
              role="button"
              aria-label={`Layer ${i + 1}: ${l.effect}`}
            >
              <button
                className={l.enabled ? 'eye on' : 'eye'}
                onClick={(e) => {
                  e.stopPropagation();
                  patchLayer(i, { enabled: !l.enabled });
                }}
                title={l.enabled ? 'Disable layer' : 'Enable layer'}
                aria-pressed={l.enabled}
              >
                {l.enabled ? '◉' : '○'}
              </button>
              <span className="layer-name">{l.effect}</span>
              <span className="dim layer-meta">
                {l.opacity}% · {l.blend}
              </span>
              <input
                type="range"
                min={0}
                max={100}
                value={l.opacity}
                onClick={(e) => e.stopPropagation()}
                onChange={(e) => patchLayer(i, { opacity: Number(e.target.value) })}
                aria-label={`Layer ${i + 1} opacity`}
              />
              <div className="toggle-group mini">
                <button
                  className={l.blend === 'add' ? 'tab on' : 'tab'}
                  onClick={(e) => {
                    e.stopPropagation();
                    patchLayer(i, { blend: l.blend === 'add' ? 'normal' : 'add' });
                  }}
                  title="Toggle blend mode"
                >
                  {l.blend}
                </button>
              </div>
              <div className="layer-btns">
                <button className="ghost small" disabled={i === 0} onClick={(e) => { e.stopPropagation(); moveLayer(i, -1); }} title="Move up">↑</button>
                <button className="ghost small" disabled={i === stack.length - 1} onClick={(e) => { e.stopPropagation(); moveLayer(i, 1); }} title="Move down">↓</button>
                <button className="ghost small" onClick={(e) => { e.stopPropagation(); duplicateLayer(i); }} title="Duplicate">⧉</button>
                <button className="ghost small" onClick={(e) => { e.stopPropagation(); deleteLayer(i); }} title="Delete">✕</button>
              </div>
            </div>
          ))}
        </div>

        {/* CENTER: live canvas */}
        <div className="canvas-pane">
          <div className="studio-col-head">
            <span>
              Live canvas <span className="dim">· {rows}×{cols} · click a key</span>
            </span>
            <span className="dim key-heat" title="Key events received for this device">
              keys: {keyHeat}
            </span>
          </div>
          <LiveCanvas
            client={client}
            device={device?.id ?? ''}
            layers={stack}
            rows={rows}
            cols={cols}
            disabled={!deviceAvailable}
            onCellClick={triggerCell}
            onKeyHeatmap={setKeyHeat}
          />
          <div className="canvas-foot">
            <button className={audioOn ? 'small on-tool' : 'small'} onClick={() => void toggleAudio()} disabled={offline} title="Toggle microphone input for audio-reactive layers">
              🎙 {audioOn ? 'mic on' : 'mic off'}
            </button>
            <AudioMeter client={client} on={audioOn} />
            <span className="dim">preview is a client-side approximation</span>
          </div>
        </div>

        {/* RIGHT: inspector */}
        <div className="inspector-pane">
          <div className="studio-col-head">
            <span>Inspector</span>
          </div>
          {sel ? (
            <Inspector layer={sel} onChange={(patch) => patchLayer(selected, patch)} />
          ) : (
            <div className="dim empty-layers">Select or add a layer to edit its parameters.</div>
          )}
        </div>
      </div>

      {/* BOTTOM: status + actions */}
      <div className="studio-bottom">
        <code className="stack-json" title="Current stack as sent to set_layers">
          {stack.length === 0
            ? '[]'
            : JSON.stringify(
                stack.map((l) => ({ effect: l.effect, opacity: l.opacity, blend: l.blend, enabled: l.enabled })),
              )}
        </code>
        <div className="row-actions">
          <span className={'apply-state ' + applyState} aria-live="polite">
            {applyState === 'pending' ? 'applying…' : applyState === 'applied' ? 'applied ✓' : applyState === 'error' ? 'error' : ''}
          </span>
          <button onClick={saveToProfile} disabled={!deviceAvailable || stack.length === 0}>
            Save to profile…
          </button>
          <button className="primary" onClick={applyNow} disabled={!deviceAvailable || stack.length === 0}>
            Apply
          </button>
        </div>
      </div>

      {savingName != null && (
        <div className="drawer-backdrop" onClick={(e) => e.target === e.currentTarget && setSavingName(null)}>
          <div className="prompt" role="dialog" aria-label="Save layer stack to profile">
            <h3>Save layer stack</h3>
            <p className="dim">
              Captures this device's {stack.length}-layer stack as a profile entry. The profile editor opens next — give it
              bindings there.
            </p>
            <input
              autoFocus
              placeholder="Profile name (e.g. Valorant)"
              value={savingName}
              onChange={(e) => setSavingName(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && void confirmSave()}
            />
            <div className="row-actions end">
              <button className="ghost" onClick={() => setSavingName(null)}>
                Cancel
              </button>
              <button className="primary" onClick={() => void confirmSave()}>
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

/** Corner VU meter fed by daemon audio push events. */
function AudioMeter({ client, on }: { client: ChromaClient; on: boolean }) {
  const [level, setLevel] = useState(0);
  useEffect(() => {
    if (!on) {
      setLevel(0);
      return;
    }
    return client.onEvent((ev) => ev.event === 'audio' && setLevel(ev.level));
  }, [client, on]);
  return (
    <span className={on ? 'vu on' : 'vu'} title={on ? `audio level ${level}/1000` : 'audio off'} aria-label="Audio level">
      <i style={{ width: `${Math.round((level / 1000) * 100)}%` }} />
    </span>
  );
}
