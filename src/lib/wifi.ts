import type { WifiSecurity } from "../types";

/**
 * Backslash-escape the characters the Wi-Fi QR format reserves. Without this
 * a password containing `;` silently truncates and the phone fails to join.
 */
function escape(value: string): string {
  return value.replace(/([\\;,:"])/g, "\\$1");
}

/**
 * The `WIFI:` payload phone cameras recognise (ZXing's de-facto standard,
 * understood by iOS and Android alike). Scanning it offers to join the network.
 */
export function wifiQrPayload({
  ssid,
  password,
  security,
  hidden = false,
}: {
  ssid: string;
  password: string;
  security: WifiSecurity;
  hidden?: boolean;
}): string {
  const parts = [`T:${security}`, `S:${escape(ssid)}`];
  if (security !== "nopass") parts.push(`P:${escape(password)}`);
  if (hidden) parts.push("H:true");
  return `WIFI:${parts.join(";")};;`;
}
