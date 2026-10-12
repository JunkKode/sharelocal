/**
 * The phone/browser client.
 *
 * Loaded by scanning the QR code on a desktop. It runs the same bundle, but a
 * layout built for one thumb: big targets, a single column, no chrome.
 *
 * Sending and receiving are separate tabs. With both on one long page, files
 * sent *to* the phone landed below the fold, under the send controls, and
 * vanished the moment they finished — the most common "where did it go?".
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  ShareLocalClient,
  browserIsSupported,
  clearToken,
  describeBrowser,
  type UploadProgress,
} from "../platform/browser";
import { pairingTokenFromUrl, scrubPairingToken } from "../platform";
import type { AppEvent, DeviceInfo, FileMeta, SessionStatus, SessionView } from "../types";
import {
  baseName,
  describeFiles,
  formatBytes,
  formatEta,
  formatPercent,
  formatSpeed,
  percent,
} from "../lib/format";
import { canReadClipboard } from "../lib/clipboard";
import { Icon, type IconName } from "../components/Icon";
import { TextPreview } from "../components/TextComposer";
import { Toasts, type Toast } from "../components/Toasts";
import {
  Badge,
  Button,
  Card,
  IconButton,
  ProgressBar,
  Spinner,
  cx,
  inputClass,
} from "../components/ui";

type Phase = "checking" | "unsupported" | "needs-pairing" | "ready";
type Tab = "send" | "receive";

/** A file picked on the phone but not sent yet. */
interface Staged {
  key: string;
  file: File;
  /** Object URL for an image thumbnail; revoked when the file is dropped. */
  preview: string | null;
}

const FINISHED: SessionStatus[] = ["completed", "cancelled", "failed", "rejected"];
const IS_IOS = typeof navigator !== "undefined" && describeBrowser().os === "ios";

