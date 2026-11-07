//! chromargb-daemon: WebSocket JSON IPC server on 127.0.0.1:7474.
//!
//! Device discovery + static color + effects engine + per-zone matrix frames +
//! hotplug refresh + on-disk per-game profiles with a foreground-app watcher.
//! Effects run in the engine's own scheduler thread (daemon idle = no frame
//! work); the daemon only relays start/stop commands and writes manual
//! static/matrix frames through the shared backends.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::error::{Error as WsError, ProtocolError};
use tokio_tungstenite::tungstenite::Message;

use chromargb_drivers::hidapi::HidApi;
use chromargb_drivers::{
    devices_table, discover, open_backend, DiscoveredDevice, DriverBackend, DriverKind, Rgb,
};
use chromargb_effects::{Effect, Engine, Layer};

const ADDR: &str = "127.0.0.1:7474";

/// Engine renders at 25 FPS: the Razer driver's fast frame path
/// (fire-and-drain row reports, one validated frame per 61) keeps a frame
/// ~<5% CPU, and ripples need ~25 FPS to look liquid. After `engine.stop()`
/// (a fire-and-forget channel command) one frame may still be in flight;
/// sleeping ~2 frame periods before a manual write guarantees our frame
/// lands last.
const SETTLE: Duration = Duration::from_millis(80);

/// Foreground-app poll interval for the profile auto-switcher.
const WATCH_POLL: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Error classification (drives the UI's Fix Permissions flow)
// ---------------------------------------------------------------------------

/// "permission" | "busy" | "unknown"
fn classify_error(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("privilege violation")
        || m.contains("permission")
        || m.contains("not permitted")
        || m.contains("input monitoring")
        || m.contains("could not open any hid interface")
    {
        "permission"
    } else if m.contains("already open") || m.contains("busy") || m.contains("0xe00002c5") {
        "busy"
    } else {
        "unknown"
    }
}

/// Ask macOS for Input Monitoring via the public IOKit SPI-adjacent API —
/// shows the system TCC prompt once (per app), so the user can grant without
/// digging through System Settings. kIOReturnSuccess (0) = granted.
/// Linked straight to the IOKit framework hidapi already links — no new dep.
#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDRequestAccess(access_type: u32) -> i32;
    fn IOHIDCheckAccess(access_type: u32) -> i32;
}

/// kIOHIDRequestAccessTypeListenEvent — the Input Monitoring grant that
/// gates IOHIDDeviceOpen on vendor interfaces (10.15+).
#[cfg(target_os = "macos")]
const HID_ACCESS_LISTEN_EVENT: u32 = 1;

#[cfg(target_os = "macos")]
fn request_hid_access() -> bool {
    // Safety: both functions are plain syscalls into IOKit, no state.
    unsafe { IOHIDRequestAccess(HID_ACCESS_LISTEN_EVENT) == 0 }
}

#[cfg(not(target_os = "macos"))]
fn request_hid_access() -> bool {
    false
}

/// One HID handle per device, shared between the effects engine's scheduler
/// thread and the daemon's manual writes. macOS opens HID devices
/// exclusively (second open fails with 0xE00002C5 "device already open"), so
/// set_color/set_matrix MUST write through the same backend the engine owns
/// — a fresh `open_backend()` per request is not possible on this platform.
struct SharedBackend(Arc<Mutex<Box<dyn DriverBackend>>>);

impl DriverBackend for SharedBackend {
    fn info(&self) -> chromargb_drivers::DeviceInfo {
        self.0.lock().unwrap().info()
    }
    fn set_static(&mut self, color: Rgb) -> chromargb_drivers::Result<()> {
        self.0.lock().unwrap().set_static(color)
    }
    fn write_frame(&mut self, colors: &[Rgb]) -> chromargb_drivers::Result<()> {
        self.0.lock().unwrap().write_frame(colors)
    }
    fn supports_per_led(&self) -> bool {
        self.0.lock().unwrap().supports_per_led()
    }
}

/// Manual clone (DiscoveredDevice has no derive and lib.rs is owned by
/// another workstream).
fn clone_dev(d: &DiscoveredDevice) -> DiscoveredDevice {
    DiscoveredDevice {
        info: d.info.clone(),
        serial: d.serial.clone(),
        path: d.path.clone(),
    }
}

struct State {
    /// Process-wide HidApi (see [`shared_hid`] for why exactly one).
    api: &'static Mutex<HidApi>,
    devices: RwLock<Vec<DiscoveredDevice>>,
    /// The one shared backend per device (see [`SharedBackend`]). Absent =>
    /// the open failed and the device is unavailable (reason in open_errors).
    handles: Mutex<HashMap<String, Arc<Mutex<Box<dyn DriverBackend>>>>>,
    /// Open error per unavailable device id; relayed verbatim to IPC callers.
    open_errors: Mutex<HashMap<String, String>>,
    /// Effects engine (owns a SharedBackend per device). Behind a Mutex:
    /// Engine is !Sync. Locked only for sync start/stop calls — never across
    /// an await.
    engine: Arc<Mutex<Engine>>,
    /// Last successfully applied static color per device id.
    colors: Mutex<HashMap<String, Rgb>>,
    /// Active effect name per device id (absent = static/manual lighting).
    effects: Mutex<HashMap<String, String>>,
    /// Active layer stack JSON per device id (set_layers; the validated
    /// source for get_state + profile capture). Absent = no layers active.
    layers: Mutex<HashMap<String, Value>>,
    /// Last applied matrix per device id, per zone name.
    matrices: Mutex<HashMap<String, HashMap<String, Vec<Rgb>>>>,
    /// Keypress reader threads (one per available Razer matrix device).
    /// rebuild_devices sets stop flags and joins the old threads, then
    /// spawns fresh ones for the new device set.
    key_readers: Mutex<Vec<KeyReader>>,
    /// Push-event fanout to connected ws clients (key presses, audio level).
    events_tx: broadcast::Sender<String>,
    /// Audio meter state (shared with the CoreAudio thread).
    audio: AudioState,
    /// Profile auto-switcher state.
    active_profile: Mutex<Option<String>>,
    watcher_on: Mutex<bool>,
    frontmost: Mutex<Option<String>>,
    config_dir: PathBuf,
}

/// One live keypress reader thread: its stop flag (polled between reads)
/// and JoinHandle. Drop order in rebuild: set flag -> sleep out the read
/// timeout -> join.
struct KeyReader {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

/// Shared audio meter state: on/off toggle (IPC set_audio), latest level
/// 0..=1000 (CoreAudio IOProc writes, get_audio_level reads). Arc'd cells:
/// the meter thread keeps writing the exact atomics IPC reads.
struct AudioState {
    on: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
}

/// ponytail: 25 FPS engine pacing — the Razer driver's fast frame path
/// (fire-and-drain row reports; one fully-validated frame per 61) makes a
/// frame ~<5% CPU, so 25 FPS keeps ripples liquid while staying inside the
/// 5% budget. Upgrade = batch row reports, then raise toward 60.
fn new_engine() -> Engine {
    Engine::with_frame_period(Duration::from_millis(40))
}

/// Config dir: $CHROMARGB_CONFIG_DIR (tests), else %APPDATA%\chromargb on
/// Windows, $HOME/.config/chromargb elsewhere.
fn default_config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CHROMARGB_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    if std::env::var("OS").as_deref() == Ok("Windows_NT") {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("chromargb");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("chromargb")
}

impl State {
    fn new(api: &'static Mutex<HidApi>, devices: Vec<DiscoveredDevice>, config_dir: PathBuf) -> State {
        let (events_tx, _) = broadcast::channel(64);
        let state = State {
            api,
            devices: RwLock::new(Vec::new()),
            handles: Mutex::new(HashMap::new()),
            open_errors: Mutex::new(HashMap::new()),
            engine: Arc::new(Mutex::new(new_engine())),
            colors: Mutex::new(HashMap::new()),
            effects: Mutex::new(HashMap::new()),
            layers: Mutex::new(HashMap::new()),
            matrices: Mutex::new(HashMap::new()),
            key_readers: Mutex::new(Vec::new()),
            events_tx,
            audio: AudioState { on: Arc::new(AtomicBool::new(false)), level: Arc::new(AtomicU32::new(0)) },
            active_profile: Mutex::new(None),
            watcher_on: Mutex::new(false),
            frontmost: Mutex::new(None),
            config_dir,
        };
        state.rebuild_devices(devices);
        state
    }

