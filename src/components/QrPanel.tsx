import { useEffect, useRef, useState } from "react";
import QRCode from "qrcode";
import type { Peer } from "../types";
import { deviceIcon, Icon } from "./Icon";
import { Modal } from "./Modal";
import { Button, IconButton, Spinner } from "./ui";

/**
 * Shows a QR code containing a one-time pairing URL for this device.
 *
 * A phone scans it, opens the address on the local network and gets the same
 * ShareLocal interface with no app to install. The token is short-lived and can
 * be revoked from here.
 */
export function QrPanel({
  open,
  url,
  expiresInSecs,
  onClose,
  onRefresh,
  onRevoke,
  peers,
  onCopyLink,
  onShareWifi,
}: {
  open: boolean;
  url: string | null;
  expiresInSecs: number;
  onClose: () => void;
  onRefresh: () => void;
  onRevoke: () => void;
  /** Every known peer; the panel watches for a new QR-paired one. */
  peers: Peer[];
  onCopyLink: (url: string) => void;
  onShareWifi: () => void;
}) {
  const [image, setImage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [joined, setJoined] = useState<Peer | null>(null);
  const seenAtOpen = useRef<Map<string, Peer["status"]> | null>(null);

  // Remember which phones were already paired when the panel opened, so the
  // one that scans the code now can be greeted by name.
  useEffect(() => {
    if (!open) {
      seenAtOpen.current = null;
      setJoined(null);
      return;
    }
    const phones = peers.filter((p) => p.source === "qr");
    if (!seenAtOpen.current) {
      seenAtOpen.current = new Map(phones.map((p) => [p.id, p.status]));
      return;
    }
    // New, or a phone that had dropped off and just scanned again.
    const fresh = phones.find((p) => {
      const before = seenAtOpen.current!.get(p.id);
      return p.status === "online" && before !== "online";
    });
    if (fresh) setJoined(fresh);
  }, [open, peers]);

  useEffect(() => {
    let cancelled = false;
    if (!url) {
      setImage(null);
      return;
    }
    QRCode.toDataURL(url, {
      errorCorrectionLevel: "M",
      margin: 1,
      width: 512,
      color: { dark: "#16161d", light: "#ffffff" },
    })
      .then((data) => {
        if (!cancelled) {
          setImage(data);
          setError(null);
        }
      })
      .catch(() => {
        if (!cancelled) setError("Could not draw the QR code.");
      });
    return () => {
      cancelled = true;
    };
  }, [url]);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Connect a phone"
      description="Scan this with a phone on the same Wi-Fi. It opens ShareLocal in the browser — nothing to install."
      footer={
        joined ? undefined : (
        <>
          <Button
            variant="ghost"
            onClick={() => {
              onRevoke();
              onClose();
            }}
          >
            Revoke code
          </Button>
          <Button icon="refresh" onClick={onRefresh}>
            New code
          </Button>
        </>
        )
      }
    >
      {joined ? (
        <div className="flex flex-col items-center gap-3 py-6 text-center animate-rise">
          <div className="grid size-16 place-items-center rounded-2xl bg-emerald-100 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300">
            <Icon name={deviceIcon(joined.kind, joined.os)} className="size-8" />
          </div>
          <div>
            <p className="flex items-center justify-center gap-1.5 font-semibold">
              <Icon name="check" className="size-4.5 text-emerald-600" strokeWidth={2.4} />
              {joined.name} is connected
            </p>
            <p className="mt-1 max-w-xs text-sm leading-relaxed text-ink-500 dark:text-ink-400">
              It now appears under Nearby devices. Click it to send files — the phone taps
              Download to save them.
            </p>
          </div>
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        </div>
      ) : (
      <div className="flex flex-col items-center gap-4">
        <div className="grid size-64 place-items-center rounded-2xl bg-white p-3 ring-1 ring-ink-200 dark:ring-ink-700">
          {image ? (
            <img src={image} alt="QR code for the ShareLocal pairing link" className="size-full" />
          ) : error ? (
            <p className="px-4 text-center text-sm text-rose-600">{error}</p>
          ) : (
            <Spinner className="size-6 text-ink-400" />
          )}
        </div>

        <ol className="w-full space-y-1.5 text-sm text-ink-600 dark:text-ink-300">
          {[
            "Open the phone camera and point it at the code.",
            "Tap the link that appears — it opens in the browser.",
            "This window confirms when the phone is connected.",
          ].map((step, i) => (
            <li key={step} className="flex gap-2.5">
              <span className="grid size-5 shrink-0 place-items-center rounded-full bg-brand-100 text-[11px] font-semibold text-brand-700 dark:bg-brand-500/15 dark:text-brand-300">
                {i + 1}
              </span>
              {step}
            </li>
          ))}
        </ol>

        {url ? (
          <div className="w-full">
            <p className="mb-1 text-center text-xs text-ink-500 dark:text-ink-400">
              or type this address into the phone's browser
            </p>
            <div className="flex items-center gap-1 rounded-xl bg-ink-100 pl-3 dark:bg-ink-800">
              <code className="min-w-0 flex-1 truncate py-2 text-center text-[13px] text-ink-700 dark:text-ink-200">
                {url.split("?")[0]}
              </code>
              <IconButton label="Copy pairing link" icon="copy" onClick={() => onCopyLink(url)} />
            </div>
          </div>
        ) : null}

        <p className="text-center text-xs text-ink-500 dark:text-ink-400">
          The phone must be on the same Wi-Fi.{" "}
          <button
            type="button"
            onClick={onShareWifi}
            className="font-medium text-brand-600 underline underline-offset-2 dark:text-brand-300"
          >
            Share the Wi-Fi with a QR code
          </button>
          . This code expires in {Math.round(expiresInSecs / 60)} minutes.
        </p>
      </div>
      )}
    </Modal>
  );
}
