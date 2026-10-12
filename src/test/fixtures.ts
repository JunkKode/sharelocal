import type { HistoryEntry, Peer, SessionView } from "../types";

export function peer(overrides: Partial<Peer> = {}): Peer {
  return {
    id: "peer-1",
    name: "Windows PC",
    os: "windows",
    kind: "desktop",
    version: 1,
    port: 47654,
    tls: false,
    pin_required: false,
    addresses: ["192.168.1.20"],
    primary: "192.168.1.20",
    source: "mdns",
    status: "online",
    same_subnet: true,
    link: "ethernet",
    rtt_ms: null,
    last_speed_bps: null,
    last_seen_ms: Date.now(),
    ...overrides,
  };
}

export function session(overrides: Partial<SessionView> = {}): SessionView {
  const total = 20 * 1024 ** 3;
  return {
    id: "session-1",
    peer: peer(),
    files: [{ rel_path: "movie.mp4", size: total, mime: "video/mp4" }],
    total_bytes: total,
    direction: "push",
    role: "sending",
    status: "transferring",
    progress: {
      file_index: 0,
      files_done: 0,
      bytes_done: 8.4 * 1024 ** 3,
      file_bytes_done: 8.4 * 1024 ** 3,
      speed_bps: 86.7 * 1024 ** 2,
      eta_secs: 130,
    },
    created_ms: Date.now(),
    ...overrides,
  };
}

export function historyEntry(overrides: Partial<HistoryEntry> = {}): HistoryEntry {
  return {
    id: "h1",
    at_ms: Date.now(),
    peer_name: "Windows PC",
    peer_id: "peer-1",
    role: "sending",
    file_count: 1,
    display_name: "movie.mp4",
    total_bytes: 8.4 * 1024 ** 3,
    duration_ms: 97_000,
    avg_speed_bps: 86 * 1024 ** 2,
    status: "completed",
    ...overrides,
  };
}
