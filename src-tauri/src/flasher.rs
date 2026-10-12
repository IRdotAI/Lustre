use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::afc::AfcClient;
use crate::airlift::{
    LINK_PREFIX, RECOVERED_PREFIX, SOURCE_PREFIX,
    build_books_plist, build_streaming_zip_archive, build_streaming_zip_archive_multi,
    restore_books, snapshot_books, stage_streaming_zip,
};
use crate::airtraffic::sync_assets_via_airtraffic;
use crate::device::{ActiveDeviceSession, ConnectionMode};
use crate::wallet_backup::{capture_original_card, load_original_assets};

#[allow(dead_code)]
pub const TARGET_WALLET_ASSETS: &[&str] = &[
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        hAlgorithm: *mut std::ffi::c_void,
        pbBuffer: *mut u8,
        cbBuffer: u32,
        dwFlags: u32,
    ) -> i32;
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 10];
    unsafe { let _ = BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 10, 2); }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Write a single file to an arbitrary path on the iOS filesystem via the
/// Books/AirTraffic symlink trick.
pub fn write_system_file<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    target_dir: &str,
    leaf_name: &str,
    payload: &[u8],
    mut log: L,
) -> Result<()>
where L: FnMut(&str),
{
    let token = generate_token();
    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let recovered = format!("{}{}", RECOVERED_PREFIX, token);

    let link_ident = format!("../../{}/p0/p1/p2/link", source);
    let payload_ident = format!("../../{}/payload", source);
    let target_dest = format!("{}/{}", link_dest, leaf_name);

    let books_ids = vec![link_ident.clone(), payload_ident.clone()];
    let assets_to_sync = [(link_ident.as_str(), link_dest.as_str()), (payload_ident.as_str(), target_dest.as_str())];

    log(&format!("Connecting for {}...", leaf_name));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)?;
    log(&format!("Connected over {}.", session.transport.label()));
    let afc = AfcClient::new(&session)?;

    let snapshot = snapshot_books(&afc)?;
    let archive = build_streaming_zip_archive(target_dir, payload)?;
    let books_plist = build_books_plist(&books_ids)?;

    let write_res = (|| -> Result<()> {
        log(&format!("Staging {} bytes via StreamingZip...", archive.len()));
        stage_streaming_zip(&session, &source, &archive)?;

        if !afc.exists(&source) || !afc.exists(&format!("{}/p0/p1/p2/link", source)) {
            bail!("StreamingZip staging failed — objects missing on AFC");
        }

        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;

        log(&format!("Syncing {} via AirTraffic...", leaf_name));
        sync_assets_via_airtraffic(udid, session.transport, &assets_to_sync, &mut log)?;
        Ok(())
    })();

    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_path(&recovered);
    let _ = afc.remove_tree(&source);
    sleep(Duration::from_millis(800));
    let restore_res = restore_books(&afc, &snapshot);

    write_res?;
    restore_res.context("Books cleanup failed")?;
    log(&format!("Written: {}", leaf_name));
    Ok(())
}

