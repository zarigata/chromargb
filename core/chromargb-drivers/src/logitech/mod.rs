//! Logitech LIGHTSYNC driver — HID++ 2.0 protocol, zone-effect devices.
//!
//! Implemented from OpenRGB's unified Logitech HID++ 2.0 controller:
//!   Controllers/LogitechController/LogitechHIDPP20Controller/LogitechHIDPP20Controller.{h,cpp}
//!   Controllers/LogitechController/LogitechProtocolCommon.h
//!   Controllers/LogitechController/RGBController_LogitechHIDPP20.cpp
//!   Controllers/LogitechController/LogitechControllerDetect.cpp
//! (https://github.com/CalcProgrammer1/OpenRGB, master; fetched 2026-11-07)
//!
//! Transport (LogitechProtocolCommon.h, SendStandard in LogitechHIDPP20Controller.cpp):
//!   short frame:  [0x10][dev_idx][feat_idx][func|SW_ID][payload ≤ 3][pad]  (7 bytes)
//!   long  frame:  [0x11][dev_idx][feat_idx][func|SW_ID][payload ≤16][pad] (20 bytes)
//!   Written with hid_write on the HID++ collection (interface 1, usage page
//!   0xFF00, usage 2 — LogitechControllerDetect.cpp REGISTER_HID_DETECTOR_IPU
//!   for G502 HERO 0xC08B). macOS exposes both TLCs through one handle, so the
//!   payload-size heuristic picks the frame (PrefersShortFrame).
//!
//! Feature discovery (GetFeatureIndex): root feature 0x0000, function 0x00,
//! payload = feature u16 BE; response data[0] = feature index (0 = absent),
//! data[2] = version.
//!
//! Lighting on the G502 HERO: feature RGB_EFFECTS 0x8071, SetEffectByIndex
//! (fn 0x10) with a 16-byte payload, one LED per zone cluster (zone 0 = logo,
//! zone 1 = wheel; locations 0x0002/0x0001 reported by GetRgbClusterInfo).
//! On 0x8071 every mode write carries persist=true (observed vendor-app wire
//! capture; persist=false is accepted but not committed).
//!
//! Claim (zone-only devices): RGB_EFFECTS SetSWControl (fn 0x50) payload
//! [0x01, 0x03, 0x05] — set / mode 3 / flags 5 (effect control + NV config).
//! The two-phase flags=6->5 dance exists to hide the onboard->host flash while
//! per-key painting takes over; a zone-only device has no per-key layer, so the
//! steady-state flags=5 claim is written directly (matches the post-paint
//! state OpenRGB leaves the device in).

use std::collections::HashMap;

use crate::{DeviceInfo, DiscoveredDevice, DriverBackend, Error, Result, Rgb};

// --- Transport (LogitechProtocolCommon.h) ----------------------------------
pub const REPORT_SHORT: u8 = 0x10;
pub const REPORT_LONG: u8 = 0x11;
pub const REPORT_SHORT_LEN: usize = 7;
pub const REPORT_LONG_LEN: usize = 20;
/// HID++ software ID; ties a reply to its requester (HIDPP20_SW_ID).
pub const SW_ID: u8 = 0x07;
/// Wired devices are addressed as 0xFF (LOGITECH_DEFAULT_DEVICE_INDEX).
pub const DEVICE_IDX: u8 = 0xFF;

// --- Features (LogitechHIDPP20Controller.h) ---------------------------------
pub const FEAT_ROOT: u16 = 0x0000;
pub const FEAT_COLOR_LED_EFFECTS: u16 = 0x8070;
pub const FEAT_RGB_EFFECTS: u16 = 0x8071;

/// Root.GetFeature(fn 0) doubles as the fn id for GetInfo-style queries.
pub const FN_GET_FEATURE: u8 = 0x00;
/// RGB_EFFECTS: GetInfo = fn 0, SetEffectByIndex = fn 0x10, SetSWControl = 0x50.
pub const FN_GET_INFO: u8 = 0x00;
pub const FN_SET_EFFECT: u8 = 0x10;
pub const FN_SW_CONTROL: u8 = 0x50;

