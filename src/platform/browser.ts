/**
 * The phone/browser client.
 *
 * It speaks the same protocol as a desktop peer: redeem the QR pairing token,
 * open the WebSocket control channel, then move bytes over HTTP. Uploads are
 * streamed from the `File` object, so a large video is not read into memory.
 */

import { MAX_TEXT_BYTES } from "../types";
import type { AppEvent, DeviceInfo, FileMeta, SessionStatus, SessionView } from "../types";

export interface JoinResult {
  clientId: string;
  clientToken: string;
  device: DeviceInfo;
}

export interface UploadProgress {
  fileIndex: number;
  fileName: string;
  bytesDone: number;
  totalBytes: number;
  speedBps: number;
  etaSecs: number | null;
}

const CLIENT_ID_KEY = "sharelocal.client-id";
/** How often the phone pings the computer over the control connection. */
const HEARTBEAT_MS = 10_000;
/** Silence after which the connection is assumed dead and reopened. */
const HEARTBEAT_DEAD_MS = 30_000;
const CLIENT_TOKEN_KEY = "sharelocal.client-token";

/**
 * 32 hex characters of randomness.
 *
 * Deliberately not `crypto.randomUUID`: that is restricted to secure contexts,
 * and ShareLocal is served over plain HTTP on a LAN address, where it does not
 * exist. `getRandomValues` has no such restriction; the `Math.random` path is a
 * last resort for ancient engines. This value is only an identity — the actual
 * authorisation is the pairing token the desktop mints.
 */
function randomId(): string {
  const bytes = new Uint8Array(16);
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    crypto.getRandomValues(bytes);
  } else {
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Storage can throw outright (Safari private browsing, blocked site data), so
 * every access is guarded and the client still works without persistence — it
 * just has to be re-paired after a reload.
 */
function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStored(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Nothing to do: the session simply will not survive a reload.
  }
}

let memoryClientId: string | null = null;

/** A stable per-browser id so a page reload keeps the same identity. */
export function localClientId(): string {
  const stored = readStored(CLIENT_ID_KEY);
  if (stored) return stored;
  if (!memoryClientId) memoryClientId = randomId();
  writeStored(CLIENT_ID_KEY, memoryClientId);
  return memoryClientId;
}

export function storedToken(): string | null {
  return readStored(CLIENT_TOKEN_KEY);
}

export function storeToken(token: string): void {
  writeStored(CLIENT_TOKEN_KEY, token);
}

export function clearToken(): void {
  try {
    localStorage.removeItem(CLIENT_TOKEN_KEY);
  } catch {
    // Ignore: see `writeStored`.
  }
}

/** Best-effort description of the phone, used as its display name. */
export function describeBrowser(): { name: string; os: string; kind: string } {
  const ua = navigator.userAgent;
  const os = /iPhone|iPad|iPod/i.test(ua)
    ? "ios"
    : /Android/i.test(ua)
      ? "android"
      : /Mac OS X/i.test(ua)
        ? "macos"
        : /Windows/i.test(ua)
          ? "windows"
          : /Linux/i.test(ua)
            ? "linux"
            : "browser";
  const kind = /iPad|Tablet/i.test(ua) ? "tablet" : /Mobi|Android|iPhone/i.test(ua) ? "phone" : "browser";
  const browser = /CriOS|Chrome/i.test(ua)
    ? "Chrome"
    : /FxiOS|Firefox/i.test(ua)
      ? "Firefox"
      : /Edg/i.test(ua)
        ? "Edge"
        : /Safari/i.test(ua)
          ? "Safari"
          : "Browser";
  const device = kind === "phone" ? (os === "ios" ? "iPhone" : "Phone") : kind === "tablet" ? "Tablet" : "Browser";
  return { name: `${device} · ${browser}`, os, kind };
}

/**
 * Browsers older than these cannot stream an upload or run the UI.
 *
 * Note what is *not* checked: anything gated behind a secure context. The page
 * is served over plain HTTP from a LAN address, so `crypto.subtle` and
 * `crypto.randomUUID` are absent on perfectly capable browsers.
 */
export function browserIsSupported(): boolean {
  return (
    typeof WebSocket !== "undefined" &&
    typeof fetch !== "undefined" &&
    typeof Blob !== "undefined" &&
    typeof File !== "undefined" &&
    typeof XMLHttpRequest !== "undefined"
  );
}