    /// (Re)open every device, rebuild the engine, swap device state. Shared
    /// by startup and refresh_devices. Lighting state (colors/effects/
    /// matrices) resets: the new engine starts dark — ceiling accepted, a
    /// re-apply of the active profile below softens it for profile users.
    /// Returns (ready, unavailable).
    fn rebuild_devices(&self, devices: Vec<DiscoveredDevice>) -> (usize, usize) {
        // Release OLD handles FIRST: macOS HID is exclusive-access, so new
        // opens fail while the old engine still holds the devices. Dropping
        // the old engine stops its scheduler thread AND closes its handles.
        *self.engine.lock().unwrap() = new_engine();
        let mut h = self.handles.lock().unwrap();
        let old = std::mem::take(&mut *h);
        drop(h);
        drop(old);
        let mut handles = HashMap::new();
        let mut open_errors = HashMap::new();
        let mut engine = new_engine();
        let mut requested_access = false;
        for dev in &devices {
            match open_backend(dev) {
                Ok(backend) => {
                    let shared = Arc::new(Mutex::new(backend));
                    handles.insert(dev.info.id.clone(), Arc::clone(&shared));
                    engine.add_device(&dev.info.id, Box::new(SharedBackend(Arc::clone(&shared))));
                }
                Err(e) => {
                    let mut msg = e.to_string();
                    eprintln!("device {} ({}) unavailable: {msg}", dev.info.id, dev.info.name);
                    if classify_error(&msg) == "permission" && !requested_access {
                        // One TCC prompt per rebuild: request Input Monitoring
                        // and retry this open if granted. A later grant is
                        // picked up by refresh_devices (the UI's Retry).
                        requested_access = true;
                        if request_hid_access() {
                            println!("input monitoring granted — retrying device opens");
                            match open_backend(dev) {
                                Ok(backend) => {
                                    let shared = Arc::new(Mutex::new(backend));
                                    handles.insert(dev.info.id.clone(), Arc::clone(&shared));
                                    engine.add_device(
                                        &dev.info.id,
                                        Box::new(SharedBackend(Arc::clone(&shared))),
                                    );
                                    continue;
                                }
                                Err(e2) => msg = e2.to_string(),
                            }
                        }
                    }
                    if classify_error(&msg) == "permission" {
                        println!(
                            "fix: System Settings > Privacy & Security > Input Monitoring (or Accessibility) \
                             -> enable for this app/terminal, then click Retry in ChromaRGB"
                        );
                    }
                    open_errors.insert(dev.info.id.clone(), msg);
                }
            }
        }
        let ready = handles.len();
        let unavail = open_errors.len();
        *self.engine.lock().unwrap() = engine;
        *self.handles.lock().unwrap() = handles;
        *self.open_errors.lock().unwrap() = open_errors;
        *self.devices.write().unwrap() = devices;
        self.colors.lock().unwrap().clear();
        self.effects.lock().unwrap().clear();
        self.layers.lock().unwrap().clear();
        self.matrices.lock().unwrap().clear();
        // Stop old key readers first (they read the device set we just
        // swapped), then spawn for the new AVAILABLE Razer matrix devices.
        self.stop_key_readers();
        self.spawn_key_readers();
        (ready, unavail)
    }

    /// Set every reader's stop flag and join the threads. Joins block up to
    /// one read timeout (~150ms per thread); they run one at a time in the
    /// worst case, still well under interactive notice.
    fn stop_key_readers(&self) {
        let mut readers = self.key_readers.lock().unwrap();
        for mut r in std::mem::take(&mut *readers) {
            r.stop.store(true, Ordering::Relaxed);
            if let Some(h) = r.handle.take() {
                let _ = h.join();
            }
        }
    }

    /// One reader thread per AVAILABLE device that is a Razer keyboard with
    /// a matrix zone. The thread shares the process-wide HidApi (see
    /// [`shared_hid`]); an open failure (commonly missing Input Monitoring
    /// for the input interface) is logged once and the thread exits — the
    /// UI-click trigger still works as fallback.
    fn spawn_key_readers(&self) {
        let mut readers = Vec::new();
        for dev in self.devices.read().unwrap().iter() {
            if dev.info.kind != DriverKind::Razer
                || !dev.info.zones.iter().any(|z| z.matrix.is_some())
                || !self.handles.lock().unwrap().contains_key(&dev.info.id)
            {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let handle = thread::Builder::new()
                .name(format!("chromargb-keys-{}", dev.info.id))
                .spawn({
                    let stop = Arc::clone(&stop);
                    let dev = clone_dev(dev);
                    let engine = Arc::clone(&self.engine);
                    let tx = self.events_tx.clone();
                    move || key_reader_thread(dev, stop, engine, tx)
                })
                .ok()
                .map(|h| h);
            readers.push(KeyReader { stop, handle });
        }
        *self.key_readers.lock().unwrap() = readers;
    }

    /// Full re-scan: HID refresh + discovery + OpenRGB bridge + re-open.
    fn refresh(&self) -> Result<(usize, usize, usize), String> {
        // Scope the api guard to discovery: rebuild joins reader threads
        // that lock this same api for their input-interface open, so holding
        // it across rebuild could deadlock a join against a blocked open.
        let mut devices = {
            let mut api = self.api.lock().unwrap();
            api.refresh_devices().map_err(|e| e.to_string())?;
            discover(&api)
        };
        match chromargb_drivers::openrgb_bridge::discover_bridge("127.0.0.1:6742") {
            Ok(bridged) => devices.extend(bridged),
            Err(_) => {} // missing OpenRGB server is normal
        }
        let total = devices.len();
        let (ready, unavail) = self.rebuild_devices(devices);
        Ok((total, ready, unavail))
    }
}

/// One HidApi per PROCESS. hidapi's macOS backend registers HID devices
/// with the thread's CFRunLoop; a second HidApi constructed after another
/// test's run loop was scheduled trips CoreFoundation's
/// _CFAssertMismatchedTypeID (SIGILL). Production already has exactly one;
/// tests must match that shape. Lazy + OnceLock, never refreshed here.
fn shared_hid() -> &'static std::sync::Mutex<HidApi> {
    static API: std::sync::OnceLock<std::sync::Mutex<HidApi>> = std::sync::OnceLock::new();
    API.get_or_init(|| {
        let api = HidApi::new().expect("hidapi init");
        std::sync::Mutex::new(api)
    })
}

/// startup_state kept for tests: shares the process-wide HidApi (harmless
/// without hardware) and the default config dir.
fn startup_state(devices: Vec<DiscoveredDevice>) -> State {
    set_shared_api(shared_hid());
    State::new(shared_hid(), devices, default_config_dir())
}

// ---------------------------------------------------------------------------
// Device helpers
// ---------------------------------------------------------------------------

/// Resolve `device` (id or exact name) to a cloned registry entry.
fn find_device(state: &State, device: &str) -> Result<DiscoveredDevice, String> {
    state
        .devices
        .read()
        .unwrap()
        .iter()
        .find(|d| d.info.id == device || d.info.name == device)
        .map(clone_dev)
        .ok_or_else(|| format!("unknown device: {device}"))
}

fn unavailable_error(state: &State, dev: &DiscoveredDevice) -> String {
    let reason = state
        .open_errors
        .lock()
        .unwrap()
        .get(&dev.info.id)
        .cloned()
        .unwrap_or_else(|| "backend failed to open at startup".into());
    format!("device {} unavailable: {reason}", dev.info.id)
}

fn require_handle(
    state: &State,
    dev: &DiscoveredDevice,
) -> Result<Arc<Mutex<Box<dyn DriverBackend>>>, String> {
    state
        .handles
        .lock()
        .unwrap()
        .get(&dev.info.id)
        .cloned()
        .ok_or_else(|| unavailable_error(state, dev))
}

/// Stop any running effect on `dev` and wait out the engine's frame cycle so
/// a manual write through the shared handle lands after the last effect
/// frame. Callers must then write promptly.
fn stop_effect_for_manual_write(state: &State, dev: &DiscoveredDevice) {
    let _ = state.engine.lock().unwrap().stop(&dev.info.id);
    state.effects.lock().unwrap().remove(&dev.info.id);
    state.layers.lock().unwrap().remove(&dev.info.id);
    thread::sleep(SETTLE);
}

// --- apply helpers shared by IPC handlers and profile_apply ---------------

fn apply_static(state: &State, dev: &DiscoveredDevice, color: Rgb) -> Result<(), String> {
    let handle = require_handle(state, dev)?;
    stop_effect_for_manual_write(state, dev);
    handle.lock().unwrap().set_static(color).map_err(|e| e.to_string())?;
    state.colors.lock().unwrap().insert(dev.info.id.clone(), color);
    state.matrices.lock().unwrap().remove(&dev.info.id);
    Ok(())
}

fn apply_effect(
    state: &State,
    dev: &DiscoveredDevice,
    name: &str,
    params: &Value,
) -> Result<(), String> {
    if state.handles.lock().unwrap().get(&dev.info.id).is_none() {
        return Err(unavailable_error(state, dev));
    }
    let effect = Effect::from_json(name, params).map_err(|e| e)?;
    state
        .engine
        .lock()
        .unwrap()
        .start(&dev.info.id, effect)
        .map_err(|e| e.to_string())?;
    state.effects.lock().unwrap().insert(dev.info.id.clone(), name.to_string());
    state.layers.lock().unwrap().remove(&dev.info.id);
    state.colors.lock().unwrap().remove(&dev.info.id);
    state.matrices.lock().unwrap().remove(&dev.info.id);
    Ok(())
}

/// Parse `layers` (array of layer entries) and (re)apply the stack on one
/// device. On any parse error returns "layers[i]: <message>" so callers can
/// relay it verbatim. Shared by set_layers IPC and the profile "layers"
/// kind. Stores the validated JSON on success (get_state + profile capture).
fn apply_layers(state: &State, dev: &DiscoveredDevice, layers: &Value) -> Result<(), String> {
    let Some(entries) = layers.as_array() else {
        return Err("layers: expected an array of layer objects".into());
    };
    let mut parsed = Vec::with_capacity(entries.len());
    for (i, l) in entries.iter().enumerate() {
        let Some(effect) = l.get("effect").and_then(Value::as_str) else {
            return Err(format!("layers[{i}]: missing 'effect' name"));
        };
        let params = l.get("params").cloned().unwrap_or_else(|| json!({}));
        let layer = Layer::from_json(effect, &params, l).map_err(|e| format!("layers[{i}]: {e}"))?;
        parsed.push(layer);
    }
    if state.handles.lock().unwrap().get(&dev.info.id).is_none() {
        return Err(unavailable_error(state, dev));
    }
    state
        .engine
        .lock()
        .unwrap()
        .set_layers(&dev.info.id, parsed)
        .map_err(|e| e.to_string())?;
    state.layers.lock().unwrap().insert(dev.info.id.clone(), layers.clone());
    state.colors.lock().unwrap().remove(&dev.info.id);
    state.effects.lock().unwrap().remove(&dev.info.id);
    state.matrices.lock().unwrap().remove(&dev.info.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Keypress reader (interactive triggers + ws push events)
// ---------------------------------------------------------------------------

/// Reader loop for one Razer matrix device's keyboard input interface.
/// Reads boot reports (100ms timeout), maps usages to matrix cells, and for
/// each NEWLY pressed key: triggers an engine event at the cell center
/// (normalized) and broadcasts a push frame to ws clients. Open failure =
/// log once + return (UI-click trigger is the fallback); a read error (USB
/// unplug) ends the loop until the next rebuild re-spawns it.
fn key_reader_thread(
    dev: DiscoveredDevice,
    stop: Arc<AtomicBool>,
    engine: Arc<Mutex<Engine>>,
    tx: broadcast::Sender<String>,
) {
    // matrix dims come from the device's own zone geometry (6x22 for the
    // BlackWidow Elite); ripple/reactive normalize against them
    let Some((rows, cols)) = dev.info.zones.iter().find_map(|z| z.matrix) else {
        return;
    };
    let input = {
        // The reader shares the process-wide HidApi (CoreFoundation SIGILL
        // otherwise — see shared_hid); API_CELL hands out the &'static.
        // The mutex guard lives only for the open call itself.
        let Some(api) = API_CELL.get() else {
            return; // no shared HidApi registered: nothing to read
        };
        let api_guard = api.lock().unwrap();
        match chromargb_drivers::razer::open_input_interface(&api_guard, dev.info.vid, dev.info.pid) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "key reader {}: input interface unavailable ({e}) — \
                     interactive key triggers off; UI canvas clicks still work",
                    dev.info.id
                );
                return;
            }
        }
    };
    let mut pressed = HashMap::new(); // usage -> (), membership only
    let mut buf = [0u8; 8];
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Ok(n) = input.read_timeout(&mut buf, 100) else {
            return; // device gone: next rebuild re-spawns
        };
        if n != 8 {
            // Only true 8-byte boot-keyboard reports decode here; other
            // lengths (e.g. 4-byte consumer-page media reports from the same
            // interface) would alias usages onto wrong cells — e.g. consumer
            // 0xE2 (mute) IS keyboard Left Alt 0xE2.
            continue;
        }
        let usages = chromargb_drivers::razer::decode_boot_report(&buf[..n]);
        for usage in usages {
            if pressed.insert(usage, ()).is_some() {
                continue; // key repeat: only new presses trigger
            }
            let Some((row, col)) = chromargb_drivers::razer::key_to_cell(usage) else {
                continue;
            };
            let _ = engine.lock().unwrap().trigger(
                &dev.info.id,
                (col as f32 + 0.5) / cols as f32,
                (row as f32 + 0.5) / rows as f32,
            );
            let _ = tx.send(
                json!({"event": "key", "device": dev.info.id, "row": row, "col": col, "code": usage})
                    .to_string(),
            );
        }
        // release pruning: keep only usages present in the current report
        pressed.retain(|&u, _| buf[..n].iter().skip(2).take(6).any(|&b| b as u16 == u));
    }
}

