//! OpenRGB network bridge client (E1.37 SDK protocol over TCP, default port
//! 6742). Asks a local OpenRGB server (server mode) for its controllers and
//! exposes them as ordinary ChromaRGB devices, covering the wider vendor
//! ecosystem (Corsair, ASUS, MSI, Gigabyte, G.Skill, Cooler Master, HyperX,
//! NZXT, SteelSeries, ...) without native drivers.
//!
//! Protocol sources (fetched and verified byte-for-byte against the OpenRGB
//! sources, Nov 2026):
//! - https://openrgb.org/sdk.html (SDK overview)
//! - https://raw.githubusercontent.com/CalcProgrammer1/OpenRGB/master/NetworkProtocol.h
//!   (packet IDs; NetPacketHeader = magic "ORGB", u32 dev_id, u32 pkt_id,
//!   u32 size, little-endian — dev_id comes BEFORE pkt_id)
//! - https://raw.githubusercontent.com/CalcProgrammer1/OpenRGB/master/Documentation/OpenRGBSDK.md
//!   (packet tables; controller-count/controller-data flows)
//! - https://raw.githubusercontent.com/CalcProgrammer1/OpenRGB/master/RGBController/RGBController.cpp
//!   (SetDeviceDescription / SetZoneDescription / SetModeDescription /
//!   SetColorDescription: exact field order and widths)
//! - https://raw.githubusercontent.com/CalcProgrammer1/OpenRGB/master/RGBController/RGBController_Network.cpp
//!   (CreateUpdateLEDsPacket: u32 self-inclusive data_size, u16 count, colors)
//! - https://raw.githubusercontent.com/CalcProgrammer1/OpenRGB/master/RGBController/RGBControllerInterface.h
//!   (ToRGBColor = (b<<16)|(g<<8)|r -> memcpy'd little-endian = wire bytes
//!   [R,G,B,0]: 4 bytes per LED, alpha byte 0)
//!
//! Scope (ponytail): controller count, controller data, UpdateLEDs. No modes,
//! segments, or custom modes are touched.

use std::ffi::CString;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::{DeviceInfo, DiscoveredDevice, DriverBackend, Result, Rgb, Zone};

// --- Wire constants (sources above) -----------------------------------------

/// Header: 'O','R','G','B' then u32 dev_id, u32 pkt_id, u32 size, all LE.
const HDR: usize = 16;
/// Highest protocol version we speak. 3 keeps the on-wire controller format
/// stable (segments land at v4, zone flags at v5, unique device IDs at v6)
/// and every server since OpenRGB 0.7 (2020) speaks it. See OpenRGBSDK.md
/// "Protocol Versions".
const PROTOCOL_VERSION: u32 = 3;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// OpenRGB SDK max packet size is 8 MiB (NetworkProtocol.h); guard allocs.
const MAX_PACKET: usize = 8 * 1024 * 1024;

// Packet IDs from NetworkProtocol.h
const PKT_REQUEST_CONTROLLER_COUNT: u32 = 0;
const PKT_REQUEST_CONTROLLER_DATA: u32 = 1;
const PKT_REQUEST_PROTOCOL_VERSION: u32 = 40;
const PKT_RGBCONTROLLER_UPDATELEDS: u32 = 1050;

// --- Errors -------------------------------------------------------------------
//
// The bridge speaks TCP, not HID, and crate::Error only has Hid/Unsupported
// (lib.rs is owned by another worker); wire errors map into Unsupported with
// a clear prefix at the public seam via From below.

#[derive(Debug)]
enum WireError {
    Io(String),
    Protocol(String),
}

impl From<WireError> for crate::Error {
    fn from(e: WireError) -> crate::Error {
        crate::Error::Unsupported(match e {
            WireError::Io(s) => format!("openrgb bridge i/o: {s}"),
            WireError::Protocol(s) => format!("openrgb bridge protocol: {s}"),
        })
    }
}

