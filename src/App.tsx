import { useState, useEffect, useCallback, useRef } from "react";
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import {
  DeviceInfo, SavedCard, ScanUpdate, FlashProgress,
  getDevices, getSavedCards, renameCard, deleteCard,
  startScan, stopScan, flashSkin, restoreOriginal,
  checkBackupExists, detectBankName, flashSkinBatch,
  onScanUpdate, onFlashProgress, onFlashLog, onFlashError,
  onBatchProgress, onBatchDone, checkAppleSupport,
} from "./lib/tauri";

type Tab = "flash" | "batch" | "gallery";

export default function App() {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [cards, setCards] = useState<SavedCard[]>([]);
  const [selectedCard, setSelectedCard] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [logs, setLogs] = useState<{ text: string; type: string }[]>([]);
  const [tab, setTab] = useState<Tab>("flash");
  const [imagePath, setImagePath] = useState<string>("");
  const [progress, setProgress] = useState<FlashProgress | null>(null);
  const [renamingHash, setRenamingHash] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [batchSelected, setBatchSelected] = useState<Set<string>>(new Set());
  const [appleStatus, setAppleStatus] = useState<string>("");
  const logRef = useRef<HTMLDivElement>(null);

  const activeDevice = devices[0] ?? null;

  const addLog = useCallback((text: string, type = "info") => {
    setLogs(prev => [...prev.slice(-100), { text, type }]);
    setTimeout(() => logRef.current?.scrollTo(0, logRef.current.scrollHeight), 50);
  }, []);

  // on mount: check apple support, load devices and cards
  useEffect(() => {
    checkAppleSupport()
      .then(msg => setAppleStatus(msg))
      .catch(err => setAppleStatus(`⚠ ${err}`));

    refreshDevices();
    getSavedCards().then(setCards);
  }, []);

  // event listeners
  useEffect(() => {
    const unsubs: Array<() => void> = [];

    onScanUpdate((u: ScanUpdate) => {
      if (u.kind === "log") addLog(u.message);
      else if (u.kind === "card") {
        addLog(u.message, "success");
        getSavedCards().then(setCards);
      }
      else if (u.kind === "done") { addLog(u.message, "success"); setScanning(false); }
      else if (u.kind === "error") { addLog(u.message, "error"); setScanning(false); }
    }).then(u => unsubs.push(u));

    onFlashProgress((p) => setProgress(p)).then(u => unsubs.push(u));
    onFlashLog((m) => addLog(m)).then(u => unsubs.push(u));
    onFlashError((m) => { addLog(m, "error"); setProgress(null); }).then(u => unsubs.push(u));
    onBatchProgress((p) => setProgress({ step: p.current, total: p.total, message: p.message })).then(u => unsubs.push(u));
    onBatchDone((d) => { addLog(d.message, "success"); setProgress(null); }).then(u => unsubs.push(u));

    return () => unsubs.forEach(u => u());
  }, [addLog]);

  async function refreshDevices() {
    const result = await getDevices();
    setDevices(result.devices);
    if (result.error) addLog(result.error, "error");
  }

  async function handleStartScan() {
    setScanning(true);
    addLog("Starting scan...");
    await startScan(activeDevice?.udid);
  }

  async function handleStopScan() {
    await stopScan();
    setScanning(false);
  }

  async function handleFlash() {
    if (!activeDevice || !selectedCard || !imagePath) return;
    addLog(`Flashing skin to ${selectedCard}...`);
    await flashSkin(activeDevice.udid, selectedCard, imagePath);
  }

  async function handleBatchFlash() {
    if (!activeDevice || batchSelected.size === 0 || !imagePath) return;
    addLog(`Batch flashing to ${batchSelected.size} cards...`);
    await flashSkinBatch(activeDevice.udid, Array.from(batchSelected), imagePath);
  }

  async function handleRestore() {
    if (!activeDevice || !selectedCard) return;
    addLog(`Restoring original for ${selectedCard}...`);
    await restoreOriginal(activeDevice.udid, selectedCard);
  }

  async function handleRenameSubmit(hash: string) {
    if (renameValue.trim()) {
      const updated = await renameCard(hash, renameValue.trim());
      setCards(updated);
    }
    setRenamingHash(null);
    setRenameValue("");
  }

  async function handleDeleteCard(hash: string) {
    const updated = await deleteCard(hash);
    setCards(updated);
    if (selectedCard === hash) setSelectedCard(null);
  }

  async function handleDetectBank(hash: string) {
    if (!activeDevice) return;
    addLog(`Detecting bank for ${hash.slice(0, 12)}...`);
    try {
      const name = await detectBankName(activeDevice.udid, hash);
      if (name) {
        const updated = await renameCard(hash, name);
        setCards(updated);
        addLog(`Detected: ${name}`, "success");
      } else {
        addLog("Could not auto-detect bank name", "error");
      }
    } catch (e: any) {
      addLog(`Detection failed: ${e}`, "error");
    }
  }

  function toggleBatchCard(hash: string) {
    setBatchSelected(prev => {
      const next = new Set(prev);
      if (next.has(hash)) next.delete(hash);
      else next.add(hash);
      return next;
    });
  }

  async function handleFilePick() {
    const selected = await pickFile({
      multiple: false,
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (selected) {
      // single-file dialog returns string | null in tauri v2
      const filePath = String(selected);
      setImagePath(filePath);
      addLog(`Selected: ${filePath.split(/[/\\]/).pop()}`);
    }
  }

  const currentCard = cards.find(c => c.hash === selectedCard);

  return (
    <div className="app-layout">
      <div className="app-bg" />

      {/* titlebar */}
      <div className="titlebar">
        <span className="titlebar-title">✦ Lustre</span>
      </div>

      <div className="main-content">
        {/* sidebar */}
        <div className="sidebar">
          {/* device status */}
          <div className="glass device-bar">
            <div className={`device-indicator ${activeDevice ? "connected" : "disconnected"}`} />
            <div className="device-info">
              {activeDevice ? (
                <>
                  <div className="device-name">{activeDevice.name}</div>
                  <div className="device-meta">
                    {activeDevice.product_type} · iOS {activeDevice.ios_version} · {activeDevice.transports.join(" + ")}
                  </div>
                </>
              ) : (
                <>
                  <div className="device-name" style={{ color: "var(--text-secondary)" }}>No device</div>
                  <div className="device-meta">Connect an iPhone via USB</div>
                </>
              )}
            </div>
            <button className="btn btn-sm" onClick={refreshDevices}>↻</button>
          </div>

          {/* scan controls */}
          <div className="glass" style={{ padding: "12px" }}>
            <div className="section-label">Card Scanner</div>
            <div style={{ padding: "4px 0", display: "flex", gap: 8 }}>
              {scanning ? (
                <button className="btn btn-danger btn-sm" onClick={handleStopScan} style={{ flex: 1 }}>
                  ■ Stop
                </button>
              ) : (
                <button
                  className="btn btn-primary btn-sm"
                  onClick={handleStartScan}
                  disabled={!activeDevice}
                  style={{ flex: 1 }}
                >
                  ▶ Scan for Cards
                </button>
              )}
            </div>
          </div>

          {/* card list */}
          <div className="glass" style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
            <div className="section-label" style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <span>Saved Cards ({cards.length})</span>
            </div>
            <div className="card-list">
              {cards.length === 0 ? (
                <div className="empty-state">
                  <div className="empty-icon">💳</div>
                  <div className="empty-text">No cards yet</div>
                  <div className="empty-hint">
                    Scan for cards by opening Apple Wallet on your iPhone
                  </div>
                </div>
              ) : (
                cards.map(card => (
                  <div
                    key={card.hash}
                    className={`card-item ${selectedCard === card.hash ? "selected" : ""}`}
                    onClick={() => setSelectedCard(card.hash)}
                  >
                    <div className="card-icon">💳</div>
                    <div className="card-details">
                      {renamingHash === card.hash ? (
                        <input
                          className="rename-input"
                          value={renameValue}
                          onChange={e => setRenameValue(e.target.value)}
                          onBlur={() => handleRenameSubmit(card.hash)}
                          onKeyDown={e => {
                            if (e.key === "Enter") handleRenameSubmit(card.hash);
                            if (e.key === "Escape") { setRenamingHash(null); setRenameValue(""); }
                          }}
                          autoFocus
                          onClick={e => e.stopPropagation()}
                        />
                      ) : (
                        <div
                          className="card-name"
                          onDoubleClick={(e) => {
                            e.stopPropagation();
                            setRenamingHash(card.hash);
                            setRenameValue(card.name);
                          }}
                        >
                          {card.name}
                        </div>
                      )}
                      <div className="card-hash-preview">{card.hash.slice(0, 16)}…</div>
                    </div>
                  </div>
                ))
              )}
            </div>
          </div>
        </div>

        {/* main panel */}
        <div className="glass panel">
          {/* tabs */}
          <div className="panel-header">
            <div className="tab-bar">
              <button className={`tab ${tab === "flash" ? "active" : ""}`} onClick={() => setTab("flash")}>
                Flash Skin
              </button>
              <button className={`tab ${tab === "batch" ? "active" : ""}`} onClick={() => setTab("batch")}>
                Batch Flash
              </button>
              <button className={`tab ${tab === "gallery" ? "active" : ""}`} onClick={() => setTab("gallery")}>
                Gallery
              </button>
            </div>
          </div>

          {/* flash tab */}
          {tab === "flash" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
              {selectedCard && currentCard ? (
                <>
                  <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                    <span className="panel-title">{currentCard.name}</span>
                    <button
                      className="btn btn-sm"
                      onClick={() => { setRenamingHash(currentCard.hash); setRenameValue(currentCard.name); }}
                    >
                      ✏️ Rename
                    </button>
                    {activeDevice && (
                      <button
                        className="btn btn-sm"
                        onClick={() => handleDetectBank(currentCard.hash)}
                      >
                        🏦 Auto-Detect
                      </button>
                    )}
                    <button
                      className="btn btn-sm btn-danger"
                      onClick={() => handleDeleteCard(currentCard.hash)}
                    >
                      🗑
                    </button>
                  </div>

                  <div className="drop-zone" onClick={handleFilePick}>
                    {imagePath ? (
                      <div>
                        <div className="drop-zone-text">✓ {imagePath.split(/[/\\]/).pop()}</div>
                        <div className="drop-zone-hint">Click to change image</div>
                      </div>
                    ) : (
                      <div>
                        <div className="drop-zone-text">Click to select a card skin image</div>
                        <div className="drop-zone-hint">PNG, JPEG, or WebP · 1536×969 recommended</div>
                      </div>
                    )}
                  </div>

                  <div className="action-row">
                    <button
                      className="btn btn-primary"
                      onClick={handleFlash}
                      disabled={!activeDevice || !imagePath || !!progress}
                    >
                      ⚡ Flash Skin
                    </button>
                    <button
                      className="btn"
                      onClick={handleRestore}
                      disabled={!activeDevice || !!progress}
                    >
                      ↩ Restore Original
                    </button>
                  </div>

                  {progress && (
                    <div>
                      <div style={{ fontSize: 12, color: "var(--text-secondary)", marginBottom: 6 }}>
                        {progress.message}
                      </div>
                      <div className="progress-bar">
                        <div
                          className="progress-fill"
                          style={{ width: `${(progress.step / progress.total) * 100}%` }}
                        />
                      </div>
                    </div>
                  )}
                </>
              ) : (
                <div className="empty-state">
                  <div className="empty-icon">🎨</div>
                  <div className="empty-text">Select a card to customize</div>
                  <div className="empty-hint">
                    Pick a card from the sidebar, then choose an image to flash as the card face
                  </div>
                </div>
              )}
            </div>
          )}

          {/* batch tab */}
          {tab === "batch" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
              <div className="panel-title">Batch Flash</div>
              <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>
                Select multiple cards to apply the same skin to all of them at once.
              </div>

              <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                {cards.map(card => (
                  <div
                    key={card.hash}
                    className="check-item"
                    onClick={() => toggleBatchCard(card.hash)}
                  >
                    <div className={`checkbox ${batchSelected.has(card.hash) ? "checked" : ""}`}>
                      {batchSelected.has(card.hash) && "✓"}
                    </div>
                    <div className="card-name">{card.name}</div>
                  </div>
                ))}
              </div>

              {cards.length > 0 && (
                <>
                  <div className="drop-zone" onClick={handleFilePick}>
                    {imagePath ? (
                      <div>
                        <div className="drop-zone-text">✓ {imagePath.split(/[/\\]/).pop()}</div>
                        <div className="drop-zone-hint">Click to change image</div>
                      </div>
                    ) : (
                      <div>
                        <div className="drop-zone-text">Click to select skin image</div>
                        <div className="drop-zone-hint">This skin will be applied to all selected cards</div>
                      </div>
                    )}
                  </div>

                  <button
                    className="btn btn-primary"
                    onClick={handleBatchFlash}
                    disabled={!activeDevice || batchSelected.size === 0 || !imagePath || !!progress}
                  >
                    ⚡ Flash {batchSelected.size} Card{batchSelected.size !== 1 ? "s" : ""}
                  </button>
                </>
              )}

              {progress && (
                <div>
                  <div style={{ fontSize: 12, color: "var(--text-secondary)", marginBottom: 6 }}>
                    {progress.message}
                  </div>
                  <div className="progress-bar">
                    <div
                      className="progress-fill"
                      style={{ width: `${(progress.step / progress.total) * 100}%` }}
                    />
                  </div>
                </div>
              )}
            </div>
          )}

          {/* gallery tab */}
          {tab === "gallery" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
              <div className="panel-title">Skin Gallery</div>
              <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>
                Pre-made skins coming soon. Drop your own PNG files into the skins folder to add them here.
              </div>

              <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))", gap: 12 }}>
                {["Matte Black", "Brushed Gold", "Midnight Gradient", "Holographic"].map(name => (
                  <div
                    key={name}
                    className="glass"
                    style={{
                      padding: 16,
                      textAlign: "center",
                      cursor: "pointer",
                      display: "flex",
                      flexDirection: "column",
                      alignItems: "center",
                      gap: 8,
                    }}
                  >
                    <div style={{
                      width: 100, height: 63, borderRadius: 8,
                      background: name === "Matte Black"
                        ? "linear-gradient(135deg, #1a1a2e, #16213e)"
                        : name === "Brushed Gold"
                        ? "linear-gradient(135deg, #b8860b, #daa520)"
                        : name === "Midnight Gradient"
                        ? "linear-gradient(135deg, #0f0c29, #302b63, #24243e)"
                        : "linear-gradient(135deg, #ff6b6b, #feca57, #48dbfb, #ff9ff3)",
                    }} />
                    <div style={{ fontSize: 12, fontWeight: 500 }}>{name}</div>
                    <span className="badge badge-warning">Soon</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* log panel */}
          {logs.length > 0 && (
            <div className="log-panel" ref={logRef}>
              {logs.map((log, i) => (
                <div key={i} className={`log-line ${log.type}`}>{log.text}</div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
