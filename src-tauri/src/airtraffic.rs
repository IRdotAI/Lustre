use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::apple::{ATHostConnectionRef, get_apple_libraries};
use crate::device::{DeviceTransport, ensure_transport_available};

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        hAlgorithm: *mut std::ffi::c_void,
        pbBuffer: *mut u8,
        cbBuffer: u32,
        dwFlags: u32,
    ) -> i32;
}

fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    unsafe { let _ = BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 16, 2); }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
        bytes[6], bytes[7], bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

pub fn sync_assets_via_airtraffic<L>(
    udid: &str,
    transport: DeviceTransport,
    assets: &[(&str, &str)],
    mut log: L,
) -> Result<()>
where L: FnMut(&str),
{
    let udid_owned = udid.to_string();
    let assets_owned: Vec<(String, String)> = assets.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();

    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::spawn(move || {
        let refs: Vec<(&str, &str)> = assets_owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let tx_log = tx.clone();
        let res = sync_internal(&udid_owned, transport, &refs, move |msg| { let _ = tx_log.send(SyncEvent::Log(msg.to_string())); });
        let _ = tx.send(SyncEvent::Done(res));
    });

    let base_timeout = if transport == DeviceTransport::Wifi { 120 } else { 60 };
    let total_timeout = base_timeout.max(assets.len() as u64 * 2);
    let start = std::time::Instant::now();

    loop {
        let elapsed = start.elapsed();
        if elapsed >= Duration::from_secs(total_timeout) {
            bail!("AirTraffic timed out ({}s). Unlock iPhone, open Books app, close iTunes.", total_timeout);
        }
        match rx.recv_timeout(Duration::from_secs(total_timeout) - elapsed) {
            Ok(SyncEvent::Log(msg)) => log(&msg),
            Ok(SyncEvent::Done(res)) => return res,
            Err(_) => bail!("AirTraffic timed out ({}s).", total_timeout),
        }
    }
}

enum SyncEvent {
    Log(String),
    Done(Result<()>),
}

