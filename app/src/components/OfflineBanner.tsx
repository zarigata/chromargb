import type { Status } from '../ipc';
import { DAEMON_WS_URL } from '../ipc';

export function OfflineBanner({ status }: { status: Status }) {
  if (status === 'online') return null;
  const connecting = status === 'connecting';
  return (
    <div className={connecting ? 'banner connecting' : 'banner offline'} role="status">
      <span className="banner-icon" aria-hidden>
        {connecting ? '◌' : '⚠'}
      </span>
      {connecting ? (
        <span>Connecting to daemon…</span>
      ) : (
        <span>
          Daemon offline — retrying every 3&nbsp;s ({DAEMON_WS_URL}). Effects keep running on any devices the daemon
          already drives.
        </span>
      )}
    </div>
  );
}