export function BrowserApp() {
  const clientRef = useRef<ShareLocalClient | null>(null);
  if (!clientRef.current && browserIsSupported()) clientRef.current = new ShareLocalClient();
  const client = clientRef.current;

  const [phase, setPhase] = useState<Phase>("checking");
  const [tab, setTab] = useState<Tab>("send");
  const [device, setDevice] = useState<DeviceInfo | null>(null);
  const [connected, setConnected] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [progress, setProgress] = useState<UploadProgress | null>(null);
  const [sendStatus, setSendStatus] = useState<SessionStatus | null>(null);
  const [sending, setSending] = useState(false);
  const [staged, setStaged] = useState<Staged[]>([]);
  const [incoming, setIncoming] = useState<SessionView[]>([]);
  const [seen, setSeen] = useState<Set<string>>(() => new Set());
  const [pin, setPin] = useState("");
  const [needsPin, setNeedsPin] = useState(false);
  const [text, setText] = useState("");
  const [sendingText, setSendingText] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  const sendingRef = useRef(false);
  sendingRef.current = sending;
  const deviceRef = useRef<DeviceInfo | null>(null);
  deviceRef.current = device;

  const notify = useCallback((level: Toast["level"], message: string) => {
    setToasts((current) => [
      ...current.slice(-3),
      { id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, level, message },
    ]);
  }, []);

  // Pair (if we arrived with a token), then open the control channel.
  useEffect(() => {
    if (!client) {
      setPhase("unsupported");
      return;
    }
    let cancelled = false;

    (async () => {
      const token = pairingTokenFromUrl();
      try {
        if (token) {
          await client.join(token);
          scrubPairingToken();
        } else if (!client.paired) {
          if (!cancelled) setPhase("needs-pairing");
          return;
        }
        const info = await client.deviceInfo();
        if (cancelled) return;
        setDevice(info);
        setNeedsPin(info.pin_required);
        setPhase("ready");
        client.connect();
      } catch (err) {
        if (cancelled) return;
        // A stale token in storage is the usual cause; force a fresh scan.
        clearToken();
        setPhase("needs-pairing");
        notify("error", err instanceof Error ? err.message : "Could not connect.");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [client, notify]);

  useEffect(() => {
    if (!client) return;
    const offConnection = client.onConnectionChange(setConnected);
    const offEvent = client.onEvent((event: AppEvent) => {
      if (event.type === "incoming-offer" && event.session.role === "sending") {
        // The desktop is offering us files to download.
        // `session.peer` is this phone (the session's other end, seen from the
        // computer), so the sender is named from `device` instead.
        const session = event.session;
        setIncoming((current) => [session, ...current.filter((s) => s.id !== session.id)]);
        navigator.vibrate?.(120);
        // Jump to it — unless a send is running, which the user is watching;
        // then a toast and the tab badge say it arrived.
        if (sendingRef.current) {
          notify("info", `${deviceRef.current?.name ?? "The computer"} sent you ${describePayload(session)}.`);
        } else {
          setTab("receive");
        }
      }
      if (event.type === "session-updated") {
        setIncoming((current) =>
          current.map((s) => (s.id === event.session.id ? event.session : s)),
        );
      }
      if (event.type === "notice") notify(event.level, event.message);
    });
    return () => {
      offConnection();
      offEvent();
    };
  }, [client, notify]);

  // Viewing the Receive tab marks everything in it as seen.
  useEffect(() => {
    if (tab !== "receive") return;
    setSeen((current) => {
      if (incoming.every((s) => current.has(s.id))) return current;
      return new Set([...current, ...incoming.map((s) => s.id)]);
    });
  }, [tab, incoming]);

  const unseen = incoming.filter((s) => !seen.has(s.id)).length;

  useEffect(() => {
    const name = device?.name ? `ShareLocal · ${device.name}` : "ShareLocal";
    document.title = unseen > 0 ? `(${unseen}) ${name}` : name;
  }, [unseen, device]);

  // Drop every thumbnail URL when the page goes away.
  const stagedRef = useRef(staged);
  stagedRef.current = staged;
  useEffect(
    () => () => stagedRef.current.forEach((s) => s.preview && URL.revokeObjectURL(s.preview)),
    [],
  );

  const dismissToast = useCallback(
    (id: string) => setToasts((current) => current.filter((t) => t.id !== id)),
    [],
  );

  const stage = (files: File[]) => {
    if (files.length === 0) return;
    setStaged((current) => {
      const known = new Set(current.map((s) => s.key));
      const added = files
        .map((file) => ({
          key: `${(file as File & { webkitRelativePath?: string }).webkitRelativePath || file.name}:${file.size}:${file.lastModified}`,
          file,
          preview: file.type.startsWith("image/") ? URL.createObjectURL(file) : null,
        }))
        .filter((s) => {
          if (known.has(s.key)) {
            if (s.preview) URL.revokeObjectURL(s.preview);
            return false;
          }
          known.add(s.key);
          return true;
        });
      return [...current, ...added];
    });
  };

  const unstage = (key: string) => {
    setStaged((current) => {
      const gone = current.find((s) => s.key === key);
      if (gone?.preview) URL.revokeObjectURL(gone.preview);
      return current.filter((s) => s.key !== key);
    });
  };

  const clearStaged = () => {
    setStaged((current) => {
      current.forEach((s) => s.preview && URL.revokeObjectURL(s.preview));
      return [];
    });
  };

  const send = async () => {
    const files = staged.map((s) => s.file);
    if (!client || files.length === 0) return;
    setSending(true);
    setProgress(null);
    setSendStatus("pending");
    const controller = new AbortController();
    abortRef.current = controller;
    try {
      await client.sendFiles(files, {
        pin: needsPin ? pin : null,
        signal: controller.signal,
        onProgress: setProgress,
        onStatus: setSendStatus,
      });
      notify("info", `Sent ${files.length} ${files.length === 1 ? "item" : "items"} to ${device?.name ?? "the computer"}.`);
      clearStaged();
    } catch (err) {
      notify("error", err instanceof Error ? err.message : "The transfer failed.");
    } finally {
      setSending(false);
      setProgress(null);
      setSendStatus(null);
      abortRef.current = null;
    }
  };

  const shareText = async () => {
    if (!client || !text.trim()) return;
    setSendingText(true);
    try {
      await client.sendText(text, { pin: needsPin ? pin : null });
      notify("info", "Text sent.");
      setText("");
    } catch (err) {
      notify("error", err instanceof Error ? err.message : "The text could not be sent.");
    } finally {
      setSendingText(false);
    }
  };

  if (phase === "checking") {
    return (
      <div className="grid h-full place-items-center">
        <Spinner className="size-6 text-ink-400" />
      </div>
    );
  }

  if (phase === "unsupported") {
    return (
      <CenteredMessage icon="alert" title="This browser is too old">
        ShareLocal needs a current version of Safari, Chrome, Firefox or Edge. Update your browser
        and open the link again.
      </CenteredMessage>
    );
  }

  if (phase === "needs-pairing") {
    return (
      <CenteredMessage icon="qr" title="Scan the QR code again">
        This link has expired or was never paired. On the computer running ShareLocal, press
        “Connect phone” and scan the new code.
      </CenteredMessage>
    );
  }

  const computer = device?.name ?? "the computer";
  const pinMissing = needsPin && pin.length < 4;

  return (
    <div className="mx-auto flex min-h-full w-full max-w-md flex-col gap-4 p-4 pb-10">
      <header className="flex items-center gap-3 pt-2">
        <span className="grid size-10 place-items-center rounded-xl bg-brand-600 text-white">
          <Icon name={device?.kind === "laptop" ? "laptop" : "desktop"} className="size-5" strokeWidth={2} />
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-[15px] font-semibold">{device?.name ?? "ShareLocal"}</p>
          <p className="flex items-center gap-1.5 text-[13px] text-ink-500 dark:text-ink-400">
            <span
              className={cx("size-1.5 rounded-full", connected ? "bg-emerald-500" : "bg-amber-500")}
            />
            {connected ? "Connected on this network" : "Reconnecting…"}
          </p>
        </div>
      </header>

      <div
        role="tablist"
        aria-label="Send or receive"
        className="sticky top-2 z-20 grid grid-cols-2 gap-1 rounded-2xl bg-ink-100/90 p-1 backdrop-blur dark:bg-ink-900/90"
      >
        <TabButton active={tab === "send"} icon="upload" onClick={() => setTab("send")}>
          Send
        </TabButton>
        <TabButton active={tab === "receive"} icon="download" onClick={() => setTab("receive")} badge={unseen}>
          Receive
        </TabButton>
      </div>

      {tab === "send" ? (
        <>
          {needsPin ? (
            <Card className="p-4">
              <label htmlFor="pin" className="flex items-center gap-1.5 text-sm font-medium">
                <Icon name="lock" className="size-4 text-ink-400" />
                Transfer PIN
              </label>
              <p className="mt-0.5 mb-2 text-xs text-ink-500 dark:text-ink-400">
                {computer} asks for a PIN before accepting anything.
              </p>
              <input
                id="pin"
                className={cx(inputClass, "text-center text-lg tracking-[0.4em]")}
                inputMode="numeric"
                maxLength={10}
                placeholder="••••"
                value={pin}
                onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))}
              />
            </Card>
          ) : null}

          {sending ? (
            <SendProgress
              status={sendStatus}
              progress={progress}
              count={staged.length}
              computer={computer}
              onCancel={() => abortRef.current?.abort()}
            />
          ) : staged.length > 0 ? (
            <StagedList
              staged={staged}
              computer={computer}
              disabled={pinMissing}
              disabledReason={pinMissing ? "Enter the PIN above first." : null}
              onAdd={stage}
              onRemove={unstage}
              onClear={clearStaged}
              onSend={() => void send()}
            />
          ) : (
            <Pickers computer={computer} onPick={stage} />
          )}

          <Card className="p-4">
            <label htmlFor="snippet" className="flex items-center gap-1.5 text-sm font-medium">
              <Icon name="text" className="size-4 text-ink-400" />
              Send text
            </label>
            <p className="mt-0.5 mb-2 text-xs text-ink-500 dark:text-ink-400">
              A link, a code, an address — it lands on the clipboard over there.
            </p>
            <textarea
              id="snippet"
              rows={3}
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder="Type or paste here…"
              className="w-full resize-y rounded-xl bg-white p-3 text-base leading-relaxed text-ink-900 ring-1 ring-ink-200 focus:ring-2 focus:ring-brand-500 focus:outline-none dark:bg-ink-800 dark:text-ink-100 dark:ring-ink-700"
            />
            <div className="mt-2 flex gap-2">
              {/* Reading the clipboard needs HTTPS, which a LAN address does not
                  have; a button that can only fail is worse than none. */}
              {canReadClipboard() ? (
                <Button
                  size="lg"
                  icon="copy"
                  className="flex-1"
                  onClick={async () => {
                    try {
                      setText(await navigator.clipboard.readText());
                    } catch {
                      notify("warn", "Your browser would not share the clipboard — paste it in instead.");
                    }
                  }}
                >
                  Paste
                </Button>
              ) : null}
              <Button
                variant="primary"
                size="lg"
                icon="upload"
                className="flex-1"
                disabled={!text.trim() || sendingText || pinMissing}
                onClick={() => void shareText()}
              >
                {sendingText ? "Sending…" : "Send text"}
              </Button>
            </div>
          </Card>
        </>
      ) : (
        <ReceiveList
          sessions={incoming}
          client={client!}
          computer={computer}
          onDismiss={(id) => setIncoming((current) => current.filter((s) => s.id !== id))}
        />
      )}

      <p className="mt-auto pt-6 text-center text-xs leading-relaxed text-ink-400 dark:text-ink-500">
        Files move directly between your devices on this network. Nothing is uploaded to the
        internet.
      </p>

      <Toasts toasts={toasts} onDismiss={dismissToast} />
    </div>
  );
}

function TabButton({
  active,
  icon,
  badge = 0,
  onClick,
  children,
}: {
  active: boolean;
  icon: IconName;
  badge?: number;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={cx(
        "flex h-11 items-center justify-center gap-2 rounded-xl text-[15px] font-medium transition",
        active
          ? "bg-white text-ink-900 shadow-sm dark:bg-ink-700 dark:text-white"
          : "text-ink-500 dark:text-ink-400",
      )}
    >
      <Icon name={icon} className="size-4.5" />
      {children}
      {badge > 0 ? (
        <span className="grid min-w-5 place-items-center rounded-full bg-brand-600 px-1.5 text-[11px] font-semibold text-white">
          {badge}
        </span>
      ) : null}
    </button>
  );
}

// ------------------------------------------------------------------ send

/**
 * The four ways to pick something. Hidden `<input>`s do the work; the
 * difference is all in `accept` / `capture`, which is what makes a phone open
 * its photo library rather than the Files app.
 */
function Pickers({ computer, onPick }: { computer: string; onPick: (files: File[]) => void }) {
  const photos = useRef<HTMLInputElement>(null);
  const camera = useRef<HTMLInputElement>(null);
  const files = useRef<HTMLInputElement>(null);
  const folder = useRef<HTMLInputElement>(null);

  const take = (e: React.ChangeEvent<HTMLInputElement>) => {
    onPick(Array.from(e.target.files ?? []));
    e.target.value = "";
  };

  return (
    <Card className="p-4">
      <p className="font-medium">Send to {computer}</p>
      <p className="mt-0.5 text-sm text-ink-500 dark:text-ink-400">
        Choose what to send — you can review it before it goes.
      </p>

      <div className="mt-4 grid grid-cols-2 gap-2.5">
        <PickTile
          icon="image"
          label="Photos & Videos"
          hint="From your gallery"
          primary
          onClick={() => photos.current?.click()}
        />
        <PickTile icon="camera" label="Camera" hint="Take one now" onClick={() => camera.current?.click()} />
        <PickTile icon="file" label="Files" hint="Documents, anything" onClick={() => files.current?.click()} />
        {IS_IOS ? null : (
          <PickTile icon="folder" label="Folder" hint="Keeps its structure" onClick={() => folder.current?.click()} />
        )}
      </div>

      {/* `image/*,video/*` is what makes iOS offer the Photo Library and
          Android open its gallery, instead of a file browser. */}
      <input ref={photos} type="file" accept="image/*,video/*" multiple hidden onChange={take} />
      {/* `capture` goes straight to the camera; desktop browsers ignore it
          and show a normal file picker, which is the sensible fallback. */}
      <input ref={camera} type="file" accept="image/*" capture="environment" hidden onChange={take} />
      <input ref={files} type="file" multiple hidden onChange={take} />
      <input
        ref={folder}
        type="file"
        hidden
        // Non-standard but supported by every desktop browser and Android
        // Chrome. iOS Safari does not support it, so the tile is hidden there.
        {...{ webkitdirectory: "", directory: "" }}
        onChange={take}
      />
    </Card>
  );
}

function PickTile({
  icon,
  label,
  hint,
  primary,
  onClick,
}: {
  icon: IconName;
  label: string;
  hint: string;
  primary?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cx(
        "flex flex-col items-start gap-2.5 rounded-2xl p-3.5 text-left transition active:scale-[0.98]",
        primary
          ? "bg-brand-600 text-white shadow-sm shadow-brand-600/25 dark:bg-brand-500"
          : "bg-ink-50 text-ink-900 ring-1 ring-ink-200/80 dark:bg-ink-800/60 dark:text-ink-100 dark:ring-ink-700",
      )}
    >
      <span
        className={cx(
          "grid size-10 place-items-center rounded-xl",
          primary
            ? "bg-white/20"
            : "bg-brand-100 text-brand-700 dark:bg-brand-500/15 dark:text-brand-300",
        )}
      >
        <Icon name={icon} className="size-5.5" />
      </span>
      <span>
        <span className="block text-[15px] font-semibold leading-tight">{label}</span>
        <span className={cx("mt-0.5 block text-xs", primary ? "text-white/80" : "text-ink-500 dark:text-ink-400")}>
          {hint}
        </span>
      </span>
    </button>
  );
}

