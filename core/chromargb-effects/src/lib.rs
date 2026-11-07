//! chromargb-effects: effect library + one background scheduler thread that
//! renders layered, interactive, audio-reactive frames to any number of
//! devices.
//!
//! Design (daemon integration points):
//! - [`Layer`] — one entry of a device's layer stack: an [`Effect`] plus
//!   opacity/blend/enabled, parsed from the IPC JSON (`set_layers`) via
//!   [`Layer::from_json`].
//! - [`Effect`] — effect params, parsed via [`Effect::from_json`].
//! - [`Engine`] — owns `Box<dyn DriverBackend>` devices keyed by device id.
//!   `Engine::add_device` / `start` / `stop` / `stop_all` remain the whole
//!   API the daemon already calls (`start` = one-opaque-layer stack, so old
//!   callers keep working unchanged); new: `set_layers`, `trigger`,
//!   `set_audio_level`. One scheduler thread renders+composites every
//!   enabled layer per frame; `Drop` stops the thread, so an engine can
//!   never be dropped while effects still run (a daemon wiring cannot leak
//!   the thread).
//! - Effects are pure functions `(t_ms, effect, geometry, out) -> &mut [Rgb]`
//!   (see [`render`]): no allocation, no hardware — unit-testable directly.
//!   Interactive effects (ripple/reactive) read a slice of pending
//!   [`Event`]s; the engine owns the timing, tests fabricate events.
//!
//! Frame loop discipline: each device's frame + scratch `Vec<Rgb>` and event
//! queue are pre-allocated at `add_device` time and only written in place
//! afterwards — zero allocations per frame once the engine is running.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chromargb_drivers::{DriverBackend, Rgb};

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
}

/// Typed effect + params, parsed from an IPC `set_effect` message:
/// `{"effect": "wave", "params": {"speed": 50, "direction": "left"}}`.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Static { color: [u8; 3] },
    Wave { speed: u8, direction: Direction },
    Spectrum { speed: u8 },
    Breathe { speed: u8, color: Option<[u8; 3]> },
    Gradient { colors: Vec<[u8; 3]> },
    Ripple { color: [u8; 3], speed: u8 },
    Reactive { color: [u8; 3], fade: u8 },
    Starlight { colors: Vec<[u8; 3]>, speed: u8 },
    Fire { speed: u8 },
    AudioMeter { color: Option<[u8; 3]>, bands: u8 },
}

impl Effect {
    /// Parse `effect` (name) + `params` (object) from an IPC message.
    /// Unknown names, missing/invalid params, or out-of-range values produce
    /// a clear error naming the effect and field.
    pub fn from_json(effect: &str, params: &serde_json::Value) -> Result<Effect, String> {
        use serde_json::Value;
        let obj = params.as_object();
        let missing = |f: &str| format!("effect '{effect}': missing param '{f}'");
        let bad = |f: &str, why: &str| format!("effect '{effect}': param '{f}' {why}");
        let speed = |o: &serde_json::Map<String, Value>| -> Result<u8, String> {
            match o.get("speed") {
                None => Err(missing("speed")),
                Some(Value::Number(n)) => n
                    .as_u64()
                    .filter(|v| *v <= 100)
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or_else(|| bad("speed", "must be an integer 0-100")),
                Some(_) => Err(bad("speed", "must be an integer 0-100")),
            }
        };
        let color = |v: &Value| -> Option<[u8; 3]> {
            let a = v.as_array()?;
            if a.len() != 3 {
                return None;
            }
            Some([
                u8::try_from(a[0].as_u64()?).ok()?,
                u8::try_from(a[1].as_u64()?).ok()?,
                u8::try_from(a[2].as_u64()?).ok()?,
            ])
        };
        let fade = |o: &serde_json::Map<String, Value>| -> Result<u8, String> {
            match o.get("fade") {
                None => Err(missing("fade")),
                Some(Value::Number(n)) => n
                    .as_u64()
                    .filter(|v| *v <= 100)
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or_else(|| bad("fade", "must be an integer 0-100")),
                Some(_) => Err(bad("fade", "must be an integer 0-100")),
            }
        };
        let bands = |o: &serde_json::Map<String, Value>| -> Result<u8, String> {
            match o.get("bands") {
                None => Err(missing("bands")),
                Some(Value::Number(n)) => n
                    .as_u64()
                    .filter(|v| (1..=100).contains(v))
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or_else(|| bad("bands", "must be an integer 1-100")),
                Some(_) => Err(bad("bands", "must be an integer 1-100")),
            }
        };
        match effect {
            "static" => Ok(Effect::Static {
                color: obj
                    .and_then(|o| o.get("color"))
                    .and_then(color)
                    .ok_or_else(|| bad("color", "missing or not [r,g,b] (0-255 each)"))?,
            }),
            "spectrum" => Ok(Effect::Spectrum {
                speed: speed(obj.ok_or_else(|| missing("speed"))?)?,
            }),
            "breathe" => {
                let o = obj.ok_or_else(|| missing("speed"))?;
                let c = match o.get("color") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        Some(color(v).ok_or_else(|| bad("color", "not [r,g,b] (0-255 each)"))?)
                    }
                };
                Ok(Effect::Breathe { speed: speed(o)?, color: c })
            }
            "wave" => {
                let o = obj.ok_or_else(|| missing("speed"))?;
                let direction = match o.get("direction").and_then(Value::as_str) {
                    Some("left") => Direction::Left,
                    Some("right") => Direction::Right,
                    _ => return Err(bad("direction", "must be \"left\" or \"right\"")),
                };
                Ok(Effect::Wave { speed: speed(o)?, direction })
            }
            "gradient" => {
                let Some(stops) = obj
                    .and_then(|o| o.get("colors"))
                    .and_then(Value::as_array)
                    .filter(|a| a.len() >= 2)
                else {
                    return Err(bad("colors", "missing or fewer than 2 [r,g,b] stops"));
                };
                let parsed = stops
                    .iter()
                    .map(|v| {
                        color(v).ok_or_else(|| bad("colors", "every stop must be [r,g,b] (0-255 each)"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Effect::Gradient { colors: parsed })
            }
            "ripple" => {
                let o = obj.ok_or_else(|| missing("color"))?;
                Ok(Effect::Ripple {
                    color: o
                        .get("color")
                        .and_then(color)
                        .ok_or_else(|| bad("color", "missing or not [r,g,b] (0-255 each)"))?,
                    speed: speed(o)?,
                })
            }
            "reactive" => {
                let o = obj.ok_or_else(|| missing("color"))?;
                Ok(Effect::Reactive {
                    color: o
                        .get("color")
                        .and_then(color)
                        .ok_or_else(|| bad("color", "missing or not [r,g,b] (0-255 each)"))?,
                    fade: fade(o)?,
                })
            }
            "starlight" => {
                let o = obj.ok_or_else(|| missing("speed"))?;
                let Some(stops) = o
                    .get("colors")
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty())
                else {
                    return Err(bad("colors", "missing or empty; need >= 1 [r,g,b] color"));
                };
                let parsed = stops
                    .iter()
                    .map(|v| {
                        color(v).ok_or_else(|| bad("colors", "every color must be [r,g,b] (0-255 each)"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Effect::Starlight { colors: parsed, speed: speed(o)? })
            }
            "fire" => Ok(Effect::Fire {
                speed: speed(obj.ok_or_else(|| missing("speed"))?)?,
            }),
            "audio_meter" => {
                let o = obj.ok_or_else(|| missing("bands"))?;
                let c = match o.get("color") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        Some(color(v).ok_or_else(|| bad("color", "not [r,g,b] (0-255 each)"))?)
                    }
                };
                Ok(Effect::AudioMeter { color: c, bands: bands(o)? })
            }
            other => Err(format!(
                "unknown effect '{other}' (expected static, wave, spectrum, breathe, gradient, ripple, reactive, starlight, fire or audio_meter)"
            )),
        }
    }
}

/// How a layer blends onto the stack below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    /// lerp over the stack by opacity (fully replaces at opacity 1)
    Normal,
    /// saturating add, scaled by opacity
    Add,
}

