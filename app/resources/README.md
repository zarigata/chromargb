# resources/

Drop the daemon binary here as `chromargb-daemon` (`chromargb-daemon.exe` on Windows).

- Dev: Electron spawns `./resources/chromargb-daemon` if it exists.
- Packaged: electron-builder copies this directory via `extraResources` to
  `<app Resources>/resources/`, and the main process spawns it from there.

A missing binary is fine — the UI shows the "daemon offline" banner and keeps
retrying ws://127.0.0.1:7474 every 3 s.