/** What is about to be sent, with a last chance to change it. */
function StagedList({
  staged,
  computer,
  disabled,
  disabledReason,
  onAdd,
  onRemove,
  onClear,
  onSend,
}: {
  staged: Staged[];
  computer: string;
  disabled: boolean;
  disabledReason: string | null;
  onAdd: (files: File[]) => void;
  onRemove: (key: string) => void;
  onClear: () => void;
  onSend: () => void;
}) {
  const more = useRef<HTMLInputElement>(null);
  const total = staged.reduce((sum, s) => sum + s.file.size, 0);
  const images = staged.filter((s) => s.preview);
  const others = staged.filter((s) => !s.preview);

  return (
    <Card className="p-4 animate-rise">
      <div className="flex items-start justify-between gap-3">
        <div>
          <p className="font-medium">
            {staged.length} {staged.length === 1 ? "item" : "items"} ready
          </p>
          <p className="text-sm text-ink-500 dark:text-ink-400">
            {formatBytes(total)} · to {computer}
          </p>
        </div>
        <button
          type="button"
          onClick={onClear}
          className="h-9 rounded-lg px-2 text-sm font-medium text-ink-500 dark:text-ink-400"
        >
          Clear
        </button>
      </div>

      {images.length > 0 ? (
        <div className="mt-3 grid grid-cols-4 gap-1.5">
          {images.map((s) => (
            <div key={s.key} className="relative aspect-square overflow-hidden rounded-lg bg-ink-100 dark:bg-ink-800">
              <img src={s.preview!} alt={s.file.name} className="size-full object-cover" />
              <button
                type="button"
                aria-label={`Remove ${s.file.name}`}
                onClick={() => onRemove(s.key)}
                className="absolute top-1 right-1 grid size-6 place-items-center rounded-full bg-black/55 text-white"
              >
                <Icon name="close" className="size-3.5" strokeWidth={2.4} />
              </button>
            </div>
          ))}
        </div>
      ) : null}

      {others.length > 0 ? (
        <ul className="mt-3 flex max-h-64 flex-col divide-y divide-ink-100 overflow-y-auto dark:divide-ink-800">
          {others.map((s) => (
            <li key={s.key} className="flex items-center gap-3 py-2">
              <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-ink-100 text-ink-500 dark:bg-ink-800 dark:text-ink-400">
                <Icon name={fileIcon(s.file.type)} className="size-4.5" />
              </span>
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">
                  {(s.file as File & { webkitRelativePath?: string }).webkitRelativePath || s.file.name}
                </p>
                <p className="text-xs text-ink-500 dark:text-ink-400">{formatBytes(s.file.size)}</p>
              </div>
              <IconButton label={`Remove ${s.file.name}`} icon="close" onClick={() => onRemove(s.key)} />
            </li>
          ))}
        </ul>
      ) : null}

      <div className="mt-4 flex gap-2">
        <Button size="lg" icon="plus" onClick={() => more.current?.click()}>
          Add
        </Button>
        <Button variant="primary" size="lg" icon="upload" className="flex-1" disabled={disabled} onClick={onSend}>
          Send {staged.length} {staged.length === 1 ? "item" : "items"}
        </Button>
      </div>
      {disabledReason ? (
        <p className="mt-2 text-center text-xs text-amber-700 dark:text-amber-300">{disabledReason}</p>
      ) : null}

      <input
        ref={more}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          onAdd(Array.from(e.target.files ?? []));
          e.target.value = "";
        }}
      />
    </Card>
  );
}

