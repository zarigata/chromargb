import { useCallback, useMemo, useState } from 'react';
import { useDaemon } from './api';
import { DeviceCard } from './components/DeviceCard';
import { EffectStudio } from './components/EffectStudio';
import { Header } from './components/Header';
import { MatrixEditor } from './components/MatrixEditor';
import { OfflineBanner } from './components/OfflineBanner';
import { ProfilesPanel } from './components/ProfilesPanel';
import { Sidebar, type Page } from './components/Sidebar';
import { Studio } from './components/Studio';
import type { ActiveEffect, Device, DeviceZone, EffectKind, EffectParams, LayerSpec, ProfileDeviceEntry, RGB, StateEntry } from './types';

export default function App() {
  const { client, status, state } = useDaemon();
  const [studioDeviceId, setStudioDeviceId] = useState<string | null>(null);
  const [matrixTarget, setMatrixTarget] = useState<{ device: Device; zone: DeviceZone; current?: RGB[] } | null>(null);
  const [page, setPage] = useState<Page>('devices');
  const [toast, setToast] = useState<string | null>(null);
  /** Profile entry staged by Studio's "Save to profile" — opened in Profiles. */
  const [stagedEntry, setStagedEntry] = useState<{ device: string; entry: ProfileDeviceEntry } | null>(null);

  const flash = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((t) => t === msg ? null : t), 2600);
  }, []);

  const devices = state?.devices ?? [];
  const active = state?.active ?? {};
  const rawEntries = state?.entries ?? [];
  const studioDevice = devices.find((d) => d.id === studioDeviceId) ?? null;

  // get_state reports the effect name only; overlay the params from the last
  // action applied through this UI so previews and the studio stay accurate.
  const [lastApplied, setLastApplied] = useState<Record<string, ActiveEffect>>({});
  const mergedActive: Record<string, ActiveEffect | undefined> = {};
  for (const [id, a] of Object.entries(active)) {
    const last = lastApplied[id];
    mergedActive[id] = a && last && last.effect === a.effect ? { effect: a.effect, params: last.params } : a;
  }

  // mode + matrix + layers per device from the raw state entries
  const stateById = useMemo(() => {
    const m: Record<string, StateEntry> = {};
    for (const e of rawEntries) m[e.id] = e;
    return m;
  }, [rawEntries]);

  const activeLayers = useMemo(() => {
    const m: Record<string, LayerSpec[] | undefined> = {};
    for (const e of rawEntries) if (e.layers && e.layers.length > 0) m[e.id] = e.layers;
    return m;
  }, [rawEntries]);

  const retryAll = useCallback(
    async () => {
      try {
        const r = await client.refreshDevices();
        flash(`rescanned: ${r.ready} ready, ${r.unavailable} unavailable`);
      } catch (e) {
        flash(`refresh failed: ${(e as Error).message}`);
      }
    },
    [client, flash],
  );

  // ---- device actions ----
  const applyStatic = useCallback(
    async (deviceId: string, color: RGB) => {
      try {
        await client.setColor(deviceId, color);
        setLastApplied((m) => ({ ...m, [deviceId]: { effect: 'static', params: { color } } }));
        await client.refresh();
      } catch (e) {
        flash(`set_color failed: ${(e as Error).message}`);
      }
    },
    [client, flash],
  );

  const applyEffect = useCallback(
    async (deviceId: string, effect: EffectKind, params: EffectParams) => {
      try {
        await client.setEffect(deviceId, effect, params as unknown as Record<string, unknown>);
        setLastApplied((m) => ({ ...m, [deviceId]: { effect, params } }));
        await client.refresh();
      } catch (e) {
        flash(`set_effect failed: ${(e as Error).message}`);
      }
    },
    [client, flash],
  );

  const stopEffect = useCallback(
    async (deviceId: string) => {
      try {
        await client.stopEffect(deviceId);
        setLastApplied((m) => {
          const { [deviceId]: _drop, ...rest } = m;
          return rest;
        });
        await client.refresh();
      } catch (e) {
        flash(`stop_effect failed: ${(e as Error).message}`);
      }
    },
    [client, flash],
  );

  const applyMatrix = useCallback(
    async (deviceId: string, zone: string, colors: RGB[]) => {
      try {
        const r = await client.setMatrix(deviceId, zone, colors);
        setLastApplied((m) => {
          const { [deviceId]: _drop, ...rest } = m;
          return rest;
        });
        flash(`matrix applied to ${zone} (${r.leds} LEDs)`);
        await client.refresh().catch(() => {});
      } catch (e) {
        flash(`set_matrix: ${(e as Error).message}`);
      }
    },
    [client, flash],
  );

  // capture a device's CURRENT lighting for the profile editor
  const currentOf = useCallback(
    (deviceId: string): ProfileDeviceEntry | null => {
      const e = stateById[deviceId];
      if (!e) return null;
      if (e.layers && e.layers.length > 0) return { kind: 'layers', layers: e.layers };
      if (e.mode === 'matrix' && e.matrix) {
        const zone = Object.keys(e.matrix)[0];
        return { kind: 'matrix', zone, colors: e.matrix[zone] };
      }
      if (e.mode === 'static' && e.color) return { kind: 'static', color: e.color };
      const a = mergedActive[deviceId];
      if (a && a.effect !== 'static') return { kind: 'effect', effect: a.effect as EffectKind, params: a.params };
      if (e.color) return { kind: 'static', color: e.color };
      return null;
    },
    [stateById, mergedActive],
  );

  const openMatrix = (d: Device, zi: number) => {
    const zone = d.zones[zi];
    const e = stateById[d.id];
    setMatrixTarget({ device: d, zone, current: e?.matrix?.[zone.name] });
  };

  // Studio "Save to profile": stage the entry, then open Profiles with a fresh
  // editor seeded around it.
  const handleSaveToProfile = useCallback(
    (deviceId: string, entry: ProfileDeviceEntry) => {
      setStagedEntry({ device: deviceId, entry });
    },
    [],
  );

  return (
    <div className="app">
      <Header status={status} onRefresh={retryAll} onFlash={flash} />
      <OfflineBanner status={status} />
      <div className="shell">
        <Sidebar page={page} onNav={setPage} />
        {page === 'devices' && (
          <main className="grid">
            {status === 'online' && devices.length === 0 && (
              <div className="empty">No RGB devices found. Plug one in and press Refresh.</div>
            )}
            {devices.map((d) => (
              <DeviceCard
                key={d.id}
                device={d}
                active={mergedActive[d.id]}
                mode={stateById[d.id]?.mode}
                onStatic={(c) => void applyStatic(d.id, c)}
                onStop={() => void stopEffect(d.id)}
                onStudio={() => setStudioDeviceId(d.id)}
                onMatrix={(zi) => openMatrix(d, zi)}
                onRetry={() => void retryAll()}
              />
            ))}
          </main>
        )}
        {page === 'studio' && (
          <Studio
            client={client}
            devices={devices}
            activeLayers={activeLayers}
            onSaveToProfile={handleSaveToProfile}
            onFlash={flash}
            status={status}
          />
        )}
        {page === 'profiles' && (
          <ProfilesPanel
            client={client}
            devices={devices}
            currentOf={currentOf}
            onFlash={flash}
            staged={stagedEntry}
            onStagedUsed={() => setStagedEntry(null)}
          />
        )}
      </div>

      {studioDevice && (
        <EffectStudio
          device={studioDevice}
          initial={mergedActive[studioDevice.id]}
          onClose={() => setStudioDeviceId(null)}
          onApply={(effect, params) => applyEffect(studioDevice.id, effect, params)}
        />
      )}
      {matrixTarget && (
        <MatrixEditor
          device={matrixTarget.device}
          zone={matrixTarget.zone}
          initial={matrixTarget.current}
          onClose={() => setMatrixTarget(null)}
          onApply={(colors) => applyMatrix(matrixTarget.device.id, matrixTarget.zone.name, colors)}
        />
      )}
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
