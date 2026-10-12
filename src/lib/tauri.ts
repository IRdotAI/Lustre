// thin wrapper around tauri invoke so we get proper types
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface DeviceInfo {
  udid: string;
  name: string;
  product_type: string;
  ios_version: string;
  build_version: string;
  transports: string[];
}

export interface SavedCard {
  hash: string;
  name: string;
}

export interface ScanUpdate {
  kind: "log" | "card" | "done" | "error";
  message: string;
  hash?: string;
  name?: string;
}

export interface GallerySkin {
  id: string;
  name: string;
  category: string;
  preview_base64?: string;
  file_path: string;
}

export interface FlashProgress {
  step: number;
  total: number;
  message: string;
}

export interface BatchProgress {
  current: number;
  total: number;
  message: string;
}

// ---- commands ----

export async function getDevices() {
  return invoke<{ devices: DeviceInfo[]; error: string | null }>("get_devices");
}

export async function checkAppleSupport() {
  return invoke<string>("check_apple_support");
}

export async function getSavedCards() {
  return invoke<SavedCard[]>("get_saved_cards");
}

export async function renameCard(hash: string, newName: string) {
  return invoke<SavedCard[]>("rename_card", { hash, newName });
}

export async function deleteCard(hash: string) {
  return invoke<SavedCard[]>("delete_card", { hash });
}

export async function startScan(udid?: string) {
  return invoke("start_scan", { udid: udid ?? null });
}

export async function stopScan() {
  return invoke("stop_scan");
}

export async function flashSkin(udid: string, cardHash: string, imagePath: string) {
  return invoke("flash_skin", { req: { udid, card_hash: cardHash, image_path: imagePath } });
}

export async function flashSkinBatch(udid: string, cardHashes: string[], imagePath: string) {
  return invoke("flash_skin_batch", { req: { udid, card_hashes: cardHashes, image_path: imagePath } });
}

export async function restoreOriginal(udid: string, cardHash: string) {
  return invoke("restore_original", { udid, cardHash });
}

export async function checkBackupExists(udid: string, cardHash: string) {
  return invoke<boolean>("check_backup_exists", { udid, cardHash });
}

export async function getGallerySkins() {
  return invoke<GallerySkin[]>("get_gallery_skins");
}

export async function detectBankName(udid: string, cardHash: string) {
  return invoke<string | null>("detect_bank_name", { udid, cardHash });
}

// ---- event listeners ----

export function onScanUpdate(handler: (update: ScanUpdate) => void) {
  return listen<ScanUpdate>("scan-update", (e) => handler(e.payload));
}

export function onFlashProgress(handler: (progress: FlashProgress) => void) {
  return listen<FlashProgress>("flash-progress", (e) => handler(e.payload));
}

export function onFlashLog(handler: (msg: string) => void) {
  return listen<string>("flash-log", (e) => handler(e.payload));
}

export function onFlashError(handler: (msg: string) => void) {
  return listen<string>("flash-error", (e) => handler(e.payload));
}

export function onBatchProgress(handler: (progress: BatchProgress) => void) {
  return listen<BatchProgress>("batch-progress", (e) => handler(e.payload));
}

export function onBatchDone(handler: (data: { total: number; message: string }) => void) {
  return listen<{ total: number; message: string }>("batch-done", (e) => handler(e.payload));
}