function SendProgress({
  status,
  progress,
  count,
  computer,
  onCancel,
}: {
  status: SessionStatus | null;
  progress: UploadProgress | null;
  count: number;
  computer: string;
  onCancel: () => void;
}) {
  const waiting = !progress && (status === "pending" || status === null);

  return (
    <Card className="p-4 animate-rise">
      {waiting ? (
        <div className="flex flex-col items-center gap-3 py-4 text-center">
          <div className="relative grid size-14 place-items-center rounded-2xl bg-brand-100 text-brand-700 dark:bg-brand-500/15 dark:text-brand-300">
            <span className="absolute inset-0 rounded-2xl bg-brand-400/30 animate-pulse-ring" />
            <Icon name="desktop" className="size-6.5" />
          </div>
          <div>
            <p className="font-medium">Waiting for {computer}</p>
            <p className="mt-1 text-sm text-ink-500 dark:text-ink-400">
              Someone needs to click Accept on the computer.
            </p>
          </div>
        </div>
      ) : progress ? (
        <>
          <div className="flex items-baseline justify-between gap-3">
            <p className="text-sm text-ink-500 dark:text-ink-400">
              Sending {count > 1 ? `${progress.fileIndex + 1} of ${count}` : ""}
            </p>
            <span className="shrink-0 text-sm font-semibold tabular-nums text-brand-600 dark:text-brand-300">
              {formatPercent(progress.bytesDone, progress.totalBytes)}
            </span>
          </div>
          <p className="mt-0.5 truncate font-medium">{baseName(progress.fileName)}</p>
          <ProgressBar className="mt-3" value={percent(progress.bytesDone, progress.totalBytes)} />
          <div className="mt-2 flex flex-wrap items-center gap-x-3 text-[13px] tabular-nums text-ink-500 dark:text-ink-400">
            <span>
              {formatBytes(progress.bytesDone)} / {formatBytes(progress.totalBytes)}
            </span>
            <span>{formatSpeed(progress.speedBps)}</span>
            <span>{formatEta(progress.etaSecs)}</span>
          </div>
        </>
      ) : (
        <div className="flex items-center justify-center gap-2 py-4 text-sm text-ink-500">
          <Spinner /> Starting…
        </div>
      )}
      <Button className="mt-3 w-full" onClick={onCancel}>
        Cancel
      </Button>
    </Card>
  );
}

