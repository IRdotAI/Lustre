/// Tauri commands — these are the functions the frontend can call.
/// Each one is tagged with #[tauri::command] so Tauri generates the
/// JS bindings automatically.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::bank_detect;
use crate::device::{self, ConnectionMode, DeviceInfo};
use crate::flasher;
use crate::gallery::{self, GallerySkin};
use crate::image_skin::PreparedSkin;
use crate::scanner::{self, SavedCard};
use crate::wallet_backup;

// global scan state so we can stop it from the frontend
static SCAN_FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

fn get_scan_flag() -> Arc<AtomicBool> {
    Arc::clone(SCAN_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))))
}

#[derive(Serialize)]
pub struct DeviceResult {
    pub devices: Vec<DeviceInfo>,
    pub error: Option<String>,
}

#[tauri::command]
pub fn get_devices() -> DeviceResult {
    match device::list_connected_devices() {
        Ok(devices) => DeviceResult { devices, error: None },
        Err(e) => DeviceResult { devices: vec![], error: Some(format!("{:#}", e)) },
    }
}

#[tauri::command]
pub fn check_apple_support() -> Result<String, String> {
    crate::apple::verify_support().map_err(|e| format!("{:#}", e))
}

#[tauri::command]
pub fn get_saved_cards() -> Vec<SavedCard> {
    scanner::load_saved_cards()
}

#[tauri::command]
pub fn rename_card(hash: String, new_name: String) -> Vec<SavedCard> {
    let mut cards = scanner::load_saved_cards();
    if let Some(card) = cards.iter_mut().find(|c| c.hash == hash) {
        card.name = new_name;
    }
    scanner::save_saved_cards(&cards);
    cards
}

#[tauri::command]
pub fn delete_card(hash: String) -> Vec<SavedCard> {
    let mut cards = scanner::load_saved_cards();
    cards.retain(|c| c.hash != hash);
    scanner::save_saved_cards(&cards);
    cards
}

#[derive(Serialize, Clone)]
pub struct ScanUpdate {
    pub kind: String, // "log", "card", "done", "error"
    pub message: String,
    pub hash: Option<String>,
    pub name: Option<String>,
}

