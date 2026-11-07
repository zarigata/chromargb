import { useCallback, useEffect, useState } from 'react';
import type { ActiveProfileInfo, Device, GameProfile, ProfileDeviceEntry, ProfileSummary } from '../types';

interface Props {
  client: {
    profileList(): Promise<ProfileSummary[]>;
    profileGet(name: string): Promise<GameProfile>;
    profileSave(p: GameProfile): Promise<{ saved: string; warnings: string[] }>;
    profileDelete(name: string): Promise<{ deleted: string }>;
    profileApply(name: string): Promise<{ results: Record<string, string>; warnings: string[] }>;
    getActiveProfile(): Promise<ActiveProfileInfo>;
  };
  devices: Device[];
  /** Current per-device lighting (effect name + matrix availability) for capture. */
  currentOf(deviceId: string): ProfileDeviceEntry | null;
  onFlash(msg: string): void;
  /** Entry staged by the Studio's "Save to profile" — opens a fresh editor seeded with it. */
  staged?: { device: string; entry: ProfileDeviceEntry } | null;
  onStagedUsed?(): void;
}

const nameFor = (devices: Device[], id: string) => devices.find((d) => d.id === id)?.name ?? id;

export function ProfilesPanel({ client, devices, currentOf, onFlash, staged, onStagedUsed }: Props) {
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [active, setActive] = useState<ActiveProfileInfo | null>(null);
  const [editing, setEditing] = useState<GameProfile | null>(null);

  // Studio staged a {kind:'layers'} capture — open a fresh editor seeded with it.
  useEffect(() => {
    if (!staged) return;
    setEditing({ name: '', bindings: [], devices: { [staged.device]: staged.entry } });
    onStagedUsed?.();
  }, [staged, onStagedUsed]);

  const reload = useCallback(() => {
    void client.profileList().then(setProfiles).catch(() => {});
    void client.getActiveProfile().then(setActive).catch(() => {});
  }, [client]);

  useEffect(() => {
    reload();
    const t = window.setInterval(reload, 4000); // active-profile indicator poll
    return () => window.clearInterval(t);
  }, [reload]);

  const newProfile = () =>
    setEditing({ name: '', bindings: [], devices: {} });

  const editExisting = async (name: string) => {
    try {
      setEditing(await client.profileGet(name));
    } catch (e) {
      onFlash(`profile_get failed: ${(e as Error).message}`);
    }
  };

  const save = async () => {
    if (!editing) return;
    if (!editing.name.trim()) {
      onFlash('profile needs a name');
      return;
    }
    try {
      const r = await client.profileSave(editing);
      onFlash(r.warnings.length ? `saved "${r.saved}" (${r.warnings.length} warning(s))` : `saved "${r.saved}"`);
      setEditing(null);
      reload();
    } catch (e) {
      onFlash(`save failed: ${(e as Error).message}`);
    }
  };

  const apply = async (name: string) => {
    try {
      const r = await client.profileApply(name);
      const errs = Object.values(r.results).filter((v) => v !== 'ok');
      onFlash(errs.length ? `"${name}": ${errs.length} device error(s)` : `"${name}" applied`);
      reload();
    } catch (e) {
      onFlash(`apply failed: ${(e as Error).message}`);
    }
  };

  const remove = async (name: string) => {
    try {
      await client.profileDelete(name);
      onFlash(`"${name}" deleted`);
      reload();
    } catch (e) {
      onFlash(`delete failed: ${(e as Error).message}`);
    }
  };

  // ---- editing helpers ----
  const captureDevice = (deviceId: string) => {
    if (!editing) return;
    const cur = currentOf(deviceId);
    if (!cur) {
      onFlash(`${nameFor(devices, deviceId)} has nothing active to capture`);
      return;
    }
    setEditing({ ...editing, devices: { ...editing.devices, [deviceId]: cur } });
  };

  const clearDevice = (deviceId: string) => {
    if (!editing) return;
    const devices = { ...editing.devices };
    delete devices[deviceId];
    setEditing({ ...editing, devices });
  };

  const setBinding = (raw: string) => {
    if (!editing) return;
    setEditing({
      ...editing,
      bindings: raw.split(',').map((s) => s.trim()).filter(Boolean),
    });
  };

  return (
    <section className="profiles">
      <div className="profiles-head">
        <h2>Game profiles</h2>
        {active && (
          <span className="watcher" title={active.frontmost ? `frontmost: ${active.frontmost}` : undefined}>
            {active.name ? `active: ${active.name}` : 'no profile active'}
            {active.watcher === 'on' && active.frontmost ? ` · watching (${active.frontmost})` : active.watcher === 'on' ? ' · watching' : ''}
          </span>
        )}
        <button onClick={newProfile}>New profile</button>
      </div>

      <div className="profile-list">
        {profiles.length === 0 && (
          <div className="dim">No profiles yet — create one, bind it to a game's app name, and it switches automatically when that game is focused.</div>
        )}
        {profiles.map((p) => (
          <div key={p.name} className={`profile-row${active?.name === p.name ? ' current' : ''}`}>
            <div>
              <div className="profile-name">{p.name}</div>
              <div className="dim">
                {p.devices.length} device(s){p.bindings.length ? ` · binds: ${p.bindings.join(', ')}` : ' · manual'}
              </div>
            </div>
            <div className="row-actions">
              <button onClick={() => void apply(p.name)}>Apply</button>
              <button className="ghost" onClick={() => void editExisting(p.name)}>Edit</button>
              <button className="ghost" onClick={() => void remove(p.name)}>Delete</button>
            </div>
          </div>
        ))}
      </div>

      {editing && (
        <div className="profile-editor">
          <div className="row">
            <input
              autoFocus
              placeholder="Profile name (e.g. Valorant)"
              value={editing.name}
              onChange={(e) => setEditing({ ...editing, name: e.target.value })}
            />
            <input
              placeholder="Auto-switch when app name contains… (comma-separated)"
              value={editing.bindings.join(', ')}
              onChange={(e) => setBinding(e.target.value)}
            />
          </div>
          <div className="edit-list">
            {devices.map((d) => {
              const entry = editing.devices[d.id];
              return (
                <div key={d.id} className="edit-row">
                  <span className={d.available ? '' : 'dim'}>{d.name}</span>
                  <span className="dim entry-desc">
                    {entry
                      ? entry.kind === 'matrix'
                        ? `matrix (${entry.zone}, ${entry.colors.length} LEDs)`
                        : entry.kind === 'effect'
                          ? `effect: ${entry.effect}`
                          : entry.kind === 'layers'
                            ? `${entry.layers.length} layers`
                            : 'static color'
                      : '—'}
                  </span>
                  <div className="row-actions">
                    <button disabled={!d.available} onClick={() => captureDevice(d.id)} title="Capture this device's current lighting into the profile">
                      Capture current
                    </button>
                    {entry && <button className="ghost" onClick={() => clearDevice(d.id)}>Remove</button>}
                  </div>
                </div>
              );
            })}
          </div>
          <div className="row-actions end">
            <button className="ghost" onClick={() => setEditing(null)}>Cancel</button>
            <button onClick={() => void save()}>Save profile</button>
          </div>
        </div>
      )}
    </section>
  );
}