/// ponytail ceiling: the reader threads find the shared HidApi via a
/// process-global OnceLock instead of receiving `&'static Mutex<HidApi>`
/// through the spawn closure (State.api is a plain shared reference and
/// cannot cross into an owned thread). Upgrade = make State.api an
/// Arc'd handle threaded into every reader spawn.
static API_CELL: std::sync::OnceLock<&'static Mutex<HidApi>> = std::sync::OnceLock::new();

fn set_shared_api(api: &'static Mutex<HidApi>) {
    let _ = API_CELL.set(api);
}

// ---------------------------------------------------------------------------
// Audio meter (macOS CoreAudio; no-op elsewhere)
// ---------------------------------------------------------------------------

/// Latest meter level 0..=1000 (0 when the meter is off or silent).
fn audio_level_now(state: &State) -> u32 {
    state.audio.level.load(Ordering::Relaxed).min(1000)
}

/// ponytail: hand-rolled CoreAudio extern "C" bindings — no new dependency,
/// same pattern as the daemon's IOKit link above. Ceiling: fixed C types,
/// default input device only (pick BlackHole/other inputs in macOS Sound
/// settings), no AudioToolbox convenience APIs. Upgrade = coreaudio-sys if
/// device enumeration/selection is ever needed.
#[cfg(target_os = "macos")]
mod coreaudio {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde_json::json;
    use tokio::sync::broadcast;