type WireResult<T> = std::result::Result<T, WireError>;

// --- Frame I/O ------------------------------------------------------------------

fn send_packet(stream: &mut TcpStream, dev_id: u32, pkt_id: u32, data: &[u8]) -> WireResult<()> {
    let mut buf = Vec::with_capacity(HDR + data.len());
    buf.extend_from_slice(b"ORGB");
    buf.extend_from_slice(&dev_id.to_le_bytes());
    buf.extend_from_slice(&pkt_id.to_le_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(data);
    stream.write_all(&buf).map_err(|e| WireError::Io(e.to_string()))
}

/// One received packet: pkt_id + payload (dev_id is parsed for framing but
/// unused — replies are matched by pkt_id).
struct Packet {
    pkt_id: u32,
    payload: Vec<u8>,
}

fn recv_packet(stream: &mut TcpStream) -> WireResult<Packet> {
    let mut hdr = [0u8; HDR];
    stream
        .read_exact(&mut hdr)
        .map_err(|e| WireError::Io(e.to_string()))?;
    if &hdr[..4] != b"ORGB" {
        return Err(WireError::Protocol("bad packet magic".into()));
    }
    let pkt_id = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
    let size = u32::from_le_bytes(hdr[12..16].try_into().unwrap()) as usize;
    if size > MAX_PACKET {
        return Err(WireError::Protocol(format!("packet too large: {size}")));
    }
    let mut payload = vec![0u8; size];
    stream
        .read_exact(&mut payload)
        .map_err(|e| WireError::Io(e.to_string()))?;
    Ok(Packet { pkt_id, payload })
}

/// Read packets until one with `pkt_id` arrives. Servers may interleave
/// unsolicited notifications (device-list-updated, server name, ...); skip.
fn recv_matching(stream: &mut TcpStream, pkt_id: u32) -> WireResult<Packet> {
    loop {
        let p = recv_packet(stream)?;
        if p.pkt_id == pkt_id {
            return Ok(p);
        }
    }
}

// --- Little-endian cursor over the controller-data block ------------------------

/// Cursor whose errors carry the byte offset where parsing went wrong, so a
/// misaligned parse against a real server fails loud instead of half-working.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> WireResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or_else(|| {
                WireError::Protocol(format!(
                    "controller data truncated at byte {} (wanted {n})",
                    self.pos
                ))
            })?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u16le(&mut self) -> WireResult<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32le(&mut self) -> WireResult<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    /// Length-prefixed string: u16 length includes the NUL (OpenRGB SDK).
    fn string(&mut self) -> WireResult<String> {
        let len = self.u16le()? as usize;
        let bytes = self.take(len)?;
        if bytes.last() != Some(&0) {
            return Err(WireError::Protocol(format!(
                "string missing NUL at byte {}",
                self.pos - len
            )));
        }
        String::from_utf8(bytes[..len - 1].to_vec())
            .map_err(|_| WireError::Protocol("string not UTF-8".into()))
    }
    /// One LED entry at protocol <= 5: name string + u32 value (value was
    /// removed in protocol 6; we negotiate 3).
    fn led(&mut self) -> WireResult<()> {
        self.string()?;
        self.u32le()?;
        Ok(())
    }
    fn done(&self) -> WireResult<()> {
        if self.pos != self.buf.len() {
            return Err(WireError::Protocol(format!(
                "trailing bytes: consumed {} of {}",
                self.pos,
                self.buf.len()
            )));
        }
        Ok(())
    }
}

// --- Controller-data parsing -----------------------------------------------------

/// The subset of a controller we keep: name, zones, total LED count.
struct Controller {
    name: String,
    zones: Vec<Zone>,
}

