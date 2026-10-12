/** Typed wrappers over the Tauri command surface. */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type {
  AppEvent,
  ConflictPolicy,
  HistoryEntry,
  NetworkInterface,
  PasswordAccess,
  Peer,
  SessionView,
  Settings,
  Snapshot,
  UsbConnection,
  UsbDevice,
  UsbEntry,
  UsbProgress,
  UsbStorage,
  WifiInfo,
} from "../types";

export const desktop = {
  snapshot: () => invoke<Snapshot>("get_snapshot"),
  listPeers: () => invoke<Peer[]>("list_peers"),
  refreshPeers: () => invoke<void>("refresh_peers"),
  listInterfaces: () => invoke<NetworkInterface[]>("list_interfaces"),

  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (patch: Settings) => invoke<Settings>("update_settings", { patch }),
  setDownloadDir: (path: string) => invoke<string>("set_download_dir", { path }),

  sendFiles: (peerId: string, paths: string[], pin?: string | null) =>
    invoke<string>("send_files", { peerId, paths, pin: pin ?? null }),
  sendText: (peerId: string, text: string, pin?: string | null) =>
    invoke<string>("send_text", { peerId, text, pin: pin ?? null }),
  sendClipboard: (peerId: string, pin?: string | null) =>
    invoke<string>("send_clipboard", { peerId, pin: pin ?? null }),
  copyToClipboard: (text: string) => invoke<void>("copy_to_clipboard", { text }),

  sendFilesToWeb: (clientId: string, paths: string[]) =>
    invoke<string>("send_files_to_web", { clientId, paths }),
  sessionToken: (sessionId: string) => invoke<string>("session_token", { sessionId }),

  acceptOffer: (sessionId: string, conflict?: ConflictPolicy) =>
    invoke<void>("accept_offer", { sessionId, conflict: conflict ?? null }),
  rejectOffer: (sessionId: string) => invoke<void>("reject_offer", { sessionId }),
  cancelTransfer: (sessionId: string) => invoke<void>("cancel_transfer", { sessionId }),
  pauseTransfer: (sessionId: string, paused: boolean) =>
    invoke<void>("pause_transfer", { sessionId, paused }),
  listSessions: () => invoke<SessionView[]>("list_sessions"),
  clearFinished: () => invoke<void>("clear_finished"),

  history: () => invoke<HistoryEntry[]>("get_history"),
  clearHistory: () => invoke<void>("clear_history"),
  removeHistoryEntry: (id: string) => invoke<void>("remove_history_entry", { id }),

  createPairing: () => invoke<{ url: string; expires_in_secs: number }>("create_pairing"),
  revokePairing: () => invoke<void>("revoke_pairing"),

  openPath: (path: string) => invoke<void>("open_path", { path }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  openDownloadDir: () => invoke<void>("open_download_dir"),

  wifiInfo: () => invoke<WifiInfo>("wifi_info"),
  wifiPassword: (ssid: string) => invoke<string>("wifi_password", { ssid }),
  wifiPasswordAccess: (ssid: string) => invoke<PasswordAccess>("wifi_password_access", { ssid }),
  wifiRemember: (ssid: string, password: string) => invoke<void>("wifi_remember", { ssid, password }),
  wifiForget: (ssid: string) => invoke<void>("wifi_forget", { ssid }),
  /** Saves into the download folder under `name`; returns the full path. */
  savePng: (name: string, pngBase64: string) => invoke<string>("save_png", { name, pngBase64 }),
  copyImage: (pngBase64: string) => invoke<void>("copy_image_to_clipboard", { pngBase64 }),

  // USB (Android over MTP). Failures reject with a `UsbError` object.
  usbDevices: () => invoke<UsbDevice[]>("usb_devices"),
  usbConnect: (id: string, release: boolean) => invoke<UsbConnection>("usb_connect", { id, release }),
  usbDisconnect: () => invoke<void>("usb_disconnect"),
  usbStorages: () => invoke<UsbStorage[]>("usb_storages"),
  usbList: (storage: string, parent: string | null, hidden = false) =>
    invoke<UsbEntry[]>("usb_list", { storage, parent, hidden }),
  usbThumbnail: (storage: string, handle: string) =>
    invoke<string>("usb_thumbnail", { storage, handle }),
  usbCreateFolder: (storage: string, parent: string | null, name: string) =>
    invoke<string>("usb_create_folder", { storage, parent, name }),
  usbDelete: (storage: string, handles: string[]) => invoke<void>("usb_delete", { storage, handles }),
  usbDownload: (storage: string, handles: string[], dest: string | null) =>
    invoke<string>("usb_download", { storage, handles, dest }),
  usbUpload: (storage: string, parent: string | null, paths: string[]) =>
    invoke<string>("usb_upload", { storage, parent, paths }),
  usbCancel: () => invoke<void>("usb_cancel"),
};

/** The mouse's back / forward side buttons, caught natively (`mouse_nav.rs`). */
export function subscribeToMouseNav(handler: (direction: "back" | "forward") => void): () => void {
  const promise = listen<"back" | "forward">("mouse-nav", (e) => handler(e.payload));
  return () => {
    promise.then((un) => un()).catch(() => {});
  };
}

export function subscribeToUsbProgress(handler: (progress: UsbProgress) => void): () => void {
  const promise = listen<UsbProgress>("usb-progress", (e) => handler(e.payload));
  return () => {
    promise.then((un) => un()).catch(() => {});
  };
}

/** Event names the Rust side emits, in the order they are declared there. */
const EVENT_NAMES = [
  "peers-changed",
  "incoming-offer",
  "session-updated",
  "session-removed",
  "network-changed",
  "settings-changed",
  "history-changed",
  "notice",
] as const;

/** Subscribe to every core event with one call. */
export function subscribeToEvents(handler: (event: AppEvent) => void): () => void {
  const unlisteners: Promise<UnlistenFn>[] = EVENT_NAMES.map((name) =>
    listen<AppEvent>(name, (e) => handler(e.payload)),
  );
  return () => {
    unlisteners.forEach((p) => p.then((un) => un()).catch(() => {}));
  };
}

export type DragState =
  | { phase: "idle" }
  | { phase: "over"; x: number; y: number }
  | { phase: "drop"; paths: string[] };

/**
 * Native drag-and-drop from Finder / Explorer / a Linux file manager.
 *
 * The webview hands us real filesystem paths, which is what lets a dropped
 * 20 GB file stream from disk instead of being read into the page.
 */
export function subscribeToFileDrops(handler: (state: DragState) => void): () => void {
  const promise = getCurrentWebview().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === "over") {
      handler({ phase: "over", x: payload.position.x, y: payload.position.y });
    } else if (payload.type === "drop") {
      handler({ phase: "drop", paths: payload.paths });
    } else {
      handler({ phase: "idle" });
    }
  });
  return () => {
    promise.then((un) => un()).catch(() => {});
  };
}
