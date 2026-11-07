//! Razer native driver — 91-byte HID feature-report protocol.
//!
//! Protocol implemented from the OpenRGB sources (fetched 2026-11-07,
//! CalcProgrammer1/OpenRGB @ master):
//! - `Controllers/RazerController/RazerController/RazerController.cpp` —
//!   report struct, XOR CRC, exchange/retry logic, effect report builders
//! - `Controllers/RazerController/RazerController/RazerController.h` —
//!   status/effect/enum constants, packed `razer_report` layout
//! - `Controllers/RazerController/RazerController/RGBController_Razer.cpp` —
//!   brightness is set to max on mode activation
//! - `Controllers/RazerController/RazerDevices.cpp` — `blackwidow_elite_device`:
//!   EXTENDED matrix, transaction id 0x1F, LED id 0x05 (BACKLIGHT), 6x22
//! - `Controllers/RazerController/RazerControllerDetect.cpp` + `DetectionManager.h`
//!   — BlackWidow Elite control interface registered as interface 2,
//!   usage_page 0x01, usage 0x02 (`REGISTER_HID_DETECTOR_IPU` arg order:
//!   name, func, vid, pid, interface, page, usage)
//!
//! Key constants digest (RazerController.h/.cpp): 91-byte packed report
//! `[0]=report_id 0x00 [1]=status [2]=transaction_id [3..4]=remaining_packets
//! [5]=protocol_type [6]=data_size [7]=command_class [8]=command_id
//! [9..88]=arguments[80] [89]=crc [90]=reserved`; crc = XOR of bytes
//! [3..=88]; status: 0=new 1=busy 2=success 3=failure. (Note: the project
//! plan's "feature report 0x03" was wrong — report id is 0x00.)
//!
//! Hardware test (needs the keyboard attached AND HID permission):
//! ```text
//! cargo test -p chromargb-drivers razer -- --ignored --nocapture
//! ```
//! macOS blocks feature-report access to keyboards without entitlements
//! (IOKit privilege violation). Grant the running terminal Input Monitoring
//! under System Settings → Privacy & Security → Input Monitoring, or run
//! the test via `sudo`. The test prints a clear diagnostic and passes as
//! "skipped" when permission is missing.

use std::{thread::sleep, time::Duration};

use crate::{DeviceInfo, DiscoveredDevice, DriverBackend, Error, Result, Rgb};
use hidapi::{HidApi, HidDevice};

// ---------------------------------------------------------------------------
// Protocol constants — BlackWidow Elite (0x1532:0x0228), verified vs OpenRGB.
// When more Razer models land in the registry, promote these to a per-PID
// table copied from RazerDevices.cpp (transaction_id / led_id / rows / cols).
// ---------------------------------------------------------------------------

const REPORT_LEN: usize = 91;
const OFF_STATUS: usize = 1;
const OFF_TRANS: usize = 2;
const OFF_DATA_SIZE: usize = 6;
const OFF_CLASS: usize = 7;
const OFF_ID: usize = 8;
const OFF_ARGS: usize = 9;
const OFF_CRC: usize = 89;

/// BlackWidow Elite transaction id (`blackwidow_elite_device`).
const TRANS_ID: u8 = 0x1f;
/// Backlight LED id — the keyboard matrix.
const LED_ID: u8 = 0x05;

const STATUS_NEW: u8 = 0x00;
const STATUS_BUSY: u8 = 0x01;
const STATUS_SUCCESS: u8 = 0x02;

/// Extended-matrix lighting command class ("lighting and power").
const CLASS_LIGHTING: u8 = 0x0f;
/// Set effect (static / custom-frame mode / ...).
const ID_SET_EFFECT: u8 = 0x02;
/// Send one custom-frame row.
const ID_FRAME_ROW: u8 = 0x03;
/// Set brightness.
const ID_BRIGHTNESS: u8 = 0x04;

const EFFECT_STATIC: u8 = 0x01;
const EFFECT_CUSTOM_FRAME: u8 = 0x08;

/// Device class + firmware-version query, used as the control-interface probe.
const CLASS_DEVICE: u8 = 0x00;
const ID_GET_FIRMWARE: u8 = 0x81;

/// OpenRGB keeps keyboard brightness at max (GetMaxBrightness default 255);
/// sent once on first lighting command ("mode activation" in RGBController_Razer).
/// ponytail: fixed at max for v1 — one command path, no brightness API.
const MAX_BRIGHTNESS: u8 = 0xff;