/// Parse one controller-data block, protocol 3 format. Field order verified
/// against RGBController::SetDeviceDescription.
fn parse_controller(payload: &[u8]) -> WireResult<Controller> {
    let mut c = Cursor::new(payload);
    // Field order per OpenRGBSDK.md "Device Data" and SetDeviceDescription:
    // type, name, vendor (v1+), description, version, serial, location.
    let _type_ = c.u32le()?; // device type (keyboard/mouse/...)
    let name = c.string()?;
    let _vendor = c.string()?; // present since protocol 1
    let _description = c.string()?;
    let _version = c.string()?;
    let _serial = c.string()?;
    let _location = c.string()?;
    let num_modes = c.u16le()? as usize;
    let _active_mode = c.u32le()?;
    for _ in 0..num_modes {
        skip_mode(&mut c)?;
    }
    let num_zones = c.u16le()? as usize;
    let mut zones = Vec::with_capacity(num_zones);
    for _ in 0..num_zones {
        zones.push(parse_zone(&mut c)?);
    }
    let num_leds = c.u16le()? as usize;
    for _ in 0..num_leds {
        c.led()?;
    }
    let num_colors = c.u16le()? as usize;
    c.take(num_colors * 4)?;
    // Protocol 3 ends here (led_display_names/flags/display_name/configuration
    // arrive only at v5/v6; we negotiate 3).
    c.done()?;

    let total_leds: usize = zones.iter().map(|z| z.leds).sum();
    if total_leds != num_leds {
        return Err(WireError::Protocol(format!(
            "zone LEDs ({total_leds}) != LED entries ({num_leds})"
        )));
    }

    Ok(Controller {
        name: if name.is_empty() {
            "OpenRGB device".into()
        } else {
            name
        },
        zones,
    })
}

/// Protocol-3 mode block (RGBController::SetModeDescription): u16 name len +
/// string, then 12 u32s — value, flags, speed_min, speed_max, brightness_min,
/// brightness_max (v3+), colors_min, colors_max, speed, brightness (v3+),
/// direction, color_mode — then u16 num_colors + 4 bytes each.
fn skip_mode(c: &mut Cursor) -> WireResult<()> {
    c.string()?;
    for _ in 0..12 {
        c.u32le()?;
    }
    let n = c.u16le()? as usize;
    c.take(n * 4)?;
    Ok(())
}

/// Protocol-3 zone block (RGBController::SetZoneDescription): u16 name len +
/// string, u32 type, u32 leds_min, u32 leds_max, u32 leds_count, u16
/// matrix_len (0 = no matrix; else 8 + height*width*4), then the matrix:
/// u32 height, u32 width, height*width u32 LED indices. (Segments arrive at
/// protocol 4; we negotiate 3.)
fn parse_zone(c: &mut Cursor) -> WireResult<Zone> {
    let name = c.string()?;
    let _zone_type = c.u32le()?;
    let _leds_min = c.u32le()?;
    let _leds_max = c.u32le()?;
    let leds = c.u32le()? as usize;
    let matrix_len = c.u16le()? as usize;
    let mut matrix = None;
    if matrix_len > 0 {
        let height = c.u32le()? as u8;
        let width = c.u32le()? as u8;
        c.take(height as usize * width as usize * 4)?; // key -> LED index map
        matrix = Some((height, width));
    }
    Ok(Zone { name, leds, matrix })
}

// --- Discovery ---------------------------------------------------------------------

