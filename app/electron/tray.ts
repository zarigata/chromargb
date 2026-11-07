import { app, Menu, Tray } from 'electron';
import path from 'node:path';

let tray: Tray | null = null;

export function createTray(opts: { show: () => void; quit: () => void }): Tray {
  const iconPath = app.isPackaged
    ? path.join(process.resourcesPath, 'resources', 'tray.png')
    : path.join(app.getAppPath(), 'resources', 'tray.png');
  tray = new Tray(iconPath);
  tray.setToolTip('ChromaRGB');
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: 'Open ChromaRGB', type: 'normal', click: opts.show },
      { type: 'separator' },
      { label: 'Quit', type: 'normal', click: opts.quit },
    ]),
  );
  tray.on('click', opts.show); // macOS: plain left-click opens the window
  return tray;
}