    use chromargb_effects::Engine;

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyData(
            inObjectID: u32,
            inAddress: *const AudioObjectPropertyAddress,
            inQualifierDataSize: u32,
            inQualifierData: *const c_void,
            ioDataSize: *mut u32,
            outData: *mut c_void,
        ) -> i32; // OSStatus
        fn AudioDeviceCreateIOProcID(
            inDevice: u32,
            inProc: AudioDeviceIOProc,
            inClientData: *mut c_void,
            outIOProcID: *mut *mut c_void,
        ) -> i32;
        fn AudioDeviceDestroyIOProcID(inDevice: u32, inIOProcID: *mut c_void) -> i32;
        fn AudioDeviceStart(inDevice: u32, inIOProcID: *mut c_void) -> i32;
    }

    // kAudioObjectSystemObject / kAudioHardwarePropertyDefaultInputDevice /
    // 'glob' scope / main element, from CoreAudioBaseTypes.h + Hardware.h.
    const SYSTEM_OBJECT: u32 = 1;
    const DEFAULT_INPUT_DEVICE: u32 = 0x23;
    const SCOPE_GLOBAL: u32 = 0x676c_6f62;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        mSelector: u32,
        mScope: u32,
        mElement: u32,
    }

    #[repr(C)]
    struct AudioBuffer {
        mNumberChannels: u32,
        mDataByteSize: u32,
        mData: *mut c_void,
    }

    #[repr(C)]
    struct AudioBufferList {
        mNumberBuffers: u32,
        mBuffers: [AudioBuffer; 1],
    }

    type AudioDeviceIOProc = extern "C" fn(
        inDevice: u32,
        inNow: *const c_void,
        inInputData: *const AudioBufferList,
        inInputTime: *const c_void,
        inOutputData: *mut c_void,
        inClientData: *mut c_void,
    ) -> i32;

    /// Callback context. Leaked ONCE per process at first meter start; the
    /// IOProc outlives the spawning thread, so ownership stays with CoreAudio.
    /// `engine` is State's own Arc — rebuild swaps the Engine INSIDE the
    /// mutex, so the meter always writes the current one.
    struct Ctx {
        engine: Arc<Mutex<Engine>>,
        tx: broadcast::Sender<String>,
        on: Arc<AtomicBool>,
        level: Arc<AtomicU32>,
        last_bcast_ms: AtomicU64,
    }

    /// RMS over the input buffers -> level 0..=1000 -> engine + ws push
    /// (throttled to >=100ms between pushes). Lock-free besides the brief
    /// engine mutex; no allocation on the audio thread.
    extern "C" fn io_proc(
        _dev: u32,
        _now: *const c_void,
        input: *const AudioBufferList,
        _in_time: *const c_void,
        _out: *mut c_void,
        client: *mut c_void,
    ) -> i32 {
        let ctx = unsafe { &*(client as *const Ctx) };
        if !ctx.on.load(Ordering::Relaxed) {
            return 0;
        }
        // Safety: CoreAudio passes a valid ABL for the input scope; the
        // buffer array is a flexible-array-style trailing field.
        let mut sum = 0.0f64;
        let mut count = 0usize;
        unsafe {
            let abl = &*input;
            let bufs: &[AudioBuffer] =
                std::slice::from_raw_parts(abl.mBuffers.as_ptr(), abl.mNumberBuffers as usize);
            for b in bufs {
                if b.mData.is_null() || b.mDataByteSize < 4 {
                    continue;
                }
                let samples = std::slice::from_raw_parts(
                    b.mData as *const f32,
                    b.mDataByteSize as usize / 4,
                );
                sum += samples.iter().map(|&s| (s * s) as f64).sum::<f64>();
                count += samples.len();
            }
        }
        let rms = if count > 0 { (sum / count as f64).sqrt() } else { 0.0 };
        let level = (rms.min(1.0) * 1000.0) as u32;
        ctx.level.store(level, Ordering::Relaxed);
        ctx.engine.lock().unwrap().set_audio_level(level);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now_ms.saturating_sub(ctx.last_bcast_ms.load(Ordering::Relaxed)) >= 100 {
            ctx.last_bcast_ms.store(now_ms, Ordering::Relaxed);
            let _ = ctx.tx.send(json!({"event": "audio", "level": level}).to_string());
        }
        0
    }

    /// First call spawns the ONE meter thread for the process (opens the
    /// default input device, installs the IOProc, AudioDeviceStart); later
    /// calls are no-ops — toggling is just the shared `on` flag. Any
    /// failure logs once and leaves the thread dead: startup must not
    /// block or panic on a missing input device.
    pub fn spawn_meter_once(
        engine: Arc<Mutex<Engine>>,
        tx: broadcast::Sender<String>,
        on: Arc<AtomicBool>,
        level: Arc<AtomicU32>,
    ) -> bool {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::SeqCst) {
            return true;
        }
        thread::Builder::new()
            .name("chromargb-audio".into())
            .spawn(move || {
                // Bump STARTED back down on every failure exit so a later
                // set_audio {on:true} can retry after e.g. a mic is plugged
                // in — the flag only latches once the IOProc truly runs.
                macro_rules! bail {
                    ($($arg:tt)*) => {{
                        eprintln!($($arg)*);
                        STARTED.store(false, Ordering::SeqCst);
                        return;
                    }};
                }
                let addr = AudioObjectPropertyAddress {
                    mSelector: DEFAULT_INPUT_DEVICE,
                    mScope: SCOPE_GLOBAL,
                    mElement: 0, // kAudioObjectPropertyElementMain
                };
                let mut dev: u32 = 0;
                let mut size: u32 = 4;
                // Safety: plain property get by address into a local.
                let st = unsafe {
                    AudioObjectGetPropertyData(
                        SYSTEM_OBJECT,
                        &addr,
                        0,
                        std::ptr::null(),
                        &mut size,
                        &mut dev as *mut u32 as *mut c_void,
                    )
                };
                if st != 0 || dev == 0 {
                    bail!("audio meter: no default input device (status {st:#x}) — audio-reactive lighting off");
                }
                let ctx = Box::into_raw(Box::new(Ctx {
                    engine,
                    tx,
                    on,
                    level,
                    last_bcast_ms: AtomicU64::new(0),
                }));
                let mut ioproc: *mut c_void = std::ptr::null_mut();
                let st = unsafe { AudioDeviceCreateIOProcID(dev, io_proc, ctx as *mut c_void, &mut ioproc) };
                if st != 0 {
                    eprintln!("audio meter: AudioDeviceCreateIOProcID failed (status {st:#x}) — audio-reactive lighting off");
                    unsafe { drop(Box::from_raw(ctx)) };
                    STARTED.store(false, Ordering::SeqCst);
                    return;
                }
                let st = unsafe { AudioDeviceStart(dev, ioproc) };
                if st != 0 {
                    eprintln!("audio meter: AudioDeviceStart failed (status {st:#x}) — audio-reactive lighting off");
                    unsafe {
                        AudioDeviceDestroyIOProcID(dev, ioproc);
                        drop(Box::from_raw(ctx));
                    }
                    STARTED.store(false, Ordering::SeqCst);
                    return;
                }
                println!("audio meter: running on default input device {dev}");
                // The IOProc is process-lifetime; this thread just parks.
                loop {
                    thread::sleep(Duration::from_secs(3600));
                }
            })
            .is_ok()
    }
}

#[cfg(target_os = "macos")]
use coreaudio::spawn_meter_once;

/// IPC set_audio {on}: toggles the meter. The first "on" spawns the meter
/// (macOS only); toggling is the shared flag the IOProc checks per
/// callback. "off" also zeroes the level so layers drop to black.
fn set_audio(state: &State, id: Value, params: &Value) -> Value {
    let Some(on) = params.get("on").and_then(Value::as_bool) else {
        return err_response(id, "missing param 'on' (bool)");
    };
    if on && !state.audio.on.load(Ordering::Relaxed) {
        #[cfg(target_os = "macos")]
        {
            if !spawn_meter_once(
                Arc::clone(&state.engine),
                state.events_tx.clone(),
                Arc::clone(&state.audio.on),
                Arc::clone(&state.audio.level),
            ) {
                return err_response(id, "audio meter thread failed to start (see daemon log)");
            }
        }
    }
    state.audio.on.store(on, Ordering::Relaxed);
    if !on {
        state.audio.level.store(0, Ordering::Relaxed);
        state.engine.lock().unwrap().set_audio_level(0);
    }
    ok_response(id, json!({"on": on}))
}

