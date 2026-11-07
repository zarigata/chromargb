import { contextBridge, ipcRenderer } from 'electron';

// Deliberately tiny: versions + autostart toggles + change events. Nothing else.
// The renderer talks to the daemon over plain WebSocket (ws://127.0.0.1:7474),
// so no privileged channel is needed for device control.
contextBridge.exposeInMainWorld('versions', {
  node: process.versions.node,
  chrome: process.versions.chrome,
  electron: process.versions.electron,
});

contextBridge.exposeInMainWorld('autostart', {
  get: (): Promise<boolean> => ipcRenderer.invoke('chroma:get-autostart'),
  set: (on: boolean): Promise<boolean> => ipcRenderer.invoke('chroma:set-autostart', on),
  onChange: (cb: (on: boolean) => void): (() => void) => {
    const listener = (_e: Electron.IpcRendererEvent, on: boolean) => cb(on);
    ipcRenderer.on('chroma:autostart-changed', listener);
    return () => ipcRenderer.removeListener('chroma:autostart-changed', listener);
  },
});

contextBridge.exposeInMainWorld('updater', {
  check: (): Promise<{ latest: boolean; current: string; available?: string; url?: string; name?: string; size?: number; error?: string }> =>
    ipcRenderer.invoke('chroma:update-check'),
  download: (url: string, name: string): Promise<{ ok?: boolean; path?: string; error?: string }> =>
    ipcRenderer.invoke('chroma:update-download', url, name),
});