/// COLOR_LED_EFFECTS fallback fns (LogitechProtocolCommon.h).
pub const FN_8070_GET_INFO: u8 = 0x10;
pub const FN_8070_SET_EFFECT: u8 = 0x30;
pub const FN_8070_SW_CONTROL: u8 = 0x50;

/// Effect ids (SetZoneEffect switch in LogitechHIDPP20Controller.cpp).
pub const EFFECT_STATIC: u16 = 0x0001;
pub const EFFECT_BREATHING: u16 = 0x000A;

/// Default period (ms) for effects that take one; OpenRGB G502 Breathing
/// default period (RGBController_LogitechHIDPP20.cpp mode table).
pub const DEFAULT_PERIOD_MS: u16 = 5000;
/// Full brightness (0..100 scale used by the protocol).
pub const FULL_BRIGHTNESS: u8 = 100;

/// 0x8071 SetEffectByIndex payload length.
const SET_EFFECT_LEN: usize = 16;

/// HID++ vendor collection: usage page 0xFF00, usage 2 carries both the
/// short (0x10) and long (0x11) reports; usage 1 is the very-long-only one
/// (LogitechControllerDetect.cpp registers G502 HERO on exactly this tuple).
const HIDPP_USAGE_PAGE: u16 = 0xFF00;
const HIDPP_USAGE: u16 = 2;

/// HID++ read window per attempt (ms). OpenRGB's read_window for probe/reliable
/// policies is hundreds of ms; 300 ms covers a busy first contact.
const READ_TIMEOUT_MS: i32 = 300;
/// Send attempts per command; OpenRGB's reliable policy is 7 with backoff, but
/// a wired mouse answers in low single-digit ms — 3 tight retries is plenty.
const ATTEMPTS: usize = 3;
/// SetSWControl claim payload for 0x8071: set / mode 3 / flags 5.
const SW_CONTROL_CLAIM: [u8; 3] = [0x01, 0x03, 0x05];

pub struct LogitechBackend {
    info: DeviceInfo,
    dev: hidapi::HidDevice,
    /// feature page actually in use (0x8071 or 0x8070).
    feature_page: u16,
    /// feature index for `feature_page` (0 = discovery not run / absent).
    feature_idx: u8,
    /// cache of root.GetFeature lookups (index; 0 = absent).
    feature_cache: HashMap<u16, u8>,
    /// (cluster index, effect index) per zone, in registry zone order.
    /// Effect indices are per-cluster and discovered from the device.
    zones: Vec<ZoneCluster>,
    claimed: bool,
    /// latched when a short frame goes unanswered (macOS: rare, but the
    /// collection may not expose a short report).
    long_only: bool,
}

#[derive(Debug, Clone)]
struct ZoneCluster {
    cluster_idx: u8,
    /// effect index of the Static effect within this cluster.
    static_idx: u8,
    /// SetEffect fn id (0x10 on 0x8071, 0x30 on 0x8070).
    set_effect_fn: u8,
}