/// Query the OpenRGB server at `addr` ("host:port") and translate its
/// controllers into discovered devices.
///
/// No server running is the normal case on most machines: connect errors map
/// to `Ok(vec![])`, not an error.
pub fn discover_bridge(addr: &str) -> Result<Vec<DiscoveredDevice>> {
    let Ok(addr) = addr.parse::<SocketAddr>() else {
        return Err(WireError::Protocol(format!("bad bridge address: {addr}")).into());
    };
    let mut stream = match TcpStream::connect_timeout(&addr, IO_TIMEOUT) {
        Ok(s) => s,
        Err(_) => return Ok(vec![]), // no server: normal, not an error
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    // Version handshake (OpenRGBSDK.md): offer 3; the server replies with its
    // highest version and both speak min(). A protocol-0 server never replies
    // (the read times out) — assume 0. We still parse at most v3, so anything
    // at or above 3 is treated as 3.
    send_packet(&mut stream, 0, PKT_REQUEST_PROTOCOL_VERSION, &PROTOCOL_VERSION.to_le_bytes())?;
    let version = match recv_packet(&mut stream) {
        Ok(p) if p.pkt_id == PKT_REQUEST_PROTOCOL_VERSION => {
            p.payload.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
        }
        _ => 0,
    }
    .min(PROTOCOL_VERSION);

    // Controller count (indexed scheme: count reply is one u32 at < v6; the
    // v6 unique-ID list is irrelevant since we negotiate 3).
    send_packet(&mut stream, 0, PKT_REQUEST_CONTROLLER_COUNT, &[])?;
    let p = recv_matching(&mut stream, PKT_REQUEST_CONTROLLER_COUNT)?;
    let Some(count_bytes) = p.payload.get(..4) else {
        return Err(WireError::Protocol("controller count reply too short".into()).into());
    };
    let count = u32::from_le_bytes(count_bytes.try_into().unwrap()) as usize;

    let mut out = Vec::with_capacity(count);
    for idx in 0..count {
        // Request payload: negotiated protocol version (u32) — absent at v0.
        let version_bytes = version.to_le_bytes();
        let param: &[u8] = if version > 0 { &version_bytes } else { &[] };
        send_packet(&mut stream, idx as u32, PKT_REQUEST_CONTROLLER_DATA, &param)?;
        let p = recv_matching(&mut stream, PKT_REQUEST_CONTROLLER_DATA)?;
        // Response = u32 data_size (self-inclusive) + device data block
        // (OpenRGBSDK.md "NET_PACKET_ID_REQUEST_CONTROLLER_DATA").
        let data_size = p
            .payload
            .get(..4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| WireError::Protocol("controller data reply too short".into()))?;
        if data_size as usize != p.payload.len() {
            return Err(WireError::Protocol(format!(
                "controller data size field {data_size} != packet size {}",
                p.payload.len()
            ))
            .into());
        }
        let ctrl = parse_controller(&p.payload[4..])?;
        out.push(DiscoveredDevice {
            info: DeviceInfo {
                id: format!("openrgb:{idx}"),
                name: ctrl.name,
                vendor: "OpenRGB".into(),
                vid: 0,
                pid: 0,
                kind: crate::DriverKind::Bridge,
                zones: ctrl.zones,
                note: None,
            },
            serial: None,
            // Not a HID path; nothing in the bridge ever opens it.
            path: CString::new("openrgb").unwrap(),
        });
    }
    Ok(out)
}

// --- Backend -------------------------------------------------------------------------

/// Stateless bridge backend: opens a fresh TCP connection per command call.
/// Boring and robust — no reconnection logic, no shared state to go stale.
pub struct BridgeBackend {
    info: DeviceInfo,
    /// Controller index on the OpenRGB server (from the `openrgb:{idx}` id).
    device_idx: u32,
    addr: String,
}

impl BridgeBackend {
    pub fn open(dev: &DiscoveredDevice) -> Result<Self> {
        let idx = dev
            .info
            .id
            .strip_prefix("openrgb:")
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| {
                crate::Error::from(WireError::Protocol(format!(
                    "bad bridge device id: {}",
                    dev.info.id
                )))
            })?;
        Ok(Self {
            info: dev.info.clone(),
            device_idx: idx,
            addr: "127.0.0.1:6742".into(),
        })
    }

    /// One UPDATELEDS packet on a fresh connection. Payload per
    /// RGBController_Network::CreateUpdateLEDsPacket / SetColorDescription:
    /// u32 data_size (self-inclusive), u16 num_colors, then one u32 color per
    /// LED, 0x00BBGGRR -> little-endian wire bytes [R, G, B, 0].
    fn send_colors(&self, colors: &[Rgb]) -> Result<()> {
        if colors.len() > u16::MAX as usize {
            return Err(WireError::Protocol("too many LEDs for one packet".into()).into());
        }
        let mut stream =
            TcpStream::connect(&self.addr).map_err(|e| WireError::Io(e.to_string()))?;
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

        let mut body = Vec::with_capacity(4 + 2 + colors.len() * 4);
        body.extend_from_slice(&((4 + 2 + colors.len() * 4) as u32).to_le_bytes());
        body.extend_from_slice(&(colors.len() as u16).to_le_bytes());
        for c in colors {
            body.extend_from_slice(&[c.r, c.g, c.b, 0]);
        }
        send_packet(&mut stream, self.device_idx, PKT_RGBCONTROLLER_UPDATELEDS, &body)?;
        Ok(())
    }
}

