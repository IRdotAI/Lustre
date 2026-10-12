use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Built-in gallery of card skin presets.
/// Skins are stored as PNG files in %LOCALAPPDATA%/Lustre/skins/
/// Each skin has metadata in a companion .json file.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GallerySkin {
    pub id: String,
    pub name: String,
    pub category: String,
    pub preview_base64: Option<String>,
    pub file_path: String,
}

fn skins_dir() -> PathBuf {
    let local = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    let dir = PathBuf::from(local).join("Lustre").join("skins");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Get all available skins from the gallery directory.
pub fn list_gallery_skins() -> Vec<GallerySkin> {
    let dir = skins_dir();
    let mut skins = Vec::new();

    // look for .json metadata files
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(skin) = serde_json::from_str::<GallerySkin>(&content) {
                        skins.push(skin);
                    }
                }
            }
        }
    }

    // if gallery is empty, return some built-in placeholder entries
    // users can add their own skins to the skins directory
    if skins.is_empty() {
        skins = default_gallery();
    }

    skins.sort_by(|a, b| a.name.cmp(&b.name));
    skins
}

/// Save an imported skin image to the gallery.
pub fn import_skin(name: &str, category: &str, png_data: &[u8]) -> anyhow::Result<GallerySkin> {
    let dir = skins_dir();
    let id = format!("custom-{}", crate::flasher::generate_token());

    let png_path = dir.join(format!("{}.png", id));
    let json_path = dir.join(format!("{}.json", id));

    fs::write(&png_path, png_data)?;

    // generate a tiny base64 preview (first 2KB or so)
    let preview = if png_data.len() > 0 {
        use base64::Engine;
        Some(base64::engine::general_purpose::STANDARD.encode(png_data))
    } else {
        None
    };

    let skin = GallerySkin {
        id: id.clone(),
        name: name.to_string(),
        category: category.to_string(),
        preview_base64: preview,
        file_path: png_path.to_string_lossy().to_string(),
    };

    let json = serde_json::to_string_pretty(&skin)?;
    fs::write(json_path, json)?;

    Ok(skin)
}

fn default_gallery() -> Vec<GallerySkin> {
    // these are just metadata entries — users would add actual PNG files
    vec![
        GallerySkin {
            id: "matte-black".into(),
            name: "Matte Black".into(),
            category: "Solid Colors".into(),
            preview_base64: None,
            file_path: String::new(),
        },
        GallerySkin {
            id: "brushed-gold".into(),
            name: "Brushed Gold".into(),
            category: "Metallic".into(),
            preview_base64: None,
            file_path: String::new(),
        },
        GallerySkin {
            id: "midnight-gradient".into(),
            name: "Midnight Gradient".into(),
            category: "Gradients".into(),
            preview_base64: None,
            file_path: String::new(),
        },
        GallerySkin {
            id: "holographic".into(),
            name: "Holographic".into(),
            category: "Effects".into(),
            preview_base64: None,
            file_path: String::new(),
        },
    ]
}