// --------------------------------------------------------------- receive

function ReceiveList({
  sessions,
  client,
  computer,
  onDismiss,
}: {
  sessions: SessionView[];
  client: ShareLocalClient;
  computer: string;
  onDismiss: (id: string) => void;
}) {
  if (sessions.length === 0) {
    return (
      <Card>
        <div className="flex flex-col items-center gap-3 px-6 py-12 text-center">
          <div className="grid size-14 place-items-center rounded-2xl bg-ink-100 text-ink-400 dark:bg-ink-800 dark:text-ink-500">
            <Icon name="download" className="size-6.5" />
          </div>
          <p className="font-medium">Nothing received yet</p>
          <p className="max-w-xs text-sm leading-relaxed text-ink-500 dark:text-ink-400">
            On {computer}, click this phone under Nearby devices and pick files. They show up here
            — keep this page open.
          </p>
        </div>
      </Card>
    );
  }

  const hasMedia = sessions.some((s) => s.files.some((f) => isMedia(f)));

  return (
    <div className="flex flex-col gap-3">
      {sessions.map((session) => (
        <IncomingCard
          key={session.id}
          session={session}
          client={client}
          computer={computer}
          onDismiss={onDismiss}
        />
      ))}
      {IS_IOS && hasMedia ? (
        <p className="flex gap-2 rounded-xl bg-ink-100 px-3 py-2.5 text-xs leading-relaxed text-ink-600 dark:bg-ink-800/70 dark:text-ink-300">
          <Icon name="image" className="mt-px size-4 shrink-0" />
          <span>
            Downloads go to the Files app → Downloads. To put a photo or video in Photos, open it
            there, tap Share, then “Save Image” or “Save Video”.
          </span>
        </p>
      ) : null}
    </div>
  );
}