type Report = [u8; REPORT_LEN];

// ---------------------------------------------------------------------------
// Packet builders (pure — unit-tested without hardware)
// ---------------------------------------------------------------------------

fn base_report(class: u8, id: u8, data_size: u8) -> Report {
    let mut buf = [0u8; REPORT_LEN];
    buf[OFF_TRANS] = TRANS_ID;
    buf[OFF_DATA_SIZE] = data_size;
    buf[OFF_CLASS] = class;
    buf[OFF_ID] = id;
    buf
}

/// XOR of bytes [3..=88] — `RazerController::razer_calculate_crc` (the loop
/// runs `for i in 3..89` over the 91-byte struct).
fn crc(buf: &Report) -> u8 {
    buf[3..OFF_CRC].iter().fold(0, |acc, &b| acc ^ b)
}

/// Static whole-matrix color: `razer_create_mode_static_extended_matrix_report`.
fn static_report(color: Rgb) -> Report {
    let mut buf = base_report(CLASS_LIGHTING, ID_SET_EFFECT, 0x09);
    let a = &mut buf[OFF_ARGS..];
    a[0] = 0x00; // storage: no save
    a[1] = LED_ID;
    a[2] = EFFECT_STATIC;
    a[5] = 0x01; // color count
    a[6] = color.r;
    a[7] = color.g;
    a[8] = color.b;
    buf
}

/// Matrix brightness: `razer_create_brightness_extended_matrix_report`.
fn brightness_report(level: u8) -> Report {
    let mut buf = base_report(CLASS_LIGHTING, ID_BRIGHTNESS, 0x03);
    let a = &mut buf[OFF_ARGS..];
    a[0] = 0x00; // storage: no save
    a[1] = LED_ID;
    a[2] = level;
    buf
}

/// One custom-frame row: `razer_create_custom_frame_extended_matrix_report`.
/// `colors` is the full row-major frame; `row` and `cols` pick the slice.
fn frame_row_report(row: u8, cols: u8, colors: &[Rgb]) -> Report {
    let mut buf = base_report(CLASS_LIGHTING, ID_FRAME_ROW, (cols as usize * 3 + 5) as u8);
    let a = &mut buf[OFF_ARGS..];
    a[2] = row;
    a[3] = 0; // start col
    a[4] = cols - 1; // stop col
    let off = row as usize * cols as usize;
    for (i, c) in colors[off..off + cols as usize].iter().enumerate() {
        a[5 + i * 3] = c.r;
        a[6 + i * 3] = c.g;
        a[7 + i * 3] = c.b;
    }
    buf
}

/// Apply the custom frame: `razer_create_mode_custom_extended_matrix_report`.
fn custom_mode_report() -> Report {
    let mut buf = base_report(CLASS_LIGHTING, ID_SET_EFFECT, 0x0c);
    let a = &mut buf[OFF_ARGS..];
    a[0] = 0x00;
    a[1] = 0x00;
    a[2] = EFFECT_CUSTOM_FRAME;
    buf
}

/// Firmware query — the probe that confirms the control interface.
fn firmware_report() -> Report {
    base_report(CLASS_DEVICE, ID_GET_FIRMWARE, 0x02)
}

/// Matrix geometry from the registry zone, validated against protocol caps
/// (row payload must fit the 80-byte argument block: cols*3+5 <= 80).
fn matrix_dims(info: &DeviceInfo) -> Result<(u8, u8)> {
    let [zone] = info.zones.as_slice() else {
        return Err(Error::Unsupported(format!(
            "razer {}: expected exactly one matrix zone",
            info.name
        )));
    };
    let Some((rows, cols)) = zone.matrix else {
        return Err(Error::Unsupported(format!(
            "razer {}: zone has no matrix geometry",
            info.name
        )));
    };
    if rows == 0 || cols == 0 || cols > 25 {
        return Err(Error::Unsupported(format!(
            "razer: unsupported matrix {rows}x{cols}"
        )));
    }
    if rows as usize * cols as usize != zone.leds {
        return Err(Error::Unsupported(format!(
            "razer: matrix {rows}x{cols} does not match {} leds",
            zone.leds
        )));
    }
    Ok((rows, cols))
}

// ---------------------------------------------------------------------------
// ANSI keymap — HID usage -> matrix cell (BlackWidow Elite, 6 rows x 22 cols)
// ---------------------------------------------------------------------------

