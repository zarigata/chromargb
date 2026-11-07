# ChromaRGB — Universal RGB Control for Every Keyboard, Mouse & Device

**One studio for all your RGB.** ChromaRGB is a free, open-source, cross-platform RGB lighting controller — a modern alternative to Razer Synapse, Logitech G HUB, Corsair iCUE and ASUS Aura Sync — that works with **macOS, Windows and Linux** simultaneously, with layered effects, per-key control, interactive triggers and audio-reactive lighting.

> ⚡ Like Razer Chroma Studio — but universal, lightweight (≈2% CPU), and vendor-agnostic.

![ChromaRGB Studio](https://img.shields.io/badge/UI-Chroma%20Studio-7c5cff) ![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-00e07c) ![License](https://img.shields.io/badge/license-MIT-blue)

---

## Why ChromaRGB?

| Problem with vendor tools | ChromaRGB |
|---|---|
| One app per brand, all running at once | **One daemon** drives every supported device together |
| 500 MB+ installs, login walls, telemetry | **~10 MB**, no account, no cloud, fully offline |
| Closed ecosystems, per-key lighting locked to their hardware | Open Rust + TypeScript codebase, **extensible drivers** |
| CPU-hungry compositors | **≈2% CPU** at 25 FPS via a zero-allocation frame scheduler |

## Features

### 🎨 Chroma Studio — layered lighting compositor
- **Unlimited effect layers** per device with **opacity** and **Normal / Additive blending** — stack wave + fire + ripple like Photoshop layers
- **10 effects**: Static, Wave, Spectrum, Breathe, Gradient, **Ripple**, **Reactive**, **Starlight**, **Fire**, **Audio Meter**
- **Physical keyboard canvas** — real staggered ANSI layout with key legends for Razer 6×22 matrices; click any key to spawn a live ripple on hardware
- **One-click presets**: Wave, Spectrum, Purple Ripple, Fire, Starlight, Audio Meter
- **Apply-live** mode: edits stream to hardware as you drag sliders

### ⌨️ Interactive triggers
- **Keypress ripples** — type on your keyboard and watch lighting radiate from each key (HID input monitoring; the canvas mirrors it live)
- **Click/touch triggers** — click the canvas, ripple on the real device

### 🎵 Audio-reactive lighting
- Built-in **CoreAudio RMS meter** (macOS; default mic or a loopback device like BlackHole for system audio)
- **Audio Meter effect** bands the keyboard by amplitude, live VU readout in the UI

### 🎮 Game profiles
- Per-game layer stacks with **executable bindings** — lighting switches automatically when a game launches
- Save / capture current / one-click apply

### 🖥️ Devices
- **Razer** BlackWidow-class keyboards (per-key 6×22 matrix, 103-key ANSI map extracted from OpenRGB layouts)
- **Logitech** G lights (G502, Lightspeed receivers)
- **Anything OpenRGB exposes** via the built-in OpenRGB SDK bridge (Corsair, ASUS, MSI, …)

## Install

Download the latest build for your OS from [**Releases**](../../releases):

| OS | File | Notes |
|---|---|---|
| macOS (Apple Silicon) | `ChromaRGB-…-arm64.dmg` | Grant **Input Monitoring** for keypress effects |
| macOS (Intel) | `ChromaRGB-…-x64.dmg` | Same |
| Windows | `ChromaRGB-Setup-….exe` | NSIS installer |
| Linux | `ChromaRGB-….AppImage` | chmod +x and run |

> The app auto-detects updates: **Check updates** in the header pulls the newest release from GitHub.

### macOS permissions (one-time)
1. **Input Monitoring** — enables keypress-ripple triggers (System Settings → Privacy & Security)
2. **Microphone** — enables audio-reactive lighting (only if you use it)

## Build from source

```bash
# Rust core (daemon, drivers, effects engine)
cd core && cargo build --release

# Electron app
cd app && npm install && npm run dist   # or dist:mac / dist:win / dist:linux
```

Requires Rust stable, Node 20+. Continuous builds for every push live in [Actions](../../actions).

## Architecture

```
┌────────────── Electron UI (React + TS) ──────────────┐
│  Sidebar · Devices · Chroma Studio · Profiles        │
│  live canvas (physical layout) · inspector · presets │
└─────────────────── WebSocket JSON-RPC ────────────────┘
                        │  ws://127.0.0.1:7474
┌──────────── chromargb-daemon (Rust, tokio) ──────────┐
│  device discovery · HID keypress reader              │
│  CoreAudio RMS meter · profiles · game watcher       │
├──────────────────────────────────────────────────────┤
│  chromargb-effects: layer compositor @ 25 FPS        │
│  (Normal/Add blend, ripple/reactive/starlight/fire)  │
├──────────────────────────────────────────────────────┤
│  chromargb-drivers: Razer · Logitech · OpenRGB bridge│
└──────────────────────────────────────────────────────┘
```

- **Effects are pure functions** — every effect is unit-tested frame-by-frame with no hardware
- **Zero-allocation frame loop** — buffers pre-allocated per device; the scheduler never allocates while running
- **Fast USB path** — fire-and-forget row reports with periodic validated resync keeps 25 FPS at ~2% CPU
- **Push events over WebSocket** — keypresses and audio levels stream to the UI in real time

## Roadmap

- [ ] More vendors: Corsair native, SteelSeries, Keychron
- [ ] Effect sharing (import/export layer stacks as files)
- [ ] Advanced triggers (alarm, Discord, stream events)

## Contributing

PRs welcome — driver backends are ~200 lines each (see `core/chromargb-drivers/src/razer/mod.rs` as the template). `cargo test --workspace && cd app && npm run check` must pass.

## License

MIT — see [LICENSE](LICENSE).