/// Write multiple files in a single atomic batch.
pub fn write_system_files_batch<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    target_dir: &str,
    items: &[(&str, &[u8])],
    mut log: L,
) -> Result<()>
where L: FnMut(&str),
{
    if items.is_empty() { return Ok(()); }
    if items.len() == 1 {
        return write_system_file(udid, connection_mode, target_dir, items[0].0, items[0].1, log);
    }

    let token = generate_token();
    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let recovered = format!("{}{}", RECOVERED_PREFIX, token);

    let link_ident = format!("../../{}/p0/p1/p2/link", source);
    let mut books_ids = vec![link_ident.clone()];
    let mut assets_to_sync: Vec<(String, String)> = vec![(link_ident, link_dest.clone())];

    for (idx, (leaf, _)) in items.iter().enumerate() {
        let pi = format!("../../{}/payload_{}", source, idx);
        let td = format!("{}/{}", link_dest, leaf);
        books_ids.push(pi.clone());
        assets_to_sync.push((pi, td));
    }

    log(&format!("Batch: {} files for {}...", items.len(), target_dir));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)?;
    let afc = AfcClient::new(&session)?;
    let snapshot = snapshot_books(&afc)?;

    let archive = build_streaming_zip_archive_multi(target_dir, items)?;
    let books_plist = build_books_plist(&books_ids)?;

    let write_res = (|| -> Result<()> {
        log(&format!("Staging {} bytes ({} files)...", archive.len(), items.len()));
        stage_streaming_zip(&session, &source, &archive)?;

        let link_obj = format!("{}/p0/p1/p2/link", source);
        let payload_obj = format!("{}/payload_0", source);
        let fallback_obj = format!("{}/payload", source);
        if !afc.exists(&source) || !afc.exists(&link_obj) || (!afc.exists(&payload_obj) && !afc.exists(&fallback_obj)) {
            bail!("Staging objects missing");
        }

        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;

        log(&format!("Syncing batch ({} items)...", items.len()));
        let refs: Vec<(&str, &str)> = assets_to_sync.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        sync_assets_via_airtraffic(udid, session.transport, &refs, &mut log)?;
        Ok(())
    })();

    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_path(&recovered);
    let _ = afc.remove_tree(&source);
    sleep(Duration::from_millis(800));
    let restore_res = restore_books(&afc, &snapshot);

    write_res?;
    restore_res.context("Books cleanup failed")?;
    log(&format!("Batch of {} files done!", items.len()));
    Ok(())
}

/// Flash a custom skin image onto a Wallet card.
pub fn flash_wallet_skin<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    skin_png: &[u8],
    skin_pdf: &[u8],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    anyhow::ensure!(crate::scanner::is_valid_card_hash(card_hash), "Invalid card hash");

    log(&format!("Target: {}", card_hash));
    let resolved = capture_original_card(udid, connection_mode, card_hash, &mut log)?
        .unwrap_or_else(|| card_hash.to_string());
    let pkpass = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", resolved);

    progress(1, 3, "Writing card artwork...");
    let assets: [(&str, &[u8]); 3] = [
        ("cardBackgroundCombined@3x.png", skin_png),
        ("cardBackgroundCombined@2x.png", skin_png),
        ("cardBackgroundCombined.pdf", skin_pdf),
    ];

    if write_system_files_batch(udid, connection_mode, &pkpass, &assets, &mut log).is_err() {
        for (asset, data) in &assets {
            write_system_file(udid, connection_mode, &pkpass, asset, data, &mut log)?;
        }
    }

    invalidate_caches(udid, connection_mode, &resolved, &mut progress, &mut log);
    progress(3, 3, "Done! Close and reopen Wallet to see your new skin.");
    Ok(())
}

/// Restore a card's original artwork from backup.
pub fn restore_wallet_original<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let originals = load_original_assets(udid, card_hash)?;
    let refs: Vec<(&str, &[u8])> = originals.iter().map(|(a, d)| (a.as_str(), d.as_slice())).collect();

    progress(1, 3, "Restoring original artwork...");
    let pkpass = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", card_hash);

    if write_system_files_batch(udid, connection_mode, &pkpass, &refs, &mut log).is_err() {
        for (asset, data) in &originals {
            write_system_file(udid, connection_mode, &pkpass, asset, data, &mut log)?;
        }
    }

    invalidate_caches(udid, connection_mode, card_hash, &mut progress, &mut log);
    progress(3, 3, "Original card face restored!");
    Ok(())
}

fn invalidate_caches<F, L>(
    udid: &str,
    mode: ConnectionMode,
    card_hash: &str,
    progress: &mut F,
    log: &mut L,
) where F: FnMut(usize, usize, &str), L: FnMut(&str),
{
    let corrupted: [(&str, &[u8]); 3] = [
        ("FrontFace", b"corrupted"),
        ("PlaceHolder", b"corrupted"),
        ("Preview", b"corrupted"),
    ];

    for (i, ext) in [".cache", ".pkcache"].iter().enumerate() {
        let step = 2 + i;
        let cache_dir = format!("/var/mobile/Library/Passes/Cards/{}{}", card_hash, ext);
        progress(step, 3, &format!("Clearing {} cache...", ext));
        if write_system_files_batch(udid, mode, &cache_dir, &corrupted, &mut *log).is_err() {
            for (leaf, data) in &corrupted {
                let _ = write_system_file(udid, mode, &cache_dir, leaf, data, &mut *log);
            }
        }
    }
}