/// One layer of a device's effect stack.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub effect: Effect,
    /// 0..=1
    pub opacity: f32,
    pub blend: Blend,
    pub enabled: bool,
}

impl Layer {
    /// Parse one layer entry of a `set_layers` message:
    /// `{"effect": "ripple", "params": {...}, "opacity": 100, "blend": "add",
    ///   "enabled": false}`. `opacity` is an int 0-100 (stored /100),
    /// `blend` "normal" (default) or "add", `enabled` defaults true. Effect
    /// errors pass through from [`Effect::from_json`] unchanged.
    pub fn from_json(
        effect_name: &str,
        params: &serde_json::Value,
        layer_obj: &serde_json::Value,
    ) -> Result<Layer, String> {
        use serde_json::Value;
        let opacity = match layer_obj.get("opacity") {
            None => 1.0,
            Some(Value::Number(n)) => n
                .as_u64()
                .filter(|v| *v <= 100)
                .map(|v| v as f32 / 100.0)
                .ok_or_else(|| "layer: param 'opacity' must be an integer 0-100".to_string())?,
            Some(_) => {
                return Err("layer: param 'opacity' must be an integer 0-100".to_string())
            }
        };
        let blend = match layer_obj.get("blend") {
            None => Blend::Normal,
            Some(Value::String(s)) if s == "normal" => Blend::Normal,
            Some(Value::String(s)) if s == "add" => Blend::Add,
            Some(_) => {
                return Err("layer: param 'blend' must be \"normal\" or \"add\"".to_string())
            }
        };
        let enabled = match layer_obj.get("enabled") {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("layer: param 'enabled' must be a boolean".to_string()),
        };
        Ok(Layer {
            effect: Effect::from_json(effect_name, params)?,
            opacity,
            blend,
            enabled,
        })
    }
}

// ---------------------------------------------------------------------------
// Effect math (pure, no allocation — unit-testable without hardware)
// ---------------------------------------------------------------------------

/// hsv -> rgb; h in degrees (wraps at 360), s and v in 0..=1. Writes `out` in
/// place. Exact at the usual test points (pure red at h=0, etc.).
fn hsv2rgb(h: f32, s: f32, v: f32, out: &mut Rgb) {
    let c = v * s;
    let hp = ((h % 360.0 + 360.0) % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to_u8 = |f: f32| (255.0 * (f + m).clamp(0.0, 1.0) + 0.5) as u8;
    out.r = to_u8(r);
    out.g = to_u8(g);
    out.b = to_u8(b);
}

/// Linear interpolation between two colors at `p` in 0..=1. Writes `out`.
fn lerp_rgb(a: [u8; 3], b: [u8; 3], p: f32, out: &mut Rgb) {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * p + 0.5) as u8;
    out.r = f(a[0], b[0]);
    out.g = f(a[1], b[1]);
    out.b = f(a[2], b[2]);
}