impl LogitechBackend {
    /// Open the HID++ vendor collection (usage page 0xFF00, usage 2 — NOT the
    /// discovery path, which may be the mouse interface). Kept cheap: no
    /// protocol traffic until the first lighting command triggers lazy
    /// discovery.
    pub fn open(dev: &DiscoveredDevice) -> Result<Self> {
        let api = hidapi::HidApi::new()?;
        let path = api
            .device_list()
            .find(|d| {
                d.vendor_id() == dev.info.vid
                    && d.product_id() == dev.info.pid
                    && d.usage_page() == HIDPP_USAGE_PAGE
                    && d.usage() == HIDPP_USAGE
            })
            .map(|d| d.path().to_owned())
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "{}: no HID++ interface (usage page 0x{HIDPP_USAGE_PAGE:04X} usage 2) found",
                    dev.info.name
                ))
            })?;
        let handle = api.open_path(&path)?;
        Ok(Self {
            info: dev.info.clone(),
            dev: handle,
            feature_page: 0,
            feature_idx: 0,
            feature_cache: HashMap::new(),
            zones: Vec::new(),
            claimed: false,
            long_only: false,
        })
    }

    fn total_leds(&self) -> usize {
        self.info.zones.iter().map(|z| z.leds).sum()
    }

    /// Root.GetFeature: resolve a feature page to its device index.
    fn get_feature_index(&mut self, feature: u16) -> Result<u8> {
        if let Some(&idx) = self.feature_cache.get(&feature) {
            return Ok(idx);
        }
        let resp = self.send(FEAT_ROOT_IDX, FN_GET_FEATURE, &feature.to_be_bytes())?;
        let idx = resp[0];
        self.feature_cache.insert(feature, idx);
        Ok(idx)
    }

    /// Discover RGB feature + zone clusters. Idempotent.
    fn ensure_discovered(&mut self) -> Result<()> {
        if !self.zones.is_empty() {
            return Ok(());
        }

        // 0x8071 first, 0x8070 fallback (DiscoverRGBEffects).
        let (page, idx) = match self.get_feature_index(FEAT_RGB_EFFECTS)? {
            0 => match self.get_feature_index(FEAT_COLOR_LED_EFFECTS)? {
                0 => {
                    return Err(Error::Unsupported(format!(
                        "{}: no RGB_EFFECTS 0x8071 / COLOR_LED_EFFECTS 0x8070 feature; \
                         device firmware exposes no zone lighting control",
                        self.info.name
                    )))
                }
                idx => (FEAT_COLOR_LED_EFFECTS, idx),
            },
            idx => (FEAT_RGB_EFFECTS, idx),
        };
        self.feature_page = page;
        self.feature_idx = idx;

        // GetInfo: cluster count.
        let (fn_info, data) = if page == FEAT_RGB_EFFECTS {
            (FN_GET_INFO, [0xFFu8, 0xFF, 0x00])
        } else {
            (FN_8070_GET_INFO, [0u8; 3])
        };
        let resp = self.send(idx, fn_info, &data[..if page == FEAT_RGB_EFFECTS { 3 } else { 0 }])?;
        let cluster_count = if page == FEAT_RGB_EFFECTS {
            resp[2]
        } else {
            resp[0]
        } as usize;

        // Registry zone count is our LED contract (2 on G502); the device may
        // enumerate more clusters than we expose.
        let zone_count = self.info.zones.len().max(1).min(cluster_count.max(1));
        for z in 0..zone_count {
            // GetRgbClusterInfo per cluster; locations (logo=0x0002,
            // wheel=0x0001 on G502) only label the zones.
            let query = if page == FEAT_RGB_EFFECTS {
                [z as u8, 0xFF]
            } else {
                [z as u8, 0x00]
            };
            let resp = self.send(idx, fn_info, &query)?;
            let effect_count = if page == FEAT_RGB_EFFECTS {
                resp[4]
            } else {
                resp[3]
            } as usize;

            // GetEffectInfo per effect to find the Static slot (effect ids are
            // NOT the effect indices; each cluster indexes its own table).
            let mut static_idx = 0;
            let eff_info_fn = if page == FEAT_RGB_EFFECTS {
                FN_GET_INFO
            } else {
                FN_8070_EFFECT_INFO
            };
            for e in 0..effect_count.min(16) {
                let q = [z as u8, e as u8, 0x00, 0x00];
                if let Ok(r) = self.send(idx, eff_info_fn, &q) {
                    let effect_id = ((r[2] as u16) << 8) | r[3] as u16;
                    if effect_id == EFFECT_STATIC {
                        static_idx = e as u8;
                        break;
                    }
                }
            }

            let set_effect_fn = if page == FEAT_RGB_EFFECTS {
                FN_SET_EFFECT
            } else {
                FN_8070_SET_EFFECT
            };
            self.zones.push(ZoneCluster {
                cluster_idx: z as u8,
                static_idx,
                set_effect_fn,
            });
        }

        if self.zones.is_empty() {
            return Err(Error::Unsupported(format!(
                "{}: RGB feature 0x{:04X} reports no zone clusters",
                self.info.name, page
            )));
        }
        Ok(())
    }

    /// Claim SW control once (zone devices: steady-state flags=5 claim).
    fn ensure_claimed(&mut self) -> Result<()> {
        if self.claimed {
            return Ok(());
        }
        // 0x8071: [set, mode, flags]; 0x8070: [enabled, persist].
        let data: &[u8] = if self.feature_page == FEAT_RGB_EFFECTS {
            &SW_CONTROL_CLAIM
        } else {
            &[0x01, 0x01]
        };
        self.send(self.feature_idx, FN_SW_CONTROL, data)?;
        self.claimed = true;
        Ok(())
    }

    /// Send a HID++ feature call and return the 16-byte response payload
    /// (bytes 4..20 of the reply frame).
    fn send(&mut self, feat_idx: u8, function: u8, payload: &[u8]) -> Result<[u8; 16]> {
        let mut last_err = None;
        for _ in 0..ATTEMPTS {
            match self.send_once(feat_idx, function, payload) {
                Ok(resp) => return Ok(resp),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Hid("no attempts made".into())))
    }

    fn send_once(&mut self, feat_idx: u8, function: u8, payload: &[u8]) -> Result<[u8; 16]> {
        // Wire: write the frame, then read until a reply matches feat+func+SW_ID
        // (SendAcked). A stale/unrelated frame is discarded and reading
        // continues within the window.
        let mut frame = [0u8; REPORT_LONG_LEN];
        let (report_id, frame_len) = if payload.len() <= 3 && !self.long_only {
            (REPORT_SHORT, REPORT_SHORT_LEN)
        } else {
            (REPORT_LONG, REPORT_LONG_LEN)
        };
        frame[0] = report_id;
        frame[1] = DEVICE_IDX;
        frame[2] = feat_idx;
        frame[3] = function | SW_ID;
        let n = payload.len().min(16);
        frame[4..4 + n].copy_from_slice(&payload[..n]);
        let wire: Vec<u8> = frame[..frame_len].to_vec();
        self.dev.write(&wire)?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(READ_TIMEOUT_MS as u64);
        let mut buf = [0u8; 64];
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let rem_ms = remaining.as_millis() as i32;
            if rem_ms <= 0 {
                return Err(Error::Hid(format!(
                    "HID++: no reply to feat 0x{feat_idx:02x} fn 0x{function:02x} within {READ_TIMEOUT_MS} ms"
                )));
            }
            let n = self.dev.read_timeout(&mut buf, rem_ms)?;
            if n == 0 {
                continue;
            }
            // Only HID++ frames on the wire (keystrokes/motion share the node).
            if !matches!(buf[0], REPORT_SHORT | REPORT_LONG | 0x12) {
                continue;
            }
            if buf[1] != DEVICE_IDX {
                continue;
            }
            // HID++ error frame: feat=0xFF, func byte = failing feature,
            // payload[0] = failing function, payload[1] = error code.
            if buf[2] == 0xFF {
                let err_feat = buf[3];
                let err_func = buf[4];
                let err_code = buf[5];
                if err_feat == feat_idx && (err_func & 0xF0) == (function & 0xF0) {
                    return Err(Error::Hid(format!(
                        "HID++ error 0x{err_code:02x} (feat 0x{feat_idx:02x} fn 0x{function:02x})"
                    )));
                }
                continue; // error for someone else's request
            }
            if buf[2] == feat_idx && (buf[3] & 0xF0) == (function & 0xF0) && (buf[3] & 0x0F) == SW_ID
            {
                // Latch long-only on a silent short frame? Silence already
                // returned the timeout error above; here the reply matched.
                let mut payload = [0u8; 16];
                payload.copy_from_slice(&buf[4..20]);
                return Ok(payload);
            }
            // Non-matching frame: stale, keep reading.
        }
    }
}

