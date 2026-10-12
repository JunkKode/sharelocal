/**
 * Mirrors the serde representation of the Rust types in `src-tauri/src/model.rs`.
 * Keep the two in step — the Rust tests assert the wire shape.
 */

export type LinkKind = "wifi" | "ethernet" | "unknown";
export type DiscoverySource = "mdns" | "udp" | "qr" | "manual";
export type PeerStatus = "online" | "stale" | "offline";
export type Direction = "push" | "pull";
export type Role = "sending" | "receiving";
export type ConflictPolicy = "keepboth" | "replace" | "cancel";
export type PayloadKind = "files" | "text";

/** Text this size or smaller travels with the offer instead of over HTTP. */
export const MAX_TEXT_BYTES = 64 * 1024;

export type SessionStatus =
  | "pending"
  | "accepted"
  | "rejected"
  | "transferring"
  | "paused"
  | "completed"
  | "cancelled"
  | "failed";

export interface DeviceInfo {
  id: string;
  name: string;
  os: string;
  kind: string;
  version: number;
  port: number;
  tls: boolean;
  pin_required: boolean;
}

/** `Peer` flattens `DeviceInfo`, so the device fields sit at the top level. */
export interface Peer extends DeviceInfo {
  addresses: string[];
  primary: string;
  source: DiscoverySource;
  status: PeerStatus;
  same_subnet: boolean;
  link: LinkKind;
  rtt_ms: number | null;
  last_speed_bps: number | null;
  last_seen_ms: number;
}

export interface FileMeta {
  rel_path: string;
  size: number;
  mime?: string | null;
  modified_ms?: number | null;
}

export interface Progress {
  file_index: number;
  files_done: number;
  bytes_done: number;
  file_bytes_done: number;
  speed_bps: number;
  eta_secs: number | null;
}

export type TransferErrorCode =
  | "device-offline"
  | "connection-refused"
  | "network-changed"
  | "interrupted"
  | "not-enough-space"
  | "permission-denied"
  | "file-missing"
  | "duplicate-name"
  | "invalid-token"
  | "invalid-path"
  | "pin-rejected"
  | "rejected"
  | "client-isolation"
  | "other";

export interface TransferError {
  code: TransferErrorCode;
  detail: string;
}

export interface SessionView {
  id: string;
  peer: DeviceInfo;
  files: FileMeta[];
  total_bytes: number;
  direction: Direction;
  role: Role;
  status: SessionStatus;
  progress: Progress;
  created_ms: number;
  error?: TransferError | null;
  saved_paths?: string[];
  /** Incoming offers: `rel_path`s that already exist in the download folder. */
  conflicts?: string[];
  kind?: PayloadKind;
  /** The shared snippet, for text transfers. */
  text?: string | null;
}

export interface NetworkStatus {
  online: boolean;
  interface: string | null;
  ipv4: string | null;
  ipv6: string | null;
  link: LinkKind;
  port: number;
  tls: boolean;
  mdns_ok: boolean;
  udp_ok: boolean;
  isolation_suspected: boolean;
}

export interface HistoryEntry {
  id: string;
  at_ms: number;
  peer_name: string;
  peer_id: string;
  role: Role;
  file_count: number;
  display_name: string;
  total_bytes: number;
  duration_ms: number;
  avg_speed_bps: number;
  status: SessionStatus;
  saved_paths?: string[];
  kind?: PayloadKind;
  text?: string | null;
}

export interface Settings {
  device_name: string;
  download_dir: string;
  auto_accept: boolean;
  pin: string | null;
  max_concurrent: number;
  conflict_policy: ConflictPolicy;
  port: number;
  preferred_interface: string | null;
  discovery_mdns: boolean;
  discovery_udp: boolean;
  tls: boolean;
  theme: "system" | "light" | "dark";
  autostart: boolean;
  start_minimized: boolean;
  notifications: boolean;
}

export interface Snapshot {
  device: DeviceInfo;
  network: NetworkStatus;
  peers: Peer[];
  sessions: SessionView[];
  settings: Settings;
}

/** `security` is the `T:` field of a Wi-Fi QR code. */
export type WifiSecurity = "WPA" | "WEP" | "nopass";

export interface WifiInfo {
  ssid: string | null;
  security: WifiSecurity;
}

// ------------------------------------------------------------------- USB
// Mirrors `src-tauri/src/usb.rs`. Ids are strings: the USB location and MTP
// handles are u64 on the Rust side, wider than a JavaScript number.

export interface UsbDevice {
  id: string;
  name: string;
  serial: string | null;
  speed: string | null;
}

export interface UsbStorage {
  id: string;
  name: string;
  total: number;
  free: number;
  writable: boolean;
}

export interface UsbConnection {
  id: string;
  name: string;
  storages: UsbStorage[];
  can_upload: boolean;
  can_delete: boolean;
  thumbnails: boolean;
}

export type UsbEntryKind = "folder" | "image" | "video" | "audio" | "file";

export interface UsbEntry {
  handle: string;
  name: string;
  size: number;
  folder: boolean;
  modified_ms: number | null;
  kind: UsbEntryKind;
}

export interface UsbError {
  code: string;
  message: string;
}

export interface UsbProgress {
  job: string;
  direction: "download" | "upload";
  current: string;
  files_done: number;
  files_total: number;
  bytes_done: number;
  bytes_total: number;
  speed_bps: number;
  finished: boolean;
  error: UsbError | null;
  saved: string[];
}

/** What the Wi-Fi password button will involve (see `wifi::PasswordAccess`). */
export interface PasswordAccess {
  remembered: boolean;
  biometrics: boolean;
  user_name: string;
}

export interface NetworkInterface {
  name: string;
  ip: string;
  link: LinkKind;
}

export type NoticeLevel = "info" | "warn" | "error";

export type AppEvent =
  | { type: "peers-changed"; peers: Peer[] }
  | { type: "incoming-offer"; session: SessionView }
  | { type: "session-updated"; session: SessionView }
  | { type: "session-removed"; id: string }
  | { type: "network-changed"; network: NetworkStatus }
  | { type: "settings-changed" }
  | { type: "history-changed" }
  | { type: "notice"; level: NoticeLevel; code: string; message: string };

/** A file chosen for sending: a real path on desktop, a `File` in a browser. */
export type Payload =
  | { kind: "paths"; paths: string[] }
  | { kind: "files"; files: File[] };
