// Preload bridge (see electron/preload.ts) + dev mock marker.
interface Window {
  versions?: { node: string; chrome: string; electron: string };
  autostart?: {
    get(): Promise<boolean>;
    set(on: boolean): Promise<boolean>;
    onChange(cb: (on: boolean) => void): () => void;
  };
  updater?: {
    check(): Promise<{ latest: boolean; current: string; available?: string; url?: string; name?: string; size?: number; error?: string }>;
    download(url: string, name: string): Promise<{ ok?: boolean; path?: string; error?: string }>;
  };
  ChromaMock?: { createSocket: () => unknown };
}
