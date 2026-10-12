import { beforeEach, describe, expect, it, vi } from "vitest";
import {
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
