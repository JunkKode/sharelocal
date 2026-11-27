import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  AUTH_EXPIRED_CLOSE_CODE,
  ShareLocalClient,
  browserIsSupported,
  describeBrowser,
  localClientId,
  sanitiseRelPath,
  storeToken,
  storedToken,
  clearToken,
} from "./browser";
import { pairingTokenFromUrl } from "./index";

function setUserAgent(value: string) {
  Object.defineProperty(navigator, "userAgent", { value, configurable: true });
}

describe("sanitiseRelPath", () => {
  it("keeps folder structure from a folder picker", () => {
    expect(sanitiseRelPath("Trip/day1/a.jpg")).toBe("Trip/day1/a.jpg");
    expect(sanitiseRelPath("movie.mp4")).toBe("movie.mp4");
  });

  it("normalises Windows separators", () => {
    expect(sanitiseRelPath("Trip\\day1\\a.jpg")).toBe("Trip/day1/a.jpg");
  });

  it("strips traversal segments before they are ever sent", () => {
    expect(sanitiseRelPath("../../etc/passwd")).toBe("etc/passwd");
    expect(sanitiseRelPath("a/../../b.txt")).toBe("a/b.txt");
    expect(sanitiseRelPath("/etc/passwd")).toBe("etc/passwd");
    expect(sanitiseRelPath("./x/./y.txt")).toBe("x/y.txt");
  });

  it("always yields something usable", () => {
    expect(sanitiseRelPath("")).toBe("file");
    expect(sanitiseRelPath("../..")).toBe("file");
    expect(sanitiseRelPath("   ")).toBe("file");
  });
});

describe("describeBrowser", () => {
  it("recognises an iPhone", () => {
    setUserAgent(
      "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    );
    const info = describeBrowser();
    expect(info.os).toBe("ios");
    expect(info.kind).toBe("phone");
    expect(info.name).toContain("iPhone");
    expect(info.name).toContain("Safari");
  });

  it("recognises Android Chrome", () => {
    setUserAgent(
      "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
    );
    const info = describeBrowser();
    expect(info.os).toBe("android");
    expect(info.kind).toBe("phone");
    expect(info.name).toContain("Chrome");
  });

  it("falls back sensibly on a desktop browser", () => {
    setUserAgent(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    );
    const info = describeBrowser();
    expect(info.os).toBe("windows");
    expect(info.kind).toBe("browser");
  });
});

describe("browserIsSupported", () => {
  it("accepts a current browser", () => {
    expect(browserIsSupported()).toBe(true);
  });

  it("rejects one without the APIs streaming uploads need", () => {
    const original = globalThis.WebSocket;
    // @ts-expect-error deliberately removing the API to model an old browser
    delete globalThis.WebSocket;
    expect(browserIsSupported()).toBe(false);
    globalThis.WebSocket = original;
  });
});

describe("client identity", () => {
  beforeEach(() => localStorage.clear());

  it("is stable across reloads", () => {
    const first = localClientId();
    expect(first).toHaveLength(32);
    expect(localClientId()).toBe(first);
  });

  it("stores and clears the pairing token", () => {
    expect(storedToken()).toBeNull();
    storeToken("abc123");
    expect(storedToken()).toBe("abc123");
    clearToken();
    expect(storedToken()).toBeNull();
  });
});

describe("pairingTokenFromUrl", () => {
  it("reads the token the QR code carries", () => {
    expect(pairingTokenFromUrl("?t=abc-123")).toBe("abc-123");
    expect(pairingTokenFromUrl("?other=1&t=xyz")).toBe("xyz");
  });

  it("returns null when there is none", () => {
    expect(pairingTokenFromUrl("")).toBeNull();
    expect(pairingTokenFromUrl("?t=")).toBeNull();
    expect(pairingTokenFromUrl("?other=1")).toBeNull();
  });
});

describe("secure-context independence", () => {
  // ShareLocal is served over plain HTTP from a LAN address, so anything
  // gated behind a secure context is simply missing on real phones.
  it("does not require crypto.randomUUID", () => {
    localStorage.clear();
    const original = crypto.randomUUID;
    // @ts-expect-error modelling a non-secure context
    delete (crypto as Crypto).randomUUID;

    expect(browserIsSupported()).toBe(true);
    const id = localClientId();
    expect(id).toHaveLength(32);
    expect(id).toMatch(/^[0-9a-f]{32}$/);

    crypto.randomUUID = original;
    vi.restoreAllMocks();
  });

  it("still produces an id when storage throws", () => {
    const spy = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const setSpy = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });

    expect(() => localClientId()).not.toThrow();
    expect(localClientId()).toHaveLength(32);
    expect(storedToken()).toBeNull();

    spy.mockRestore();
    setSpy.mockRestore();
  });
});

