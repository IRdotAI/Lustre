use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use regex::Regex;

use crate::device::{ActiveDeviceSession, ConnectionMode};

#[cfg(windows)]
unsafe extern "system" {
    fn setsockopt(s: usize, level: i32, optname: i32, optval: *const i8, optlen: i32) -> i32;
}

#[cfg(windows)]
const SOL_SOCKET: i32 = 0xffff;
#[cfg(windows)]
const SO_RCVTIMEO: i32 = 0x1006;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCard {
    pub hash: String,
    pub name: String,
}

pub fn get_cards_storage_path() -> PathBuf {
    let local = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    let dir = PathBuf::from(local).join("Lustre");
    let _ = fs::create_dir_all(&dir);
    dir.join("cards.json")
}

/// Check if something looks like a real Apple Wallet card hash.
/// These are base64-encoded SHA-1 (27-28 chars) or SHA-256 (43-44 chars).
pub fn is_valid_card_hash(h: &str) -> bool {
    let trimmed = h.trim_matches(['\'', '"']).trim_end_matches(['.', ',']);
    let len = trimmed.len();

    if len != 27 && len != 28 && len != 43 && len != 44 {
        return false;
    }

    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '-' | '_' | '=')) {
        return false;
    }

    // too many underscores/hyphens = probably a system identifier
    if trimmed.chars().filter(|&c| c == '_').count() > 1
        || trimmed.chars().filter(|&c| c == '-').count() > 2
    {
        return false;
    }

    // blocklist for system junk that keeps showing up in syslog
    let lower = trimmed.to_lowercase();
    let blocklist = [
        "mobileasset", "com_apple", "com.", "apple.", "curtain", "binder",
        "optimizer", "system", "uaf", "siri", "dialog", "planner",
        "linguistic", "timing", "model", "translation", "visual",
        "device", "override", "motion", "search",
    ];
    if blocklist.iter().any(|kw| lower.contains(kw)) {
        return false;
    }

    // '=' padding only at the end
    if let Some(pos) = trimmed.find('=') {
        if pos < len - 2 { return false; }
    }

    // normalize and decode
    let mut b64 = trimmed.replace('-', "+").replace('_', "/");
    while b64.len() % 4 != 0 { b64.push('='); }

    use base64::Engine;
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&b64) {
        if decoded.len() == 20 || decoded.len() == 32 {
            // reject trivial patterns
            if decoded.iter().all(|&b| b == decoded[0]) { return false; }
            let has_high = decoded.iter().any(|&b| b >= 128);
            let has_low = decoded.iter().any(|&b| b < 128);
            if !has_high || !has_low { return false; }

            let unique: HashSet<u8> = decoded.iter().copied().collect();
            if unique.len() < 12 { return false; }

            if DUMMY_HASHES.contains(&trimmed) || DUMMY_HASHES.iter().any(|d| d.trim_end_matches('=') == trimmed) {
                return false;
            }
            return true;
        }
    }
    false
}

pub fn load_saved_cards() -> Vec<SavedCard> {
    let path = get_cards_storage_path();
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(cards) = serde_json::from_str::<Vec<SavedCard>>(&content) {
            let valid: Vec<SavedCard> = cards.into_iter().filter(|c| is_valid_card_hash(&c.hash)).collect();
            save_saved_cards(&valid);
            return valid;
        }
    }
    // try migrating from old AirCard storage
    migrate_from_aircard()
}

/// If someone is upgrading from AirCard, grab their existing cards
fn migrate_from_aircard() -> Vec<SavedCard> {
    let local = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    let old_path = PathBuf::from(local).join("AirCard").join("cards.json");
    if let Ok(content) = fs::read_to_string(&old_path) {
        if let Ok(cards) = serde_json::from_str::<Vec<SavedCard>>(&content) {
            let valid: Vec<SavedCard> = cards.into_iter().filter(|c| is_valid_card_hash(&c.hash)).collect();
            if !valid.is_empty() {
                save_saved_cards(&valid);
                return valid;
            }
        }
    }
    Vec::new()
}

pub fn save_saved_cards(cards: &[SavedCard]) {
    let path = get_cards_storage_path();
    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for c in cards {
        if is_valid_card_hash(&c.hash) && seen.insert(c.hash.clone()) {
            unique.push(c.clone());
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&unique) {
        let _ = fs::write(path, json);
    }
}

pub fn add_or_update_card(hash: &str, name: &str) {
    if !is_valid_card_hash(hash) { return; }
    let mut cards = load_saved_cards();
    if let Some(existing) = cards.iter_mut().find(|c| c.hash == hash) {
        if !name.is_empty() && (existing.name.is_empty() || existing.name.starts_with("Card ")) {
            existing.name = name.to_string();
        }
    } else {
        cards.push(SavedCard {
            hash: hash.to_string(),
            name: if name.is_empty() {
                format!("Card {}", cards.len() + 1)
            } else {
                name.to_string()
            },
        });
    }
    save_saved_cards(&cards);
}

// keywords we look for in syslog lines to identify wallet activity
const WALLET_KEYWORDS: &[&str] = &[
    "passd", "passbook", "passkit", "stockholm", "nanopassd",
    "npkcompanion", "wallet", "/cards/", "/passes/",
];

// known dummy/test hashes that apple uses internally
const DUMMY_HASHES: &[&str] = &[
    "M6nDwZrkYbFlsodLgCbvyFZQ1cc=",
    "kJL-D0rr-SZhbj2c8nK-OQ9hCMY=",
    "hwAtAmHKYwsQrJbT5cTNDsaxVME=",
];

use std::sync::LazyLock;

static DESC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:description|localizedDescription|passName|title)\s*[:=]\s*['"]([^'"]+)['"]"#).unwrap()
});

