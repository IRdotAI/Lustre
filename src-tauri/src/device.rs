use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::apple::{
    AMDServiceConnectionRef, AMDeviceRef, AppleLibraries, get_apple_libraries,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DeviceTransport {
    Usb,
    Wifi,
    Other,
}

impl DeviceTransport {
    fn from_usbmux(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "usb" => Self::Usb,
            "network" => Self::Wifi,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Wifi => "WiFi",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConnectionMode {
    Auto,
    Usb,
    Wifi,
}

impl ConnectionMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Usb, Self::Wifi];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (USB preferred)",
            Self::Usb => "USB only",
            Self::Wifi => "WiFi only",
        }
    }

    fn accepts(self, transport: DeviceTransport) -> bool {
        match self {
            Self::Auto => true,
            Self::Usb => transport == DeviceTransport::Usb,
            Self::Wifi => transport == DeviceTransport::Wifi,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub ios_version: String,
    pub build_version: String,
    pub transports: Vec<DeviceTransport>,
}

impl DeviceInfo {
    pub fn has_transport(&self, transport: DeviceTransport) -> bool {
        self.transports.contains(&transport)
    }

    pub fn supports(&self, mode: ConnectionMode) -> bool {
        self.transports.iter().copied().any(|t| mode.accepts(t))
    }

    pub fn transport_summary(&self) -> String {
        self.transports.iter().map(|t| t.label()).collect::<Vec<_>>().join(" + ")
    }
}

impl std::fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f, "{} ({}, iOS {} [{}]) [{}]",
            self.name, self.product_type, self.ios_version,
            self.build_version, self.transport_summary(),
        )
    }
}

#[derive(Clone)]
pub struct UsbmuxDeviceEntry {
    pub udid: String,
    pub transport: DeviceTransport,
    pub properties_plist: Vec<u8>,
}

/// Talk to the local usbmuxd daemon on port 27015 and get a list of
/// all connected iOS devices with their serial numbers + transport type.
pub fn query_usbmux_devices() -> Result<Vec<UsbmuxDeviceEntry>> {
    let addr: SocketAddr = "127.0.0.1:27015".parse().unwrap();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .context("Can't reach usbmuxd at 127.0.0.1:27015 — is iTunes or Apple Mobile Device Support running?")?;

    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    // build a ListDevices request
    let mut req_dict = HashMap::new();
    req_dict.insert("MessageType".to_string(), plist::Value::String("ListDevices".to_string()));
    req_dict.insert("ClientVersionString".to_string(), plist::Value::String("lustre".to_string()));
    req_dict.insert("ProgName".to_string(), plist::Value::String("lustre".to_string()));

    let mut plist_bytes = Vec::new();
    plist::to_writer_xml(&mut plist_bytes, &plist::Value::Dictionary(req_dict.into_iter().collect()))
        .context("Failed to serialize ListDevices plist")?;

    // usbmux header: length (payload+16), version=1, type=8 (plist), tag=1
    let length = (plist_bytes.len() + 16) as u32;
    let mut header = Vec::with_capacity(16);
    header.extend_from_slice(&length.to_le_bytes());
    header.extend_from_slice(&1u32.to_le_bytes());
    header.extend_from_slice(&8u32.to_le_bytes());
    header.extend_from_slice(&1u32.to_le_bytes());

    stream.write_all(&header)?;
    stream.write_all(&plist_bytes)?;
    stream.flush()?;

    let mut resp_header = [0u8; 16];
    stream.read_exact(&mut resp_header)?;

    let resp_len = u32::from_le_bytes([resp_header[0], resp_header[1], resp_header[2], resp_header[3]]) as usize;
    if resp_len < 16 {
        bail!("Bogus usbmux response length: {}", resp_len);
    }

    let mut payload = vec![0u8; resp_len - 16];
    stream.read_exact(&mut payload)?;

    let val = plist::Value::from_reader(std::io::Cursor::new(payload))
        .context("Bad plist in usbmux response")?;
    let root_dict = val.as_dictionary().context("Expected dict in usbmux response")?;
    let device_list = root_dict.get("DeviceList").and_then(|v| v.as_array())
        .context("Missing DeviceList in usbmux response")?;

    let mut result = Vec::new();
    for entry in device_list {
        if let Some(d) = entry.as_dictionary() {
            if let Some(props_val) = d.get("Properties") {
                if let Some(props_dict) = props_val.as_dictionary() {
                    let serial = props_dict.get("SerialNumber")
                        .and_then(|v| v.as_string()).unwrap_or_default().to_string();
                    let conn_type = props_dict.get("ConnectionType")
                        .and_then(|v| v.as_string()).unwrap_or("USB");
                    let transport = DeviceTransport::from_usbmux(conn_type);

                    let mut props_binary = Vec::new();
                    plist::to_writer_binary(&mut props_binary, props_val)
                        .context("Failed to serialize device props")?;

                    result.push(UsbmuxDeviceEntry {
                        udid: serial,
                        transport,
                        properties_plist: props_binary,
                    });
                }
            }
        }
    }

    Ok(result)
}

