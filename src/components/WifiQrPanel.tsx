import { useEffect, useMemo, useState } from "react";
import QRCode from "qrcode";
import type { PasswordAccess, WifiInfo, WifiSecurity } from "../types";
import { wifiQrPayload } from "../lib/wifi";
import { Icon } from "./Icon";
import { Modal } from "./Modal";
import { Button, IconButton, Spinner, cx, inputClass } from "./ui";

const SECURITY_LABEL: Record<WifiSecurity, string> = {
  WPA: "WPA / WPA2 / WPA3",
  WEP: "WEP",
  nopass: "None (open network)",
};

const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

/**
 * A QR code that joins this Wi-Fi network when a phone camera scans it — for
 * guests, and for getting a phone onto the network before it scans the
 * ShareLocal pairing code.
 *
 * The network is looked up in the background before the panel opens, so it is
 * filled in at once. The password is never read without asking: the first
 * time macOS shows its own administrator dialog; after that ShareLocal keeps a
 * copy and Touch ID is all it takes. Once unlocked in a session the QR code is
 * ready the moment the panel opens.
 */
export function WifiQrPanel({
  open,
  onClose,
  info,
  detecting,
  passwordFor,
  loadAccess,
  loadPassword,
  onRemember,
  onDownload,
  onCopyImage,
  onCopyText,
}: {
  open: boolean;
  onClose: () => void;
  /** The current network, looked up ahead of time; `null` until known. */
  info: WifiInfo | null;
  detecting: boolean;
  /** A password already unlocked this session, if any. */
  passwordFor: (ssid: string) => string | undefined;
  loadAccess: (ssid: string) => Promise<PasswordAccess>;
  loadPassword: (ssid: string) => Promise<string>;
  /** A password that was used: keep it for next time. */
  onRemember: (ssid: string, password: string) => void;
  /** Receives a PNG data URL and a suggested file name. */
  onDownload: (pngDataUrl: string, name: string) => void;
  onCopyImage: (pngDataUrl: string) => void;
  onCopyText: (text: string, what: string) => void;
}) {
  const [ssid, setSsid] = useState("");
  const [password, setPassword] = useState("");
  const [security, setSecurity] = useState<WifiSecurity>("WPA");
  const [hidden, setHidden] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  const [fetchingPassword, setFetchingPassword] = useState(false);
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const [access, setAccess] = useState<PasswordAccess | null>(null);

  // Each time the panel opens: the current network, and its password if it
  // was already unlocked this session — then the code is ready immediately.
  useEffect(() => {
    if (!open) return;
    setShowPassword(false);
    setPasswordError(null);
    const name = info?.ssid ?? "";
    setSsid(name);
    if (info) setSecurity(info.security);
    setPassword(name ? (passwordFor(name) ?? "") : "");
    // `info` arriving while open is handled below; this runs on open only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // The background lookup finished after the panel opened.
  useEffect(() => {
    if (!open || !info?.ssid) return;
    setSsid((current) => {
      if (current) return current;
      setSecurity(info.security);
      setPassword(passwordFor(info.ssid!) ?? "");
      return info.ssid!;
    });
  }, [open, info, passwordFor]);

  // What the password button will do for this network.
  useEffect(() => {
    if (!open) return;
    const name = ssid.trim();
    if (!name) {
      setAccess(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      loadAccess(name)
        .then((a) => {
          if (!cancelled) setAccess(a);
        })
        .catch(() => {});
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [open, ssid, loadAccess]);

  const needsPassword = security !== "nopass";
  const ready = ssid.trim().length > 0 && (!needsPassword || password.length > 0);
  const payload = useMemo(
    () => (ready ? wifiQrPayload({ ssid: ssid.trim(), password, security, hidden }) : null),
    [ready, ssid, password, security, hidden],
  );

  useEffect(() => {
    let cancelled = false;
    if (!payload) {
      setPreview(null);
      return;
    }
    QRCode.toDataURL(payload, {
      errorCorrectionLevel: "M",
      margin: 1,
      width: 512,
      color: { dark: "#16161d", light: "#ffffff" },
    })
      .then((data) => {
        if (!cancelled) setPreview(data);
      })
      .catch(() => {
        if (!cancelled) setPreview(null);
      });
    return () => {
      cancelled = true;
    };
  }, [payload]);

  const fillPassword = async () => {
    const name = ssid.trim();
    setFetchingPassword(true);
    setPasswordError(null);
    try {
      const found = await loadPassword(name);
      setPassword(found);
      onRemember(name, found);
      setAccess((a) => (a ? { ...a, remembered: true } : a));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      // Dismissing the system sheet is a choice, not an error.
      if (message !== "cancelled") setPasswordError(message);
    } finally {
      setFetchingPassword(false);
    }
  };

  /** Using the code (saving, copying) means the password is right: keep it. */
  const used = () => {
    if (needsPassword && password && ssid.trim()) onRemember(ssid.trim(), password);
  };

  const withCard = async (use: (png: string) => void) => {
    if (!payload) return;
    used();
    use(await renderCard(payload, ssid.trim()));
  };

  const unlockLabel = access?.remembered
    ? access.biometrics
      ? "Touch ID"
      : "Unlock"
    : IS_MAC
      ? "From Keychain"
      : "Saved password";

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Share Wi-Fi"
      description="Point a phone camera at this code to join the network — no typing the password."
      footer={<Button onClick={onClose}>Done</Button>}
    >
      <div className="flex flex-col gap-4">
        <div className="mx-auto grid size-56 place-items-center rounded-2xl bg-white p-3 ring-1 ring-ink-200 dark:ring-ink-700">
          {preview ? (
            <img src={preview} alt={`QR code to join ${ssid}`} className="size-full" />
          ) : detecting && !ssid ? (
            <div className="flex flex-col items-center gap-2 text-ink-500">
              <Spinner className="size-6" />
              <span className="text-xs">Finding your network…</span>
            </div>
          ) : ssid.trim() && needsPassword ? (
            // The one step left is the password: offer it right where the
            // code will appear.
            <div className="flex flex-col items-center gap-3 px-4 text-center text-ink-600">
              <Icon name={access?.remembered && access.biometrics ? "fingerprint" : "lock"} className="size-8 text-brand-500" />
              <Button
                variant="primary"
                size="sm"
                disabled={fetchingPassword}
                onClick={() => void fillPassword()}
              >
                {fetchingPassword ? <Spinner /> : null}
                {access?.remembered
                  ? access.biometrics
                    ? "Unlock with Touch ID"
                    : "Unlock password"
                  : "Get password"}
              </Button>
              <span className="text-[11px] leading-snug text-ink-500">or type it below</span>
            </div>
          ) : (
            <div className="flex flex-col items-center gap-2 px-6 text-center text-ink-500">
              <Icon name="wifi" className="size-7 text-ink-300" />
              <span className="text-xs leading-relaxed">Enter the network name.</span>
            </div>
          )}
        </div>

        <div className="grid grid-cols-3 gap-2">
          <Button
            size="sm"
            icon="download"
            disabled={!payload}
            onClick={() => void withCard((png) => onDownload(png, `Wi-Fi ${ssid.trim()}`))}
          >
            Save PNG
          </Button>
          <Button
            size="sm"
            icon="copy"
            disabled={!payload}
            onClick={() => void withCard(onCopyImage)}
          >
            Copy image
          </Button>
          <Button
            size="sm"
            icon="text"
            disabled={!payload}
            onClick={() => {
              used();
              const lines = [`Wi-Fi: ${ssid.trim()}`];
              if (needsPassword) lines.push(`Password: ${password}`);
              onCopyText(lines.join("\n"), "Wi-Fi details");
            }}
          >
            Copy text
          </Button>
        </div>

        <div className="flex flex-col gap-3 border-t border-ink-200/70 pt-4 dark:border-ink-800">
          <div>
            <label htmlFor="wifi-ssid" className="mb-1.5 block text-sm font-medium">
              Network name
            </label>
            <input
              id="wifi-ssid"
              className={inputClass}
              value={ssid}
              placeholder={detecting ? "Detecting…" : "e.g. Home Wi-Fi"}
              onChange={(e) => setSsid(e.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
          </div>

          {needsPassword ? (
            <div>
              <label htmlFor="wifi-password" className="mb-1.5 block text-sm font-medium">
                Password
              </label>
              <div className="flex gap-2">
                <div className="relative flex-1">
                  <input
                    id="wifi-password"
                    className={cx(inputClass, "pr-20")}
                    type={showPassword ? "text" : "password"}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <div className="absolute inset-y-0 right-1 flex items-center">
                    <IconButton
                      label={showPassword ? "Hide password" : "Show password"}
                      icon={showPassword ? "eyeOff" : "eye"}
                      onClick={() => setShowPassword((v) => !v)}
                    />
                    <IconButton
                      label="Copy password"
                      icon="copy"
                      disabled={!password}
                      onClick={() => {
                        used();
                        onCopyText(password, "Password");
                      }}
                    />
                  </div>
                </div>
                <Button
                  icon={fetchingPassword ? undefined : access?.remembered && access.biometrics ? "fingerprint" : "lock"}
                  disabled={!ssid.trim() || fetchingPassword}
                  onClick={() => void fillPassword()}
                >
                  {fetchingPassword ? <Spinner /> : null}
                  {unlockLabel}
                </Button>
              </div>
              {passwordError ? (
                <p className="mt-1.5 text-xs text-rose-600 dark:text-rose-400">{passwordError}</p>
              ) : access?.user_name && !password ? (
                // Only macOS reports a user name: elsewhere there is no
                // administrator dialog to prepare anyone for.
                access.remembered ? (
                  <p className="mt-1.5 text-xs text-ink-500 dark:text-ink-400">
                    ShareLocal remembers this password.{" "}
                    {access.biometrics ? "Touch ID unlocks it" : "Your Mac password unlocks it"} — no
                    user name needed.
                  </p>
                ) : (
                  <p className="mt-1.5 rounded-lg bg-ink-100 px-2.5 py-2 text-xs leading-relaxed text-ink-600 dark:bg-ink-800 dark:text-ink-300">
                    The first time, macOS asks for an administrator name and password. Type{" "}
                    <strong className="font-semibold text-ink-900 dark:text-white">
                      {access.user_name}
                    </strong>{" "}
                    as the user name and your Mac login password. After that,{" "}
                    {access.biometrics ? "Touch ID is enough" : "only your password is needed"}.
                  </p>
                )
              ) : null}
            </div>
          ) : null}

          <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
            <label className="flex items-center gap-2 text-sm">
              <span className="text-ink-600 dark:text-ink-300">Security</span>
              <select
                value={security}
                onChange={(e) => setSecurity(e.target.value as WifiSecurity)}
                className="h-9 rounded-lg bg-white px-2 text-sm ring-1 ring-ink-200 focus:ring-2 focus:ring-brand-500 focus:outline-none dark:bg-ink-800 dark:ring-ink-700"
              >
                {(Object.keys(SECURITY_LABEL) as WifiSecurity[]).map((key) => (
                  <option key={key} value={key}>
                    {SECURITY_LABEL[key]}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex items-center gap-2 text-sm text-ink-600 dark:text-ink-300">
              <input
                type="checkbox"
                checked={hidden}
                onChange={(e) => setHidden(e.target.checked)}
                className="size-4 accent-brand-600"
              />
              Hidden network
            </label>
          </div>
        </div>
      </div>
    </Modal>
  );
}

/**
 * The QR on a printable card with the network name under it — what gets
 * saved or copied, so the image still makes sense in a chat or on a wall.
 */
async function renderCard(payload: string, ssid: string): Promise<string> {
  const qrSize = 520;
  const pad = 40;
  const width = qrSize + pad * 2;
  const height = qrSize + pad * 2 + 110;

  const qr = document.createElement("canvas");
  await QRCode.toCanvas(qr, payload, {
    errorCorrectionLevel: "M",
    margin: 0,
    width: qrSize,
    color: { dark: "#16161d", light: "#ffffff" },
  });

  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return qr.toDataURL("image/png");

  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, width, height);
  ctx.drawImage(qr, pad, pad, qrSize, qrSize);

  ctx.textAlign = "center";
  ctx.fillStyle = "#16161d";
  ctx.font = "600 34px -apple-system, 'Segoe UI', Roboto, sans-serif";
  ctx.fillText(fitText(ctx, ssid, width - pad * 2), width / 2, qrSize + pad + 58);
  ctx.fillStyle = "#6b6b81";
  ctx.font = "24px -apple-system, 'Segoe UI', Roboto, sans-serif";
  ctx.fillText("Scan with your camera to join this Wi-Fi", width / 2, qrSize + pad + 96);

  return canvas.toDataURL("image/png");
}

function fitText(ctx: CanvasRenderingContext2D, text: string, max: number): string {
  if (ctx.measureText(text).width <= max) return text;
  let cut = text;
  while (cut.length > 1 && ctx.measureText(`${cut}…`).width > max) cut = cut.slice(0, -1);
  return `${cut}…`;
}
