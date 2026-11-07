// Shared client singleton + React hook.
import { useEffect, useState } from 'react';
import { ChromaClient, type DaemonSocket, type SocketFactory, type Status } from './ipc';
import type { DaemonState } from './types';

let client: ChromaClient | null = null;

export function getClient(): ChromaClient {
  if (!client) {
    const mock = (window as unknown as { ChromaMock?: { createSocket(): DaemonSocket } }).ChromaMock;
    const factory: SocketFactory | undefined = mock ? () => mock.createSocket() : undefined;
    client = new ChromaClient(factory);
  }
  return client;
}

export function useDaemon(): { client: ChromaClient; status: Status; state: DaemonState | null } {
  const c = getClient();
  const [status, setStatus] = useState<Status>(c.status);
  const [state, setState] = useState<DaemonState | null>(c.state);

  useEffect(
    () =>
      c.subscribe((s, st) => {
        setStatus(s);
        setState(st);
      }),
    [c],
  );

  return { client: c, status, state };
}