/// Extracted from OpenRGB: `razer_blackwidow_elite_layout`
/// (Controllers/RazerController/RazerDevices.cpp) applied over the
/// ANSI-QWERTY base layout (KeyboardLayoutManager.cpp) exactly as
/// `RGBController_Razer::SetupZones` composes it (base zones -> ansi_qwerty
/// overlay -> device overlay edits, with insert-shift-right column shifts).
/// Standard keyboard-page usages only: the media keys (prev/play/next/mute),
/// the Fn key and the Logo LED have no standard HID usage and are skipped
/// (109 lit cells -> 103 mappable usages). Sorted by usage; lookup is a
/// linear scan (ponytail: fine at human keypress rates; a [u8; 256]
/// usage->cell table is the upgrade if a macro engine ever hammers it).
pub const KEYMAP: &[(u16 /*hid usage*/, u8 /*row*/, u8 /*col*/)] = &[
    (0x04, 3, 2),  // A
    (0x05, 4, 7),  // B
    (0x06, 4, 5),  // C
    (0x07, 3, 4),  // D
    (0x08, 2, 4),  // E
    (0x09, 3, 5),  // F
    (0x0A, 3, 6),  // G
    (0x0B, 3, 7),  // H
    (0x0C, 2, 9),  // I
    (0x0D, 3, 8),  // J
    (0x0E, 3, 9),  // K
    (0x0F, 3, 10), // L
    (0x10, 4, 9),  // M
    (0x11, 4, 8),  // N
    (0x12, 2, 10), // O
    (0x13, 2, 11), // P
    (0x14, 2, 2),  // Q
    (0x15, 2, 5),  // R
    (0x16, 3, 3),  // S
    (0x17, 2, 6),  // T
    (0x18, 2, 8),  // U
    (0x19, 4, 6),  // V
    (0x1A, 2, 3),  // W
    (0x1B, 4, 4),  // X
    (0x1C, 2, 7),  // Y
    (0x1D, 4, 3),  // Z
    (0x1E, 1, 11), // 0
    (0x1F, 1, 2),  // 1
    (0x20, 1, 3),  // 2
    (0x21, 1, 4),  // 3
    (0x22, 1, 5),  // 4
    (0x23, 1, 6),  // 5
    (0x24, 1, 7),  // 6
    (0x25, 1, 8),  // 7
    (0x26, 1, 9),  // 8
    (0x27, 1, 10), // 9
    (0x28, 3, 14), // Enter
    (0x29, 0, 1),  // Esc
    (0x2A, 1, 14), // Backspace
    (0x2B, 2, 1),  // Tab
    (0x2C, 5, 6),  // Space
    (0x2D, 1, 12), // -
    (0x2E, 1, 13), // =
    (0x2F, 2, 12), // [
    (0x30, 2, 13), // ]
    (0x31, 2, 14), // \ (ANSI)
    (0x33, 3, 11), // ;
    (0x34, 3, 12), // '
    (0x35, 1, 1),  // `
    (0x36, 4, 10), // ,
    (0x37, 4, 11), // .
    (0x38, 4, 12), // /
    (0x39, 3, 1),  // Caps Lock
    (0x3A, 0, 3),  // F1
    (0x3B, 0, 4),  // F2
    (0x3C, 0, 5),  // F3
    (0x3D, 0, 6),  // F4
    (0x3E, 0, 7),  // F5
    (0x3F, 0, 8),  // F6
    (0x40, 0, 9),  // F7
    (0x41, 0, 10), // F8
    (0x42, 0, 11), // F9
    (0x43, 0, 12), // F10
    (0x44, 0, 13), // F11
    (0x45, 0, 14), // F12
    (0x46, 0, 15), // Print Screen
    (0x47, 0, 16), // Scroll Lock
    (0x48, 0, 17), // Pause
    (0x49, 1, 15), // Insert
    (0x4A, 1, 16), // Home
    (0x4B, 1, 17), // Page Up
    (0x4C, 2, 15), // Delete
    (0x4D, 2, 16), // End
    (0x4E, 2, 17), // Page Down
    (0x4F, 5, 17), // Right Arrow
    (0x50, 5, 15), // Left Arrow
    (0x51, 5, 16), // Down Arrow
    (0x52, 4, 16), // Up Arrow
    (0x53, 1, 18), // Num Lock
    (0x54, 1, 19), // Numpad /
    (0x55, 1, 20), // Numpad *
    (0x56, 1, 21), // Numpad -
    (0x57, 2, 21), // Numpad +
    (0x58, 4, 21), // Numpad Enter
    (0x59, 4, 18), // Numpad 1
    (0x5A, 4, 19), // Numpad 2
    (0x5B, 4, 20), // Numpad 3
    (0x5C, 3, 18), // Numpad 4
    (0x5D, 3, 19), // Numpad 5
    (0x5E, 3, 20), // Numpad 6
    (0x5F, 2, 18), // Numpad 7
    (0x60, 2, 19), // Numpad 8
    (0x61, 2, 20), // Numpad 9
    (0x62, 5, 19), // Numpad 0
    (0x63, 5, 20), // Numpad .
    (0x65, 5, 13), // Menu
    (0xE0, 5, 1),  // Left Ctrl
    (0xE1, 4, 1),  // Left Shift
    (0xE2, 5, 3),  // Left Alt
    (0xE3, 5, 2),  // Left GUI (Windows)
    (0xE4, 5, 14), // Right Ctrl
    (0xE5, 4, 14), // Right Shift
    (0xE6, 5, 10), // Right Alt (AltGr)
];

