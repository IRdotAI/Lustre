use std::collections::HashMap;
use std::ptr;

use anyhow::{Result, bail};

use crate::afc::AfcClient;
use crate::apple::{CFTypeRef, get_apple_libraries};
use crate::device::ActiveDeviceSession;

pub const SOURCE_PREFIX: &str = "lustre-src-";
pub const LINK_PREFIX: &str = "lustre-link-";
pub const RECOVERED_PREFIX: &str = "lustre-rec-";

pub const TRACKED_BOOKS_FILES: &[&str] = &[
    "Books/Books.plist",
    "Books/Sync/Books.plist",
    "Books/Sync/Upload.plist",
    "Books/Sync/Database/OutstandingAssets_4.sqlite",
    "Books/Sync/Database/OutstandingAssets_4.sqlite-shm",
    "Books/Sync/Database/OutstandingAssets_4.sqlite-wal",
];

const SZ_EXTRA_ID: u16 = 0x5A53;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

struct ZipEntry {
    name: String,
    mode: u32,
    data: Vec<u8>,
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if (crc & 1) != 0 { (crc >> 1) ^ 0xEDB88320 } else { crc >> 1 };
        }
    }
    !crc
}

#[allow(dead_code)]
pub fn build_streaming_zip_archive(target: &str, payload: &[u8]) -> Result<Vec<u8>> {
    build_streaming_zip_archive_multi(target, &[("payload", payload)])
}

pub fn build_streaming_zip_archive_multi(target: &str, items: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let target_tail = target.strip_prefix('/').unwrap_or(target);

    // ZipMetadata.plist — Apple requires this for StreamingZip
    let mut meta_plist = Vec::new();
    let mut meta_dict = HashMap::new();
    meta_dict.insert("Version".to_string(), plist::Value::Integer(2.into()));
    plist::to_writer_binary(&mut meta_plist, &plist::Value::Dictionary(meta_dict.into_iter().collect()))?;

    let mut entries = Vec::new();

    entries.push(ZipEntry { name: "META-INF/".into(), mode: S_IFDIR | 0o755, data: vec![] });
    entries.push(ZipEntry { name: "META-INF/com.apple.ZipMetadata.plist".into(), mode: S_IFREG | 0o600, data: meta_plist });

    for dir in &["p0/", "p0/p1/", "p0/p1/p2/"] {
        entries.push(ZipEntry { name: dir.to_string(), mode: S_IFDIR | 0o755, data: vec![] });
    }

    // symlink that points to the target directory
    let symlink_target = format!("../../../{}", target_tail);
    entries.push(ZipEntry { name: "p0/p1/p2/link".into(), mode: S_IFLNK | 0o777, data: symlink_target.into_bytes() });

    // create intermediate dirs for target path
    let mut cursor = String::new();
    for component in target_tail.split('/').filter(|c| !c.is_empty()) {
        cursor.push_str(component);
        cursor.push('/');
        entries.push(ZipEntry { name: cursor.clone(), mode: S_IFDIR | 0o755, data: vec![] });
    }

    // payload files
    if items.len() == 1 && items[0].0 == "payload" {
        entries.push(ZipEntry { name: "payload".into(), mode: S_IFREG | 0o600, data: items[0].1.to_vec() });
    } else {
        for (idx, (_leaf, payload)) in items.iter().enumerate() {
            entries.push(ZipEntry { name: format!("payload_{}", idx), mode: S_IFREG | 0o600, data: payload.to_vec() });
        }
        if !items.is_empty() {
            entries.push(ZipEntry { name: "payload".into(), mode: S_IFREG | 0o600, data: items[0].1.to_vec() });
        }
    }

    // pack everything into a stored zip with Apple's custom extra fields
    let mut output = Vec::new();
    let mut cd_entries = Vec::new();

    for entry in entries {
        let offset = output.len() as u32;
        let crc = crc32(&entry.data);
        let name_bytes = entry.name.as_bytes();

        let extra_mode = (entry.mode & 0xFFFF) as u16;
        let mut extra = Vec::new();
        extra.extend_from_slice(&SZ_EXTRA_ID.to_le_bytes());
        extra.extend_from_slice(&2u16.to_le_bytes());
        extra.extend_from_slice(&extra_mode.to_le_bytes());

        // local file header
        output.extend_from_slice(&0x04034b50u32.to_le_bytes());
        output.extend_from_slice(&20u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0x2800u16.to_le_bytes());
        output.extend_from_slice(&0x5D30u16.to_le_bytes());
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        output.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        output.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        output.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        output.extend_from_slice(name_bytes);
        output.extend_from_slice(&extra);
        output.extend_from_slice(&entry.data);

        cd_entries.push((entry.name, entry.mode, crc, entry.data.len() as u32, offset, extra));
    }

    let cd_start = output.len() as u32;
    for (name, mode, crc, len, offset, extra) in &cd_entries {
        let name_bytes = name.as_bytes();
        let ext_attr = (mode & 0xFFFF) << 16;

        output.extend_from_slice(&0x02014b50u32.to_le_bytes());
        output.extend_from_slice(&((3u16 << 8) | 20u16).to_le_bytes());
        output.extend_from_slice(&20u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0x2800u16.to_le_bytes());
        output.extend_from_slice(&0x5D30u16.to_le_bytes());
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&len.to_le_bytes());
        output.extend_from_slice(&len.to_le_bytes());
        output.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        output.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&ext_attr.to_le_bytes());
        output.extend_from_slice(&offset.to_le_bytes());
        output.extend_from_slice(name_bytes);
        output.extend_from_slice(extra);
    }

    let cd_len = (output.len() as u32) - cd_start;
    let count = cd_entries.len() as u16;

    output.extend_from_slice(&0x06054b50u32.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
    output.extend_from_slice(&count.to_le_bytes());
    output.extend_from_slice(&count.to_le_bytes());
    output.extend_from_slice(&cd_len.to_le_bytes());
    output.extend_from_slice(&cd_start.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());

    Ok(output)
}