// --- Packet builders (pure; unit-tested) ------------------------------------

/// Short (7-byte) HID++ frame when payload fits, else long (20-byte).
pub fn hidpp_frame(dev_idx: u8, feat_idx: u8, function: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0u8; if payload.len() <= 3 { REPORT_SHORT_LEN } else { REPORT_LONG_LEN }];
    frame[0] = if payload.len() <= 3 { REPORT_SHORT } else { REPORT_LONG };
    frame[1] = dev_idx;
    frame[2] = feat_idx;
    frame[3] = function | SW_ID;
    let n = payload.len().min(16);
    frame[4..4 + n].copy_from_slice(&payload[..n]);
    frame
}

/// Root.GetFeature request payload: feature id, big-endian.
pub fn get_feature_payload(feature: u16) -> [u8; 2] {
    feature.to_be_bytes()
}

/// SetEffectByIndex payload for Static (effect 0x0001):
/// [cluster, effect_idx, R, G, B, 0x02, 0, 0, 0, 0, 0, 0, persist, 0, 0, 0].
/// Byte 5 = 0x02 marks "fixed color" (0x00 there = Off / pass-through).
pub fn static_effect_payload(cluster: u8, effect_idx: u8, color: Rgb) -> [u8; 16] {
    let mut p = [0u8; SET_EFFECT_LEN];
    p[0] = cluster;
    p[1] = effect_idx;
    p[2] = color.r;
    p[3] = color.g;
    p[4] = color.b;
    p[5] = if color.r != 0 || color.g != 0 || color.b != 0 {
        0x02
    } else {
        0x00
    };
    p[12] = 1; // persist=true on 0x8071 (vendor-app wire capture)
    p
}