fn apply_matrix(
    state: &State,
    dev: &DiscoveredDevice,
    zone_name: &str,
    colors: &[Rgb],
) -> Result<(), String> {
    let handle = require_handle(state, dev)?;
    let Some(zone) = dev.info.zones.iter().find(|z| z.name == zone_name) else {
        return Err(format!("unknown zone: {zone_name}"));
    };
    if colors.len() != zone.leds {
        return Err(format!(
            "zone '{zone_name}' expects {} colors, got {}",
            zone.leds,
            colors.len()
        ));
    }
    stop_effect_for_manual_write(state, dev);
    // Full-device frame, row-major per zone. Other zones black — ponytail
    // ceiling: a multi-zone device loses its other zone on a single-zone
    // write; upgrade = per-zone write_frame or client-sends-full-frame.
    let mut frame: Vec<Rgb> = Vec::with_capacity(dev.info.zones.iter().map(|z| z.leds).sum());
    for z in &dev.info.zones {
        if z.name == zone_name {
            frame.extend_from_slice(colors);
        } else {
            frame.extend(std::iter::repeat(Rgb { r: 0, g: 0, b: 0 }).take(z.leds));
        }
    }
    handle.lock().unwrap().write_frame(&frame).map_err(|e| e.to_string())?;
    state.colors.lock().unwrap().remove(&dev.info.id);
    state
        .matrices
        .lock()
        .unwrap()
        .entry(dev.info.id.clone())
        .or_default()
        .insert(zone_name.to_string(), colors.to_vec());
    Ok(())
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

/// [A-Za-z0-9 _-]; everything else becomes '_' (no path traversal).
fn sanitize_profile_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn profile_path(state: &State, name: &str) -> PathBuf {
    state.config_dir.join("profiles").join(format!("{}.json", sanitize_profile_name(name)))
}

fn load_profiles(state: &State) -> Vec<Value> {
    let dir = state.config_dir.join("profiles");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        if e.path().extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(text) = std::fs::read_to_string(e.path()) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn profile_by_name(state: &State, name: &str) -> Option<Value> {
    load_profiles(state)
        .into_iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
}

/// Pure: does `frontmost` match any binding (case-insensitive substring,
/// binding inside frontmost)?
fn profile_matches(frontmost: &str, bindings: &[String]) -> bool {
    let f = frontmost.to_ascii_lowercase();
    bindings.iter().any(|b| f.contains(&b.to_ascii_lowercase()))
}

fn apply_profile(state: &State, profile: &Value) -> (Map<String, Value>, Vec<String>) {
    let mut results = Map::new();
    let mut warnings = Vec::new();
    let Some(devices) = profile.get("devices").and_then(Value::as_object) else {
        return (results, warnings);
    };
    for (device_id, entry) in devices {
        let dev = match find_device(state, device_id) {
            Ok(d) => d,
            Err(_) => {
                warnings.push(format!("device {device_id} not present; entry skipped"));
                continue;
            }
        };
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
        let r = match kind {
            "static" => {
                match parse_color(entry.get("color")) {
                    Some(c) => apply_static(state, &dev, c),
                    None => Err("invalid color".into()),
                }
            }
            "effect" => {
                let name = entry.get("effect").and_then(Value::as_str).unwrap_or("");
                let params = entry.get("params").cloned().unwrap_or_else(|| json!({}));
                apply_effect(state, &dev, name, &params)
            }
            "layers" => {
                apply_layers(state, &dev, entry.get("layers").unwrap_or(&Value::Null))
            }
            "matrix" => {
                let zone = entry.get("zone").and_then(Value::as_str).unwrap_or("");
                match parse_colors(entry.get("colors")) {
                    Some(colors) => apply_matrix(state, &dev, zone, &colors),
                    None => Err("invalid colors".into()),
                }
            }
            other => Err(format!("unknown profile entry kind: {other}")),
        };
        results.insert(
            device_id.clone(),
            match r {
                Ok(()) => json!("ok"),
                Err(e) => json!(format!("error: {e}")),
            },
        );
    }
    (results, warnings)
}

// ---------------------------------------------------------------------------
// Foreground-app watcher (macOS via osascript; disables itself elsewhere)
// ---------------------------------------------------------------------------

fn frontmost_app() -> Option<String> {
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to get name of first application process whose frontmost is true")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

fn spawn_watcher(state: Arc<State>) {
    thread::spawn(move || {
        poll_once(&state); // immediate first poll: get_active_profile is real from t≈0
        loop {
            thread::sleep(WATCH_POLL);
            if !*state.watcher_on.lock().unwrap() {
                continue;
            }
            poll_once(&state);
        }
    });
}

fn poll_once(state: &State) {
    if !*state.watcher_on.lock().unwrap() {
        return;
    }
    let Some(app) = frontmost_app() else {
        // osascript missing/failed (non-mac, or no Automation grant):
        // disable so we stop paying the spawn cost.
        *state.watcher_on.lock().unwrap() = false;
        return;
    };
    *state.frontmost.lock().unwrap() = Some(app.clone());
    let matched = load_profiles(&state)
        .into_iter()
        .find(|p| {
            let bindings = p
                .get("bindings")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            profile_matches(&app, &bindings)
        })
        .and_then(|p| p.get("name").and_then(Value::as_str).map(str::to_string));
    let mut active = state.active_profile.lock().unwrap();
    if matched != *active {
        if let Some(name) = &matched {
            if let Some(p) = profile_by_name(state, name) {
                let _ = apply_profile(state, &p); // per-device errors swallowed; UI can re-apply manually
            }
        }
        *active = matched;
    }
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

async fn handle_connection(
    state: Arc<State>,
    stream: TcpStream,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    let mut events = state.events_tx.subscribe();
    loop {
        tokio::select! {
            // Push frames (key presses, audio level) — send failure means
            // the client is gone; treat like a closed connection.
            ev = events.recv() => match ev {
                Ok(frame) => {
                    if ws.send(Message::Text(frame.into())).await.is_err() {
                        return Ok(());
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // slow client missed n pushes; nothing to recover — next
                    // frame continues
                    let _ = n;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let resp = match serde_json::from_str::<Value>(&text) {
                        Ok(req) => handle_request(&state, &req),
                        Err(e) => err_response(Value::Null, &format!("bad json: {e}")),
                    };
                    if ws.send(Message::Text(resp.to_string().into())).await.is_err() {
                        return Ok(());
                    }
                }
                Some(Ok(Message::Close(_))) => return Ok(()),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e),
                None => return Ok(()),
            },
        }
    }
}

/// A client dropping the TCP connection without a close handshake is routine
/// (crashed UI, sleep/wake); it must not count as a daemon error.
fn is_benign_disconnect(e: &WsError) -> bool {
    matches!(e, WsError::ConnectionClosed)
        || matches!(e, WsError::Protocol(p) if matches!(p, ProtocolError::ResetWithoutClosingHandshake))
}

fn ok_response(id: Value, data: Value) -> Value {
    json!({"id": id, "ok": true, "data": data})
}

fn err_response(id: Value, msg: &str) -> Value {
    json!({"id": id, "ok": false, "error": msg})
}

fn device_json(state: &State, d: &DiscoveredDevice) -> Value {
    let mut v = serde_json::to_value(&d.info).unwrap();
    let err = state.open_errors.lock().unwrap().get(&d.info.id).cloned();
    match &err {
        Some(e) => {
            v["available"] = json!(false);
            v["openError"] = json!(e);
            v["errorKind"] = json!(classify_error(e));
        }
        None => {
            v["available"] = json!(true);
            v["openError"] = Value::Null;
            v["errorKind"] = Value::Null;
        }
    }
    v
}

fn handle_request(state: &State, req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = req.get("method").and_then(Value::as_str) else {
        return err_response(id, "missing 'method'");
    };
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "list_devices" => ok_response(
            id,
            json!(state.devices.read().unwrap().iter().map(|d| device_json(state, d)).collect::<Vec<_>>()),
        ),
        "get_state" => {
            let colors = state.colors.lock().unwrap();
            let effects = state.effects.lock().unwrap();
            let layers = state.layers.lock().unwrap();
            let matrices = state.matrices.lock().unwrap();
            let devs: Vec<Value> = state
                .devices
                .read()
                .unwrap()
                .iter()
                .map(|d| {
                    let color_v = colors.get(&d.info.id).map(|c| json!([c.r, c.g, c.b]));
                    let color = color_v.as_ref();
                    let effect = effects.get(&d.info.id).map(|e| json!(e)).unwrap_or(Value::Null);
                    let layer_stack = layers
                        .get(&d.info.id)
                        .map(|l| l.clone())
                        .unwrap_or(Value::Null);
                    let matrix = matrices.get(&d.info.id).map(|zones| {
                        let m: Map<String, Value> = zones
                            .iter()
                            .map(|(z, c)| {
                                (
                                    z.clone(),
                                    Value::Array(c.iter().map(|p| json!([p.r, p.g, p.b])).collect::<Vec<_>>()),
                                )
                            })
                            .collect();
                        Value::Object(m)
                    });
                    let mode = if effect.is_string() {
                        "effect"
                    } else if layer_stack.is_array() {
                        "layers"
                    } else if matrix.is_some() {
                        "matrix"
                    } else if color.map(|c| c.is_array()).unwrap_or(false) {
                        "static"
                    } else {
                        "idle"
                    };
                    json!({"id": d.info.id, "color": color, "effect": effect, "mode": mode, "matrix": matrix, "layers": layer_stack})
                })
                .collect();
            ok_response(id, json!(devs))
        }
        "refresh_devices" => match state.refresh() {
            Ok((total, ready, unavail)) => {
                ok_response(id, json!({"devices": total, "ready": ready, "unavailable": unavail}))
            }
            Err(e) => err_response(id, &e),
        },
        "set_color" => set_color(state, id, &params),
        "set_effect" => set_effect(state, id, &params),
        "stop_effect" => stop_effect(state, id, &params),
        "set_layers" => set_layers(state, id, &params),
        "trigger" => trigger(state, id, &params),
        "set_audio" => set_audio(state, id, &params),
        "get_audio_level" => ok_response(
            id,
            json!({
                "level": audio_level_now(state),
                "on": state.audio.on.load(Ordering::Relaxed),
            }),
        ),
        "set_matrix" => set_matrix(state, id, &params),
        "profile_save" => profile_save(state, id, &params),
        "profile_list" => ok_response(
            id,
            json!(load_profiles(state)
                .iter()
                .map(|p| json!({
                    "name": p.get("name").cloned().unwrap_or(Value::Null),
                    "bindings": p.get("bindings").cloned().unwrap_or(json!([])),
                    "devices": p.get("devices").and_then(Value::as_object).map(|d| d.keys().collect::<Vec<_>>()).unwrap_or_default(),
                }))
                .collect::<Vec<_>>()),
        ),
        "profile_get" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return err_response(id, "missing param 'name'");
            };
            match profile_by_name(state, name) {
                Some(p) => ok_response(id, p),
                None => err_response(id, &format!("no such profile: {name}")),
            }
        }
        "profile_delete" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return err_response(id, "missing param 'name'");
            };
            match std::fs::remove_file(profile_path(state, name)) {
                Ok(()) => ok_response(id, json!({"deleted": name})),
                Err(e) => err_response(id, &format!("delete failed: {e}")),
            }
        }
        "profile_apply" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return err_response(id, "missing param 'name'");
            };
            let Some(p) = profile_by_name(state, name) else {
                return err_response(id, &format!("no such profile: {name}"));
            };
            let (results, warnings) = apply_profile(state, &p);
            *state.active_profile.lock().unwrap() = Some(name.to_string());
            ok_response(id, json!({"results": results, "warnings": warnings}))
        }
        "get_active_profile" => {
            let name = state.active_profile.lock().unwrap().clone();
            let frontmost = state.frontmost.lock().unwrap().clone();
            ok_response(
                id,
                json!({
                    "name": name,
                    "watcher": if *state.watcher_on.lock().unwrap() { "on" } else { "off" },
                    "frontmost": frontmost,
                }),
            )
        }
        other => err_response(id, &format!("unknown method: {other}")),
    }
}