export class ShareLocalClient {
  readonly clientId = localClientId();
  private token: string | null = storedToken();
  private socket: WebSocket | null = null;
  private reconnectTimer: number | null = null;
  private heartbeatTimer: number | null = null;
  private lastHeard = 0;
  private wakeListenersAdded = false;
  private closed = false;
  private listeners = new Set<(event: AppEvent) => void>();
  private connectionListeners = new Set<(connected: boolean) => void>();

  /** Redeem a QR pairing token. Only needed once per browser. */
  async join(pairingToken: string): Promise<JoinResult> {
    const info = describeBrowser();
    const response = await fetch("/api/web/session", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        pairing_token: pairingToken,
        client_id: this.clientId,
        name: info.name,
        os: info.os,
        kind: info.kind,
      }),
    });
    if (!response.ok) {
      throw new Error(await describeError(response, "This QR code is no longer valid."));
    }
    const body = await response.json();
    this.token = body.client_token;
    storeToken(body.client_token);
    return {
      clientId: body.client_id,
      clientToken: body.client_token,
      device: body.device,
    };
  }

  get paired(): boolean {
    return Boolean(this.token);
  }

  get bearer(): string | null {
    return this.token;
  }

  async deviceInfo(): Promise<DeviceInfo> {
    const response = await fetch("/api/info");
    if (!response.ok) throw new Error("Could not reach ShareLocal on this network.");
    return response.json();
  }

  onEvent(handler: (event: AppEvent) => void): () => void {
    this.listeners.add(handler);
    return () => this.listeners.delete(handler);
  }

  onConnectionChange(handler: (connected: boolean) => void): () => void {
    this.connectionListeners.add(handler);
    return () => this.connectionListeners.delete(handler);
  }

  /** Open the control channel, reconnecting on its own if the link drops. */
  connect(): void {
    if (this.closed || !this.token) return;
    if (this.socket && this.socket.readyState <= WebSocket.OPEN) return;

    const scheme = window.location.protocol === "https:" ? "wss" : "ws";
    const url = `${scheme}://${window.location.host}/ws?client_id=${encodeURIComponent(
      `web-${this.clientId}`,
    )}&token=${encodeURIComponent(this.token)}`;

    const socket = new WebSocket(url);
    this.socket = socket;
    this.watchForWake();

    socket.onopen = () => {
      this.lastHeard = Date.now();
      this.startHeartbeat(socket);
      this.connectionListeners.forEach((l) => l(true));
    };
    socket.onmessage = (message) => {
      this.lastHeard = Date.now();
      try {
        const frame = JSON.parse(message.data);
        if (frame?.type === "event" && frame.event) {
          this.listeners.forEach((l) => l(frame.event as AppEvent));
        }
      } catch {
        // A frame we cannot parse is not worth tearing the connection down for.
      }
    };
    socket.onclose = () => {
      // An old socket replaced after a wake-up closes late; it must not mark
      // the new one as disconnected.
      if (this.socket !== socket) return;
      this.socket = null;
      this.stopHeartbeat();
      this.connectionListeners.forEach((l) => l(false));
      this.scheduleReconnect();
    };
    socket.onerror = () => socket.close();
  }

  /**
   * A ping every few seconds, answered by a pong. A phone that slept with the
   * page open can come back to a socket that still says "open" but is dead;
   * when the pongs stop, it is closed and reopened instead of sitting there
   * looking connected.
   */
  private startHeartbeat(socket: WebSocket): void {
    this.stopHeartbeat();
    this.heartbeatTimer = window.setInterval(() => {
      if (socket.readyState !== WebSocket.OPEN) return;
      if (Date.now() - this.lastHeard > HEARTBEAT_DEAD_MS) {
        socket.close();
        return;
      }
      try {
        socket.send(JSON.stringify({ type: "ping", t: Date.now() }));
      } catch {
        socket.close();
      }
    }, HEARTBEAT_MS);
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer !== null) window.clearInterval(this.heartbeatTimer);
    this.heartbeatTimer = null;
  }

  /**
   * Mobile browsers freeze a page in the background and kill its socket.
   * Reconnect the moment the page is looked at again rather than waiting.
   */
  private watchForWake(): void {
    if (this.wakeListenersAdded || typeof document === "undefined") return;
    this.wakeListenersAdded = true;
    const wake = () => {
      if (this.closed || document.visibilityState === "hidden") return;
      const socket = this.socket;
      const dead =
        !socket ||
        socket.readyState > WebSocket.OPEN ||
        (socket.readyState === WebSocket.OPEN && Date.now() - this.lastHeard > HEARTBEAT_DEAD_MS);
      if (!dead) return;
      socket?.close();
      if (this.reconnectTimer !== null) window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
      this.connect();
    };
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("pageshow", wake);
    window.addEventListener("online", wake);
  }

  private scheduleReconnect(): void {
    if (this.closed || this.reconnectTimer !== null) return;
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, 1500);
  }

  disconnect(): void {
    this.closed = true;
    this.stopHeartbeat();
    if (this.reconnectTimer !== null) window.clearTimeout(this.reconnectTimer);
    this.socket?.close();
    this.socket = null;
  }

  /**
   * Offer files to the desktop, wait for it to be accepted, then upload each
   * file with a streamed `PUT`.
   */
  async sendFiles(
    files: File[],
    options: {
      pin?: string | null;
      onProgress?: (progress: UploadProgress) => void;
      onStatus?: (status: SessionStatus) => void;
      signal?: AbortSignal;
    } = {},
  ): Promise<void> {
    if (!this.token) throw new Error("Scan the QR code again to reconnect.");
    if (files.length === 0) return;

    const metas: FileMeta[] = files.map((file) => ({
      // `webkitRelativePath` is set when a whole folder was picked, which is
      // how folder structure survives the trip.
      rel_path: sanitiseRelPath((file as FileWithPath).webkitRelativePath || file.name),
      size: file.size,
      mime: file.type || null,
      modified_ms: file.lastModified ?? null,
    }));

    const offer = await fetch("/api/web/offer", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${this.token}`,
      },
      body: JSON.stringify({ files: metas, pin: options.pin ?? null, client_id: `web-${this.clientId}` }),
      signal: options.signal,
    });
    if (!offer.ok) {
      throw new Error(await describeError(offer, "The other device refused the transfer."));
    }
    const created = await offer.json();
    let sessionToken: string | undefined = created.token;
    options.onStatus?.(created.status);

    if (!sessionToken) {
      sessionToken = await this.waitForAcceptance(
        created.session_id,
        options.signal,
        options.onStatus,
        // Already announced above; do not announce it again on the first poll.
        created.status,
      );
    }

    const totalBytes = metas.reduce((sum, m) => sum + m.size, 0);
    let doneBefore = 0;

    for (let index = 0; index < files.length; index += 1) {
      const file = files[index]!;
      await this.uploadOne(created.session_id, sessionToken, index, file, {
        signal: options.signal,
        onBytes: (sent, speed) => {
          const bytesDone = doneBefore + sent;
          options.onProgress?.({
            fileIndex: index,
            fileName: metas[index]!.rel_path,
            bytesDone,
            totalBytes,
            speedBps: speed,
            etaSecs: speed > 0 ? (totalBytes - bytesDone) / speed : null,
          });
        },
      });
      doneBefore += file.size;
    }
    options.onStatus?.("completed");
  }

  private async waitForAcceptance(
    sessionId: string,
    signal?: AbortSignal,
    onStatus?: (status: SessionStatus) => void,
    alreadyReported: SessionStatus | null = null,
  ): Promise<string> {
    const deadline = Date.now() + 120_000;
    // Only report a status the caller has not already been told about. This
    // loop polls every 700ms, so passing every response through would fire a
    // "waiting to be accepted" toast several times a second for as long as the
    // other device takes to answer.
    let reported: SessionStatus | null = alreadyReported;

    while (Date.now() < deadline) {
      if (signal?.aborted) throw new Error("Cancelled.");
      const response = await fetch(`/api/offer/${encodeURIComponent(sessionId)}`, {
        headers: { Authorization: `Bearer ${this.token}` },
        signal,
      });
      if (response.ok) {
        const body = await response.json();
        if (body.status !== reported) {
          reported = body.status;
          onStatus?.(body.status);
        }
        if (body.status === "accepted" && body.token) return body.token;
        if (body.status === "rejected") throw new Error("The other device declined the transfer.");
        if (body.status === "cancelled" || body.status === "failed") {
          throw new Error("The transfer was cancelled.");
        }
      }
      await sleep(700);
    }
    throw new Error("The other device did not answer.");
  }

  /**
   * One file, one request. `XMLHttpRequest` is used rather than `fetch`
   * because it is the only API every mobile browser gives upload progress for.
   */
  private uploadOne(
    sessionId: string,
    token: string,
    index: number,
    file: File,
    options: { signal?: AbortSignal; onBytes: (sent: number, speedBps: number) => void },
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open("PUT", `/api/transfer/${encodeURIComponent(sessionId)}/${index}`, true);
      xhr.setRequestHeader("Authorization", `Bearer ${token}`);
      xhr.setRequestHeader("X-Offset", "0");
      xhr.setRequestHeader("Content-Type", file.type || "application/octet-stream");

      const started = performance.now();
      xhr.upload.onprogress = (event) => {
        const seconds = (performance.now() - started) / 1000;
        options.onBytes(event.loaded, seconds > 0.2 ? event.loaded / seconds : 0);
      };
      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) resolve();
        else reject(new Error(parseErrorBody(xhr.responseText, `Upload failed (${xhr.status}).`)));
      };
      xhr.onerror = () => reject(new Error("The connection dropped during the upload."));
      xhr.onabort = () => reject(new Error("Cancelled."));
      options.signal?.addEventListener("abort", () => xhr.abort(), { once: true });

      // Passing the `File` directly streams it from disk.
      xhr.send(file);
    });
  }

  /**
   * Share a snippet with the desktop. It travels inside the offer, so there is
   * no upload step — the moment it is accepted it has arrived.
   */
  async sendText(text: string, options: { pin?: string | null; signal?: AbortSignal } = {}): Promise<void> {
    if (!this.token) throw new Error("Scan the QR code again to reconnect.");
    const body = text.replace(/[\r\n]+$/, "");
    if (!body.trim()) throw new Error("There is nothing to send.");

    const bytes = new TextEncoder().encode(body).length;
    if (bytes > MAX_TEXT_BYTES) {
      throw new Error(
        `Text is limited to ${MAX_TEXT_BYTES / 1024} KB — send it as a file instead.`,
      );
    }

    const response = await fetch("/api/web/offer", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${this.token}`,
      },
      body: JSON.stringify({
        files: [{ rel_path: "shared text", size: bytes, mime: "text/plain" }],
        client_id: `web-${this.clientId}`,
        kind: "text",
        text: body,
        pin: options.pin ?? null,
      }),
      signal: options.signal,
    });
    if (!response.ok) {
      throw new Error(await describeError(response, "The other device refused the text."));
    }

    const created = await response.json();
    if (created.status === "accepted") return;
    // Otherwise the desktop is asking its user; wait for the answer.
    await this.waitForAcceptance(created.session_id, options.signal, undefined, created.status);
  }

  /** Download a file the desktop has offered to this browser. */
  downloadUrl(session: SessionView, index: number, token: string): string {
    return `/api/transfer/${encodeURIComponent(session.id)}/${index}?token=${encodeURIComponent(token)}`;
  }

  async cancel(sessionId: string, token: string): Promise<void> {
    await fetch(`/api/transfer/${encodeURIComponent(sessionId)}?token=${encodeURIComponent(token)}`, {
      method: "DELETE",
    });
  }
}

/** `webkitRelativePath` is only populated when a folder was picked. */
type FileWithPath = File & { webkitRelativePath?: string };

/**
 * Client-side tidy-up of the relative path. The receiver sanitises it again —
 * this is only so the user sees a sensible name before sending.
 */
export function sanitiseRelPath(input: string): string {
  const parts = input
    .replace(/\\/g, "/")
    .split("/")
    .map((part) => part.trim())
    .filter((part) => part && part !== "." && part !== "..");
  return parts.join("/") || "file";
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function parseErrorBody(text: string, fallback: string): string {
  try {
    const parsed = JSON.parse(text);
    if (typeof parsed?.detail === "string") return parsed.detail;
  } catch {
    // Not a structured error; fall through.
  }
  return fallback;
}

async function describeError(response: Response, fallback: string): Promise<string> {
  const text = await response.text().catch(() => "");
  return parseErrorBody(text, fallback);
}