/// SetEffectByIndex payload for Breathing (effect 0x000A):
/// [cluster, effect_idx, R, G, B, period_hi, period_lo, 0, brightness, ...persist].
pub fn breathing_effect_payload(
    cluster: u8,
    effect_idx: u8,
    color: Rgb,
    period_ms: u16,
    brightness: u8,
) -> [u8; 16] {
    let mut p = [0u8; SET_EFFECT_LEN];
    p[0] = cluster;
    p[1] = effect_idx;
    p[2] = color.r;
    p[3] = color.g;
    p[4] = color.b;
    p[5] = (period_ms >> 8) as u8;
    p[6] = period_ms as u8;
    p[8] = brightness;
    p[12] = 1;
    p
}

impl DriverBackend for LogitechBackend {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    /// All zones static color: claim, then one SetEffectByIndex per zone
    /// (DeviceUpdateLEDs zone path).
    fn set_static(&mut self, color: Rgb) -> Result<()> {
        self.ensure_discovered()?;
        self.ensure_claimed()?;
        let zones = self.zones.clone();
        for z in &zones {
            let payload = static_effect_payload(z.cluster_idx, z.static_idx, color);
            self.send(self.feature_idx, z.set_effect_fn, &payload)?;
        }
        Ok(())
    }

    /// Per-LED frame; logo = colors[0], wheel = colors[1] (registry order).
    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        if colors.len() != self.total_leds() {
            return Err(Error::Unsupported(format!(
                "{}: write_frame expects {} colors, got {}",
                self.info.name,
                self.total_leds(),
                colors.len()
            )));
        }
        self.ensure_discovered()?;
        self.ensure_claimed()?;
        let zones = self.zones.clone();
        for (i, z) in zones.iter().enumerate() {
            let payload = static_effect_payload(z.cluster_idx, z.static_idx, colors[i]);
            self.send(self.feature_idx, z.set_effect_fn, &payload)?;
        }
        Ok(())
    }
}

