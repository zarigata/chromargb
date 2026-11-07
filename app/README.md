# ChromaRGB — Electron UI

Dark, single-page Electron + Vite + React + TypeScript app that talks to the
`chromargb-daemon` over a JSON WebSocket at `ws://127.0.0.1:7474`.

## Layout

```
app/
├── electron/          # main process (window, tray, daemon spawn) + preload bridge
├── src/               # renderer
│   ├── ipc.ts         # typed WS client: reconnect loop, request ids, timeouts
│   ├── mock.ts        # in-memory daemon (3 fake devices) for VITE_DEV_MOCK=1
│   ├── types.ts       # IPC contract types — single source of truth for the schema
│   └── components/    # Header, DeviceCard, EffectStudio, MatrixEditor, previews
├── resources/         # daemon binary goes here (extraResources source)
└── build/icon.png     # electron-builder icon
```

## Prerequisites

- Node ≥ 20 (built with v22)
- Rust toolchain (to build the daemon)
- `chromargb-daemon` binary (optional in dev — see Mock mode)

## Run core + app together (dev)

```sh
cd core && cargo build --release && cp target/release/chromargb-daemon ../app/resources/ && cd ../app && npm run dev:live
```

To run the daemon manually instead of the autospawn:

```sh
cargo run --release -p chromargb-daemon   # in core/
```

## Run against real hardware (live)

The app autospawns the daemon: on startup the Electron main process launches
`./resources/chromargb-daemon` (dev) or `<Resources>/resources/chromargb-daemon`
(packaged) and the UI connects to `ws://127.0.0.1:7474`, retrying every 3 s.
So the normal flow is just:

```sh
npm run dev:live        # vite + electron; daemon autospawned from app/resources/
```

(or `npm run dev` for mock mode — see below.)

A daemon you start yourself works too (`cd core && ./target/release/chromargb-daemon`).
If one is already listening on 7474, the autospawned instance exits and the UI
attaches to yours — macOS HID devices open exclusively, so **only one daemon
may own the hardware**.

**macOS permission note (Logitech / some devices):** the daemon opens HID
devices at startup; a device it cannot open (e.g. G502 HERO without
permission → `privilege violation`) is still listed but marked
`available: false` — its card is grayed out with the reason, and writes to it
return `device <id> unavailable: <error>`. Fix by granting the daemon
**System Settings → Privacy & Security → Input Monitoring** (add the
`chromargb-daemon` binary or the terminal/Electron app that spawns it), or run
the daemon with `sudo`. Then restart the daemon.

The frozen IPC contract (`list_devices`, `get_state` → `[{id, color, effect}]`,
`set_color`, `set_effect`, `stop_effect`, `set_matrix`) is documented in
`core/README.md`; `app/src/types.ts` mirrors it.

### Live E2E (headless, real hardware)

With the daemon running on 7474:

```sh
npm run build
SMOKE_SHOT=$PWD/e2e-screenshot.png node_modules/.bin/electron smoke.cjs dist/renderer live
```

Drives the **real UI components** (color input → `set_color` red, effect
studio → `wave`, Stop button → `stop_effect`, matrix editor paint →
`set_matrix`), asserts every daemon response `ok:true`, checks the status dot,
card count vs `list_devices`, and the grayed-out unavailable card. Captures
`e2e-screenshot.png` when `SMOKE_SHOT` is set. `LIVE_DEVICE=<id>` overrides the
target (default: first available matrix device). `SMOKE_HOLD=<sec>` /
`SMOKE_HOLD_WAVE=1` keep the window open for perf sampling.

## Mock mode (no daemon / no hardware)

```sh
npm run dev            # Vite dev server + Electron, mock daemon (3 fake devices)
```

`npm run dev` sets `VITE_DEV_MOCK=1`, which injects `window.ChromaMock` at
startup: the client then talks to an in-memory daemon that implements the
frozen IPC contract — an available Razer-style matrix keyboard, an
`available: false` Logitech-style mouse with a permission note, and a bridge
device. The UI is fully demonstrable with no daemon and no hardware.

No daemon / daemon down: the header shows a red dot + offline banner and the
client retries the WebSocket every 3 s. Effects keep running on the daemon
side; the UI is just a remote.

## Build

```sh
npm run build          # tsc --noEmit (renderer + electron) + vite build → dist/
npm run dist:mac       # dmg (arm64 + x64)
npm run dist:win       # NSIS x64
npm run dist:linux     # AppImage
```

Outputs land in `release/`.

## Verify

```sh
npm run check                                            # assert-based unit self-check (no framework)
VITE_DEV_MOCK=1 node_modules/.bin/vite build --outDir dist/renderer-mock --emptyOutDir
node_modules/.bin/electron smoke.cjs dist/renderer-mock  # headless: expect online + 3 cards
node_modules/.bin/electron smoke.cjs dist/renderer offline  # headless: expect offline banner + red dot
# live (real daemon on 7474 + real hardware): see "Live E2E" above
```

## Packaging the daemon

Copy the daemon binary into `app/resources/` as:

- `chromargb-daemon` (macOS/Linux)
- `chromargb-daemon.exe` (Windows)

electron-builder ships the whole `resources/` dir via `extraResources`. At
startup the main process spawns `<Resources>/resources/chromargb-daemon`
(`./resources/chromargb-daemon` in dev) and ignores failure — the WebSocket
reconnect loop handles a missing daemon.

## Autostart

Off by default. Toggle "autostart" in the header (persisted via Electron
`setLoginItemSettings`, uses `--hidden` flag; the window starts hidden to tray
on next login). The checkbox only appears/enables inside Electron — in a plain
browser it is inert.

## Notes

- Preload exposes only `window.versions` + the autostart toggle;
  `contextIsolation: true`, `nodeIntegration: false`, `sandbox: false`.
- Profiles (named per-device effect assignments) live in `localStorage` with
  JSON export/import.
- `set_matrix`, `stop_effect` and `list_devices.available` are implemented by
  the daemon (see `core/README.md` for the IPC contract). Effect previews
  (strip canvases) are client-side approximations, not daemon truth.