/// Rounded average of a color slice; black for an empty slice. Used by the
/// no-per-LED fallback.
#[allow(dead_code)]
fn average(colors: &[Rgb]) -> Rgb {
    let n = colors.len().max(1) as f32;
    let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
    for c in colors {
        r += c.r as u32;
        g += c.g as u32;
        b += c.b as u32;
    }
    Rgb {
        r: (r as f32 / n + 0.5) as u8,
        g: (g as f32 / n + 0.5) as u8,
        b: (b as f32 / n + 0.5) as u8,
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Per-LED render geometry, precomputed once per device at `add_device`.
#[derive(Debug, Clone)]
pub struct Geometry {
    /// column-or-index position of each LED, device order (wave/gradient)
    pub positions: Vec<f32>,
    /// last position + 1; cyclic divisor so positions land in 0..<1 without
    /// the first and last LED colliding (wave/spectrum hues are circular)
    pub span: f32,
    /// max position (= span - 1 for non-empty); linear divisor so gradient
    /// endpoints hit their stops exactly (gradient is not circular)
    pub max: f32,
    /// (row, col) of each LED, device order. Matrix zones get their real
    /// grid cell; non-matrix LEDs get (0, col-or-index) as a fallback.
    pub rows_cols: Vec<(u8, u8)>,
    /// total bounding matrix (rows, cols) across zones; None when no zone
    /// has a matrix (ripple/reactive/fire fall back to `positions` then).
    pub dims: Option<(u8, u8)>,
}

impl Geometry {
    pub fn from_zones(zones: &[chromargb_drivers::Zone]) -> Self {
        let total: usize = zones.iter().map(|z| z.leds).sum();
        let mut positions = Vec::with_capacity(total);
        let mut rows_cols = Vec::with_capacity(total);
        let mut any_matrix = false;
        let mut max_rows = 0u8;
        let mut max_cols = 0u8;
        for zone in zones {
            match zone.matrix {
                Some((rows, cols)) => {
                    any_matrix = true;
                    max_rows = max_rows.max(rows);
                    max_cols = max_cols.max(cols);
                    for i in 0..zone.leds {
                        let row = (i / cols.max(1) as usize) as u8;
                        let col = (i % cols.max(1) as usize) as u8;
                        rows_cols.push((row, col));
                        positions.push(col as f32);
                    }
                }
                None => {
                    for i in 0..zone.leds {
                        rows_cols.push((0, i as u8));
                        positions.push(i as f32);
                    }
                }
            }
        }
        let max = positions.last().copied().unwrap_or(0.0);
        Geometry {
            positions,
            span: max + 1.0,
            max,
            rows_cols,
            dims: any_matrix.then_some((max_rows, max_cols)),
        }
    }
}

/// Wave phase offset (hue cycles, 0..1) of LED `i`.
/// Left: offset = position (hue grows rightward, so as the global phase
/// advances the pattern travels LEFT). Right: mirrored offset.
#[inline]
fn wave_pos(geo: &Geometry, i: usize, direction: Direction) -> f32 {
    let p = geo.positions[i] / geo.span;
    match direction {
        Direction::Left => p,
        Direction::Right => 1.0 - p,
    }
}

/// Speed 0-100 -> wave/spectrum hue cycles per second.
/// Mapping (linear in speed): 0 => 0.05 cy/s (20 s per full hue cycle),
/// 100 => 1.0 cy/s (1 s). i.e. f = 0.05 + speed/100 * 0.95.
#[inline]
fn hue_freq(speed: u8) -> f32 {
    0.05 + speed as f32 / 100.0 * 0.95
}

/// Speed 0-100 -> breathe cycles (full dark->bright->dark) per second.
/// Mapping: 0 => 0.125 cy/s (8 s per breath) .. 100 => 5 cy/s (0.2 s).
#[inline]
fn breathe_freq(speed: u8) -> f32 {
    0.125 + speed as f32 / 100.0 * 4.875
}

// ---------------------------------------------------------------------------
// Interactive + audio helpers (pure)
// ---------------------------------------------------------------------------

/// A pending interactive event (ripple origin / reactive keypress).
/// `x`, `y` in 0..=1 normalized matrix space (x across cols, y down rows).
/// Age is a plain elapsed-milliseconds counter the engine stamps 0 on
/// trigger and advances each frame — render math stays testable without
/// real timing (see [`ripple_contribution`] / [`reactive_contribution`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub elapsed_ms: u64,
    pub x: f32,
    pub y: f32,
}

/// Speed 0-100 -> ripple ring radii/s (fraction of matrix width/s).
/// 0 => 0.05 (crawls: full sweep ~20 s), 100 => 1.0 (fast, liquid).
#[inline]
fn ripple_speed_factor(speed: u8) -> f32 {
    0.05 + speed as f32 / 100.0 * 0.95
}

/// Pure per-event ripple contribution: an expanding ring around `(x, y)`
/// (normalized 0..=1 matrix space) at `elapsed_ms` with ring radius growing
/// at `ripple_speed_factor(speed)`, intensity `(1 - r / r_max)` inside a
/// thin band. Adds `color * intensity` onto `out` (saturating) — multiple
/// events sum. Returns false when the event has fully faded (caller prunes).
fn ripple_contribution(
    elapsed_ms: u64,
    x: f32,
    y: f32,
    color: [u8; 3],
    speed: u8,
    geo: &Geometry,
    out: &mut [Rgb],
) -> bool {
    let r = elapsed_ms as f32 / 1000.0 * ripple_speed_factor(speed);
    if r >= 1.0 {
        return false; // ring swept past the far corner: faded
    }
    // ring thickness scales with the matrix diagonal so a ring stays visible
    // on both small and large grids; 0.05 floor keeps it >= ~1 cell.
    let thickness = 0.05 + 0.06 * diagonal(geo).min(1.0);
    for (i, led) in out.iter_mut().enumerate() {
        let (px, py) = cell_xy(geo, i);
        let d = ((px - x) * (px - x) + (py - y) * (py - y)).sqrt();
        let band = 1.0 - (d - r).abs() / thickness;
        if band > 0.0 {
            let intensity = band * (1.0 - r); // radial falloff
            add_scaled(led, color, intensity);
        }
    }
    true
}

/// Diagonal of the normalized matrix space (>= 1; 1.0 when no dims — the
/// strip fallback maps every LED to (0,0)).
#[inline]
fn diagonal(geo: &Geometry) -> f32 {
    match geo.dims {
        Some((rows, cols)) if rows > 0 && cols > 0 => {
            let aspect = cols as f32 / rows as f32;
            (aspect * aspect + 1.0).sqrt()
        }
        _ => 1.0,
    }
}

/// Normalized (x, y) in 0..=1 of LED `i`: by (row, col) against `dims` when
/// the device has matrix dims, else (0, position) — matrix effects on
/// non-matrix devices (strips) collapse to the left edge, matching the
/// existing column-position semantics.
#[inline]
fn cell_xy(geo: &Geometry, i: usize) -> (f32, f32) {
    match geo.dims {
        Some((rows, cols)) if rows > 0 && cols > 0 => {
            let (r, c) = geo.rows_cols[i];
            (c as f32 / cols as f32, r as f32 / rows as f32)
        }
        _ => (0.0, geo.positions[i].min(geo.max.max(1.0)) / geo.max.max(1.0)),
    }
}

/// Saturating-add `color * scale` onto `led`. Used by ripple (ring band)
/// and reactive (press decay).
#[inline]
fn add_scaled(led: &mut Rgb, color: [u8; 3], scale: f32) {
    led.r = (led.r as f32 + color[0] as f32 * scale).min(255.0) as u8;
    led.g = (led.g as f32 + color[1] as f32 * scale).min(255.0) as u8;
    led.b = (led.b as f32 + color[2] as f32 * scale).min(255.0) as u8;
}

/// Pure per-event reactive contribution: the pressed cell (nearest cell to
/// `(x, y)` in normalized space) lights full and linearly decays to black
/// over `fade` effect-time seconds. Adds onto `out`; returns false when
/// fully decayed (caller prunes it).
fn reactive_contribution(
    elapsed_ms: u64,
    x: f32,
    y: f32,
    color: [u8; 3],
    fade: u8,
    geo: &Geometry,
    out: &mut [Rgb],
) -> bool {
    let fade_secs = reactive_fade_secs(fade);
    let t = elapsed_ms as f32 / 1000.0;
    if t >= fade_secs {
        return false;
    }
    // nearest cell to the event point
    let mut best = 0usize;
    let mut best_d = f32::INFINITY;
    for (i, _) in out.iter().enumerate() {
        let (px, py) = cell_xy(geo, i);
        let d = (px - x) * (px - x) + (py - y) * (py - y);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    let brightness = 1.0 - t / fade_secs;
    add_scaled(&mut out[best], color, brightness);
    true
}

/// Fade 0-100 -> decay seconds. 0 => 0.2 s (snappy), 100 => 2.0 s (lingering).
#[inline]
fn reactive_fade_secs(fade: u8) -> f32 {
    0.2 + fade as f32 / 100.0 * 1.8
}

/// FNV-1a 32-bit hash of an LED index (stable per-cell pseudo-randomness).
#[inline]
fn fnv1a(i: usize) -> u32 {
    fnv1a_raw(i)
}

/// Heat 0..1 -> fire palette black->red->orange->yellow->white. Writes out.
fn heat_color(heat: f32, out: &mut Rgb) {
    let x = heat.clamp(0.0, 1.0);
    // 4 anchor points over 0..=1
    let anchors: [(f32, f32, f32); 5] =
        [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (1.0, 0.45, 0.0), (1.0, 1.0, 0.0), (1.0, 1.0, 1.0)];
    let p = x * 4.0;
    let seg = (p as usize).min(3);
    let f = p - seg as f32;
    let a = anchors[seg];
    let b = anchors[seg + 1];
    let ch = |lo: f32, hi: f32| (255.0 * (lo + (hi - lo) * f) + 0.5) as u8;
    out.r = ch(a.0, b.0);
    out.g = ch(a.1, b.1);
    out.b = ch(a.2, b.2);
}

/// 1-D smooth value noise: fixed gradient positions every 1.0 in x, cubic
/// smoothstep interpolation between them, gradients hashed by slot index.
#[inline]
fn noise1(x: f32, slot: u32) -> f32 {
    let i = x.floor();
    let f = x - i;
    // smoothstep
    let s = f * f * (3.0 - 2.0 * f);
    let g = |k: f32| {
        let h = fnv1a_raw(k as i64 as usize ^ slot as usize);
        (h as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let a = g(i);
    let b = g(i + 1.0);
    (a + (b - a) * s) * 0.5 + 0.5
}

/// Raw FNV-1a of a usize (no salt) — noise gradient hashing.
#[inline]
fn fnv1a_raw(x: usize) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in x.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Speed 0-100 -> fire scroll rate (rows/s at the reference row period).
#[inline]
fn fire_speed_factor(speed: u8) -> f32 {
    1.0 + speed as f32 / 100.0 * 9.0
}

/// Render one frame of `effect` at effect-time `t_ms` into `out` (one Rgb per
/// LED; `out.len()` must equal the device LED count). `events` carries the
/// pending interactive events (ripple/reactive); `audio` is the shared audio
/// level 0..=1000 (AudioMeter). Pure: no allocation, no hardware, no global
/// state — the unit tests drive this directly.
pub fn render(t_ms: u64, effect: &Effect, geo: &Geometry, events: &[Event], audio: u32, out: &mut [Rgb]) {
    let t = t_ms as f32 / 1000.0;
    match effect {
        Effect::Static { color } => {
            for led in out.iter_mut() {
                led.r = color[0];
                led.g = color[1];
                led.b = color[2];
            }
        }
        Effect::Spectrum { speed } => {
            // uniform hue across all LEDs, rotating over time
            let hue = (t * hue_freq(*speed) * 360.0) % 360.0;
            for led in out.iter_mut() {
                hsv2rgb(hue, 1.0, 1.0, led);
            }
        }
        Effect::Wave { speed, direction } => {
            // per-LED hue offset by position (matrix column, else index)
            let phase = t * hue_freq(*speed);
            for (i, led) in out.iter_mut().enumerate() {
                let hue = ((phase + wave_pos(geo, i, *direction)) % 1.0) * 360.0;
                hsv2rgb(hue, 1.0, 1.0, led);
            }
        }
        Effect::Breathe { speed, color } => {
            // (1 - cos(x)) / 2 brightness envelope in 0..=1: starts dark,
            // full at half period, dark again at the period.
            let x = t * breathe_freq(*speed) * std::f32::consts::TAU;
            let envelope = (1.0 - x.cos()) * 0.5;
            let base = color.unwrap_or([255, 255, 255]);
            let f = |c: u8| (c as f32 * envelope + 0.5) as u8;
            for led in out.iter_mut() {
                led.r = f(base[0]);
                led.g = f(base[1]);
                led.b = f(base[2]);
            }
        }
        Effect::Gradient { colors } => {
            // lerp across color stops by LED position; static in time.
            // <2 stops cannot come from the parser; render degrades to the
            // single color rather than panicking on a hand-built Effect.
            let segs = colors.len().saturating_sub(1);
            if segs == 0 {
                let c = colors.first().copied().unwrap_or([0, 0, 0]);
                for led in out.iter_mut() {
                    *led = Rgb { r: c[0], g: c[1], b: c[2] };
                }
                return;
            }
            for (i, led) in out.iter_mut().enumerate() {
                // linear normalization (see Geometry::max): first LED = first
                // stop, last LED = final stop, exactly
                let norm = if geo.max > 0.0 { geo.positions[i] / geo.max } else { 0.0 };
                let p = norm * segs as f32;
                let seg = (p as usize).min(segs - 1);
                lerp_rgb(colors[seg], colors[seg + 1], p - seg as f32, led);
            }
        }
        Effect::Ripple { color, speed } => {
            for led in out.iter_mut() {
                *led = Rgb { r: 0, g: 0, b: 0 };
            }
            for ev in events {
                ripple_contribution(ev.elapsed_ms, ev.x, ev.y, *color, *speed, geo, out);
            }
        }
        Effect::Reactive { color, fade } => {
            for led in out.iter_mut() {
                *led = Rgb { r: 0, g: 0, b: 0 };
            }
            for ev in events {
                reactive_contribution(ev.elapsed_ms, ev.x, ev.y, *color, *fade, geo, out);
            }
        }
        Effect::Starlight { colors, speed } => {
            // per-cell twinkle: cell hash picks color + phase offset,
            // brightness = (1 - cos(t * speed_factor + phase)) / 2.
            // speed 0-100 => 0.1..2.0 full cycles/s.
            let freq = 0.1 + *speed as f32 / 100.0 * 1.9;
            for (i, led) in out.iter_mut().enumerate() {
                let h = fnv1a(i);
                let phase = (h % 628) as f32 / 100.0; // 0..<6.28
                let color = colors[(h as usize >> 10) % colors.len().max(1)];
                let brightness = (1.0 - (t * freq * std::f32::consts::TAU + phase).cos()) * 0.5;
                let f = |c: u8| (c as f32 * brightness + 0.5) as u8;
                led.r = f(color[0]);
                led.g = f(color[1]);
                led.b = f(color[2]);
            }
        }
        Effect::Fire { speed } => {
            let rows = geo.dims.map(|(r, _)| r as usize).unwrap_or(1);
            let rate = fire_speed_factor(*speed);
            for (i, led) in out.iter_mut().enumerate() {
                let (r, c) = geo.rows_cols[i];
                let y = r as f32 - t * rate;
                let noise = noise1(y * 0.5 + c as f32 * 0.35, 0x5eed);
                // fire rises: row 0 (top) cooled to 25%, bottom row full
                // heat — flames start hot at the spacebar edge and cool
                // upward through the number row.
                let height = if rows > 1 { r as f32 / (rows - 1) as f32 } else { 1.0 };
                let heat = (noise * (0.25 + 0.75 * height)).clamp(0.0, 1.0);
                heat_color(heat, led);
            }
        }
        Effect::AudioMeter { color, bands } => {
            let level = (audio.min(1000) as f32) / 1000.0;
            let cols = geo.dims.map(|(_, c)| c).unwrap_or(1).max(1) as f32;
            for (i, led) in out.iter_mut().enumerate() {
                let (_, c) = geo.rows_cols[i];
                // which band this column falls into
                let band = ((c as f32 / cols) * *bands as f32).floor().min(*bands as f32 - 1.0);
                let threshold = (band + 1.0) / *bands as f32;
                if level >= threshold {
                    *led = match color {
                        Some(c) => Rgb { r: c[0], g: c[1], b: c[2] },
                        None => {
                            let hue = band / *bands as f32 * 360.0;
                            let mut tmp = Rgb { r: 0, g: 0, b: 0 };
                            hsv2rgb(hue, 1.0, 1.0, &mut tmp);
                            tmp
                        }
                    };
                } else {
                    *led = Rgb { r: 0, g: 0, b: 0 };
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Engine: layer stacks + scheduler thread
// ---------------------------------------------------------------------------

/// Scheduling error.
#[derive(Debug)]
pub enum EngineError {
    UnknownDevice,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::UnknownDevice => write!(f, "unknown device"),
        }
    }
}

/// Per-device slot: backend + geometry + layer stack + buffers, guarded by
/// one mutex (the scheduler locks each slot briefly per frame).
struct Slot {
    backend: Box<dyn DriverBackend>,
    geo: Geometry,
    /// None = dark (engine start state, no write); Some(stack) = render each
    /// frame. Some([]) = lit stack with no layers → black frame.
    layers: Option<Vec<Layer>>,
    frame: Vec<Rgb>,
    scratch: Vec<Rgb>,
    events: VecDeque<Event>,
    /// effect-time origin of the running stack (reset by set_layers)
    t0: Instant,
}

impl Slot {
    fn new(backend: Box<dyn DriverBackend>) -> Slot {
        let info = backend.info();
        let leds: usize = info.zones.iter().map(|z| z.leds).sum();
        Slot {
            geo: Geometry::from_zones(&info.zones),
            backend,
            layers: None,
            frame: vec![Rgb { r: 0, g: 0, b: 0 }; leds],
            scratch: vec![Rgb { r: 0, g: 0, b: 0 }; leds],
            events: VecDeque::new(),
            t0: Instant::now(),
        }
    }
}

/// One composited frame: render each enabled layer into `scratch`, blend
/// top-down onto `frame` (Normal = lerp by opacity, Add = saturating add
/// scaled by opacity). Pure — unit-testable without a device.
fn composite(
    t_ms: u64,
    layers: &[Layer],
    geo: &Geometry,
    events: &[Event],
    audio: u32,
    frame: &mut [Rgb],
    scratch: &mut [Rgb],
) {
    for led in frame.iter_mut() {
        *led = Rgb { r: 0, g: 0, b: 0 };
    }
    for layer in layers.iter().filter(|l| l.enabled) {
        render(t_ms, &layer.effect, geo, events, audio, &mut *scratch);
        match layer.blend {
            Blend::Normal => {
                let opacity = layer.opacity;
                for (acc, src) in frame.iter_mut().zip(scratch.iter()) {
                    acc.r = lerp_u8(acc.r, src.r, opacity);
                    acc.g = lerp_u8(acc.g, src.g, opacity);
                    acc.b = lerp_u8(acc.b, src.b, opacity);
                }
            }
            Blend::Add => {
                let opacity = layer.opacity;
                for (acc, src) in frame.iter_mut().zip(scratch.iter()) {
                    add_scaled(acc, [src.r, src.g, src.b], opacity);
                }
            }
        }
    }
}

/// Byte lerp for Normal blend.
#[inline]
fn lerp_u8(a: u8, b: u8, p: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * p + 0.5) as u8
}

/// Effects engine: owns one [`Slot`] per device behind a mutex, plus one
/// scheduler thread rendering every active stack at `frame_period` cadence.
///
/// API the daemon already calls keeps working unchanged:
/// `with_frame_period` / `add_device` / `start` / `stop` / `stop_all`.
/// New layered API: `set_layers` / `trigger` / `set_audio_level`.
///
/// `Drop` stops the thread — an engine can never be dropped while its
/// scheduler still runs (a daemon wiring cannot leak the thread).
pub struct Engine {
    inner: Arc<EngineInner>,
    /// set by Drop; the scheduler exits when it observes it
    shutdown: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

struct EngineInner {
    slots: Mutex<HashMap<String, Arc<Mutex<Slot>>>>,
    /// scheduler period; 0 would spin, so it's kept >= 1ms internally
    frame_period: Duration,
    audio: Arc<AtomicU32>,
}

impl Engine {
    pub fn with_frame_period(frame_period: Duration) -> Engine {
        let inner = Arc::new(EngineInner {
            slots: Mutex::new(HashMap::new()),
            frame_period: frame_period.max(Duration::from_millis(1)),
            audio: Arc::new(AtomicU32::new(0)),
        });
        let shutdown = Arc::new(AtomicU32::new(0));
        let thread = thread::Builder::new()
            .name("chromargb-engine".into())
            .spawn({
                let inner = Arc::clone(&inner);
                let shutdown = Arc::clone(&shutdown);
                move || scheduler_loop(inner, shutdown)
            })
            .expect("engine thread");
        Engine { inner, shutdown, thread: Some(thread) }
    }

    /// Register a device backend. Called at engine construction time with
    /// all devices; idempotent (re-adding replaces the slot).
    pub fn add_device(&mut self, id: &str, backend: Box<dyn DriverBackend>) {
        let slot = Arc::new(Mutex::new(Slot::new(backend)));
        self.inner.slots.lock().unwrap().insert(id.to_string(), slot);
    }

    /// Back-compat: single-effect start = one opaque Normal layer.
    pub fn start(&mut self, id: &str, effect: Effect) -> Result<(), EngineError> {
        self.set_layers(
            id,
            vec![Layer { effect, opacity: 1.0, blend: Blend::Normal, enabled: true }],
        )
    }

    /// Replace a device's layer stack. Empty stack = black frame (device
    /// stays owned but renders dark); unknown device = error.
    pub fn set_layers(&mut self, id: &str, layers: Vec<Layer>) -> Result<(), EngineError> {
        let slots = self.inner.slots.lock().unwrap();
        let Some(slot) = slots.get(id) else {
            return Err(EngineError::UnknownDevice);
        };
        let mut s = slot.lock().unwrap();
        s.layers = Some(layers);
        s.events.clear();
        s.t0 = Instant::now();
        Ok(())
    }

    /// Queue an interactive event (ripple origin / reactive press) for a
    /// device. Events age out and are pruned automatically by the scheduler.
    pub fn trigger(&self, id: &str, x: f32, y: f32) -> Result<(), EngineError> {
        let slots = self.inner.slots.lock().unwrap();
        let Some(slot) = slots.get(id) else {
            return Err(EngineError::UnknownDevice);
        };
        let mut s = slot.lock().unwrap();
        s.events.push_back(Event { elapsed_ms: 0, x, y });
        Ok(())
    }

    /// Shared audio level 0..=1000 (daemon's meter writes; AudioMeter layers
    /// read). One level for all devices — a single system meter.
    pub fn set_audio_level(&self, level: u32) {
        self.inner.audio.store(level.min(1000), Ordering::Relaxed);
    }

    /// Stop rendering one device; leaves its last frame on the hardware
    /// (matches the previous engine's behavior — `stop_effect` semantics).
    pub fn stop(&mut self, id: &str) -> Result<(), EngineError> {
        let slots = self.inner.slots.lock().unwrap();
        let Some(slot) = slots.get(id) else {
            return Err(EngineError::UnknownDevice);
        };
        let mut s = slot.lock().unwrap();
        s.layers = None;
        s.events.clear();
        Ok(())
    }

    /// Stop all devices (dark; engine keeps running).
    pub fn stop_all(&mut self) {
        let slots = self.inner.slots.lock().unwrap();
        for slot in slots.values() {
            let mut s = slot.lock().unwrap();
            s.layers = None;
            s.events.clear();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown.store(1, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The scheduler: every `frame_period`, for each slot with an active stack,
/// advance + prune events, render + composite all layers, write the frame.
/// Devices whose write fails are left in place (transient USB errors); a
/// hard-failed backend surfaces errors through its own `write_frame`.
fn scheduler_loop(inner: Arc<EngineInner>, shutdown: Arc<AtomicU32>) {
    let mut last = Instant::now();
    loop {
        if shutdown.load(Ordering::SeqCst) == 1 {
            return;
        }
        let period = inner.frame_period;
        let now = Instant::now();
        if now - last < period {
            thread::sleep(period - (now - last));
            continue;
        }
        last = now;
        let audio = inner.audio.load(Ordering::Relaxed);
        let slots = inner.slots.lock().unwrap();
        for slot in slots.values() {
            let mut s = slot.lock().unwrap();
            // disjoint field borrows: layers/geo read-only, events/frame/
            // scratch mutable — impossible through the guard in one piece.
            let Slot { backend, geo, layers, frame, scratch, events, t0 } = &mut *s;
            let Some(layers) = layers.as_ref() else { continue };
            // advance + prune events (ripple/reactive fade them out)
            let dt_ms = period.as_millis() as u64;
            for ev in events.iter_mut() {
                ev.elapsed_ms += dt_ms;
            }
            events.retain(|ev| ev.elapsed_ms < MAX_EVENT_AGE_MS);
            composite(
                now_epoch_ms(t0),
                layers,
                geo,
                events.as_slices().0, // front slice: fine for a handful of events
                audio,
                frame,
                scratch,
            );
            if backend.write_frame(frame).is_err() {
                // transient USB error: skip this frame; next frame retries.
            }
        }
    }
}

/// Effect-time: ms since stack start. Uses wall clock so time-based effects
/// (wave/breathe/spectrum) keep their phase across scheduler jitter.
#[inline]
fn now_epoch_ms(t0: &Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}

/// Events older than this are pruned (longest fade: reactive 2s; ripples
/// sweep the matrix in <= 20 s at speed 0, but visually gone long before).
const MAX_EVENT_AGE_MS: u64 = 20_000;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x2 matrix zone, 4 LEDs.
    fn geo2x2() -> Geometry {
        Geometry::from_zones(&[chromargb_drivers::Zone {
            name: "m".into(),
            leds: 4,
            matrix: Some((2, 2)),
        }])
    }

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    #[test]
    fn layer_from_json_roundtrip() {
        let l = Layer::from_json(
            "ripple",
            &serde_json::json!({"color": [255, 0, 255], "speed": 80}),
            &serde_json::json!({"opacity": 50, "blend": "add", "enabled": false}),
        )
        .unwrap();
        assert_eq!(l.effect, Effect::Ripple { color: [255, 0, 255], speed: 80 });
        assert_eq!(l.opacity, 0.5);
        assert_eq!(l.blend, Blend::Add);
        assert!(!l.enabled);
    }

    #[test]
    fn layer_from_json_defaults() {
        let l = Layer::from_json(
            "static",
            &serde_json::json!({"color": [1, 2, 3]}),
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(l.opacity, 1.0);
        assert_eq!(l.blend, Blend::Normal);
        assert!(l.enabled);
    }

    #[test]
    fn layer_from_json_rejects_bad_opacity() {
        assert!(Layer::from_json(
            "static",
            &serde_json::json!({"color": [1, 2, 3]}),
            &serde_json::json!({"opacity": 101})
        )
        .is_err());
    }

    #[test]
    fn composite_normal_lerps_by_opacity() {
        let geo = geo2x2();
        let layers = vec![Layer {
            effect: Effect::Static { color: [255, 0, 0] },
            opacity: 0.5,
            blend: Blend::Normal,
            enabled: true,
        }];
        let mut frame = vec![rgb(0, 0, 0); 4];
        let mut scratch = vec![rgb(0, 0, 0); 4];
        composite(0, &layers, &geo, &[], 0, &mut frame, &mut scratch);
        assert_eq!(frame, vec![rgb(128, 0, 0); 4]); // 0 + 255*0.5 + 0.5 = 128 (round-half-up)
    }

    #[test]
    fn composite_add_saturates() {
        let geo = geo2x2();
        let layers = vec![
            Layer { effect: Effect::Static { color: [200, 100, 0] }, opacity: 1.0, blend: Blend::Add, enabled: true },
            Layer { effect: Effect::Static { color: [200, 100, 0] }, opacity: 1.0, blend: Blend::Add, enabled: true },
        ];
        let mut frame = vec![rgb(0, 0, 0); 4];
        let mut scratch = vec![rgb(0, 0, 0); 4];
        composite(0, &layers, &geo, &[], 0, &mut frame, &mut scratch);
        assert_eq!(frame, vec![rgb(255, 200, 0); 4]);
    }

    #[test]
    fn composite_disabled_layer_skipped() {
        let geo = geo2x2();
        let layers = vec![
            Layer { effect: Effect::Static { color: [10, 10, 10] }, opacity: 1.0, blend: Blend::Normal, enabled: true },
            Layer { effect: Effect::Static { color: [200, 0, 0] }, opacity: 1.0, blend: Blend::Normal, enabled: false },
        ];
        let mut frame = vec![rgb(0, 0, 0); 4];
        let mut scratch = vec![rgb(0, 0, 0); 4];
        composite(0, &layers, &geo, &[], 0, &mut frame, &mut scratch);
        assert_eq!(frame, vec![rgb(10, 10, 10); 4]);
    }

    #[test]
    fn render_ripple_lights_ring_band() {
        let geo = geo2x2();
        let mut out = vec![rgb(0, 0, 0); 4];
        let events = [Event { elapsed_ms: 0, x: 0.0, y: 0.0 }];
        render(
            0,
            &Effect::Ripple { color: [255, 255, 255], speed: 50 },
            &geo,
            &events,
            0,
            &mut out,
        );
        // origin cell is inside the band at t=0 (r=0, d=0 => band=1)
        assert_eq!(out[0], rgb(255, 255, 255));
        // far corner at distance sqrt2 > thickness: dark
        assert_eq!(out[3], rgb(0, 0, 0));
    }

    #[test]
    fn render_ripple_fades_fully() {
        let geo = geo2x2();
        let mut out = vec![rgb(0, 0, 0); 4];
        // speed 0 => 0.05 radii/s => at 10 s the ring radius is 0.5 = the
        // exact distance to cell (0,1) — its band is centered on the ring.
        let events = [Event { elapsed_ms: 10_000, x: 0.0, y: 0.0 }];
        render(
            0,
            &Effect::Ripple { color: [255, 0, 0], speed: 0 },
            &geo,
            &events,
            0,
            &mut out,
        );
        assert!(out[1].r > 0, "ring must cross cell (0,1) at r=0.5");
    }

    #[test]
    fn render_reactive_decays() {
        let geo = geo2x2();
        let mut out = vec![rgb(0, 0, 0); 4];
        let events = [Event { elapsed_ms: 0, x: 0.0, y: 0.0 }];
        render(
            0,
            &Effect::Reactive { color: [255, 0, 0], fade: 50 },
            &geo,
            &events,
            0,
            &mut out,
        );
        assert_eq!(out[0], rgb(255, 0, 0)); // full at t=0
        assert_eq!(out[1], rgb(0, 0, 0)); // only nearest cell
        let mut out2 = vec![rgb(0, 0, 0); 4];
        let events2 = [Event { elapsed_ms: 2_000, x: 0.0, y: 0.0 }];
        render(0, &Effect::Reactive { color: [255, 0, 0], fade: 50 }, &geo, &events2, 0, &mut out2);
        assert_eq!(out2[0], rgb(0, 0, 0)); // fade 50 => 1.1s decay, gone at 2s
    }

    #[test]
    fn render_starlight_is_black_at_zero_brightness() {
        let geo = geo2x2();
        let mut out = vec![rgb(1, 1, 1); 4];
        // find a t where a given cell is dark; deterministic: check that
        // every cell hits zero brightness at *some* sampled t (twinkle).
        let mut all_zero_seen = vec![false; 4];
        for step in 0..200 {
            let t = step as u64 * 10;
            render(
                t,
                &Effect::Starlight { colors: vec![[255, 255, 255]], speed: 50 },
                &geo,
                &[],
                0,
                &mut out,
            );
            for (i, led) in out.iter().enumerate() {
                if *led == rgb(0, 0, 0) {
                    all_zero_seen[i] = true;
                }
            }
        }
        assert!(all_zero_seen.iter().all(|z| *z), "every cell must twinkle through 0");
    }

    #[test]
    fn render_fire_bottom_hotter_than_top() {
        // 1 col x 4 rows matrix
        let geo = Geometry::from_zones(&[chromargb_drivers::Zone {
            name: "m".into(),
            leds: 4,
            matrix: Some((4, 1)),
        }]);
        let mut out = vec![rgb(0, 0, 0); 4];
        render(0, &Effect::Fire { speed: 50 }, &geo, &[], 0, &mut out);
        // row 0 (top) vs row 3 (bottom): bottom r >= top r on average heat
        let top = out[0];
        let bottom = out[3];
        assert!(bottom.r >= top.r);
    }

    #[test]
    fn render_audio_meter_bands_light_by_level() {
        let geo = geo2x2();
        let mut out = vec![rgb(0, 0, 0); 4];
        // level 0 => all bands below threshold => black
        render(
            0,
            &Effect::AudioMeter { color: Some([0, 255, 0]), bands: 2 },
            &geo,
            &[],
            0,
            &mut out,
        );
        assert_eq!(out, vec![rgb(0, 0, 0); 4]);
        // full level => every band lit
        let mut out2 = vec![rgb(0, 0, 0); 4];
        render(
            0,
            &Effect::AudioMeter { color: Some([0, 255, 0]), bands: 2 },
            &geo,
            &[],
            1000,
            &mut out2,
        );
        assert_eq!(out2, vec![rgb(0, 255, 0); 4]);
    }

    #[test]
    fn render_audio_meter_partial_level_lights_low_band_only() {
        let geo = geo2x2();
        let mut out = vec![rgb(0, 0, 0); 4];
        // 2 bands over 2 cols: level 0.5 => band 0 (threshold 0.5) lit,
        // band 1 (threshold 1.0) dark. cols 0/1 => band 0/1.
        render(
            0,
            &Effect::AudioMeter { color: Some([9, 9, 9]), bands: 2 },
            &geo,
            &[],
            500,
            &mut out,
        );
        // geometry: 2x2 => led0=(0,0) led1=(0,1) led2=(1,0) led3=(1,1)
        assert_eq!(out[0], rgb(9, 9, 9)); // col 0 => band 0 lit
        assert_eq!(out[2], rgb(9, 9, 9)); // col 0 => band 0 lit
        assert_eq!(out[1], rgb(0, 0, 0)); // col 1 => band 1 dark
        assert_eq!(out[3], rgb(0, 0, 0));
    }

    #[test]
    fn geometry_rows_cols_map_matrix() {
        let geo = geo2x2();
        assert_eq!(geo.rows_cols, vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
        assert_eq!(geo.dims, Some((2, 2)));
    }

    #[test]
    fn effect_from_json_new_kinds() {
        assert!(Effect::from_json("ripple", &serde_json::json!({"color": [1,2,3], "speed": 10})).is_ok());
        assert!(Effect::from_json("reactive", &serde_json::json!({"color": [1,2,3], "fade": 10})).is_ok());
        assert!(Effect::from_json("starlight", &serde_json::json!({"colors": [[1,2,3]], "speed": 10})).is_ok());
        assert!(Effect::from_json("fire", &serde_json::json!({"speed": 10})).is_ok());
        assert!(Effect::from_json("audio_meter", &serde_json::json!({"bands": 4})).is_ok());
        assert!(Effect::from_json("audio_meter", &serde_json::json!({"bands": 0})).is_err());
        assert!(Effect::from_json("nope", &serde_json::json!({})).is_err());
    }
}