/** A pull-direction session: the desktop offered files, we fetch them. */
function IncomingCard({
  session,
  client,
  computer,
  onDismiss,
}: {
  session: SessionView;
  client: ShareLocalClient;
  computer: string;
  onDismiss: (id: string) => void;
}) {
  const [token, setToken] = useState<string | null>(null);
  const [tapped, setTapped] = useState<Set<number>>(() => new Set());
  const isText = session.kind === "text";
  const finished = FINISHED.includes(session.status);
  const gone = session.status === "cancelled" || session.status === "failed" || session.status === "rejected";
  const allDone = session.status === "completed" || tapped.size === session.files.length;

  // The desktop pre-accepts what it offers, so the download token is ready on
  // first fetch. Text needs none.
  useEffect(() => {
    if (isText || finished) return;
    let cancelled = false;
    fetch(`/api/offer/${encodeURIComponent(session.id)}`, {
      headers: client.bearer ? { Authorization: `Bearer ${client.bearer}` } : undefined,
    })
      .then((r) => (r.ok ? r.json() : null))
      .then((body) => {
        if (!cancelled) setToken(body?.token ?? null);
      })
      .catch(() => {
        if (!cancelled) setToken(null);
      });
    return () => {
      cancelled = true;
    };
  }, [session.id, client, isText, finished]);

  const download = (index: number) => {
    if (!token) return;
    const a = document.createElement("a");
    a.href = client.downloadUrl(session, index, token);
    a.download = baseName(session.files[index]!.rel_path);
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTapped((current) => new Set(current).add(index));
  };

  const downloadAll = async () => {
    for (let i = 0; i < session.files.length; i += 1) {
      if (tapped.has(i)) continue;
      download(i);
      // Browsers drop back-to-back downloads triggered in one tick.
      await new Promise((resolve) => setTimeout(resolve, 900));
    }
  };

  const remaining = session.files.length - tapped.size;

  return (
    <Card className="p-4 animate-rise">
      <div className="flex items-start gap-3">
        <span
          className={cx(
            "grid size-10 shrink-0 place-items-center rounded-xl",
            allDone && !isText
              ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300"
              : "bg-brand-100 text-brand-700 dark:bg-brand-500/15 dark:text-brand-300",
          )}
        >
          <Icon
            name={allDone && !isText ? "check" : isText ? "text" : sessionIcon(session.files)}
            className="size-5"
          />
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">{isText ? "Text" : describeFiles(session.files)}</p>
          <p className="mt-0.5 text-[13px] text-ink-500 dark:text-ink-400">
            From {computer}
            {isText ? "" : ` · ${session.files.length > 1 ? `${session.files.length} files · ` : ""}${formatBytes(session.total_bytes)}`}
          </p>
        </div>
        <IconButton label="Remove from list" icon="close" onClick={() => onDismiss(session.id)} />
      </div>

      {isText && session.text ? (
        <TextPreview text={session.text} className="mt-3" />
      ) : gone ? (
        <div className="mt-3">
          <Badge tone="error">No longer available</Badge>
        </div>
      ) : (
        session.files.length === 1 ? (
          allDone ? (
            <SavedNote completed={session.status === "completed"} />
          ) : (
            <Button
              variant="primary"
              size="lg"
              icon="download"
              className="mt-3 w-full"
              disabled={!token}
              onClick={() => download(0)}
            >
              {token ? "Download" : "Preparing…"}
            </Button>
          )
        ) : (
        <>
          {allDone ? (
            <SavedNote completed={session.status === "completed"} />
          ) : (
            <Button
              variant="primary"
              size="lg"
              icon="download"
              className="mt-3 w-full"
              disabled={!token}
              onClick={() => void downloadAll()}
            >
              {token
                ? `Download ${remaining === session.files.length ? "all" : "the rest"} (${remaining})`
                : "Preparing…"}
            </Button>
          )}

          <ul className="mt-2 flex max-h-72 flex-col divide-y divide-ink-100 overflow-y-auto dark:divide-ink-800">
            {session.files.map((file, index) => {
              const done = tapped.has(index) || session.status === "completed";
              return (
                <li key={file.rel_path} className="flex items-center gap-3 py-2">
                  <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-ink-100 text-ink-500 dark:bg-ink-800 dark:text-ink-400">
                    <Icon name={fileIcon(file.mime ?? "")} className="size-4" />
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm">{baseName(file.rel_path)}</p>
                    <p className="text-xs text-ink-500 dark:text-ink-400">{formatBytes(file.size)}</p>
                  </div>
                  {done ? (
                    <Icon
                      name="check"
                      className="mx-2.5 size-4.5 text-emerald-600 dark:text-emerald-400"
                      strokeWidth={2.4}
                      title="Downloaded"
                    />
                  ) : (
                    <IconButton
                      label={`Download ${baseName(file.rel_path)}`}
                      icon="download"
                      disabled={!token}
                      onClick={() => download(index)}
                    />
                  )}
                </li>
              );
            })}
          </ul>
        </>
        )
      )}
    </Card>
  );
}