fn set_color(state: &State, id: Value, params: &Value) -> Value {
    let Some(device) = params.get("device").and_then(Value::as_str) else {
        return err_response(id, "missing param 'device' (string)");
    };
    let Some(color) = parse_color(params.get("color")) else {
        return err_response(id, "missing/invalid param 'color' (expected [r,g,b], 0-255 each)");
    };
    let dev = match find_device(state, device) {
        Ok(d) => d,
        Err(e) => return err_response(id, &e),
    };
    match apply_static(state, &dev, color) {
        Ok(()) => ok_response(id, json!({"device": dev.info.id, "color": [color.r, color.g, color.b]})),
        Err(e) => err_response(id, &e),
    }
}

fn set_effect(state: &State, id: Value, params: &Value) -> Value {
    let Some(device) = params.get("device").and_then(Value::as_str) else {
        return err_response(id, "missing param 'device' (string)");
    };
    let Some(effect_name) = params.get("effect").and_then(Value::as_str) else {
        return err_response(id, "missing param 'effect' (string)");
    };
    let eff_params = params.get("params").cloned().unwrap_or_else(|| json!({}));
    let dev = match find_device(state, device) {
        Ok(d) => d,
        Err(e) => return err_response(id, &e),
    };
    match apply_effect(state, &dev, effect_name, &eff_params) {
        Ok(()) => ok_response(id, json!({"device": dev.info.id, "effect": effect_name})),
        Err(e) => err_response(id, &e),
    }
}

fn stop_effect(state: &State, id: Value, params: &Value) -> Value {
    let Some(device) = params.get("device").and_then(Value::as_str) else {
        return err_response(id, "missing param 'device' (string)");
    };
    let dev = match find_device(state, device) {
        Ok(d) => d,
        Err(e) => return err_response(id, &e),
    };
    let _ = state.engine.lock().unwrap().stop(&dev.info.id);
    state.effects.lock().unwrap().remove(&dev.info.id);
    state.layers.lock().unwrap().remove(&dev.info.id);
    ok_response(id, json!({"device": dev.info.id, "stopped": true}))
}

fn set_layers(state: &State, id: Value, params: &Value) -> Value {
    let Some(device) = params.get("device").and_then(Value::as_str) else {
        return err_response(id, "missing param 'device' (string)");
    };
    let dev = match find_device(state, device) {
        Ok(d) => d,
        Err(e) => return err_response(id, &e),
    };
    match apply_layers(state, &dev, params.get("layers").unwrap_or(&Value::Null)) {
        Ok(()) => ok_response(id, json!({"device": dev.info.id, "layers": params.get("layers").map(|l| l.as_array().map(|a| a.len()).unwrap_or(0)).unwrap_or(0)})),
        Err(e) => err_response(id, &e),
    }
}

/// IPC trigger {device, x, y}: queue an interactive event at a normalized
/// (0..=1) matrix position (UI canvas clicks). Values are clamped.
fn trigger(state: &State, id: Value, params: &Value) -> Value {
    let Some(device) = params.get("device").and_then(Value::as_str) else {
        return err_response(id, "missing param 'device' (string)");
    };
    let (Some(x), Some(y)) = (
        params.get("x").and_then(Value::as_f64),
        params.get("y").and_then(Value::as_f64),
    ) else {
        return err_response(id, "missing params 'x' and 'y' (numbers)");
    };
    let dev = match find_device(state, device) {
        Ok(d) => d,
        Err(e) => return err_response(id, &e),
    };
    let x = (x as f32).clamp(0.0, 1.0);
    let y = (y as f32).clamp(0.0, 1.0);
    match state.engine.lock().unwrap().trigger(&dev.info.id, x, y) {
        Ok(()) => ok_response(id, json!({"device": dev.info.id, "x": x, "y": y})),
        Err(e) => err_response(id, &e.to_string()),
    }
}

fn parse_colors(v: Option<&Value>) -> Option<Vec<Rgb>> {
    v?.as_array()?.iter().map(|c| parse_color(Some(c))).collect()
}

fn set_matrix(state: &State, id: Value, params: &Value) -> Value {
    let Some(device) = params.get("device").and_then(Value::as_str) else {
        return err_response(id, "missing param 'device' (string)");
    };
    let Some(zone_name) = params.get("zone").and_then(Value::as_str) else {
        return err_response(id, "missing param 'zone' (string)");
    };
    let dev = match find_device(state, device) {
        Ok(d) => d,
        Err(e) => return err_response(id, &e),
    };
    let Some(colors) = parse_colors(params.get("colors")) else {
        return err_response(id, "missing/invalid param 'colors' (expected [[r,g,b],...])");
    };
    match apply_matrix(state, &dev, zone_name, &colors) {
        Ok(()) => ok_response(id, json!({"device": dev.info.id, "zone": zone_name, "leds": colors.len()})),
        Err(e) => err_response(id, &e),
    }
}

fn profile_save(state: &State, id: Value, params: &Value) -> Value {
    let Some(profile) = params.get("profile") else {
        return err_response(id, "missing param 'profile'");
    };
    let Some(name) = profile.get("name").and_then(Value::as_str) else {
        return err_response(id, "profile.name missing (non-empty string)");
    };
    if name.trim().is_empty() {
        return err_response(id, "profile.name must be non-empty");
    }
    let mut warnings = Vec::new();
    if let Some(devices) = profile.get("devices").and_then(Value::as_object) {
        for (device_id, entry) in devices {
            match entry.get("kind").and_then(Value::as_str) {
                Some("static") => {
                    if parse_color(entry.get("color")).is_none() {
                        return err_response(id, &format!("device {device_id}: static entry needs color [r,g,b]"));
                    }
                }
                Some("effect") => {
                    let Some(effect) = entry.get("effect").and_then(Value::as_str) else {
                        return err_response(id, &format!("device {device_id}: effect entry needs 'effect' name"));
                    };
                    let p = entry.get("params").cloned().unwrap_or_else(|| json!({}));
                    if let Err(e) = Effect::from_json(effect, &p) {
                        return err_response(id, &format!("device {device_id}: {e}"));
                    }
                }
                Some("matrix") => {
                    let Some(colors) = parse_colors(entry.get("colors")) else {
                        return err_response(id, &format!("device {device_id}: matrix entry needs colors [[r,g,b],...]"));
                    };
                    // Length-check against the live device when present.
                    if let Ok(dev) = find_device(state, device_id) {
                        let zone = entry.get("zone").and_then(Value::as_str).unwrap_or("");
                        match dev.info.zones.iter().find(|z| z.name == zone) {
                            Some(z) if z.leds == colors.len() => {}
                            Some(z) => {
                                return err_response(id, &format!(
                                    "device {device_id}: zone '{zone}' expects {} colors, got {}",
                                    z.leds, colors.len()
                                ))
                            }
                            None => warnings.push(format!("device {device_id}: unknown zone '{zone}' (saved anyway; will error on apply if still unknown)")),
                        }
                    } else {
                        warnings.push(format!("device {device_id} not present; length not validated"));
                    }
                }
                Some("layers") => {
                    // Validate every layer entry parses (same parser as the
                    // set_layers IPC); apply-time behavior is identical.
                    let Some(entries) =
                        entry.get("layers").and_then(Value::as_array)
                    else {
                        return err_response(id, &format!("device {device_id}: layers entry needs 'layers' array"));
                    };
                    for (i, l) in entries.iter().enumerate() {
                        let Some(effect) = l.get("effect").and_then(Value::as_str) else {
                            return err_response(id, &format!("device {device_id}: layers[{i}] needs 'effect' name"));
                        };
                        let p = l.get("params").cloned().unwrap_or_else(|| json!({}));
                        if let Err(e) = Layer::from_json(effect, &p, l) {
                            return err_response(id, &format!("device {device_id}: layers[{i}]: {e}"));
                        }
                    }
                }
                _ => return err_response(id, &format!("device {device_id}: kind must be static|effect|matrix|layers")),
            }
        }
    }
    let path = profile_path(state, name);
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return err_response(id, &format!("could not create config dir: {e}"));
        }
    }
    match std::fs::write(&path, serde_json::to_string_pretty(profile).unwrap()) {
        Ok(()) => ok_response(id, json!({"saved": name, "warnings": warnings})),
        Err(e) => err_response(id, &format!("write failed: {e}")),
    }
}