static CARD_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"/(?:Cards|Passes/Cards)/([A-Za-z0-9+/_-]{27,44})(?:\.pkpass|\.cache|\.pkcache|/|\s|\x22|'|\)|,|$)").unwrap(),
        Regex::new(r"/([A-Za-z0-9+/_-]{27,44})\.(?:pkpass|cache|pkcache)").unwrap(),
        Regex::new(r"(?:^|[^A-Za-z0-9+/_-])([A-Za-z0-9+/_-]{27}=)(?:$|[^A-Za-z0-9+/_-])").unwrap(),
        Regex::new(r"(?i)(?:card[_\s]?(?:hash|id)|pass[_\s]?(?:hash|id)|unique[_\s]?id)\s*[:=]\s*['\x22]?([A-Za-z0-9+=_-]{27,44})").unwrap(),
    ]
});

pub fn extract_card_name_from_line(line: &str) -> Option<String> {
    if let Some(caps) = DESC_RE.captures(line) {
        if let Some(m) = caps.get(1) {
            let name = m.as_str().trim();
            if name.len() > 1 && !name.to_lowercase().contains("<private>") {
                return Some(name.to_string());
            }
        }
    }
    None
}

pub fn extract_card_hash_from_line(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    if !WALLET_KEYWORDS.iter().any(|k| lower.contains(k)) {
        return None;
    }
    for r in CARD_REGEXES.iter() {
        if let Some(caps) = r.captures(line) {
            if let Some(m) = caps.get(1) {
                let h = m.as_str().trim().trim_matches(['\'', '"']).trim_end_matches(['.', ',']);
                if is_valid_card_hash(h) {
                    let mut norm = h.to_string();
                    if norm.len() == 27 { norm.push('='); }
                    return Some(norm);
                }
            }
        }
    }
    None
}

pub fn scan_syslog_for_cards<F, L>(
    udid: Option<&str>,
    connection_mode: ConnectionMode,
    stop_flag: Arc<AtomicBool>,
    mut on_card_found: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(String, String),
    L: FnMut(String),
{
    log("Opening device session for syslog...".to_string());
    let session = ActiveDeviceSession::open(udid, connection_mode)
        .context("Failed to connect for syslog scanning")?;
    log(format!("Connected to {} over {}.", session.udid, session.transport.label()));

    let libs = &session.libs;
    log("Starting com.apple.syslog_relay...".to_string());
    let service_conn = session.start_service("com.apple.syslog_relay")
        .context("Failed to start syslog_relay")?;

    let raw_socket = unsafe { (libs.amd_service_connection_get_socket)(service_conn) };
    if raw_socket <= 0 {
        unsafe { (libs.amd_service_connection_invalidate)(service_conn) };
        anyhow::bail!("Bad syslog socket");
    }

    #[cfg(windows)]
    unsafe {
        let timeout_ms: u32 = 500;
        setsockopt(raw_socket as usize, SOL_SOCKET, SO_RCVTIMEO,
            &timeout_ms as *const u32 as *const i8, std::mem::size_of::<u32>() as i32);
    }

    log("Listening for Wallet events — open Apple Wallet or tap a card to trigger detection.".to_string());

    let mut buffer = [0u8; 8192];
    let mut line_acc = Vec::with_capacity(1024);

    while !stop_flag.load(Ordering::Relaxed) {
        let bytes_read = unsafe {
            (libs.amd_service_connection_receive)(service_conn, buffer.as_mut_ptr(), buffer.len())
        };
        if bytes_read > 0 {
            for &b in &buffer[..bytes_read as usize] {
                if b == b'\n' || b == b'\0' {
                    if !line_acc.is_empty() {
                        let line = String::from_utf8_lossy(&line_acc);
                        if let Some(hash) = extract_card_hash_from_line(&line) {
                            let name = extract_card_name_from_line(&line).unwrap_or_default();
                            log(format!("Found card! Name: '{}', Hash: {}",
                                if name.is_empty() { "Unknown" } else { &name }, hash));
                            add_or_update_card(&hash, &name);
                            on_card_found(hash, name);
                        }
                        line_acc.clear();
                    }
                } else if b != b'\r' {
                    line_acc.push(b);
                }
            }
        } else if bytes_read == 0 {
            log("Syslog socket closed.".to_string());
            break;
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    unsafe { (libs.amd_service_connection_invalidate)(service_conn); }
    log("Scan stopped.".to_string());
    Ok(())
}
