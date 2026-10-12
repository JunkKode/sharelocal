/**
 * ShareLocal ships one bundle that runs in two places: inside the Tauri window
 * on a desktop, and served over HTTP to a phone that scanned the QR code.
 * This module decides which.
 */

export const isTauri =
  typeof window !== "undefined" &&
  ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

/** The pairing token a phone arrives with, taken from `?t=…`. */
export function pairingTokenFromUrl(search = window.location.search): string | null {
  const token = new URLSearchParams(search).get("t");
  return token && token.trim() ? token : null;
}

/**
 * Strip the pairing token from the address bar once it has been redeemed, so
 * it is not left in history, bookmarks or a screenshot.
 */
export function scrubPairingToken(): void {
  if (typeof window === "undefined" || !window.history?.replaceState) return;
  const url = new URL(window.location.href);
  if (!url.searchParams.has("t")) return;
  url.searchParams.delete("t");
  window.history.replaceState({}, "", url.pathname + url.search + url.hash);
}