// FEAT_ROOT_IDX = feature index 0x00 = the root feature itself.
const FEAT_ROOT_IDX: u8 = 0x00;
/// 0x8070 GetZoneEffectInfo (LOGITECH_FP8070_EFFECT_INFO).
const FN_8070_EFFECT_INFO: u8 = 0x20;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_frame_layout() {
        // GetFeature: feat 0x00, fn 0x00, payload = feature id BE.
        let f = hidpp_frame(DEVICE_IDX, FEAT_ROOT_IDX, FN_GET_FEATURE, &get_feature_payload(FEAT_RGB_EFFECTS));
        assert_eq!(f.len(), 7);
        assert_eq!(f, vec![0x10, 0xFF, 0x00, 0x07, 0x80, 0x71, 0x00]);
    }

    #[test]
    fn long_frame_layout() {
        // SetEffectByIndex: 16-byte payload forces the long frame.
        let payload = static_effect_payload(0, 1, Rgb { r: 1, g: 2, b: 3 });
        let f = hidpp_frame(DEVICE_IDX, 1, FN_SET_EFFECT, &payload);
        assert_eq!(f.len(), 20);
        assert_eq!(f[0], 0x11);
        assert_eq!(f[1], 0xFF);
        assert_eq!(f[2], 0x01);
        assert_eq!(f[3], FN_SET_EFFECT | SW_ID);
        assert_eq!(&f[4..20], &payload);
        assert_eq!(f[4], 0); // cluster 0 (logo)
        assert_eq!(f[5], 1); // effect idx = Static
    }

    #[test]
    fn static_payload_layout() {
        let p = static_effect_payload(1, 2, Rgb { r: 0xAA, g: 0xBB, b: 0xCC });
        assert_eq!(p[0], 1); // cluster (wheel)
        assert_eq!(p[1], 2);
        assert_eq!(&p[2..5], &[0xAA, 0xBB, 0xCC]);
        assert_eq!(p[5], 0x02); // fixed-color marker
        assert_eq!(p[12], 1); // persist
        assert!(p[6..12].iter().all(|&b| b == 0));
        assert!(p[13..].iter().all(|&b| b == 0));
    }

    #[test]
    fn static_black_means_off() {
        // All-black drops the 0x02 marker: Off / per-key pass-through.
        let p = static_effect_payload(0, 0, Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(p[5], 0x00);
    }

    #[test]
    fn breathing_payload_layout() {
        let p = breathing_effect_payload(0, 3, Rgb { r: 9, g: 8, b: 7 }, 5000, 100);
        assert_eq!(&p[2..5], &[9, 8, 7]);
        assert_eq!(&p[5..7], &[0x13, 0x88]); // 5000 ms BE
        assert_eq!(p[7], 0);
        assert_eq!(p[8], 100);
        assert_eq!(p[12], 1);
    }

    #[test]
    fn zone_mapping_is_registry_order() {
        // G502: registry zones [logo, wheel]; frame colors map 1:1.
        // Logo = colors[0], wheel = colors[1]; clusters 0 and 1.
        let logo = static_effect_payload(0, 1, Rgb { r: 255, g: 0, b: 0 });
        let wheel = static_effect_payload(1, 1, Rgb { r: 0, g: 255, b: 0 });
        assert_eq!(logo[0], 0);
        assert_eq!(&logo[2..5], &[255, 0, 0]);
        assert_eq!(wheel[0], 1);
        assert_eq!(&wheel[2..5], &[0, 255, 0]);
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;
    use crate::discover;

    /// Hardware round-trip on the attached G502 HERO:
    ///   cargo test -p chromargb-drivers logitech -- --ignored --nocapture
    /// On HID permission failure this prints a skip reason, never fails.
    #[test]
    #[ignore = "needs the G502 HERO attached"]
    fn g502_static_red() {
        let api = match hidapi::HidApi::new() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("SKIP: hidapi init failed: {e}");
                return;
            }
        };
        let devices = discover(&api);
        let Some(dev) = devices.into_iter().find(|d| d.info.kind == crate::DriverKind::Logitech) else {
            eprintln!("SKIP: no Logitech device discovered (G502 HERO not attached?)");
            return;
        };
        println!("found: {} ({})", dev.info.name, dev.info.id);

        let mut backend = match LogitechBackend::open(&dev) {
            Ok(b) => b,
            Err(Error::Hid(e)) => {
                eprintln!("SKIP: could not open HID handle ({e}).");
                eprintln!("     Grant the terminal Input Monitoring permission");
                eprintln!("     (System Settings > Privacy & Security > Input Monitoring)");
                eprintln!("     or run the test under sudo, then re-run.");
                return;
            }
            Err(e) => {
                eprintln!("SKIP: open failed: {e}");
                return;
            }
        };

        match backend.set_static(Rgb { r: 255, g: 0, b: 0 }) {
            Ok(()) => println!("OK: set_static(red) sent — logo and wheel should be solid red"),
            Err(Error::Hid(e)) if e.contains("no reply") || e.contains("error") => {
                eprintln!("DIAGNOSTIC: device answered abnormally: {e}");
                eprintln!("            (device attached but not speaking HID++ 2.0 on this interface?)");
            }
            Err(e) => {
                eprintln!("DIAGNOSTIC: set_static failed: {e}");
            }
        }
    }
}