/// Map a standard HID keyboard usage to its cell on the BlackWidow Elite
/// matrix. `None` for usages with no lit cell on this layout (media keys,
/// Fn, Logo, Right GUI). Modifier usages 0xE0..0xE6 are included.
pub fn key_to_cell(usage: u16) -> Option<(u8, u8)> {
    KEYMAP
        .iter()
        .find(|&&(u, _, _)| u == usage)
        .map(|&(_, r, c)| (r, c))
}

/// Decode an 8-byte boot-keyboard report into the pressed usages:
/// byte 0 = modifier bitmap (ignored — callers wanting modifiers check the
/// bitmap against 0xE0..0xE7 themselves), byte 1 = reserved, bytes 2..8 =
/// usage array where 0x00 marks an empty slot. Shorter buffers decode the
/// slots that exist (trust boundary: len is the OS-given read size).
pub fn decode_boot_report(buf: &[u8]) -> Vec<u16> {
    buf.iter()
        .skip(2)
        .take(6)
        .filter(|&&b| b != 0)
        .map(|&b| b as u16)
        .collect()
}

/// Open the keyboard *input* interface of a Razer device: the HID interface
/// with usage_page 0x01 / usage 0x06 (boot keyboard). The daemon's
/// key-capture thread reads boot reports from this handle and feeds
/// [`key_to_cell`].
///
/// Takes the caller's `HidApi` on purpose: the daemon shares one
/// process-wide `HidApi`, and constructing a second instance in the same
/// process crashed CoreFoundation once on macOS — always reuse the
/// daemon's instance, never `HidApi::new()` here.
pub fn open_input_interface(api: &HidApi, vid: u16, pid: u16) -> Result<HidDevice> {
    let entry = api
        .device_list()
        .find(|d| {
            d.vendor_id() == vid
                && d.product_id() == pid
                && d.usage_page() == 0x01
                && d.usage() == 0x06
        })
        .ok_or_else(|| {
            Error::Unsupported(format!(
                "razer {vid:04x}:{pid:04x}: no keyboard input interface \
                 (usage_page 1, usage 6) found"
            ))
        })?;
    Ok(api.open_path(entry.path())?)
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// After this many consecutive fast (unvalidated) frames, force one
/// fully-validated frame to re-sync with the device.
const REVALIDATE_INTERVAL: u32 = 60;

/// Fast-path decision: frames 0, 61, 122, ... (every
/// `REVALIDATE_INTERVAL + 1`-th frame) run fully validated; the rest fire
/// blind and only drain stale responses. Pure seam so the pacing is
/// unit-testable.
pub fn should_validate(frame_count: u32) -> bool {
    frame_count.is_multiple_of(REVALIDATE_INTERVAL + 1)
}

pub struct RazerBackend {
    dev: HidDevice,
    info: DeviceInfo,
    rows: u8,
    cols: u8,
    brightness_maxed: bool,
    /// Frames sent since the last fully-validated one. 0 = next frame
    /// validates (drives the fast-path decision in `write_frame`).
    frame_count: u32,
}

impl RazerBackend {
    /// Open the vendor control interface. The discovery path points at the
    /// device's *first* HID interface, so re-enumerate and rank interfaces
    /// (OpenRGB registers the BlackWidow Elite on interface 2, usage_page 1,
    /// usage 2), then let a live firmware-query exchange pick the winner.
    pub fn open(dev: &DiscoveredDevice) -> Result<Self> {
        let (rows, cols) = matrix_dims(&dev.info)?;
        let api = HidApi::new()?;

        // ponytail: interfaces matched by vid+pid only — two identical
        // Razer keyboards on one bus would need serial disambiguation.
        let mut candidates: Vec<(u8, std::ffi::CString)> = api
            .device_list()
            .filter(|d| d.vendor_id() == dev.info.vid && d.product_id() == dev.info.pid)
            .map(|d| {
                (
                    iface_score(d.interface_number(), d.usage_page(), d.usage()),
                    d.path().to_owned(),
                )
            })
            .collect();
        candidates.sort_by_key(|(score, _)| *score);

        let mut fallback: Option<Self> = None;
        for (_, path) in candidates {
            let Ok(handle) = api.open_path(&path) else {
                continue;
            };
            let backend = Self {
                dev: handle,
                info: dev.info.clone(),
                rows,
                cols,
                brightness_maxed: false,
                frame_count: 0,
            };
            if backend.exchange(firmware_report()).is_ok() {
                return Ok(backend); // confirmed by a real feature-report exchange
            }
            fallback.get_or_insert(backend);
        }
        fallback.ok_or_else(|| {
            Error::Unsupported(format!(
                "razer {}: could not open any HID interface — on macOS grant this \
                 terminal Input Monitoring (Privacy & Security) or run with sudo",
                dev.info.name
            ))
        })
    }

    /// Send a report and validate the device's response — mirrors
    /// `razer_usb_query`: match (transaction id, class, id) and a final
    /// status, then require STATUS_SUCCESS.
    fn exchange(&self, mut report: Report) -> Result<()> {
        report[OFF_CRC] = crc(&report);
        for _ in 0..3 {
            sleep(Duration::from_millis(2));
            self.dev.send_feature_report(&report)?;

            let mut resp = [0u8; REPORT_LEN];
            for _ in 0..25 {
                sleep(Duration::from_millis(2));
                if self.dev.get_feature_report(&mut resp).is_err() {
                    break; // device gone mid-attempt; retry the send
                }
                let final_status =
                    resp[OFF_STATUS] != STATUS_NEW && resp[OFF_STATUS] != STATUS_BUSY;
                if resp[OFF_TRANS] == report[OFF_TRANS]
                    && resp[OFF_CLASS] == report[OFF_CLASS]
                    && resp[OFF_ID] == report[OFF_ID]
                    && final_status
                {
                    return if resp[OFF_STATUS] == STATUS_SUCCESS {
                        Ok(())
                    } else {
                        Err(Error::Unsupported(format!(
                            "razer {}: command {:#04x}/{:#04x} failed, status {:#04x}",
                            self.info.name, report[OFF_CLASS], report[OFF_ID], resp[OFF_STATUS]
                        )))
                    };
                }
            }
        }
        Err(Error::Unsupported(format!(
            "razer {}: no response to command {:#04x}/{:#04x}",
            self.info.name, report[OFF_CLASS], report[OFF_ID]
        )))
    }

    /// Max brightness once per backend lifetime (mode-activation behavior).
    fn ensure_brightness(&mut self) -> Result<()> {
        if !self.brightness_maxed {
            self.exchange(brightness_report(MAX_BRIGHTNESS))?;
            self.brightness_maxed = true;
        }
        Ok(())
    }

    /// Fire one report without reading its response. On macOS,
    /// `get_feature_report` is a synchronous `IOHIDDeviceGetReport` kernel
    /// roundtrip with NO timeout — "draining" pending responses can block
    /// indefinitely once the firmware stalls under frame flood (observed:
    /// scheduler wedged in-kernel holding the engine slot lock). So the fast
    /// path never reads; error detection is deferred to the periodic
    /// validated exchange. Unread feature-report responses do not queue in
    /// the OS (GetReport is a pull), so nothing backs up.
    fn send_fired(&self, mut report: Report) -> Result<()> {
        report[OFF_CRC] = crc(&report);
        Ok(self.dev.send_feature_report(&report)?)
    }
}

/// Rank HID interfaces for the control endpoint. Verified target: interface
/// 2 with usage_page 1 / usage 2. Lower is better.
fn iface_score(interface: i32, usage_page: u16, usage: u16) -> u8 {
    match (interface, usage_page, usage) {
        (2, 1, 2) => 0, // OpenRGB registration for BlackWidow-class keyboards
        (_, 1, 2) => 1,
        (2, _, _) => 2,
        (_, 1, 0) => 3, // task-brief guess (vendor usage); harmless extra
        _ => 4,
    }
}

impl DriverBackend for RazerBackend {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn set_static(&mut self, color: Rgb) -> Result<()> {
        self.ensure_brightness()?;
        self.exchange(static_report(color))
    }

    /// Frame flow per OpenRGB `SetLEDs`: one feature report per row, then the
    /// custom-frame mode command that applies the frame. The first frame (and
    /// every [`REVALIDATE_INTERVAL`]-th after it) runs fully validated via
    /// [`exchange`](Self::exchange); subsequent frames fire the row reports
    /// without waiting for responses and only drain pending ones (see
    /// [`send_and_drain`]). This keeps the effects engine at ~25 FPS instead
    /// of gating every row report on a validated exchange.
    ///
    /// ponytail: fire-and-drain means a device-side rejection between
    /// re-validation frames is invisible — the frame is silently dropped (an
    /// effect glitch at worst, the next validated frame resyncs the mode).
    /// Upgrade path: per-frame validation or response checksum verification.
    /// `set_static`/brightness keep the fully validated path.
    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        let leds = self.rows as usize * self.cols as usize;
        if colors.len() != leds {
            return Err(Error::Unsupported(format!(
                "razer {}: expected {leds} colors, got {}",
                self.info.name,
                colors.len()
            )));
        }
        self.ensure_brightness()?;
        let validate = should_validate(self.frame_count);
        for row in 0..self.rows {
            let report = frame_row_report(row, self.cols, colors);
            if validate {
                sleep(Duration::from_millis(1));
                self.exchange(report)?;
            } else {
                self.send_fired(report)?;
            }
        }
        let apply = custom_mode_report();
        if validate {
            sleep(Duration::from_millis(1));
            self.exchange(apply)?;
        } else {
            self.send_fired(apply)?;
        }
        // Advance every frame (validated or fast) so the schedule in
        // `should_validate` holds: validate at 0, 61, 122, ...
        self.frame_count = self.frame_count.wrapping_add(1);
        Ok(())
    }

    fn supports_per_led(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Tests — packet builders and validation, no hardware needed.
// The one hardware test is #[ignore]d; see module docs for permission setup.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn bw_info() -> DeviceInfo {
        let m = crate::lookup(0x1532, 0x0228).unwrap();
        DeviceInfo {
            id: "1532:0228:t".into(),
            name: m.name,
            vendor: m.vendor,
            vid: 0x1532,
            pid: 0x0228,
            kind: m.kind,
            zones: m.zones,
            note: m.note,
        }
    }

    #[test]
    fn crc_is_xor_of_bytes_3_to_88() {
        // Golden vector, hand-computed: only nonzero bytes in [3..89] are
        // data_size=0x02 (offset 6) and id=0x81 (offset 8) -> 0x02 ^ 0x81.
        let mut buf = [0u8; REPORT_LEN];
        buf[OFF_DATA_SIZE] = 0x02;
        buf[OFF_ID] = 0x81;
        assert_eq!(crc(&buf), 0x83);

        // Range invariants straight from the OpenRGB loop (i in 3..89):
        // transaction id excluded, data_size included, crc slot excluded.
        let mut buf = firmware_report();
        let expected = buf[3..OFF_CRC].iter().fold(0, |a, &b| a ^ b);
        buf[OFF_TRANS] ^= 0xff; // outside the CRC range
        assert_eq!(crc(&buf), expected);
        buf[OFF_DATA_SIZE] ^= 0xff; // inside the CRC range
        assert_ne!(crc(&buf), expected);
    }

    #[test]
    fn static_report_layout() {
        let buf = static_report(Rgb {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        });
        assert_eq!(buf.len(), 91);
        assert_eq!(buf[0], 0x00); // report id
        assert_eq!(buf[OFF_TRANS], 0x1f);
        assert_eq!(buf[OFF_DATA_SIZE], 0x09);
        assert_eq!(buf[OFF_CLASS], 0x0f);
        assert_eq!(buf[OFF_ID], 0x02);
        assert_eq!(
            &buf[OFF_ARGS..OFF_ARGS + 9],
            &[0x00, 0x05, 0x01, 0, 0, 0x01, 0x11, 0x22, 0x33]
        );
        assert_eq!(buf[90], 0x00); // reserved
                                   // Builders leave CRC for the send path (OpenRGB stamps it in
                                   // razer_usb_send); hand-computed golden: 09^0f^02^05^01^01^11^22^33.
        assert_eq!(buf[OFF_CRC], 0x00);
        let mut stamped = buf;
        stamped[OFF_CRC] = crc(&stamped);
        assert_eq!(stamped[OFF_CRC], 0x01);
    }

    #[test]
    fn frame_row_layout_and_row_major_mapping() {
        // Distinct color per LED; row-major with 22 cols.
        let colors: Vec<Rgb> = (0..132)
            .map(|i| Rgb {
                r: i as u8,
                g: 0,
                b: 0xff,
            })
            .collect();
        let buf = frame_row_report(3, 22, &colors);
        assert_eq!(buf[OFF_DATA_SIZE], 22 * 3 + 5); // 71
        assert_eq!(buf[OFF_CLASS], 0x0f);
        assert_eq!(buf[OFF_ID], 0x03);
        assert_eq!(buf[OFF_ARGS + 2], 3); // row index
        assert_eq!(buf[OFF_ARGS + 3], 0); // start col
        assert_eq!(buf[OFF_ARGS + 4], 21); // stop col
                                           // First LED of row 3 = index 66, payload at args[5..8].
        assert_eq!(&buf[OFF_ARGS + 5..OFF_ARGS + 8], &[66, 0, 0xff]);
        // Last LED of the row lands at args[5 + 21*3 ..].
        assert_eq!(&buf[OFF_ARGS + 5 + 63..OFF_ARGS + 5 + 66], &[87, 0, 0xff]);
        // Row payload fits the 80-byte argument block (22*3+5=71 <= 80).
        assert!(5 + 22 * 3 <= 80);
        assert_eq!(buf[OFF_CRC], 0x00); // stamped by exchange(), not the builder
    }

    #[test]
    fn keymap_integrity() {
        // Extracted from OpenRGB razer_blackwidow_elite_layout over the
        // ANSI base: unique usages AND unique cells, all inside 6x22.
        let mut usages = std::collections::HashSet::new();
        let mut cells = std::collections::HashSet::new();
        for &(u, r, c) in KEYMAP {
            assert!(usages.insert(u), "duplicate usage {u:#04x}");
            assert!(cells.insert((r, c)), "duplicate cell ({r},{c})");
            assert!(r < 6, "row {r} out of range");
            assert!(c < 22, "col {c} out of range");
        }
        // 109 lit cells; media x4 + Fn + Logo have no standard usage -> 103.
        assert_eq!(KEYMAP.len(), 103);
    }

    #[test]
    fn key_to_cell_spot_checks_from_the_table() {
        // Spot checks asserted FROM the extracted table, not from memory of
        // the C++ source: each claim restates a row of KEYMAP above.
        // Escape: the Elite overlay moved Esc to (0,1).
        assert_eq!(key_to_cell(0x29), Some((0, 1)));
        // Space: overlay removed the cell at (5,6) and shifted the row left,
        // landing Space on (5,6).
        assert_eq!(key_to_cell(0x2C), Some((5, 6)));
        // A opens the letters block at (3,2); modifier usages map too.
        assert_eq!(key_to_cell(0x04), Some((3, 2)));
        assert_eq!(key_to_cell(0xE0), Some((5, 1)));
        // Nothing maps to the row-0 media block (cols 18..21), the Logo LED
        // (no standard usage), or unknown usages.
        assert_eq!(KEYMAP.iter().find(|k| k.1 == 0 && k.2 >= 18), None);
        assert_eq!(key_to_cell(0x00), None);
        assert_eq!(key_to_cell(0x99), None);
        // Every table entry round-trips through key_to_cell.
        for &(u, r, c) in KEYMAP {
            assert_eq!(key_to_cell(u), Some((r, c)), "usage {u:#04x}");
        }
    }

    #[test]
    fn decode_boot_report_cases() {
        // Empty report: no keys.
        assert_eq!(decode_boot_report(&[0; 8]), Vec::<u16>::new());
        // Single key: A (0x04).
        assert_eq!(decode_boot_report(&[0, 0, 0x04, 0, 0, 0, 0, 0]), vec![0x04]);
        // 6-key rollover: all slots filled, preserved in slot order.
        assert_eq!(
            decode_boot_report(&[0, 0, 0x04, 0x05, 0x06, 0x2A, 0x39, 0xE1]),
            vec![0x04, 0x05, 0x06, 0x2A, 0x39, 0xE1]
        );
        // Modifier byte ignored: byte0 bitmap (e.g. 0x01 = Left Ctrl) never
        // leaks into the returned usages.
        assert_eq!(
            decode_boot_report(&[0x01, 0, 0x04, 0, 0, 0, 0, 0]),
            decode_boot_report(&[0, 0, 0x04, 0, 0, 0, 0, 0])
        );
        // Zero slots in the middle are skipped.
        assert_eq!(
            decode_boot_report(&[0, 0, 0x00, 0x29, 0x00, 0x2C, 0x00, 0x00]),
            vec![0x29, 0x2C]
        );
        // Short buffer decodes only the slots present.
        assert_eq!(decode_boot_report(&[0, 0, 0x04]), vec![0x04]);
        assert_eq!(decode_boot_report(&[0]), Vec::<u16>::new());
    }

    #[test]
    fn fast_path_validation_schedule() {
        // First frame after open/re-sync validates, then 60 fast frames,
        // then validate again — one validated frame per 61 sent.
        assert!(should_validate(0));
        for n in 1..=60 {
            assert!(!should_validate(n), "frame {n} should be fast");
        }
        assert!(should_validate(61));
        assert!(!should_validate(62));
        // Wrap-around sanity over several cycles.
        let validates: Vec<u32> = (0..200).filter(|&n| should_validate(n)).collect();
        assert_eq!(validates, vec![0, 61, 122, 183]);
    }

    #[test]
    fn custom_mode_and_brightness_layout() {
        let cm = custom_mode_report();
        assert_eq!(
            (cm[OFF_CLASS], cm[OFF_ID], cm[OFF_DATA_SIZE]),
            (0x0f, 0x02, 0x0c)
        );
        assert_eq!(&cm[OFF_ARGS..OFF_ARGS + 3], &[0x00, 0x00, 0x08]);

        let br = brightness_report(0xff);
        assert_eq!(
            (br[OFF_CLASS], br[OFF_ID], br[OFF_DATA_SIZE]),
            (0x0f, 0x04, 0x03)
        );
        assert_eq!(&br[OFF_ARGS..OFF_ARGS + 3], &[0x00, 0x05, 0xff]);
    }

    #[test]
    fn matrix_dims_validates_registry_zone() {
        let (rows, cols) = matrix_dims(&bw_info()).unwrap();
        assert_eq!((rows, cols), (6, 22));

        let mut bad = bw_info();
        bad.zones[0].leds = 131; // led count no longer rows*cols
        assert!(matrix_dims(&bad).is_err());

        let mut bad = bw_info();
        bad.zones[0].matrix = None;
        assert!(matrix_dims(&bad).is_err());

        let mut bad = bw_info();
        bad.zones.push(bad.zones[0].clone());
        assert!(matrix_dims(&bad).is_err());

        let mut bad = bw_info();
        bad.zones[0].matrix = Some((6, 26)); // row payload overflows args[80]
        assert!(matrix_dims(&bad).is_err());
    }

    #[test]
    #[ignore = "needs the Razer keyboard attached + HID permission; see module docs"]
    fn hardware_set_static() {
        let api = HidApi::new().unwrap();
        let Some(dev) = crate::discover(&api)
            .into_iter()
            .find(|d| d.info.kind == crate::DriverKind::Razer)
        else {
            println!("SKIP: no Razer device attached");
            return;
        };
        let mut backend = match RazerBackend::open(&dev) {
            Ok(b) => b,
            Err(e) => {
                println!(
                    "SKIP (open failed — on macOS this is usually the IOKit HID \
                     permission wall): {e}\n  Fix: System Settings → Privacy & \
                     Security → Input Monitoring → enable this terminal, or run \
                     the test with sudo."
                );
                return;
            }
        };
        match backend.set_static(Rgb { r: 255, g: 0, b: 0 }) {
            Ok(()) => println!("OK: keyboard should be solid red"),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("IOKit") || msg.to_lowercase().contains("permission") {
                    println!("SKIP (permission): {msg}");
                } else {
                    panic!("set_static failed on real hardware: {msg}");
                }
            }
        }
    }
}