pub fn build_books_plist(identifiers: &[String]) -> Result<Vec<u8>> {
    let rows: Vec<plist::Value> = identifiers.iter().enumerate().map(|(idx, ident)| {
        let mut row = HashMap::new();
        row.insert("Persistent ID".to_string(), plist::Value::String(ident.clone()));
        row.insert("Item ID".to_string(), plist::Value::String((idx + 1).to_string()));
        row.insert("DSID".to_string(), plist::Value::String("1".to_string()));
        plist::Value::Dictionary(row.into_iter().collect())
    }).collect();

    let mut root = HashMap::new();
    root.insert("Books".to_string(), plist::Value::Array(rows));

    let mut buf = Vec::new();
    plist::to_writer_binary(&mut buf, &plist::Value::Dictionary(root.into_iter().collect()))?;
    Ok(buf)
}

pub struct BooksSnapshot {
    pub files: HashMap<String, Option<Vec<u8>>>,
}

pub fn snapshot_books(afc: &AfcClient) -> Result<BooksSnapshot> {
    let mut files = HashMap::new();
    for &path in TRACKED_BOOKS_FILES {
        files.insert(
            path.to_string(),
            if afc.exists(path) { Some(afc.read_file(path)?) } else { None },
        );
    }
    Ok(BooksSnapshot { files })
}

pub fn restore_books(afc: &AfcClient, snapshot: &BooksSnapshot) -> Result<()> {
    let mut errors = Vec::new();
    for (path, data_opt) in &snapshot.files {
        match data_opt {
            Some(data) => {
                if let Some(parent) = path.rfind('/').map(|i| &path[..i]) {
                    let _ = afc.make_directory_recursive(parent);
                }
                if let Err(e) = afc.write_file(path, data) {
                    errors.push(format!("{}: {}", path, e));
                }
            }
            None => {
                if afc.exists(path) {
                    if let Err(e) = afc.remove_path(path) {
                        errors.push(format!("{}: {}", path, e));
                    }
                }
            }
        }
    }
    if !errors.is_empty() {
        bail!("Books restore errors: {}", errors.join("; "));
    }
    Ok(())
}

pub fn stage_streaming_zip(
    session: &ActiveDeviceSession,
    source_subdir: &str,
    archive: &[u8],
) -> Result<()> {
    let libs = get_apple_libraries()?;
    let zip_service = session.start_service("com.apple.streaming_zip_conduit")?;

    let send_res = (|| -> Result<()> {
        let mut msg_dict = HashMap::new();
        msg_dict.insert("MediaSubdir".to_string(), plist::Value::String(source_subdir.to_string()));
        let mut msg_plist = Vec::new();
        plist::to_writer_binary(&mut msg_plist, &plist::Value::Dictionary(msg_dict.into_iter().collect()))?;

        let cf_msg = libs.create_cf_plist_from_bytes(&msg_plist)?;
        let status = unsafe {
            (libs.amd_service_connection_send_message)(
                zip_service, cf_msg.raw, crate::apple::K_CFPROPERTY_LIST_BINARY_FORMAT_V1_0,
            )
        };
        if status != 0 {
            bail!("SendMessage failed ({})", status);
        }

        let mut sent = 0;
        while sent < archive.len() {
            let chunk_size = std::cmp::min(65536, archive.len() - sent);
            let s = unsafe {
                (libs.amd_service_connection_send)(zip_service, archive.as_ptr().add(sent), chunk_size)
            };
            if s <= 0 { bail!("Send failed during archive upload"); }
            sent += s as usize;
        }

        // set receive timeout
        let raw_socket = unsafe { (libs.amd_service_connection_get_socket)(zip_service) };
        if raw_socket > 0 {
            #[cfg(windows)]
            unsafe {
                unsafe extern "system" {
                    fn setsockopt(s: usize, level: i32, optname: i32, optval: *const i8, optlen: i32) -> i32;
                }
                let timeout_ms: u32 = 25000;
                let _ = setsockopt(raw_socket as usize, 0xffff, 0x1006,
                    &timeout_ms as *const u32 as *const i8, std::mem::size_of::<u32>() as i32);
            }
        }

        let mut response: CFTypeRef = ptr::null();
        let mut format: isize = 0;
        let rc = unsafe {
            (libs.amd_service_connection_receive_message)(zip_service, &mut response, &mut format)
        };
        if !response.is_null() { unsafe { (libs.cf_release)(response); } }
        if rc != 0 { bail!("StreamingZip conduit error ({})", rc); }
        Ok(())
    })();

    unsafe { (libs.amd_service_connection_invalidate)(zip_service); }
    send_res
}
