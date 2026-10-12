import { describe, expect, it } from "vitest";
import { wifiQrPayload } from "./wifi";

describe("wifiQrPayload", () => {
  it("builds a WPA payload", () => {
    expect(wifiQrPayload({ ssid: "Home", password: "secret123", security: "WPA" })).toBe(
      "WIFI:T:WPA;S:Home;P:secret123;;",
    );
  });

  it("escapes reserved characters in the name and password", () => {
    expect(wifiQrPayload({ ssid: 'Cafe;"Guest"', password: "a:b,c\\d", security: "WPA" })).toBe(
      'WIFI:T:WPA;S:Cafe\\;\\"Guest\\";P:a\\:b\\,c\\\\d;;',
    );
  });

  it("omits the password for an open network and marks hidden ones", () => {
    expect(
      wifiQrPayload({ ssid: "Open", password: "ignored", security: "nopass", hidden: true }),
    ).toBe("WIFI:T:nopass;S:Open;H:true;;");
  });
});