/// Enumerate connected devices with full metadata (name, iOS version, etc).
pub fn list_connected_devices() -> Result<Vec<DeviceInfo>> {
    let libs = get_apple_libraries()?;
    let entries = query_usbmux_devices()?;
    let mut result = Vec::new();

    for entry in entries {
        let cf_props = libs.create_cf_plist_from_bytes(&entry.properties_plist)?;
        let dev = unsafe { (libs.am_device_create_from_properties)(cf_props.raw) };
        if dev.is_null() { continue; }

        let mut name = "iPhone".to_string();
        let mut product_type = "iPhone".to_string();
        let mut ios_version = "Unknown".to_string();
        let mut build_version = "Unknown".to_string();

        unsafe {
            if (libs.am_device_connect)(dev) == 0 {
                let _ = (libs.am_device_validate_pairing)(dev);

                // pull device properties one by one
                for (key, target) in [
                    ("DeviceName", &mut name),
                    ("ProductType", &mut product_type),
                    ("ProductVersion", &mut ios_version),
                    ("BuildVersion", &mut build_version),
                ] {
                    if let Ok(k) = libs.create_cf_string(key) {
                        let v = (libs.am_device_copy_value)(dev, ptr::null(), k.raw);
                        if !v.is_null() {
                            *target = libs.to_rust_string(v);
                            (libs.cf_release)(v);
                        }
                    }
                }
                (libs.am_device_disconnect)(dev);
            }
            (libs.cf_release)(dev);
        }

        merge_device_info(&mut result, DeviceInfo {
            udid: entry.udid,
            name, product_type, ios_version, build_version,
            transports: vec![entry.transport],
        });
    }

    Ok(result)
}

fn merge_device_info(devices: &mut Vec<DeviceInfo>, incoming: DeviceInfo) {
    if let Some(existing) = devices.iter_mut().find(|d| d.udid.eq_ignore_ascii_case(&incoming.udid)) {
        for t in incoming.transports {
            if !existing.transports.contains(&t) {
                existing.transports.push(t);
            }
        }
        existing.transports.sort_by_key(|t| transport_priority(*t));

        if existing.name == "iPhone" && incoming.name != "iPhone" { existing.name = incoming.name; }
        if existing.product_type == "iPhone" && incoming.product_type != "iPhone" { existing.product_type = incoming.product_type; }
        if existing.ios_version == "Unknown" && incoming.ios_version != "Unknown" { existing.ios_version = incoming.ios_version; }
        if existing.build_version == "Unknown" && incoming.build_version != "Unknown" { existing.build_version = incoming.build_version; }
        return;
    }
    devices.push(incoming);
}

fn transport_priority(t: DeviceTransport) -> u8 {
    match t {
        DeviceTransport::Usb => 0,
        DeviceTransport::Wifi => 1,
        DeviceTransport::Other => 2,
    }
}

fn ordered_candidates(
    mut entries: Vec<UsbmuxDeviceEntry>,
    target_udid: Option<&str>,
    mode: ConnectionMode,
) -> Vec<UsbmuxDeviceEntry> {
    entries.retain(|e| {
        target_udid.map(|t| e.udid.eq_ignore_ascii_case(t)).unwrap_or(true)
            && mode.accepts(e.transport)
    });
    entries.sort_by_key(|e| transport_priority(e.transport));
    entries
}