#[tauri::command]
pub fn start_scan(
    udid: Option<String>,
    window: tauri::Window,
) -> Result<(), String> {
    let flag = get_scan_flag();

    // if already scanning, stop the old one first
    if flag.load(Ordering::Relaxed) {
        flag.store(false, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    flag.store(true, Ordering::SeqCst);
    let stop = Arc::clone(&flag);

    let udid_clone = udid.clone();
    std::thread::spawn(move || {
        let result = scanner::scan_syslog_for_cards(
            udid_clone.as_deref(),
            ConnectionMode::Auto,
            stop,
            |hash, name| {
                let _ = window.emit("scan-update", ScanUpdate {
                    kind: "card".into(),
                    message: format!("Found: {}", if name.is_empty() { &hash } else { &name }),
                    hash: Some(hash),
                    name: Some(name),
                });
            },
            |msg| {
                let _ = window.emit("scan-update", ScanUpdate {
                    kind: "log".into(),
                    message: msg,
                    hash: None,
                    name: None,
                });
            },
        );

        match result {
            Ok(_) => {
                let _ = window.emit("scan-update", ScanUpdate {
                    kind: "done".into(),
                    message: "Scan completed.".into(),
                    hash: None,
                    name: None,
                });
            }
            Err(e) => {
                let _ = window.emit("scan-update", ScanUpdate {
                    kind: "error".into(),
                    message: format!("{:#}", e),
                    hash: None,
                    name: None,
                });
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub fn stop_scan() {
    get_scan_flag().store(false, Ordering::SeqCst);
}

#[derive(Deserialize)]
pub struct FlashRequest {
    pub udid: String,
    pub card_hash: String,
    pub image_path: String,
}

#[tauri::command]
pub fn flash_skin(
    req: FlashRequest,
    window: tauri::Window,
) -> Result<(), String> {
    let img_data = std::fs::read(&req.image_path)
        .map_err(|e| format!("Can't read image: {}", e))?;

    let img = image::load_from_memory(&img_data)
        .map_err(|e| format!("Invalid image: {}", e))?;

    let skin = PreparedSkin::from_image_with_focus(img, 0.5, 0.5)
        .map_err(|e| format!("Skin prep failed: {}", e))?;

    std::thread::spawn(move || {
        let result = flasher::flash_wallet_skin(
            &req.udid,
            ConnectionMode::Auto,
            &req.card_hash,
            &skin.png,
            &skin.pdf,
            |step, total, msg| {
                let _ = window.emit("flash-progress", serde_json::json!({
                    "step": step, "total": total, "message": msg,
                }));
            },
            |msg| {
                let _ = window.emit("flash-log", msg);
            },
        );

        if let Err(e) = result {
            let _ = window.emit("flash-error", format!("{:#}", e));
        }
    });

    Ok(())
}

#[derive(Deserialize)]
pub struct BatchFlashRequest {
    pub udid: String,
    pub card_hashes: Vec<String>,
    pub image_path: String,
}

#[tauri::command]
pub fn flash_skin_batch(
    req: BatchFlashRequest,
    window: tauri::Window,
) -> Result<(), String> {
    let img_data = std::fs::read(&req.image_path)
        .map_err(|e| format!("Can't read image: {}", e))?;
    let img = image::load_from_memory(&img_data)
        .map_err(|e| format!("Invalid image: {}", e))?;
    let skin = PreparedSkin::from_image_with_focus(img, 0.5, 0.5)
        .map_err(|e| format!("Skin prep failed: {}", e))?;

    std::thread::spawn(move || {
        let total = req.card_hashes.len();
        for (idx, hash) in req.card_hashes.iter().enumerate() {
            let _ = window.emit("batch-progress", serde_json::json!({
                "current": idx + 1, "total": total,
                "message": format!("Flashing card {} of {}...", idx + 1, total),
            }));

            let result = flasher::flash_wallet_skin(
                &req.udid,
                ConnectionMode::Auto,
                hash,
                &skin.png,
                &skin.pdf,
                |_step, _total, _msg| {},
                |msg| { let _ = window.emit("flash-log", msg); },
            );

            if let Err(e) = result {
                let _ = window.emit("batch-error", serde_json::json!({
                    "hash": hash, "error": format!("{:#}", e),
                }));
            }
        }

        let _ = window.emit("batch-done", serde_json::json!({
            "total": total,
            "message": format!("Batch flash complete — {} cards updated!", total),
        }));
    });

    Ok(())
}

#[tauri::command]
pub fn restore_original(
    udid: String,
    card_hash: String,
    window: tauri::Window,
) -> Result<(), String> {
    std::thread::spawn(move || {
        let result = flasher::restore_wallet_original(
            &udid,
            ConnectionMode::Auto,
            &card_hash,
            |step, total, msg| {
                let _ = window.emit("flash-progress", serde_json::json!({
                    "step": step, "total": total, "message": msg,
                }));
            },
            |msg| { let _ = window.emit("flash-log", msg); },
        );
        if let Err(e) = result {
            let _ = window.emit("flash-error", format!("{:#}", e));
        }
    });
    Ok(())
}

#[tauri::command]
pub fn check_backup_exists(udid: String, card_hash: String) -> bool {
    wallet_backup::backup_exists(&udid, &card_hash)
}

#[tauri::command]
pub fn get_gallery_skins() -> Vec<GallerySkin> {
    gallery::list_gallery_skins()
}

#[tauri::command]
pub fn detect_bank_name(
    udid: String,
    card_hash: String,
) -> Result<Option<String>, String> {
    bank_detect::detect_bank_for_card(&udid, ConnectionMode::Auto, &card_hash)
        .map_err(|e| format!("{:#}", e))
}
