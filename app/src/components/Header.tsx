import { useEffect, useState } from 'react';
import type { Status } from '../ipc';

interface Props {
  status: Status;
  onRefresh(): void;
  onFlash(msg: string): void;
}

/** Top bar: brand, daemon status, refresh, autostart, update check. Nav lives in the Sidebar. */
export function Header({ status, onRefresh, onFlash }: Props) {
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [upd, setUpd] = useState<{ state: 'idle' | 'checking' | 'downloading' | 'latest'; msg?: string; url?: string; name?: string }>({ state: 'idle' });

  useEffect(() => {
    const b = window.autostart;
    if (!b) return; // not running under Electron (plain browser dev)
    void b.get().then(setAutostart);
    return b.onChange(setAutostart);
  }, []);

  const toggleAutostart = () => {
    const b = window.autostart;
    if (!b || autostart === null) return;
    void b.set(!autostart).then(setAutostart);
  };

  const checkUpdate = async () => {
    const u = window.updater;
    if (!u) return onFlash('updates need the desktop app');
    setUpd({ state: 'checking' });
    const r = await u.check();
    if (r.latest) setUpd({ state: 'latest' });
    else if (r.url) setUpd({ state: 'idle', msg: `v${r.available?.replace(/^v/, '')} available`, url: r.url, name: r.name });
    else setUpd({ state: 'idle', msg: r.error ?? 'check failed' });
  };

  const downloadUpdate = async () => {
    const u = window.updater;
    if (!u || !upd.url || !upd.name) return;
    setUpd({ state: 'downloading' });
    const r = await u.download(upd.url, upd.name);
    setUpd({ state: 'idle', msg: r.ok ? 'installer opened — quit ChromaRGB to install' : (r.error ?? 'download failed') });
    onFlash(r.ok ? 'Update downloaded — installer opening' : `update failed: ${r.error}`);
  };

  const dot = status === 'online' ? 'dot on' : status === 'connecting' ? 'dot wait' : 'dot off';
  const statusText = status === 'online' ? 'daemon online' : status === 'connecting' ? 'connecting…' : 'daemon offline';

  return (
    <header className="header">
      <div className="brand">
        <span className="logo">C</span>
        <h1>ChromaRGB</h1>
        <span className={dot} title={statusText} aria-label={statusText} />
        {window.versions && (
          <span className="dim" title={`electron ${window.versions.electron} · chrome ${window.versions.chrome} · node ${window.versions.node}`}>
            v{window.versions.electron}
          </span>
        )}
      </div>

      <div className="header-actions">
        {upd.state === 'idle' && !upd.msg && (
          <button onClick={() => void checkUpdate()} title="Check GitHub for a newer release">
            Check updates
          </button>
        )}
        {upd.state === 'checking' && <span className="dim">checking…</span>}
        {upd.state === 'downloading' && <span className="dim">downloading…</span>}
        {upd.state === 'latest' && <span className="dim" title="You're on the latest release">up to date ✓</span>}
        {upd.state === 'idle' && upd.msg && upd.url && (
          <>
            <span className="dim">{upd.msg}</span>
            <button className="primary small" onClick={() => void downloadUpdate()}>
              Update
            </button>
          </>
        )}
        {upd.state === 'idle' && upd.msg && !upd.url && <span className="dim">{upd.msg}</span>}
        <button onClick={onRefresh} disabled={status !== 'online'} title="Rescan USB + re-open devices (also retries after granting permissions)">
          Refresh devices
        </button>
        <label className="autostart" title="Launch ChromaRGB at login">
          <input type="checkbox" checked={autostart === true} disabled={autostart === null} onChange={toggleAutostart} />
          autostart
        </label>
      </div>
    </header>
  );
}