pub fn ensure_transport_available(udid: &str, transport: DeviceTransport) -> Result<()> {
    let ok = query_usbmux_devices()?.into_iter()
        .any(|e| e.udid.eq_ignore_ascii_case(udid) && e.transport == transport);
    if ok { return Ok(()); }
    bail!("{} is no longer available over {}. Reconnect and try again.", udid, transport.label())
}

#[allow(dead_code)]
pub struct ActiveDeviceSession {
    pub libs: Arc<AppleLibraries>,
    pub device: AMDeviceRef,
    pub udid: String,
    pub transport: DeviceTransport,
    connected: bool,
    session_started: bool,
}

impl Drop for ActiveDeviceSession {
    fn drop(&mut self) {
        unsafe {
            if self.session_started { (self.libs.am_device_stop_session)(self.device); }
            if self.connected { (self.libs.am_device_disconnect)(self.device); }
            if !self.device.is_null() { (self.libs.cf_release)(self.device); }
        }
    }
}

impl ActiveDeviceSession {
    pub fn open(target_udid: Option<&str>, mode: ConnectionMode) -> Result<Self> {
        let libs = get_apple_libraries()?;
        let entries = query_usbmux_devices()?;
        let candidates = ordered_candidates(entries, target_udid, mode);
        if candidates.is_empty() {
            let target = target_udid.unwrap_or("any paired iPhone");
            bail!("No {} connection available for {}.", mode.label(), target);
        }

        let mut failures = Vec::new();
        for entry in candidates {
            let transport = entry.transport;
            match Self::open_entry(Arc::clone(&libs), entry) {
                Ok(session) => return Ok(session),
                Err(err) => failures.push(format!("{}: {err:#}", transport.label())),
            }
        }
        bail!("Could not open device session. {}", failures.join("; "))
    }

    fn open_entry(libs: Arc<AppleLibraries>, entry: UsbmuxDeviceEntry) -> Result<Self> {
        let udid = entry.udid.clone();
        let transport = entry.transport;
        let cf_props = libs.create_cf_plist_from_bytes(&entry.properties_plist)?;

        unsafe {
            let device = (libs.am_device_create_from_properties)(cf_props.raw);
            if device.is_null() { bail!("AMDeviceCreateFromProperties returned null"); }

            let rc = (libs.am_device_connect)(device);
            if rc != 0 {
                (libs.cf_release)(device);
                bail!("AMDeviceConnect failed ({})", rc);
            }

            // try pairing if needed
            if (libs.am_device_is_paired)(device) == 0 {
                if transport == DeviceTransport::Wifi {
                    (libs.am_device_disconnect)(device);
                    (libs.cf_release)(device);
                    bail!("WiFi device not paired — connect over USB first and trust this computer");
                }
                (libs.am_device_pair)(device);
            }

            let mut vp = (libs.am_device_validate_pairing)(device);
            if vp != 0 && transport == DeviceTransport::Usb {
                (libs.am_device_pair)(device);
                vp = (libs.am_device_validate_pairing)(device);
            }
            if vp != 0 {
                (libs.am_device_disconnect)(device);
                (libs.cf_release)(device);
                bail!("Pairing validation failed ({}) — unlock your iPhone and try again", vp);
            }

            let ss = (libs.am_device_start_session)(device);
            if ss != 0 {
                (libs.am_device_disconnect)(device);
                (libs.cf_release)(device);
                bail!("AMDeviceStartSession failed ({})", ss);
            }

            Ok(Self { libs, device, udid, transport, connected: true, session_started: true })
        }
    }

    pub fn start_service(&self, service_name: &str) -> Result<AMDServiceConnectionRef> {
        let cf_name = self.libs.create_cf_string(service_name)?;
        let mut service_conn: AMDServiceConnectionRef = ptr::null_mut();
        let status = unsafe {
            (self.libs.am_device_secure_start_service)(
                self.device, cf_name.raw, ptr::null(), &mut service_conn,
            )
        };
        if status != 0 || service_conn.is_null() {
            bail!("Failed to start service '{}' (code {})", service_name, status);
        }
        Ok(service_conn)
    }
}