class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: FakeWebSocket[] = [];

  readyState = FakeWebSocket.CONNECTING;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: ((ev: { code?: number }) => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(public url: string) {
    FakeWebSocket.instances.push(this);
  }

  send(data: string) {
    this.sent.push(data);
  }

  open() {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.();
  }

  receive(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  close(code = 1000) {
    if (this.readyState === FakeWebSocket.CLOSED) return;
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose?.({ code });
  }
}

describe("ShareLocalClient reconnection", () => {
  const OriginalWebSocket = globalThis.WebSocket;

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    FakeWebSocket.instances = [];
    // @ts-expect-error test double
    globalThis.WebSocket = FakeWebSocket;
  });

  afterEach(() => {
    globalThis.WebSocket = OriginalWebSocket;
    vi.useRealTimers();
  });

  it("recovers when a socket gets stuck in CONNECTING during mobile sleep", () => {
    storeToken("valid-token");
    const client = new ShareLocalClient();
    const states: boolean[] = [];
    client.onConnectionChange((c) => states.push(c));

    client.connect();
    expect(FakeWebSocket.instances).toHaveLength(1);
    const first = FakeWebSocket.instances[0]!;
    expect(first.readyState).toBe(FakeWebSocket.CONNECTING);

    // 4 seconds later, the user wakes the phone and returns to the tab.
    vi.advanceTimersByTime(4_000);
    document.dispatchEvent(new Event("visibilitychange"));

    expect(FakeWebSocket.instances).toHaveLength(2);
    const second = FakeWebSocket.instances[1]!;
    second.open();
    expect(states[states.length - 1]).toBe(true);
    client.disconnect();
  });

  it("probes an OPEN socket on wake and reconnects if it is half-open", () => {
    storeToken("valid-token");
    const client = new ShareLocalClient();
    client.connect();
    const first = FakeWebSocket.instances[0]!;
    first.open();

    // Wake while socket still claims OPEN; client sends an immediate ping probe.
    vi.advanceTimersByTime(5_000);
    window.dispatchEvent(new Event("focus"));
    expect(first.sent.some((m) => m.includes('"type":"ping"'))).toBe(true);

    // No pong arrives within 4 seconds → half-open socket is replaced.
    vi.advanceTimersByTime(4_500);
    expect(FakeWebSocket.instances).toHaveLength(2);
    client.disconnect();
  });

  it("clears the stored token and notifies when the server closes with 4001", () => {
    storeToken("expired-token");
    const client = new ShareLocalClient();
    const expired = vi.fn();
    client.onAuthExpired(expired);

    client.connect();
    const sock = FakeWebSocket.instances[0]!;
    sock.close(AUTH_EXPIRED_CLOSE_CODE);

    expect(expired).toHaveBeenCalledTimes(1);
    expect(storedToken()).toBeNull();
    expect(client.paired).toBe(false);
    // Must not keep retrying with a rejected token.
    vi.advanceTimersByTime(10_000);
    expect(FakeWebSocket.instances).toHaveLength(1);
    client.disconnect();
  });
});
