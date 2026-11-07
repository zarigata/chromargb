# ChromaRGB core

Rust workspace: RGB device discovery + daemon IPC + native drivers (Razer,
Logitech) + effects engine, all wired together. See the contracts below.

## Layout

```
chromargb-drivers   lib  — hidapi discovery, (vid,pid) model registry, DriverBackend trait
chromargb-effects   lib  — effect library + one background scheduler thread
chromargb-daemon    bin  — WebSocket JSON IPC server on 127.0.0.1:7474
chromargb-cli       bin  — `chromargb-cli list` / `chromargb-cli doctor`
```

## Build & run

```sh
cargo build --workspace
cargo test --workspace                    # unit tests (hardware-free)
cargo test -p chromargb-drivers -- --ignored   # + real-hardware discovery test

cargo run -p chromargb-cli -- list        # print recognized RGB devices
cargo run -p chromargb-cli -- doctor      # self-checks (no lighting tests)
cargo run -p chromargb-daemon             # starts IPC server, prints devices
```

`doctor` checks: hidapi initializes, device count, per-device backend can be
constructed, plus an open-only HID probe. On macOS, opening Razer devices can
fail due to permissions — that is reported as `[WARN]`, not a crash. No
lighting commands are ever sent by doctor.

## IPC JSON contract (ws://127.0.0.1:7474)

Request: `{"id": N, "method": "...", "params": {...}}` — `id` is echoed.
Response: `{"id": N, "ok": true, "data": ...}` or `{"id": N, "ok": false, "error": "..."}`.

| Method | Params | Data / Error |
|---|---|---|
| `list_devices` | none | array of `DeviceInfo` (below) with `available`, `openError` (string\|null), `errorKind` (`"permission"` \| `"busy"` \| `"unknown"` \| null) — `false` devices (e.g. macOS HID permission denied) stay listed but reject writes; `errorKind` drives the UI's Fix Permissions / Retry flow |
| `get_state` | none | array of `{id, color, effect, mode, matrix}` — `color` `[r,g,b]` or `null`; `effect` name or `null`; `mode` ∈ effect/static/matrix/idle; `matrix` = `{zone: [[r,g,b],...]}` when a matrix was applied |
| `set_color` | `{device, color: [r,g,b]}` | `{device, color}`; stops any running effect first (static overrides); errors: unknown device, bad color, unavailable device, or driver error |
| `set_effect` | `{device, effect, params}` | `{device, effect}`; `effect` ∈ static/wave/spectrum/breathe/gradient with typed params (`Effect::from_json` validates: speed 0-100, direction "left"/"right", colors ≥ 2 stops...); parse errors name the effect + field |
| `stop_effect` | `{device}` | `{device, stopped: true}` — freezes the last rendered frame |
| `set_matrix` | `{device, zone, colors: [[r,g,b],...]}` | `{device, zone, leds}`; validates zone exists and `colors.len() == zone.leds` (row-major within the zone matrix), stops any running effect, writes the full device frame (other zones black) |
| `refresh_devices` | none | `{devices, ready, unavailable}` — HID re-enumeration + OpenRGB bridge re-query + re-open of every device (also the retry path after granting macOS permissions). Old handles are dropped **before** new opens (macOS exclusive access). Lighting state resets |
| `profile_save` | `{profile}` | `{saved, warnings[]}` — upsert `~/.config/chromargb/profiles/<name>.json`. Schema: `{name, bindings: [app-name substrings], devices: {id: {kind:"static",color} \| {kind:"effect",effect,params} \| {kind:"matrix",zone,colors}}}`. Validates entries; matrix length checked against live zone when the device is present |
| `profile_list` | none | `[{name, bindings, devices: [ids]}]` |
| `profile_get` | `{name}` | full profile object |
| `profile_delete` | `{name}` | `{deleted}` |
| `profile_apply` | `{name}` | `{results: {id: "ok"\|"error: ..."}, warnings[]}` — applies every entry to currently-available devices |
| `get_active_profile` | none | `{name, watcher: "on"\|"off", frontmost}` — the daemon polls the frontmost app every 3 s (macOS osascript) and auto-applies a matching profile (case-insensitive substring on `bindings`) |

`device` accepts the device `id` or its exact `name`. Bad request JSON gets
`{"id": null, "ok": false, "error": "bad json: ..."}`.

Notes on wiring (verified against a real BlackWidow Elite + G502 HERO):
- macOS opens HID devices **exclusively** — the daemon opens each backend
  once at startup and shares that handle between the effects engine and
  manual `set_color`/`set_matrix` writes (a second `open` fails with
  IOHID `0xE00002C5`).
- A device whose startup open fails (G502 without Input Monitoring
  permission → `privilege violation`) is served with `available: false`;
  every write method returns `device <id> unavailable: <original error>`.
- Effects render at 12 FPS: the Razer driver's validated per-frame protocol
  costs ~3.3 ms CPU, so 30 FPS would burn ~10% CPU on a Mac; 12 FPS keeps
  animation ≈4% CPU / 4.5 MB RSS with idle at 0.0%.

### `DeviceInfo` (serialized)

```json
{
  "id": "1532:0228:XX12345",   // vid:pid[:serial]
  "name": "BlackWidow Elite",
  "vendor": "Razer",
  "vid": 5426, "pid": 552,
  "kind": "Razer",              // Razer | Logitech | Generic
  "zones": [{"name": "matrix", "leds": 132, "matrix": [6, 22]}],
  "note": null,                 // set for generic/unrecognized devices
  "available": true             // backend opened at startup; false => writes reject
}
```

## DriverBackend contract (for Razer/Logitech/OpenRGB driver workers)

```rust
pub trait DriverBackend: Send {
    fn info(&self) -> DeviceInfo;                    // id, name, vendor, vid, pid, zones
    fn set_static(&mut self, color: Rgb) -> Result<()>;
    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()>; // len == total leds
    fn supports_per_led(&self) -> bool { false }
}
```

- Add your device's `(vid, pid)` to the `MODELS` registry in
  `chromargb-drivers/src/lib.rs` (copy zone specs from OpenRGB sources), or
  extend `open_backend()` to match on `kind`/`(vid,pid)` and return your
  backend instead of the protocol-less `GenericBackend`.
- Open the HID handle with `HidApi::open_path(&dev.path)`. Note: a physical
  device exposes several HID interfaces and discovery currently keeps the
  **first** one — select the correct control interface (usage page) inside
  your backend; `DiscoveredDevice` carries only the first path.
- Errors: `Error::Unsupported` until implemented, `Error::Hid(String)` from
  hidapi. Keep it that way; `thiserror` only.
- Registry zone counts are best-effort until verified against hardware
  (BlackWidow Elite: 6×22 matrix = 132; G502 HERO: logo + wheel zones).

## Registry behavior

Exact `(vid,pid)` matches return the named model. Unrecognized PIDs on Razer
(0x1532) / Logitech (0x046d) VIDs list as `Generic` with a `note` ("no
registry entry for ... PID ..."). Everything else is skipped by discovery.
