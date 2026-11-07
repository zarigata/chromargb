import { app, BrowserWindow, ipcMain, nativeTheme, shell } from 'electron';
import path from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { createTray } from './tray';

let win: BrowserWindow | null = null;
let daemon: ChildProcess | null = null;
let quitting = false;

const UPDATE_REPO = 'zarigata/chromargb'; // GitHub repo serving releases
const APP_VERSION = app.getVersion();

// Minimal updater: GitHub releases only, no dependencies. Checks the latest
// release, compares semver, downloads the platform asset, opens the
// installer. ponytail: no delta patches / auto-restart — the installer does
// that; upgrade to electron-updater if silent background updates ever matter.
function parseSemver(v: string): number[] {
  return v.replace(/^v/, '').split('.').map((n) => parseInt(n, 10) || 0);
}
function isNewer(remote: string, local: string): boolean {
  const r = parseSemver(remote);
  const l = parseSemver(local);
  for (let i = 0; i < 3; i++) if (r[i] !== l[i]) return r[i] > l[i];
  return false;
}
function pickAsset(name: string, map: Record<string, string[]>): boolean {
  return Object.entries(map).some(([needle, exts]) => name.toLowerCase().includes(needle) && exts.some((e) => name.toLowerCase().endsWith(e)));
}

ipcMain.handle('chroma:update-check', async () => {
  try {
    const res = await fetch(`https://api.github.com/repos/${UPDATE_REPO}/releases/latest`, {
      headers: { 'User-Agent': 'ChromaRGB-Updater' },
    });
    if (!res.ok) return { latest: false, error: `github ${res.status}` };
    const rel = (await res.json()) as { tag_name?: string; assets?: Array<{ name: string; browser_download_url: string; size: number }> };
    const tag = rel.tag_name ?? '';
    if (!tag || !isNewer(tag, APP_VERSION)) return { latest: true, current: APP_VERSION };
    const wanted: Record<string, string[]> = {
      darwin: ['darwin', 'macos', 'mac'],
      win32: ['win'],
      linux: ['linux', 'appimage'],
    };
    const needles: string[] = wanted[process.platform] ?? [];
    const asset = (rel.assets ?? []).find((a) => pickAsset(a.name, { [process.platform]: needles }) && (process.platform !== 'darwin' || a.name.includes(process.arch)));
    return asset
      ? { latest: false, current: APP_VERSION, available: tag, url: asset.browser_download_url, name: asset.name, size: asset.size }
      : { latest: false, current: APP_VERSION, available: tag, error: 'no matching asset for this platform' };
  } catch (e) {
    return { latest: false, error: (e as Error).message };
  }
});

ipcMain.handle('chroma:update-download', async (_e, url: string, name: string) => {
  // Downloads to tmp and opens with the OS installer handler (.dmg/.exe/.AppImage).
  try {
    const res = await fetch(url, { headers: { 'User-Agent': 'ChromaRGB-Updater' } });
    if (!res.ok) return { error: `download ${res.status}` };
    const buf = Buffer.from(await res.arrayBuffer());
    const dest = path.join(app.getPath('temp'), name);
    require('node:fs').writeFileSync(dest, buf);
    await shell.openPath(dest);
    return { ok: true, path: dest };
  } catch (e) {
    return { error: (e as Error).message };
  }
});

const DEV_URL = process.env.VITE_DEV_SERVER_URL; // injected by vite-plugin-electron
const PRELOAD = path.join(__dirname, 'preload.js');

function spawnDaemon(): void {
  const isPackaged = app.isPackaged;
  const exe = process.platform === 'win32' ? 'chromargb-daemon.exe' : 'chromargb-daemon';
  // Dev: ./resources next to repo root. Packaged: <Resources>/resources (extraResources).
  const devPath = path.join(app.getAppPath(), 'resources', exe);
  const prodPath = path.join(process.resourcesPath, 'resources', exe);
  const bin = isPackaged ? prodPath : devPath;
  try {
    daemon = spawn(bin, [], { stdio: 'ignore' });
    const tag = 'chromargb-daemon';
    daemon.on('error', () => console.log(`[${tag}] not spawnable yet (UI will keep retrying ws://127.0.0.1:7474)`));
    daemon.on('exit', (code) => console.log(`[${tag}] exited code=${code}`));
    console.log(`[${tag}] spawned ${bin}`);
  } catch {
    // Missing binary is expected in dev; WebSocket reconnect loop in the UI handles it.
  }
}

function createWindow(): void {
  win = new BrowserWindow({
    width: 1180,
    height: 800,
    minWidth: 640,
    minHeight: 480,
    backgroundColor: nativeTheme.shouldUseDarkColors ? '#10131a' : '#ffffff',
    title: 'ChromaRGB',
    webPreferences: {
      preload: PRELOAD,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });

  win.on('close', (e) => {
    if (!quitting) {
      e.preventDefault();
      win?.hide(); // hides-to-tray; effects keep running in the daemon
    }
  });

  if (DEV_URL) win.loadURL(DEV_URL);
  else win.loadFile(path.join(__dirname, '../dist/renderer/index.html'));
}

function showWindow(): void {
  if (!win) return createWindow();
  win.show();
  win.focus();
}

// Single-instance: second launch just re-opens the window.
const gotLock = app.requestSingleInstanceLock();
if (!gotLock) app.quit();
else {
  app.on('second-instance', showWindow);

  // macOS: windows activate by clicking the dock icon; Linux: activate too.
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
    else showWindow();
  });

  void app.whenReady().then(() => {
    spawnDaemon();
    createWindow();
    createTray({ show: showWindow, quit: () => { quitting = true; app.quit(); } });

    // Tray menu toggles this via webContents; the renderer asks for the current flag.
    ipcMain.handle('chroma:get-autostart', () => app.getLoginItemSettings().openAtLogin);
    ipcMain.handle('chroma:set-autostart', (_e, on: boolean) => {
      app.setLoginItemSettings({ openAtLogin: on, args: ['--hidden'] });
      return app.getLoginItemSettings().openAtLogin;
    });
    win?.webContents.on('did-finish-load', () => {
      win?.webContents.send('chroma:autostart-changed', app.getLoginItemSettings().openAtLogin);
    });
  });

  app.on('window-all-closed', () => {
    // Keep running in tray on all platforms (daemon-first design).
  });

  app.on('before-quit', () => {
    quitting = true;
    daemon?.kill();
  });
}
