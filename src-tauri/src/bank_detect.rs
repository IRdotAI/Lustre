/// Attempt to figure out which bank/card a hash belongs to by reading
/// the pass metadata from the device. This is done by looking at the
/// pass.json inside the .pkpass directory via AFC.

use anyhow::Result;

use crate::afc::AfcClient;
use crate::device::{ActiveDeviceSession, ConnectionMode};

/// Known bank/card issuers we can identify from pass metadata.
/// Maps common identifiers found in pass.json to human-readable names.
const KNOWN_ISSUERS: &[(&str, &str)] = &[
    // UK banks
    ("monzo", "Monzo"),
    ("starling", "Starling Bank"),
    ("revolut", "Revolut"),
    ("barclays", "Barclays"),
    ("hsbc", "HSBC"),
    ("lloyds", "Lloyds"),
    ("natwest", "NatWest"),
    ("nationwide", "Nationwide"),
    ("santander", "Santander"),
    ("halifax", "Halifax"),
    ("tsb", "TSB"),
    ("virgin money", "Virgin Money"),
    ("metro bank", "Metro Bank"),
    ("chase", "Chase"),
    // US banks
    ("bank of america", "Bank of America"),
    ("wells fargo", "Wells Fargo"),
    ("citibank", "Citibank"),
    ("capital one", "Capital One"),
    ("us bank", "US Bank"),
    ("pnc", "PNC"),
    ("td bank", "TD Bank"),
    ("discover", "Discover"),
    // card networks
    ("american express", "American Express"),
    ("amex", "American Express"),
    ("visa", "Visa"),
    ("mastercard", "Mastercard"),
    // other
    ("apple card", "Apple Card"),
    ("goldman sachs", "Apple Card"),
    ("cash app", "Cash App"),
    ("venmo", "Venmo"),
    ("paypal", "PayPal"),
    ("wise", "Wise"),
    ("n26", "N26"),
    ("bunq", "Bunq"),
    ("nubank", "Nubank"),
    ("curve", "Curve"),
];

/// Try to detect the bank/card name for a given card hash by reading
/// pass metadata from the device filesystem.
pub fn detect_bank_for_card(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
) -> Result<Option<String>> {
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)?;
    let afc = AfcClient::new(&session)?;

    // try different hash variants (with/without padding)
    let candidates = hash_candidates(card_hash);
    for candidate in &candidates {
        let pkpass = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", candidate);
        if !afc.exists(&pkpass) { continue; }

        // try to read pass.json — this contains issuer metadata
        let pass_json_path = format!("{}/pass.json", pkpass);
        if afc.exists(&pass_json_path) {
            if let Ok(data) = afc.read_file(&pass_json_path) {
                if let Ok(text) = String::from_utf8(data) {
                    if let Some(name) = extract_issuer_from_pass_json(&text) {
                        return Ok(Some(name));
                    }
                }
            }
        }

        // fallback: check directory listing for clues
        if let Ok(entries) = afc.list_directory(&pkpass) {
            for entry in &entries {
                let lower = entry.to_lowercase();
                for &(keyword, bank_name) in KNOWN_ISSUERS {
                    if lower.contains(keyword) {
                        return Ok(Some(bank_name.to_string()));
                    }
                }
            }
        }
    }

    Ok(None)
}

fn hash_candidates(card_hash: &str) -> Vec<String> {
    let trimmed = card_hash.trim_end_matches('=');
    let mut candidates = vec![card_hash.to_string(), trimmed.to_string()];
    if !trimmed.is_empty() {
        candidates.push(format!("{trimmed}="));
        candidates.push(format!("{trimmed}=="));
    }
    candidates.dedup();
    candidates
}

fn extract_issuer_from_pass_json(json_text: &str) -> Option<String> {
    // try proper JSON parsing first
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_text) {
        // check common fields where the issuer name might appear
        let fields_to_check = [
            "organizationName",
            "description",
            "logoText",
            "issuerDisplayName",
            "cardIssuerName",
        ];

        for field in fields_to_check {
            if let Some(value) = val.get(field).and_then(|v| v.as_str()) {
                // see if it matches a known issuer
                let lower = value.to_lowercase();
                for &(keyword, bank_name) in KNOWN_ISSUERS {
                    if lower.contains(keyword) {
                        return Some(bank_name.to_string());
                    }
                }
                // if it's a reasonable-looking name, return it as-is
                if value.len() > 1 && value.len() < 50 && !value.contains('<') {
                    return Some(value.to_string());
                }
            }
        }
    }

    // fallback: simple text search through the raw JSON
    let lower = json_text.to_lowercase();
    for &(keyword, bank_name) in KNOWN_ISSUERS {
        if lower.contains(keyword) {
            return Some(bank_name.to_string());
        }
    }

    None
}