fn sync_internal<L>(
    udid: &str,
    transport: DeviceTransport,
    assets: &[(&str, &str)],
    mut log: L,
) -> Result<()>
where L: FnMut(&str),
{
    ensure_transport_available(udid, transport)?;
    log(&format!("Connecting to AirTraffic over {}...", transport.label()));

    let libs = get_apple_libraries()?;
    let cf_udid = libs.create_cf_string(udid)?;
    let conn: ATHostConnectionRef = unsafe { (libs.at_host_connection_create)(cf_udid.raw) };
    if conn.is_null() { bail!("ATHostConnectionCreate failed"); }

    let retry_scale = if transport == DeviceTransport::Wifi { 2 } else { 1 };

    let mut run_sync = || -> Result<()> {
        log("Waiting for SyncAllowed (keep iPhone unlocked)...");
        let mut sync_ok = false;
        for _ in 0..(15 * retry_scale) {
            let msg = unsafe { (libs.at_host_connection_read_message)(conn) };
            if msg.is_null() { sleep(Duration::from_millis(150)); continue; }
            let name = libs.to_rust_string(unsafe { (libs.at_cf_message_get_name)(msg) });
            unsafe { (libs.cf_release)(msg); }
            if name == "SyncAllowed" { sync_ok = true; break; }
        }
        if !sync_ok { bail!("SyncAllowed not received — unlock iPhone and open Books app"); }

        log("Handshaking...");
        // host info
        let mut hi: HashMap<String, plist::Value> = HashMap::new();
        hi.insert("Type".into(), plist::Value::String("iTunes".into()));
        hi.insert("Version".into(), plist::Value::String("13.7.0.161".into()));
        hi.insert("MacOSVersion".into(), plist::Value::String("Windows NT 10.0".into()));
        hi.insert("SyncHostName".into(), plist::Value::String("lustre".into()));
        hi.insert("LibraryID".into(), plist::Value::String(uuid_v4()));
        hi.insert("SyncedDataclasses".into(), plist::Value::Array(vec![plist::Value::String("Book".into())]));
        hi.insert("SyncedAssetTypes".into(), plist::Value::Array(vec![plist::Value::String("Book".into())]));
        hi.insert("Wakeable".into(), plist::Value::Boolean(false));

        let mut hi_bytes = Vec::new();
        plist::to_writer_binary(&mut hi_bytes, &plist::Value::Dictionary(hi.into_iter().collect()))?;
        let cf_hi = libs.create_cf_plist_from_bytes(&hi_bytes)?;
        unsafe { (libs.at_host_connection_send_host_info)(conn, cf_hi.raw); }
        sleep(Duration::from_millis(200));

        // sync request
        let mut dc_bytes = Vec::new();
        plist::to_writer_binary(&mut dc_bytes, &plist::Value::Array(vec![plist::Value::String("Book".into())]))?;
        let cf_dc = libs.create_cf_plist_from_bytes(&dc_bytes)?;

        let mut anch_bytes = Vec::new();
        plist::to_writer_binary(&mut anch_bytes, &plist::Value::Dictionary(HashMap::<String, plist::Value>::new().into_iter().collect()))?;
        let cf_anch = libs.create_cf_plist_from_bytes(&anch_bytes)?;

        unsafe { (libs.at_host_connection_send_sync_request)(conn, cf_dc.raw, cf_anch.raw, cf_hi.raw); }

        log("Waiting for ReadyForSync...");
        let mut ready = false;
        for _ in 0..(20 * retry_scale) {
            let msg = unsafe { (libs.at_host_connection_read_message)(conn) };
            if msg.is_null() { sleep(Duration::from_millis(150)); continue; }
            let name = libs.to_rust_string(unsafe { (libs.at_cf_message_get_name)(msg) });
            unsafe { (libs.cf_release)(msg); }
            if name == "ReadyForSync" { ready = true; break; }
        }
        if !ready { bail!("ReadyForSync not received"); }

        // metadata sync finished
        let mut st: HashMap<String, plist::Value> = HashMap::new();
        st.insert("Book".into(), plist::Value::Integer(1.into()));
        let mut st_bytes = Vec::new();
        plist::to_writer_binary(&mut st_bytes, &plist::Value::Dictionary(st.into_iter().collect()))?;
        let cf_st = libs.create_cf_plist_from_bytes(&st_bytes)?;
        unsafe { (libs.at_host_connection_send_metadata_sync_finished)(conn, cf_st.raw, cf_anch.raw); }

        // read asset manifest
        let cf_key = libs.create_cf_string("AssetManifest")?;
        let mut manifest_val: Option<plist::Value> = None;
        for _ in 0..(30 * retry_scale) {
            let msg = unsafe { (libs.at_host_connection_read_message)(conn) };
            if msg.is_null() { sleep(Duration::from_millis(150)); continue; }
            let name = libs.to_rust_string(unsafe { (libs.at_cf_message_get_name)(msg) });
            if name == "AssetManifest" {
                let param = unsafe { (libs.at_cf_message_get_param)(msg, cf_key.raw) };
                if !param.is_null() {
                    if let Ok(bytes) = libs.cf_plist_to_bytes(param) {
                        manifest_val = plist::Value::from_reader(std::io::Cursor::new(bytes)).ok();
                    }
                }
                unsafe { (libs.cf_release)(msg); }
                break;
            } else if name == "SyncFailed" || name == "SyncFinished" {
                unsafe { (libs.cf_release)(msg); }
                bail!("AirTraffic: {}", name);
            }
            unsafe { (libs.cf_release)(msg); }
        }

        let manifest = manifest_val.context("AssetManifest not received")?;
        let book_entries = manifest.as_dictionary().and_then(|d| d.get("Book")).and_then(|v| v.as_array())
            .context("No Book list in manifest")?;

        let mut downloads = Vec::new();
        for entry in book_entries {
            if let Some(dict) = entry.as_dictionary() {
                if dict.get("IsDownload").and_then(|b| b.as_boolean()).unwrap_or(false) {
                    if let Some(id) = dict.get("AssetID").and_then(|s| s.as_string()) {
                        downloads.push(id.to_string());
                    }
                }
            }
        }

        for (ident, _) in assets {
            if !downloads.iter().any(|d| d == ident) {
                bail!("Asset '{}' missing from manifest", ident);
            }
        }

        // dispatch AssetCompleted
        let cf_book = libs.create_cf_string("Book")?;
        for (idx, (ident, dest)) in assets.iter().enumerate() {
            let cf_ident = libs.create_cf_string(ident)?;
            let cf_dest = libs.create_cf_string(dest)?;
            unsafe { (libs.at_host_connection_send_asset_completed)(conn, cf_ident.raw, cf_book.raw, cf_dest.raw); }
            if idx + 1 < assets.len() {
                sleep(Duration::from_millis(if idx == 0 { 400 } else { 60 }));
            }
        }

        sleep(Duration::from_millis(2000));
        Ok(())
    };

    let result = run_sync();
    unsafe { (libs.at_host_connection_release)(conn); }
    result
}