fn parse_color(v: Option<&Value>) -> Option<Rgb> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(Rgb {
        r: u8::try_from(a[0].as_u64()?).ok()?,
        g: u8::try_from(a[1].as_u64()?).ok()?,
        b: u8::try_from(a[2].as_u64()?).ok()?,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api = shared_hid();
    set_shared_api(api);
    let mut devices = discover(&api.lock().unwrap());
    // OpenRGB bridge: optional ecosystem coverage (Corsair/ASUS/MSI/...).
    match chromargb_drivers::openrgb_bridge::discover_bridge("127.0.0.1:6742") {
        Ok(bridged) => {
            if !bridged.is_empty() {
                println!("openrgb bridge: {} device(s) via 127.0.0.1:6742", bridged.len());
                devices.extend(bridged);
            }
        }
        Err(e) => eprintln!("openrgb bridge unavailable: {e}"),
    }
    print!("{}", devices_table(&devices));
    if devices.is_empty() {
        println!("no recognized RGB devices found");
    }
    let state = Arc::new(State::new(api, devices, default_config_dir()));
    {
        let handles = state.handles.lock().unwrap();
        let unavail = state.open_errors.lock().unwrap().len();
        println!("{} device(s) ready, {unavail} unavailable", handles.len());
    }
    *state.watcher_on.lock().unwrap() = true;
    spawn_watcher(Arc::clone(&state));

    // CoreAudio RMS meter (macOS only): one thread at startup opens the
    // default input device and feeds engine + ws clients (the plan's
    // expected one-time microphone TCC prompt fires here). Failure just
    // logs — startup never blocks on audio. set_audio toggles updates.
    #[cfg(target_os = "macos")]
    {
        state.audio.on.store(true, Ordering::Relaxed);
        spawn_meter_once(
            Arc::clone(&state.engine),
            state.events_tx.clone(),
            Arc::clone(&state.audio.on),
            Arc::clone(&state.audio.level),
        );
    }

    let listener = TcpListener::bind(ADDR).await?;
    println!("chromargb-daemon listening on ws://{ADDR}");
    loop {
        let (stream, _) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(state, stream).await {
                if !is_benign_disconnect(&e) {
                    eprintln!("ws connection ended with error: {e}");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chromargb_drivers::{DeviceInfo, DriverKind, Zone};

    /// Generic-kind device (vid 0x1532 pid 0x9999 mirrors lib.rs's fixed
    /// test): open_backend returns the protocol-less GenericBackend without
    /// touching real hardware, so error paths stay deterministic even with
    /// a real BlackWidow Elite attached.
    fn fake_device(id: &str) -> DiscoveredDevice {
        DiscoveredDevice {
            info: DeviceInfo {
                id: id.into(),
                name: "Unrecognized Razer device".into(),
                vendor: "Razer".into(),
                vid: 0x1532,
                pid: 0x9999,
                kind: DriverKind::Generic,
                zones: vec![Zone { name: "matrix".into(), leds: 6, matrix: Some((2, 3)) }],
                note: Some("no registry entry for Razer PID 0x9999".into()),
            },
            serial: Some("TEST".into()),
            path: std::ffi::CString::new("fake").unwrap(),
        }
    }

    fn fake_state() -> State {
        startup_state(vec![fake_device("1532:9999:TEST")])
    }

    fn state_with_dir() -> (State, PathBuf) {
        // unique per test: parallel tests must not share (or rm -rf each
        // other's) config dirs
        let dir = std::env::temp_dir().join(format!(
            "chromargb-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t").replace("tests::", "")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let api = shared_hid();
        (State::new(api, vec![fake_device("1532:9999:TEST")], dir.clone()), dir)
    }

    #[test]
    fn classify_errors() {
        assert_eq!(
            classify_error("hidapi: hid_open_path: failed to open IOHIDDevice from mach entry: (0xE00002C1) (iokit/common) privilege violation"),
            "permission"
        );
        assert_eq!(classify_error("permission denied"), "permission");
        assert_eq!(classify_error("IOHIDDevice: exclusive access and device already open (0xE00002C5)"), "busy");
        assert_eq!(classify_error("device busy"), "busy");
        assert_eq!(classify_error("something else"), "unknown");
    }

    #[test]
    fn list_devices_and_get_state() {
        let s = fake_state();
        let r = handle_request(&s, &json!({"id": 1, "method": "list_devices"}));
        assert_eq!(r["id"], 1);
        assert!(r["ok"].as_bool().unwrap());
        assert_eq!(r["data"][0]["name"], "Unrecognized Razer device");
        assert_eq!(r["data"][0]["zones"][0]["leds"], 6);
        assert_eq!(r["data"][0]["available"], true);
        assert!(r["data"][0]["openError"].is_null());
        assert!(r["data"][0]["errorKind"].is_null());

        let r = handle_request(&s, &json!({"id": 2, "method": "get_state"}));
        assert!(r["ok"].as_bool().unwrap());
        assert_eq!(r["data"][0]["id"], "1532:9999:TEST");
        assert_eq!(r["data"][0]["color"], Value::Null);
        assert_eq!(r["data"][0]["effect"], Value::Null);
        assert_eq!(r["data"][0]["mode"], "idle");
        assert!(r["data"][0]["matrix"].is_null());
    }

    #[test]
    fn set_color_unknown_device_vs_no_driver_vs_bad_color() {
        let s = fake_state();
        let r = handle_request(
            &s,
            &json!({"id": 1, "method": "set_color", "params": {"device": "nope", "color": [255, 0, 0]}}),
        );
        assert!(r["error"].as_str().unwrap().contains("unknown device"));

        let r = handle_request(
            &s,
            &json!({"id": 2, "method": "set_color", "params": {"device": "Unrecognized Razer device", "color": [1, 2, 3]}}),
        );
        assert!(r["error"].as_str().unwrap().contains("no native driver"));
        assert!(s.colors.lock().unwrap().is_empty());

        let r = handle_request(
            &s,
            &json!({"id": 3, "method": "set_color", "params": {"device": "1532:9999:TEST", "color": [300, 0, 0]}}),
        );
        assert!(!r["ok"].as_bool().unwrap());
    }

    #[test]
    fn unavailable_device_gets_clear_error_not_crash() {
        let s = fake_state();
        s.handles.lock().unwrap().remove("1532:9999:TEST");
        s.open_errors
            .lock()
            .unwrap()
            .insert("1532:9999:TEST".into(), "hidapi: privilege violation".to_string());
        for method_params in [
            json!({"method": "set_color", "params": {"device": "1532:9999:TEST", "color": [1,2,3]}}),
            json!({"method": "set_effect", "params": {"device": "1532:9999:TEST", "effect": "wave", "params": {"speed": 50, "direction": "left"}}}),
            json!({"method": "set_matrix", "params": {"device": "1532:9999:TEST", "zone": "matrix", "colors": [[1,2,3],[1,2,3],[1,2,3],[1,2,3],[1,2,3],[1,2,3]]}}),
        ] {
            let r = handle_request(&s, &method_params);
            assert!(!r["ok"].as_bool().unwrap(), "{method_params}");
            let e = r["error"].as_str().unwrap();
            assert!(e.contains("unavailable") && e.contains("privilege violation"), "{e}");
        }
        // list_devices carries the classification for the UI fix flow
        let r = handle_request(&s, &json!({"method": "list_devices"}));
        assert_eq!(r["data"][0]["available"], false);
        assert_eq!(r["data"][0]["errorKind"], "permission");
        assert!(r["data"][0]["openError"].as_str().unwrap().contains("privilege violation"));
    }

    #[test]
    fn rebuild_swaps_devices_and_resets_state() {
        let s = fake_state();
        // simulate some state
        s.colors.lock().unwrap().insert("old".into(), Rgb { r: 1, g: 2, b: 3 });
        let (ready, unavail) = s.rebuild_devices(vec![fake_device("1532:9999:NEW")]);
        assert_eq!((ready, unavail), (1, 0));
        assert!(s.handles.lock().unwrap().contains_key("1532:9999:NEW"));
        assert!(!s.handles.lock().unwrap().contains_key("1532:9999:TEST"));
        assert!(s.colors.lock().unwrap().is_empty());
        let r = handle_request(&s, &json!({"method": "list_devices"}));
        assert_eq!(r["data"][0]["id"], "1532:9999:NEW");
    }

    #[test]
    fn mode_reporting_transitions() {
        let s = fake_state();
        let dev = "1532:9999:TEST";
        let get_mode = || {
            let r = handle_request(&s, &json!({"method": "get_state"}));
            r["data"][0]["mode"].as_str().unwrap().to_string()
        };
        assert_eq!(get_mode(), "idle");

        s.colors.lock().unwrap().insert(dev.into(), Rgb { r: 9, g: 9, b: 9 });
        assert_eq!(get_mode(), "static");

        s.colors.lock().unwrap().remove(dev);
        s.effects.lock().unwrap().insert(dev.into(), "wave".into());
        assert_eq!(get_mode(), "effect");

        // layers mode when a layer stack is active (effect cleared)
        s.effects.lock().unwrap().remove(dev);
        s.layers.lock().unwrap().insert(dev.into(), json!([{"effect": "ripple", "params": {"color": [255,0,255], "speed": 80}}]));
        assert_eq!(get_mode(), "layers");
        let r = handle_request(&s, &json!({"method": "get_state"}));
        assert_eq!(r["data"][0]["layers"][0]["effect"], "ripple");
        s.layers.lock().unwrap().remove(dev);

        // matrix overrides static/effect in reporting; effect clears matrix
        s.matrices.lock().unwrap().insert(
            dev.into(),
            HashMap::from([("matrix".to_string(), vec![Rgb { r: 0, g: 0, b: 0 }; 6])]),
        );
        assert_eq!(get_mode(), "matrix");
        let r = handle_request(&s, &json!({"method": "get_state"}));
        assert_eq!(r["data"][0]["matrix"]["matrix"].as_array().unwrap().len(), 6);

        s.matrices.lock().unwrap().remove(dev);
        assert_eq!(get_mode(), "idle");
    }

    #[test]
    fn set_layers_parse_roundtrip_and_bad_layer_error() {
        let s = fake_state();
        // Bad layer: second entry has a bad param — error names the index.
        let r = handle_request(&s, &json!({"id": 1, "method": "set_layers", "params": {
            "device": "1532:9999:TEST",
            "layers": [
                {"effect": "wave", "params": {"speed": 50, "direction": "left"}},
                {"effect": "ripple", "params": {"speed": 80}}
            ]
        }}));
        assert!(!r["ok"].as_bool().unwrap(), "{r}");
        assert!(
            r["error"].as_str().unwrap().contains("layers[1]"),
            "error must name the layer index: {}",
            r["error"]
        );
        assert!(s.layers.lock().unwrap().is_empty());

        // Valid stack parses and is stored verbatim for get_state/capture.
        let r = handle_request(&s, &json!({"id": 2, "method": "set_layers", "params": {
            "device": "1532:9999:TEST",
            "layers": [
                {"effect": "wave", "params": {"speed": 50, "direction": "left"}},
                {"effect": "ripple", "params": {"color": [255, 0, 255], "speed": 80}, "opacity": 60, "blend": "add"}
            ]
        }}));
        assert!(r["ok"].as_bool().unwrap(), "{r}");
        assert_eq!(r["data"]["layers"], 2);
        let stored = s.layers.lock().unwrap().get("1532:9999:TEST").cloned().unwrap();
        assert_eq!(stored[1]["opacity"], 60);
        assert_eq!(stored[1]["blend"], "add");
        // colors/effects/matrices cleared like apply_effect does
        assert!(s.colors.lock().unwrap().is_empty());
        assert!(s.effects.lock().unwrap().is_empty());
        assert!(s.matrices.lock().unwrap().is_empty());
    }

    #[test]
    fn trigger_clamps_coordinates() {
        let s = fake_state();
        for (x, y) in [(-0.5f64, 0.5f64), (1.7, 0.2), (0.5, 3.0)] {
            let r = handle_request(&s, &json!({"id": 1, "method": "trigger", "params": {
                "device": "1532:9999:TEST", "x": x, "y": y
            }}));
            assert!(r["ok"].as_bool().unwrap(), "{r}");
            // compare in f32: the handler clamps in f32 (engine space)
            let (rx, ry) = (
                r["data"]["x"].as_f64().unwrap() as f32,
                r["data"]["y"].as_f64().unwrap() as f32,
            );
            assert_eq!(rx, (x as f32).clamp(0.0, 1.0));
            assert_eq!(ry, (y as f32).clamp(0.0, 1.0));
        }
        // unknown device errors
        let r = handle_request(&s, &json!({"method": "trigger", "params": {"device": "nope", "x": 0.5, "y": 0.5}}));
        assert!(!r["ok"].as_bool().unwrap());
    }

    #[test]
    fn profile_layers_kind_roundtrip() {
        let (s, dir) = state_with_dir();
        let layers = json!([
            {"effect": "wave", "params": {"speed": 50, "direction": "left"}},
            {"effect": "audio_meter", "params": {"bands": 6}, "opacity": 80, "blend": "add"}
        ]);
        // save validates layer entries
        let r = handle_request(&s, &json!({"method": "profile_save", "params": {"profile": {
            "name": "Layered",
            "devices": {"1532:9999:TEST": {"kind": "layers", "layers": layers}}
        }}}));
        assert!(r["ok"].as_bool().unwrap(), "{r}");
        // invalid layer entry -> hard error naming index
        let r = handle_request(&s, &json!({"method": "profile_save", "params": {"profile": {
            "name": "Bad",
            "devices": {"1532:9999:TEST": {"kind": "layers", "layers": [{"effect": "nope"}]}}
        }}}));
        assert!(!r["ok"].as_bool().unwrap());
        assert!(r["error"].as_str().unwrap().contains("layers[0]"));
        // apply goes through the same apply_layers path. The fake generic
        // device accepts engine stacks (same semantics as apply_effect:
        // backend write errors surface in the scheduler, not here), so the
        // entry applies and the validated stack lands in state.layers.
        let r = handle_request(&s, &json!({"method": "profile_apply", "params": {"name": "Layered"}}));
        assert!(r["ok"].as_bool().unwrap());
        assert_eq!(r["data"]["results"]["1532:9999:TEST"], "ok");
        let stored = s.layers.lock().unwrap().get("1532:9999:TEST").cloned().unwrap();
        assert_eq!(stored, layers);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_round_trip_and_apply_errors() {
        let (s, dir) = state_with_dir();
        // save (device present, wrong matrix length -> hard error)
        let bad = json!({"method": "profile_save", "params": {"profile": {
            "name": "Valorant",
            "bindings": ["valorant", "riot"],
            "devices": {"1532:9999:TEST": {"kind": "matrix", "zone": "matrix", "colors": [[1,2,3]]}}
        }}});

        // NOTE: config dir: State::new received `dir` — but profile_path uses
        // state.config_dir; the wrong-length check fires before any disk IO.
        let r = handle_request(&s, &bad);
        assert!(!r["ok"].as_bool().unwrap());
        assert!(r["error"].as_str().unwrap().contains("expects 6 colors"));

        // save valid profile: static entry (device present, generic driver ->
        // apply-time error expected later; save itself must succeed)
        let save = json!({"method": "profile_save", "params": {"profile": {
            "name": "Valorant",
            "bindings": ["valorant", "riot"],
            "devices": {"1532:9999:TEST": {"kind": "static", "color": [255, 0, 0]}}
        }}});
        let r = handle_request(&s, &save);
        assert!(r["ok"].as_bool().unwrap(), "{r}");
        assert!(profile_path(&s, "Valorant").exists());

        // list + get
        let r = handle_request(&s, &json!({"method": "profile_list"}));
        assert_eq!(r["data"][0]["name"], "Valorant");
        assert_eq!(r["data"][0]["devices"][0], "1532:9999:TEST");
        let r = handle_request(&s, &json!({"method": "profile_get", "params": {"name": "Valorant"}}));
        assert_eq!(r["data"]["bindings"][1], "riot");

        // apply: generic device -> per-device error surfaced, no panic
        let r = handle_request(&s, &json!({"method": "profile_apply", "params": {"name": "Valorant"}}));
        assert!(r["ok"].as_bool().unwrap());
        let res = r["data"]["results"]["1532:9999:TEST"].as_str().unwrap();
        assert!(res.starts_with("error:") && res.contains("no native driver"), "{res}");
        let r = handle_request(&s, &json!({"method": "get_active_profile"}));
        assert_eq!(r["data"]["name"], "Valorant");

        // delete
        let r = handle_request(&s, &json!({"method": "profile_delete", "params": {"name": "Valorant"}}));
        assert!(r["ok"].as_bool().unwrap());
        assert!(!profile_path(&s, "Valorant").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_apply_missing_device_is_warning_not_error() {
        let (s, dir) = state_with_dir();
        handle_request(&s, &json!({"method": "profile_save", "params": {"profile": {
            "name": "Ghost",
            "devices": {"deadbeef": {"kind": "static", "color": [0, 0, 0]}}
        }}}));
        let r = handle_request(&s, &json!({"method": "profile_apply", "params": {"name": "Ghost"}}));
        assert!(r["ok"].as_bool().unwrap());
        assert!(!r["data"]["warnings"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_matching_is_substring_case_insensitive() {
        let bindings = vec!["Valorant".to_string(), "steam".to_string()];
        assert!(profile_matches("Valorant", &bindings));
        assert!(profile_matches("valorant - client", &bindings));
        assert!(profile_matches("Steam[Helper]", &bindings));
        assert!(!profile_matches("Discord", &bindings));
        assert!(!profile_matches("valor", &bindings)); // binding must be inside frontmost
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize_profile_name("../evil"), "___evil"); // dots stripped -> traversal impossible
        assert_eq!(sanitize_profile_name("CS2 profile"), "CS2 profile");
    }
}