/** Shown once every file has been fetched. */
function SavedNote({ completed }: { completed: boolean }) {
  return (
    <p className="mt-3 flex items-center gap-2 rounded-xl bg-emerald-50 px-3 py-2.5 text-sm text-emerald-800 dark:bg-emerald-500/10 dark:text-emerald-200">
      <Icon name="check" className="size-4.5 shrink-0" strokeWidth={2.4} />
      {completed ? "Downloaded to this phone" : "Downloading — check your browser's downloads"}
    </p>
  );
}

function CenteredMessage({
  icon,
  title,
  children,
}: {
  icon: "alert" | "qr";
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="grid h-full place-items-center p-6">
      <div className="max-w-sm text-center">
        <div className="mx-auto grid size-12 place-items-center rounded-2xl bg-ink-100 text-ink-400 dark:bg-ink-800 dark:text-ink-500">
          <Icon name={icon} className="size-6" />
        </div>
        <p className="mt-3 font-medium">{title}</p>
        <p className="mt-1.5 text-sm leading-relaxed text-ink-500 dark:text-ink-400">{children}</p>
      </div>
    </div>
  );
}

// --------------------------------------------------------------- helpers

function isMedia(file: FileMeta): boolean {
  const mime = file.mime ?? "";
  return mime.startsWith("image/") || mime.startsWith("video/");
}

function fileIcon(mime: string): IconName {
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("video/")) return "camera";
  if (mime.startsWith("text/")) return "text";
  return "file";
}

function sessionIcon(files: FileMeta[]): IconName {
  return files.length > 0 && files.every(isMedia) ? "image" : "file";
}

function describePayload(session: SessionView): string {
  if (session.kind === "text") return "some text";
  const n = session.files.length;
  if (n === 1) return baseName(session.files[0]!.rel_path);
  if (session.files.every(isMedia)) return `${n} photos and videos`;
  return `${n} files`;
}