impl DriverBackend for BridgeBackend {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn set_static(&mut self, color: Rgb) -> Result<()> {
        let leds: usize = self.info.zones.iter().map(|z| z.leds).sum();
        self.send_colors(&vec![color; leds])
    }

    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        let expected: usize = self.info.zones.iter().map(|z| z.leds).sum();
        if colors.len() != expected {
            return Err(WireError::Protocol(format!(
                "write_frame: got {} colors, device has {expected} LEDs",
                colors.len()
            ))
            .into());
        }
        self.send_colors(colors)
    }

    fn supports_per_led(&self) -> bool {
        true
    }
}

// --- Tests: std::net mock server, assert-based, no frameworks ------------------------
//
// The mock speaks the real server side (packet framing, version reply,
// controller-count reply, a protocol-3 controller block), so it doubles as
// executable documentation of the wire format. Ephemeral ports only — tests
// run in parallel.

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    // -- mock server side ------------------------------------------------------

    fn pkt(dev_id: u32, pkt_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut b = Vec::with_capacity(HDR + payload.len());
        b.extend_from_slice(b"ORGB");
        b.extend_from_slice(&dev_id.to_le_bytes());
        b.extend_from_slice(&pkt_id.to_le_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(payload);
        b
    }

    fn put_str16(out: &mut Vec<u8>, s: &str) {
        let bytes = s.as_bytes();
        out.extend_from_slice(&((bytes.len() + 1) as u16).to_le_bytes());
        out.extend_from_slice(bytes);
        out.push(0);
    }

    fn read_pkt(stream: &mut TcpStream) -> std::io::Result<(u32, u32, Vec<u8>)> {
        let mut hdr = [0u8; HDR];
        stream.read_exact(&mut hdr)?;
        assert_eq!(&hdr[..4], b"ORGB", "client sent bad magic");
        let dev_id = u32::from_le_bytes(hdr[4..8].try_into().unwrap());
        let pkt_id = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
        let size = u32::from_le_bytes(hdr[12..16].try_into().unwrap()) as usize;
        let mut payload = vec![0u8; size];
        stream.read_exact(&mut payload)?;
        Ok((dev_id, pkt_id, payload))
    }

    /// Protocol-3 controller block for one fake keyboard: type keyboard(1),
    /// name/vendor/description/version/serial/location strings, 1 mode,
    /// 2 zones (a 2x4 matrix with 8 LEDs + a 1-LED zone), 9 LEDs, 9 colors.
    fn controller_block() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes()); // type: keyboard
        // Field order must mirror SetDeviceDescription: name THEN vendor.
        put_str16(&mut b, "Mock Keyboard");
        put_str16(&mut b, "Mock Vendor");
        put_str16(&mut b, "mock description");
        put_str16(&mut b, "0.1-mock");
        put_str16(&mut b, "SER123");
        put_str16(&mut b, "mock:location");
        b.extend_from_slice(&1u16.to_le_bytes()); // num_modes
        b.extend_from_slice(&0u32.to_le_bytes()); // active_mode
        // mode: name + 12 u32 fields + u16 num_colors + colors
        put_str16(&mut b, "Direct");
        for v in [
            1u32,  // value
            0,     // flags
            0,     // speed_min
            255,   // speed_max
            0,     // brightness_min (v3+)
            255,   // brightness_max (v3+)
            0,     // colors_min
            1,     // colors_max
            255,   // speed
            255,   // brightness (v3+)
            0,     // direction
            0,     // color_mode
        ] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&0u16.to_le_bytes()); // mode num_colors
        b.extend_from_slice(&2u16.to_le_bytes()); // num_zones
        // zone 0: matrix zone, 2 rows x 4 cols, 8 LEDs
        put_str16(&mut b, "mock matrix");
        b.extend_from_slice(&1u32.to_le_bytes()); // type: matrix
        b.extend_from_slice(&8u32.to_le_bytes()); // leds_min
        b.extend_from_slice(&8u32.to_le_bytes()); // leds_max
        b.extend_from_slice(&8u32.to_le_bytes()); // leds_count
        // matrix: u16 len = 8 + h*w*4 = 8 + 32 = 40; h, w, h*w u32 indices
        b.extend_from_slice(&40u16.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes()); // height
        b.extend_from_slice(&4u32.to_le_bytes()); // width
        for i in 0..8u32 {
            b.extend_from_slice(&i.to_le_bytes());
        }
        // zone 1: single-LED zone, no matrix
        put_str16(&mut b, "logo");
        b.extend_from_slice(&3u32.to_le_bytes()); // type: single
        b.extend_from_slice(&1u32.to_le_bytes()); // leds_min
        b.extend_from_slice(&1u32.to_le_bytes()); // leds_max
        b.extend_from_slice(&1u32.to_le_bytes()); // leds_count
        b.extend_from_slice(&0u16.to_le_bytes()); // matrix_len 0
        b.extend_from_slice(&9u16.to_le_bytes()); // num_leds
        for i in 0..9u32 {
            put_str16(&mut b, &format!("LED {i}"));
            b.extend_from_slice(&i.to_le_bytes()); // led value (present <= v5)
        }
        b.extend_from_slice(&9u16.to_le_bytes()); // num_colors
        for _ in 0..9 {
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        b
    }

    /// Serve one client connection: reply to handshake/count/data requests,
    /// forward any UPDATELEDS payload to `tx`, return on EOF or UPDATELEDS.
    /// Returning lets the accept loop handle the backend's next fresh
    /// connection (the backend is stateless: one connection per command).
    fn serve_conn(stream: &mut TcpStream, tx: &std::sync::mpsc::Sender<(u32, Vec<u8>)>) {
        loop {
            let Ok((dev_id, pkt_id, payload)) = read_pkt(stream) else {
                return; // client closed; normal between commands
            };
            match pkt_id {
                PKT_REQUEST_PROTOCOL_VERSION => {
                    let reply = pkt(0, PKT_REQUEST_PROTOCOL_VERSION, &3u32.to_le_bytes());
                    let _ = stream.write_all(&reply);
                }
                PKT_REQUEST_CONTROLLER_COUNT => {
                    let reply = pkt(0, PKT_REQUEST_CONTROLLER_COUNT, &1u32.to_le_bytes());
                    let _ = stream.write_all(&reply);
                }
                PKT_REQUEST_CONTROLLER_DATA => {
                    // Reply = u32 data_size (self-inclusive) + device block.
                    let block = controller_block();
                    let mut body = Vec::with_capacity(4 + block.len());
                    body.extend_from_slice(&((4 + block.len()) as u32).to_le_bytes());
                    body.extend_from_slice(&block);
                    let _ = stream.write_all(&pkt(dev_id, PKT_REQUEST_CONTROLLER_DATA, &body));
                }
                PKT_RGBCONTROLLER_UPDATELEDS => {
                    let _ = tx.send((dev_id, payload));
                    return;
                }
                other => panic!("mock: unexpected packet id {other}"),
            }
        }
    }

    /// Accept connections in a loop (one per client call — the backend is
    /// stateless), serving each until EOF or UPDATELEDS.
    fn spawn_mock(tx: std::sync::mpsc::Sender<(u32, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                serve_conn(&mut stream, &tx);
            }
        });
        addr
    }

    #[test]
    fn discovery_translates_controllers() {
        let (tx, _rx) = std::sync::mpsc::channel(); // discovery never sends updates
        let addr = spawn_mock(tx);
        let devs = discover_bridge(&addr).unwrap();
        assert_eq!(devs.len(), 1);
        let d = &devs[0];
        assert_eq!(d.info.id, "openrgb:0");
        assert_eq!(d.info.vendor, "OpenRGB");
        assert_eq!(d.info.name, "Mock Keyboard");
        assert_eq!(d.info.kind, crate::DriverKind::Bridge);
        assert_eq!(d.serial, None);
        assert_eq!(d.path.to_bytes(), b"openrgb");
        assert_eq!(d.info.zones.len(), 2);
        assert_eq!(d.info.zones[0].name, "mock matrix");
        assert_eq!(d.info.zones[0].leds, 8);
        assert_eq!(d.info.zones[0].matrix, Some((2, 4))); // (rows, cols)
        assert_eq!(d.info.zones[1].name, "logo");
        assert_eq!(d.info.zones[1].leds, 1);
        assert_eq!(d.info.zones[1].matrix, None);
    }

    #[test]
    fn discovery_refused_is_empty() {
        // Port 1 on 127.0.0.1: nothing listens; refused is the normal case.
        let devs = discover_bridge("127.0.0.1:1").unwrap();
        assert!(devs.is_empty());
    }

    #[test]
    fn set_static_hits_wire_and_frame_is_validated() {
        let (tx, rx) = std::sync::mpsc::channel::<(u32, Vec<u8>)>();
        let addr = spawn_mock(tx);

        // Discovery on connection 1...
        let devs = discover_bridge(&addr).unwrap();
        let mut backend = BridgeBackend::open(&devs[0]).unwrap();
        assert!(backend.supports_per_led());
        // Point the stateless backend at the mock (default is 127.0.0.1:6742).
        backend.addr = addr.clone();

        // ...set_static on connection 2.
        backend.set_static(Rgb { r: 255, g: 128, b: 1 }).unwrap();
        let (dev_id, payload) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("mock never saw the update packet");
        assert_eq!(dev_id, 0);
        // u32 data_size (self-inclusive) + u16 count + 9 LEDs x 4 bytes
        assert_eq!(payload.len(), 4 + 2 + 9 * 4);
        assert_eq!(&payload[..4], &42u32.to_le_bytes());
        assert_eq!(&payload[4..6], &9u16.to_le_bytes());
        assert_eq!(&payload[6..10], &[255, 128, 1, 0]); // wire order R,G,B,0

        // Wrong-length frames are rejected before any wire I/O.
        let err = backend.write_frame(&[Rgb { r: 0, g: 0, b: 0 }; 8]).unwrap_err();
        assert!(err.to_string().contains("8 colors"), "got: {err}");
    }

    #[test]
    fn parse_controller_is_strict() {
        let block = controller_block();
        // Well-formed block parses fully and sums LEDs.
        let c = parse_controller(&block).unwrap();
        assert_eq!(c.zones.iter().map(|z| z.leds).sum::<usize>(), 9);
        assert_eq!(c.zones.len(), 2);
        // Trailing byte -> rejected (no silent misalignment).
        let mut trailing = block.clone();
        trailing.push(0);
        assert!(parse_controller(&trailing).is_err());
        // Truncated / empty -> rejected.
        assert!(parse_controller(&block[..block.len() - 1]).is_err());
        assert!(parse_controller(&[]).is_err());
    }
}
