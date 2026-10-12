# Lustre

Apple Wallet card skin customizer for Windows. Change the face of any card in your iPhone's Wallet with your own images.

![Windows 11](https://img.shields.io/badge/Windows-11-0078D6?logo=windows11)
![Rust](https://img.shields.io/badge/Rust-2024-DEA584?logo=rust)
![License](https://img.shields.io/badge/license-MIT-green)

## What it does

Lustre lets you apply custom skins to Apple Wallet cards (credit/debit/transit) via a USB connection to your iPhone. The app communicates with the device using Apple's private frameworks to replace card artwork on the fly.

### Features

- **Custom card skins** — Flash any PNG/JPEG/WebP image as a card face
- **Auto-detect bank name** — Reads card metadata from the device to identify issuers
- **Renamable cards** — Give your saved card hashes friendly names for easy tracking
- **Batch flash** — Apply the same skin to multiple cards at once
- **Skin gallery** — Built-in gallery for managing your card skin collection
- **One-click restore** — Revert any card to its original artwork from backup
- **Real-time scanner** — Detects card hashes via syslog as you open Wallet

## Requirements

- **Windows 10/11** (x64)
- **iPhone** running iOS 18.0 – 27.0.1 (patched in 27.2 beta 3+)
- **USB cable** connected to the PC
- **Apple Mobile Device Support** — installed automatically with iTunes, or via 3uTools

## How it works

1. Connect your iPhone via USB
2. Open Lustre — it detects your device automatically
3. Click **Scan for Cards** and open Apple Wallet on your phone
4. Select a card from the list and pick an image
5. Hit **Flash Skin** — done. Close and reopen Wallet to see it.

The skin is written to the card's `.pkpass` bundle on the device filesystem using AFC (Apple File Conduit) and synced through the AirTraffic conduit to trigger a cache refresh.

## Building from source

```bash
# prerequisites: Rust toolchain + Node.js 18+
cd lustre
npm install
cargo tauri build
```

The release binary lands in `src-tauri/target/release/lustre.exe`.

## Tech stack

- **Tauri 2** — Rust backend + WebView2 frontend, single lightweight `.exe`
- **React 18 + TypeScript** — Frontend with a liquid glass-inspired dark UI
- **Apple private APIs** — `MobileDevice.dll`, `CoreFoundation.dll`, `AirTrafficHost.dll`

## Compatibility

| iOS Version | Status |
|---|---|
| 18.0 – 18.x | ✅ Fully supported |
| 19.0 – 26.x | ✅ Fully supported |
| 27.0 – 27.0.1 | ✅ Supported |
| 27.2 beta 3+ | ❌ Patched by Apple |

## License

MIT
