use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::afc::AfcClient;
use crate::device::{ActiveDeviceSession, ConnectionMode};

pub const CARD_ARTWORK_ASSETS: [&str; 3] = [
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

fn backup_root() -> PathBuf {
    let local = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    PathBuf::from(local).join("Lustre").join("backups")
}

fn backup_dir(udid: &str, card_hash: &str) -> PathBuf {
    backup_root().join(format!("{}-{}", safe_name(udid), stable_hash(card_hash)))
}

fn safe_name(s: &str) -> String {
    let r: String = s.chars().map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }
    }).collect();
    if r.is_empty() { "unknown".into() } else { r }
}

fn stable_hash(s: &str) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn backup_exists(udid: &str, card_hash: &str) -> bool {
    let dir = backup_dir(udid, card_hash);
    CARD_ARTWORK_ASSETS.iter().any(|a| dir.join(a).is_file())
}

fn card_hash_candidates(card_hash: &str) -> Vec<String> {
    let trimmed = card_hash.trim_end_matches('=');
    let mut candidates = vec![card_hash.to_string(), trimmed.to_string()];
    if !trimmed.is_empty() {
        candidates.push(format!("{trimmed}="));
        candidates.push(format!("{trimmed}=="));
    }
    candidates.dedup();
    candidates
}

fn find_device_card_hash(afc: &AfcClient, card_hash: &str) -> Result<Option<String>> {
    for candidate in card_hash_candidates(card_hash) {
        let path = format!("/var/mobile/Library/Passes/Cards/{candidate}.pkpass");
        if afc.exists(&path) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

pub fn capture_original_card<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    mut log: L,
) -> Result<Option<String>>
where L: FnMut(&str),
{
    let dir = backup_dir(udid, card_hash);
    fs::create_dir_all(&dir)?;

    log("Checking for existing backup...");
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)?;
    let afc = AfcClient::new(&session)?;

    let resolved = match find_device_card_hash(&afc, card_hash)? {
        Some(h) => h,
        None if backup_exists(udid, card_hash) => {
            log("Backup exists but card directory not visible on device.");
            return Ok(Some(card_hash.to_string()));
        }
        None => {
            log(&format!("Card directory not found for {} — continuing without backup.", card_hash));
            return Ok(None);
        }
    };

    if resolved != card_hash {
        log(&format!("Using variant hash: {}", resolved));
    }

    let pkpass = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", resolved);
    let mut saved = 0;

    for asset in CARD_ARTWORK_ASSETS {
        let backup_path = dir.join(asset);
        if backup_path.is_file() {
            saved += 1;
            continue;
        }

        let device_path = format!("{pkpass}/{asset}");
        if !afc.exists(&device_path) { continue; }

        let data = afc.read_file(&device_path)?;
        write_atomic(&backup_path, &data)?;
        saved += 1;
        log(&format!("Backed up: {}", asset));
    }

    if saved == 0 {
        log("Card exists but no artwork assets found for backup.");
    }
    Ok(Some(resolved))
}

pub fn load_original_assets(udid: &str, card_hash: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let dir = backup_dir(udid, card_hash);
    let mut assets = Vec::new();
    for asset in CARD_ARTWORK_ASSETS {
        let path = dir.join(asset);
        if path.is_file() {
            assets.push((asset.to_string(), fs::read(&path)?));
        }
    }
    if assets.is_empty() {
        bail!("No backup found for hash {}", card_hash);
    }
    Ok(assets)
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    if path.exists() { fs::remove_file(path)?; }
    fs::rename(tmp, path)?;
    Ok(())
}
