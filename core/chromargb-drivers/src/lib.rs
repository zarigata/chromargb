//! chromargb-drivers — USB HID discovery and the `DriverBackend` contract.
//!
//! Phase 2 scope: discovery + a hand-written (vid,pid) registry. Protocol
//! implementations (Razer, Logitech, OpenRGB bridge) land in later phases and
//! plug in via [`open_backend`].

use hidapi::HidApi;
use serde::Serialize;

pub use hidapi;

pub mod logitech;
pub mod openrgb_bridge;
pub mod razer;

pub const RAZER_VID: u16 = 0x1532;
pub const LOGITECH_VID: u16 = 0x046d;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("hidapi: {0}")]
    Hid(String),
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Error::Hid(e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A lighting zone: name, LED count, and optional key-matrix geometry.
#[derive(Debug, Clone, Serialize)]
pub struct Zone {
    pub name: String,
    pub leds: usize,
    /// (rows, cols) for per-key devices; None for plain zones.
    pub matrix: Option<(u8, u8)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DriverKind {
    Razer,
    Logitech,
    Bridge,
    Generic,
}

/// Static description of a device, as reported over IPC to the daemon/UI.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub vid: u16,
    pub pid: u16,
    pub kind: DriverKind,
    pub zones: Vec<Zone>,
    /// Set for recognized-vendor devices with no registry entry yet.
    pub note: Option<String>,
}

/// The one driver contract. Razer/Logitech/OpenRGB-bridge workers implement
/// this; the daemon and effects engine only talk to this trait.
pub trait DriverBackend: Send {
    /// id, name, vendor, vid, pid, zones.
    fn info(&self) -> DeviceInfo;
    /// Whole-device solid color.
    fn set_static(&mut self, color: Rgb) -> Result<()>;
    /// Per-LED frame; `colors.len()` must equal total LEDs across all zones.
    /// Used by the effects engine.
    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()>;
    /// Whether `write_frame` does real per-LED control (vs a zone fallback).
    fn supports_per_led(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

type ZoneSpec = (&'static str, usize, Option<(u8, u8)>);

struct KnownModel {
    vendor: &'static str,
    name: &'static str,
    kind: DriverKind,
    zones: &'static [ZoneSpec],
}

/// Registry extracted from OpenRGB sources (GPL-2.0-or-later):
/// Razer: Controllers/RazerController/RazerDevices.{h,cpp} (201 devices)
/// Logitech: Controllers/LogitechController/LogitechControllerDetect.cpp (34 devices)
/// Zone layouts: keyboards = matrix rows x cols per source; mice =
/// logo+wheel zones (OpenRGB G502-style LED enumeration); headsets/other = single zone.
/// Names/geometry are recognition data only — drivers still gate control.
const MODELS: &[(u16, u16, &KnownModel)] = &[
    (
        0x1532,
        0x0024,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba 2012 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0025,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba 2012 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x003e,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Epic Chroma",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0043,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder Chroma",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0044,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba 2015 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0045,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0046,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba Tournament Edition",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x004c,
        &KnownModel {
            vendor: "Razer",
            name: "Diamondback Chroma",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0050,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Hex V2",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0053,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Chroma",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0059,
        &KnownModel {
            vendor: "Razer",
            name: "Lancehead 2017 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x005a,
        &KnownModel {
            vendor: "Razer",
            name: "Lancehead 2017 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x005c,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder Elite",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0060,
        &KnownModel {
            vendor: "Razer",
            name: "Lancehead Tournament Edition",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0064,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0065,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk Essential",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0067,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Trinity",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0068,
        &KnownModel {
            vendor: "Razer",
            name: "Firefly Hyperflux",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0069,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba Hyperflux (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x006a,
        &KnownModel {
            vendor: "Razer",
            name: "Abyssus Elite DVa Edition",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x006b,
        &KnownModel {
            vendor: "Razer",
            name: "Abyssus Essential",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x006c,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba Elite",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x006e,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder Essential",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x006f,
        &KnownModel {
            vendor: "Razer",
            name: "Lancehead 2019 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0070,
        &KnownModel {
            vendor: "Razer",
            name: "Lancehead 2019 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0071,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder Essential (White Edition)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0072,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba 2018 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0073,
        &KnownModel {
            vendor: "Razer",
            name: "Mamba 2018 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0078,
        &KnownModel {
            vendor: "Razer",
            name: "Viper",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x007a,
        &KnownModel {
            vendor: "Razer",
            name: "Viper Ultimate (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x007b,
        &KnownModel {
            vendor: "Razer",
            name: "Viper Ultimate (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x007c,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder V2 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x007d,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder V2 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x007e,
        &KnownModel {
            vendor: "Razer",
            name: "Mouse Dock Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0084,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder V2",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0085,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V2",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0086,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk Ultimate",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0088,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk Ultimate (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x008a,
        &KnownModel {
            vendor: "Razer",
            name: "Viper Mini",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x008c,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder V2 Mini",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x008d,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Left Handed",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x008f,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Pro (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0090,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Pro (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0091,
        &KnownModel {
            vendor: "Razer",
            name: "Viper 8kHz",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0093,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Classic",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0098,
        &KnownModel {
            vendor: "Razer",
            name: "DeathAdder Essential V2",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0099,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00a3,
        &KnownModel {
            vendor: "Razer",
            name: "Cobra",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00a4,
        &KnownModel {
            vendor: "Razer",
            name: "Mouse Dock Pro",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x00a7,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Pro V2 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00a8,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Pro V2 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00aa,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00ab,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00ac,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro (Bluetooth)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00af,
        &KnownModel {
            vendor: "Razer",
            name: "Cobra Pro (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00b0,
        &KnownModel {
            vendor: "Razer",
            name: "Cobra Pro (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00b9,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 X HyperSpeed",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00cb,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 35K",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00cc,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro 35K (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00cd,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro 35K (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00d6,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro 35K Phantom Green Edition (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00d7,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro 35K Phantom Green Edition (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00d8,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V3 Pro 35K Phantom Green Edition (Bluetooth)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00e2,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V4 Pro (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00e3,
        &KnownModel {
            vendor: "Razer",
            name: "Basilisk V4 Pro (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00e7,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Pro V3 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x00e8,
        &KnownModel {
            vendor: "Razer",
            name: "Naga Pro V3 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0203,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow Chroma",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0204,
        &KnownModel {
            vendor: "Razer",
            name: "DeathStalker Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0205,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (2016)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0207,
        &KnownModel {
            vendor: "Razer",
            name: "Orbweaver Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0208,
        &KnownModel {
            vendor: "Razer",
            name: "Tartarus Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0209,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow Chroma Tournament Edition",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x020f,
        &KnownModel {
            vendor: "Razer",
            name: "Blade (2016)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0210,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Pro (2016)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0211,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow Chroma Overwatch",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0215,
        &KnownModel {
            vendor: "Razer",
            name: "Core",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0216,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow X Chroma",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x021a,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow X Chroma Tournament Edition",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x021e,
        &KnownModel {
            vendor: "Razer",
            name: "Ornata Chroma",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0220,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (Late 2016)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0221,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow Chroma V2",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0224,
        &KnownModel {
            vendor: "Razer",
            name: "Blade (Late 2016)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0225,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Pro (2017)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0226,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman Elite",
            kind: DriverKind::Razer,
            zones: &[("matrix", 198, Some((9, 22)))],
        },
    ),
    (
        0x1532,
        0x0227,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0228,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow Elite",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x022a,
        &KnownModel {
            vendor: "Razer",
            name: "Cynosa Chroma",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x022b,
        &KnownModel {
            vendor: "Razer",
            name: "Tartarus V2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x022d,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (2017)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x022f,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Pro (2017 FullHD)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0232,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (Late 2017)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0233,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2018 Advanced)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0234,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Pro (2019)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0239,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (2019)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x023a,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2019 Advanced)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x023b,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2018 Base)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x023f,
        &KnownModel {
            vendor: "Razer",
            name: "Cynosa Lite",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0240,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2018 Mercury)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0241,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow 2019",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0243,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman Tournament Edition",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x0244,
        &KnownModel {
            vendor: "Razer",
            name: "Tartarus Pro",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0245,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2019 Mercury)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0246,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2019 Base)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x024a,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (Late 2019)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x024c,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Pro (Late 2019)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x024d,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2019 Studio)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x024e,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V3",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0252,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (2020)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0253,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2020 Advanced)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0255,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2020 Base)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0256,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Pro 17 (2020)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0257,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman Mini",
            kind: DriverKind::Razer,
            zones: &[("matrix", 75, Some((5, 15)))],
        },
    ),
    (
        0x1532,
        0x0258,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow V3 Mini (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 80, Some((5, 16)))],
        },
    ),
    (
        0x1532,
        0x0259,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Stealth (Late 2020)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x025a,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow V3 Pro (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x025b,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow V3 Pro (Bluetooth)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x025c,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow V3 Pro (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x025d,
        &KnownModel {
            vendor: "Razer",
            name: "Ornata Chroma V2",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x025e,
        &KnownModel {
            vendor: "Razer",
            name: "Cynosa v2",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0266,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman V2 Analog",
            kind: DriverKind::Razer,
            zones: &[("matrix", 198, Some((9, 22)))],
        },
    ),
    (
        0x1532,
        0x0268,
        &KnownModel {
            vendor: "Razer",
            name: "Blade (Late 2020)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x026a,
        &KnownModel {
            vendor: "Razer",
            name: "Book 13 (2020)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x026b,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman V2 TKL",
            kind: DriverKind::Razer,
            zones: &[("matrix", 102, Some((6, 17)))],
        },
    ),
    (
        0x1532,
        0x026c,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman V2",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x026d,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2021 Advanced)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x026f,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2021 Base)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0270,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 14 (2021)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0271,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow V3 Mini (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 80, Some((5, 16)))],
        },
    ),
    (
        0x1532,
        0x0276,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (Late 2021 Advanced)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0279,
        &KnownModel {
            vendor: "Razer",
            name: "Blade Pro 17 (2021)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x027a,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2021 Base)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0282,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman Mini Analog",
            kind: DriverKind::Razer,
            zones: &[("matrix", 75, Some((5, 15)))],
        },
    ),
    (
        0x1532,
        0x0287,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4",
            kind: DriverKind::Razer,
            zones: &[("matrix", 184, Some((8, 23)))],
        },
    ),
    (
        0x1532,
        0x028a,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 15 (2022)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x028c,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 14 (2022)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x028d,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 Pro",
            kind: DriverKind::Razer,
            zones: &[("matrix", 184, Some((8, 23)))],
        },
    ),
    (
        0x1532,
        0x028f,
        &KnownModel {
            vendor: "Razer",
            name: "Ornata V3",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0290,
        &KnownModel {
            vendor: "Razer",
            name: "DeathStalker V2 Pro (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0292,
        &KnownModel {
            vendor: "Razer",
            name: "DeathStalker V2 Pro (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0293,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 X",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0294,
        &KnownModel {
            vendor: "Razer",
            name: "Ornata V3 X",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0295,
        &KnownModel {
            vendor: "Razer",
            name: "Deathstalker V2",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0296,
        &KnownModel {
            vendor: "Razer",
            name: "DeathStalker V2 Pro TKL (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 102, Some((6, 17)))],
        },
    ),
    (
        0x1532,
        0x0298,
        &KnownModel {
            vendor: "Razer",
            name: "DeathStalker V2 Pro TKL (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 102, Some((6, 17)))],
        },
    ),
    (
        0x1532,
        0x029d,
        &KnownModel {
            vendor: "Razer",
            name: "Blade 14 (2023)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x02a1,
        &KnownModel {
            vendor: "Razer",
            name: "Ornata V3 rev2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x02a2,
        &KnownModel {
            vendor: "Razer",
            name: "Ornata V3 X Rev2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x02a3,
        &KnownModel {
            vendor: "Razer",
            name: "Ornata V3 TKL",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x02a5,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 75% (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x02a6,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman V3 Pro",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x02a7,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman V3 Pro TKL White",
            kind: DriverKind::Razer,
            zones: &[("matrix", 114, Some((6, 19)))],
        },
    ),
    (
        0x1532,
        0x02b3,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 Pro 75% (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x02b4,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 Pro 75% (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x02d2,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 Low Profile TKL (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x02d4,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 Low Profile TKL (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x02d5,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 TKL (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x02d7,
        &KnownModel {
            vendor: "Razer",
            name: "Blackwidow V4 TKL (Wired)",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x02e4,
        &KnownModel {
            vendor: "Razer",
            name: "Huntsman V3 HE Magnetic Mini",
            kind: DriverKind::Razer,
            zones: &[("matrix", 132, Some((6, 22)))],
        },
    ),
    (
        0x1532,
        0x0504,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken 7.1 Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0510,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken 7.1 V2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0517,
        &KnownModel {
            vendor: "Razer",
            name: "Nommo Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0518,
        &KnownModel {
            vendor: "Razer",
            name: "Nommo Pro",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0527,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken Ultimate",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0532,
        &KnownModel {
            vendor: "Razer",
            name: "Leviathan V2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0533,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken V3 HyperSense",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0537,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken V3 X",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0549,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken V3",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x054a,
        &KnownModel {
            vendor: "Razer",
            name: "Leviathan V2 X",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0554,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken Kitty V2 Pro",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0560,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken Kitty Black Edition V2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x056b,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken V4 (Wired)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x056c,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken V4 (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0587,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken Kitty V3 Pro (Wired)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0588,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken Kitty V3 Pro (Wireless)",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0a24,
        &KnownModel {
            vendor: "Razer",
            name: "BlackWidow V3 TKL",
            kind: DriverKind::Razer,
            zones: &[("matrix", 108, Some((6, 18)))],
        },
    ),
    (
        0x1532,
        0x0c00,
        &KnownModel {
            vendor: "Razer",
            name: "Firefly",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0c01,
        &KnownModel {
            vendor: "Razer",
            name: "Goliathus",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0c02,
        &KnownModel {
            vendor: "Razer",
            name: "Goliathus Extended",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0c04,
        &KnownModel {
            vendor: "Razer",
            name: "Firefly V2",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0c05,
        &KnownModel {
            vendor: "Razer",
            name: "Strider Chroma",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0c06,
        &KnownModel {
            vendor: "Razer",
            name: "Goliathus Chroma 3XL",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0c08,
        &KnownModel {
            vendor: "Razer",
            name: "Firefly V2 Pro",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f03,
        &KnownModel {
            vendor: "Razer",
            name: "Tiamat 7.1 V2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f07,
        &KnownModel {
            vendor: "Razer",
            name: "Chroma Mug Holder",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f08,
        &KnownModel {
            vendor: "Razer",
            name: "Base Station Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f09,
        &KnownModel {
            vendor: "Razer",
            name: "Chroma HDK",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f0d,
        &KnownModel {
            vendor: "Razer",
            name: "Laptop Stand Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f0e,
        &KnownModel {
            vendor: "Razer",
            name: "Chroma PC Case Lighting Kit",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f13,
        &KnownModel {
            vendor: "Razer",
            name: "Lian Li O11 Dynamic - Razer Edition",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f19,
        &KnownModel {
            vendor: "Razer",
            name: "Kraken Kitty Edition",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f1a,
        &KnownModel {
            vendor: "Razer",
            name: "Core X",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f1b,
        &KnownModel {
            vendor: "Razer",
            name: "Seiren Emote",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f1d,
        &KnownModel {
            vendor: "Razer",
            name: "Mouse Bungee V3 Chroma",
            kind: DriverKind::Razer,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f1f,
        &KnownModel {
            vendor: "Razer",
            name: "Chroma Addressable RGB Controller",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f20,
        &KnownModel {
            vendor: "Razer",
            name: "Base Station V2 Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f21,
        &KnownModel {
            vendor: "Razer",
            name: "Thunderbolt 4 Dock Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f26,
        &KnownModel {
            vendor: "Razer",
            name: "Charging Pad Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f2b,
        &KnownModel {
            vendor: "Razer",
            name: "Laptop Stand Chroma V2",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f35,
        &KnownModel {
            vendor: "Razer",
            name: "Hanbo Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x1532,
        0x0f52,
        &KnownModel {
            vendor: "Razer",
            name: "Thunderbolt 5 Dock Chroma",
            kind: DriverKind::Razer,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x0a5b,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G933 Lightsync Headset",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x0a5c,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G633 Gaming Headset",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x0a87,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G935 Gaming Headset",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x0a89,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G635 Gaming Headset",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x0ab5,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G733 Gaming Headset",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x0afe,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G733 Gaming Headset",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x0b1f,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G733 Gaming Headset",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x4053,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G900 Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0x405d,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G403 Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0x405f,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech Powerplay Mat",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0x4067,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G903 Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0x4070,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G703 Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0x4079,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G Pro Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0x407f,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G502 Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0x4086,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G703 HERO Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0x4087,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G903 HERO Wireless Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc081,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G900 Wireless Gaming Mouse (wired)",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc082,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G403 Wireless Gaming Mouse (wired)",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc083,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G403 Prodigy Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc086,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G903 Wireless Gaming Mouse (wired)",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc087,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G703 Wireless Gaming Mouse (wired)",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc088,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G Pro Wireless Gaming Mouse (wired)",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc08b,
        &KnownModel {
            vendor: "Logitech",
            name: "G502 HERO",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc091,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G903 HERO Wireless Gaming Mouse (wired)",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc092,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G203 Lightsync",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0xc332,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G502 Proteus Spectrum Gaming Mouse",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc336,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G213",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0xc33e,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G915 Wireless RGB Mechanical Gaming Keyboard (Wired)",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0xc343,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G915TKL Wireless RGB Mechanical Gaming Keyboard (Wired)",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0xc539,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech Lightspeed Receiver",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0xc53a,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G Powerplay Mousepad",
            kind: DriverKind::Logitech,
            zones: &[("logo", 1, None), ("wheel", 1, None)],
        },
    ),
    (
        0x046d,
        0xc541,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G915 Wireless RGB Mechanical Gaming Keyboard",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0xc545,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G915TKL Wireless RGB Mechanical Gaming Keyboard",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
    (
        0x046d,
        0xc547,
        &KnownModel {
            vendor: "Logitech",
            name: "Logitech G915 Wireless RGB Mechanical Gaming Keyboard (Receiver 2)",
            kind: DriverKind::Logitech,
            zones: &[("zone0", 1, None)],
        },
    ),
];
/// Resolved model data for a (vid,pid): exact registry hit, a generic entry
/// for known-but-unmapped Razer/Logitech VIDs, or `None` (not ours; skipped).
pub struct Model {
    pub vendor: String,
    pub name: String,
    pub kind: DriverKind,
    pub zones: Vec<Zone>,
    pub note: Option<String>,
}

pub fn lookup(vid: u16, pid: u16) -> Option<Model> {
    if let Some((_, _, m)) = MODELS.iter().find(|(v, p, _)| *v == vid && *p == pid) {
        return Some(Model {
            vendor: m.vendor.into(),
            name: m.name.into(),
            kind: m.kind,
            zones: m
                .zones
                .iter()
                .map(|(n, l, mx)| Zone {
                    name: (*n).into(),
                    leds: *l,
                    matrix: *mx,
                })
                .collect(),
            note: None,
        });
    }
    // Unrecognized PIDs on known vendor buses: list as generic, with a note.
    let vendor = match vid {
        RAZER_VID => "Razer",
        LOGITECH_VID => "Logitech",
        _ => return None,
    };
    Some(Model {
        vendor: vendor.into(),
        name: format!("Unrecognized {vendor} device"),
        kind: DriverKind::Generic,
        zones: Vec::new(),
        note: Some(format!(
            "no registry entry for {vendor} PID {pid:#06x}; needs a driver before it can be controlled"
        )),
    })
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

pub struct DiscoveredDevice {
    pub info: DeviceInfo,
    pub serial: Option<String>,
    /// hidapi device path; pass to `HidApi::open_path` to get a handle.
    pub path: std::ffi::CString,
}

/// Enumerate HID devices and keep the recognized ones.
///
/// A physical device exposes several HID interfaces (keyboard, consumer
/// control, vendor...); identical (vid,pid,serial) tuples are deduplicated to
/// one entry and the first interface's path is kept.
///
/// ponytail: the first interface is kept unfiltered — Razer control lives on a
/// specific usage page; the driver phases must select the interface by usage
/// page instead of just using `path`.
pub fn discover(api: &HidApi) -> Vec<DiscoveredDevice> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dev in api.device_list() {
        let (vid, pid) = (dev.vendor_id(), dev.product_id());
        let Some(model) = lookup(vid, pid) else { continue };
        let serial = dev
            .serial_number()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // ponytail: two identical serial-less devices would collide on this id;
        // if that ever bites, suffix the hidapi path.
        let id = match &serial {
            Some(s) => format!("{vid:04x}:{pid:04x}:{s}"),
            None => format!("{vid:04x}:{pid:04x}"),
        };
        if !seen.insert(id.clone()) {
            continue; // same physical device on another HID interface
        }
        out.push(DiscoveredDevice {
            info: DeviceInfo {
                id,
                name: model.name,
                vendor: model.vendor,
                vid,
                pid,
                kind: model.kind,
                zones: model.zones,
                note: model.note,
            },
            serial,
            path: dev.path().to_owned(),
        });
    }
    out
}

/// One line per device, aligned columns. Shared by the CLI and the daemon.
pub fn devices_table(devices: &[DiscoveredDevice]) -> String {
    let mut out = format!(
        "{:<30} {:<9} {:<24} {:<10} {:<8} {:<14} {}\n",
        "ID", "VENDOR", "NAME", "VID:PID", "KIND", "ZONES", "NOTE"
    );
    for d in devices {
        let i = &d.info;
        let leds: usize = i.zones.iter().map(|z| z.leds).sum();
        let zones = if i.zones.is_empty() {
            "-".to_string()
        } else {
            format!("{} ({} leds)", i.zones.len(), leds)
        };
        out += &format!(
            "{:<30} {:<9} {:<24} {:<10} {:<8} {:<14} {}\n",
            i.id,
            i.vendor,
            i.name,
            format!("{:04x}:{:04x}", i.vid, i.pid),
            format!("{:?}", i.kind),
            zones,
            i.note.as_deref().unwrap_or(""),
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Backends
// ---------------------------------------------------------------------------

/// Protocol-less placeholder: holds device info, refuses lighting writes.
/// Used until the Razer/Logitech driver phases replace it in `open_backend`.
pub struct GenericBackend {
    info: DeviceInfo,
}

impl GenericBackend {
    pub fn new(info: DeviceInfo) -> Self {
        Self { info }
    }
}

impl DriverBackend for GenericBackend {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn set_static(&mut self, _color: Rgb) -> Result<()> {
        Err(Error::Unsupported(format!(
            "no native driver implemented for {} yet",
            self.info.name
        )))
    }

    fn write_frame(&mut self, _colors: &[Rgb]) -> Result<()> {
        Err(Error::Unsupported(format!(
            "no native driver implemented for {} yet",
            self.info.name
        )))
    }
}

/// Open a backend for a discovered device.
///
/// Driver workers: match on `dev.info.kind` here to hand out your backend
/// (opening the HID handle via `HidApi::open_path(&dev.path)`). Everything
/// gets the generic protocol-less backend until those phases land.
pub fn open_backend(dev: &DiscoveredDevice) -> Result<Box<dyn DriverBackend>> {
    match dev.info.kind {
        DriverKind::Razer => Ok(Box::new(razer::RazerBackend::open(dev)?)),
        DriverKind::Logitech => Ok(Box::new(logitech::LogitechBackend::open(dev)?)),
        DriverKind::Bridge => Ok(Box::new(openrgb_bridge::BridgeBackend::open(dev)?)),
        DriverKind::Generic => Ok(Box::new(GenericBackend::new(dev.info.clone()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exact_hits() {
        let m = lookup(0x1532, 0x0228).unwrap();
        assert_eq!(m.vendor, "Razer");
        assert_eq!(m.name, "BlackWidow Elite");
        assert_eq!(m.kind, DriverKind::Razer);
        assert_eq!(m.zones.iter().map(|z| z.leds).sum::<usize>(), 132);
        assert_eq!(m.zones[0].matrix, Some((6, 22)));
        assert!(m.note.is_none());

        let m = lookup(0x046d, 0xc08b).unwrap();
        assert_eq!(m.vendor, "Logitech");
        assert_eq!(m.name, "G502 HERO");
        assert_eq!(m.kind, DriverKind::Logitech);
        assert_eq!(m.zones.len(), 2);
        assert_eq!(m.zones.iter().map(|z| z.leds).sum::<usize>(), 2);
    }

    #[test]
    fn registry_integrity_all_entries() {
        // Extracted from OpenRGB: every entry well-formed and unique.
        let mut seen = std::collections::HashSet::new();
        assert!(MODELS.len() > 200, "expected the full extracted registry, got {}", MODELS.len());
        for (vid, pid, m) in MODELS {
            assert!(*vid == RAZER_VID || *vid == LOGITECH_VID);
            assert!(seen.insert((*vid, *pid)), "duplicate (vid,pid) {:04x}:{:04x}", vid, pid);
            assert!(!m.name.is_empty());
            for (zname, leds, mx) in m.zones {
                assert!(!zname.is_empty());
                assert!(*leds > 0);
                if let Some((r, c)) = mx {
                    assert_eq!((*r as usize) * (*c as usize), *leds, "matrix dims != leds for {}", m.name);
                }
            }
        }
    }

    #[test]
    fn registry_spot_checks_against_openrgb_sources() {
        // Razer Huntsman Elite (0x0226) 9x22 per OpenRGB (underglow adds rows);
        // BlackWidow V3 Mini (Wired) is a 6x17 per-key keyboard; G915 wired is
        // recognized by name (zone geometry comes from the HID++ driver at
        // runtime, so registry zones stay minimal — listed-only).
        let m = lookup(0x1532, 0x0226).unwrap();
        assert!(m.name.contains("Huntsman Elite"), "{}", m.name);
        assert_eq!(m.zones[0].matrix, Some((9, 22)));
        let m = lookup(0x1532, 0x0257).unwrap();
        assert!(m.name.contains("Huntsman Mini"), "{}", m.name);
        assert_eq!(m.zones[0].matrix, Some((5, 15)));
        let m = lookup(0x046d, 0xc33e).unwrap();
        assert!(m.name.contains("G915"), "{}", m.name);
    }

    #[test]
    fn unknown_models_of_known_vendors_list_as_generic() {
        for vid in [RAZER_VID, LOGITECH_VID] {
            let m = lookup(vid, 0x9999).unwrap();
            assert_eq!(m.kind, DriverKind::Generic);
            assert!(m.name.contains("Unrecognized"));
            assert!(m.note.as_deref().unwrap().contains("no registry entry"));
            assert!(m.zones.is_empty());
        }
    }

    #[test]
    fn unknown_vendors_are_skipped() {
        assert!(lookup(0x1234, 0x5678).is_none());
    }

    #[test]
    fn generic_backend_constructs_but_refuses_writes() {
        // Unknown PID on a known vendor bus -> still Generic now that the
        // Razer/Logitech/Bridge backends are real.
        let m = lookup(0x1532, 0x9999).unwrap();
        assert_eq!(m.kind, DriverKind::Generic);
        let dev = DiscoveredDevice {
            info: DeviceInfo {
                id: "1532:9999:t".into(),
                name: m.name,
                vendor: m.vendor,
                vid: 0x1532,
                pid: 0x9999,
                kind: m.kind,
                zones: m.zones,
                note: m.note,
            },
            serial: None,
            path: std::ffi::CString::new("fake").unwrap(),
        };
        let mut b = open_backend(&dev).unwrap();
        assert_eq!(b.info().id, "1532:9999:t");
        assert!(!b.supports_per_led());
        let err = b.set_static(Rgb { r: 255, g: 0, b: 0 }).unwrap_err();
        assert!(err.to_string().contains("no native driver"));
        assert!(b.write_frame(&[]).is_err());
    }

    #[test]
    fn devices_table_lists_names() {
        let m = lookup(0x046d, 0xc08b).unwrap();
        let devices = vec![DiscoveredDevice {
            info: DeviceInfo {
                id: "046d:c08b".into(),
                name: m.name,
                vendor: m.vendor,
                vid: 0x046d,
                pid: 0xc08b,
                kind: m.kind,
                zones: m.zones,
                note: m.note,
            },
            serial: None,
            path: std::ffi::CString::new("fake").unwrap(),
        }];
        let table = devices_table(&devices);
        assert!(table.contains("G502 HERO"));
        assert!(table.contains("046d:c08b"));
        assert!(table.contains("2 (2 leds)"));
    }

    #[test]
    #[ignore = "needs real RGB hardware attached"]
    fn hardware_list() {
        let api = HidApi::new().unwrap();
        let devices = discover(&api);
        assert!(!devices.is_empty(), "expected at least one RGB device");
        for d in &devices {
            assert!(!d.info.name.is_empty());
            assert!(d.info.kind != DriverKind::Generic || d.info.note.is_some());
            println!("{} {} {} zones={}", d.info.id, d.info.vendor, d.info.name, d.info.zones.len());
        }
    }
}
