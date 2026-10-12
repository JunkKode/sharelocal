import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  UsbConnection,
  UsbDevice,
  UsbEntry,
  UsbEntryKind,
  UsbError,
  UsbProgress,
  UsbStorage,
} from "../types";
import { formatBytes, formatEta, formatPercent, formatSpeed, percent } from "../lib/format";
import { Icon, type IconName } from "../components/Icon";
import { Modal } from "../components/Modal";
import {
  Badge,
  Button,
  IconButton,
  ProgressBar,
  SectionTitle,
  Spinner,
  cx,
  inputClass,
} from "../components/ui";

/**
 * Everything the USB view needs from the outside world. Injected, so the view
 * can be tested — and previewed — without a phone or Tauri.
 */
export interface UsbApi {
  devices(): Promise<UsbDevice[]>;
  connect(id: string, release: boolean): Promise<UsbConnection>;
  disconnect(): Promise<void>;
  storages(): Promise<UsbStorage[]>;
  list(storage: string, parent: string | null, hidden?: boolean): Promise<UsbEntry[]>;
  thumbnail(storage: string, handle: string): Promise<string>;
  createFolder(storage: string, parent: string | null, name: string): Promise<string>;
  remove(storage: string, handles: string[]): Promise<void>;
  download(storage: string, handles: string[], dest: string | null): Promise<string>;
  upload(storage: string, parent: string | null, paths: string[]): Promise<string>;
  cancel(): Promise<void>;
  onProgress(handler: (progress: UsbProgress) => void): () => void;
  /** The mouse's side buttons, caught natively where the web view drops them. */
  onMouseNav?(handler: (direction: "back" | "forward") => void): () => void;
  pickFiles(): Promise<string[]>;
  pickFolder(): Promise<string[]>;
  pickDestination(): Promise<string | null>;
  reveal(path: string): void;
}

type Notify = (level: "info" | "warn" | "error", message: string) => void;

const POLL_MS = 2000;
/** Thumbnails cost a USB round trip each; stop asking after this many. */
const MAX_THUMBNAILS = 300;

interface Crumb {
  handle: string;
  name: string;
}

function asUsbError(err: unknown): UsbError {
  if (err && typeof err === "object" && "code" in err && "message" in err) return err as UsbError;
  return { code: "other", message: err instanceof Error ? err.message : String(err) };
}

/**
 * An Android phone on a cable, browsed like a folder. macOS cannot do this on
 * its own — it does not speak MTP — so this is the Finder window it lacks.
 */
export function UsbView({
  api,
  notify,
  droppedPaths,
  dropActive,
  onDropHandled,
}: {
  api: UsbApi;
  notify: Notify;
  /** Files dragged in from Finder while this view is showing. */
  droppedPaths: string[] | null;
  dropActive: boolean;
  onDropHandled: () => void;
}) {
  const [devices, setDevices] = useState<UsbDevice[] | null>(null);
  const [conn, setConn] = useState<UsbConnection | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<UsbError | null>(null);
  /** A phone the user disconnected on purpose is not reopened behind their back. */
  const [dismissedId, setDismissedId] = useState<string | null>(null);

  const [storageId, setStorageId] = useState<string | null>(null);
  // Folder history, like a browser's: where we are, and where Back and
  // Forward lead.
  const [nav, setNav] = useState<{ path: Crumb[]; back: Crumb[][]; forward: Crumb[][] }>({
    path: [],
    back: [],
    forward: [],
  });
  const path = nav.path;
  const [entries, setEntries] = useState<UsbEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [progress, setProgress] = useState<UsbProgress | null>(null);
  const [newFolder, setNewFolder] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [showHidden, setShowHidden] = useState(false);

  const parent = path.length ? path[path.length - 1]!.handle : null;

  /** Go somewhere new: here goes on the Back stack, Forward is cleared. */
  const navigate = useCallback((next: Crumb[]) => {
    setNav((n) =>
      samePath(n.path, next) ? n : { path: next, back: [...n.back, n.path].slice(-200), forward: [] },
    );
  }, []);
  const resetNav = useCallback(() => setNav({ path: [], back: [], forward: [] }), []);
  const goBack = useCallback(() => {
    setNav((n) =>
      n.back.length
        ? { path: n.back[n.back.length - 1]!, back: n.back.slice(0, -1), forward: [n.path, ...n.forward] }
        : n,
    );
  }, []);
  const goForward = useCallback(() => {
    setNav((n) =>
      n.forward.length
        ? { path: n.forward[0]!, back: [...n.back, n.path], forward: n.forward.slice(1) }
        : n,
    );
  }, []);
  const storage = conn?.storages.find((s) => s.id === storageId) ?? conn?.storages[0] ?? null;
  const busy = progress !== null && !progress.finished;

  // ----------------------------------------------------------- devices

  useEffect(() => {
    let cancelled = false;
    const poll = () =>
      api
        .devices()
        .then((list) => {
          if (!cancelled) setDevices(list);
        })
        .catch(() => {
          if (!cancelled) setDevices([]);
        });
    void poll();
    const timer = setInterval(poll, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [api]);

  const connect = useCallback(
    async (id: string, release: boolean) => {
      setConnecting(true);
      setError(null);
      try {
        const next = await api.connect(id, release);
        setConn(next);
        setStorageId(next.storages[0]?.id ?? null);
        resetNav();
        setDismissedId(null);
      } catch (err) {
        setConn(null);
        setError({ ...asUsbError(err), deviceId: id } as UsbError & { deviceId: string });
      } finally {
        setConnecting(false);
      }
    },
    [api, resetNav],
  );

  const errorDevice = (error as (UsbError & { deviceId?: string }) | null)?.deviceId ?? null;

  // Open the phone as soon as it appears; notice when it goes away.
  useEffect(() => {
    if (!devices) return;
    if (conn && !devices.some((d) => d.id === conn.id)) {
      setConn(null);
      setEntries([]);
      setProgress((p) => (p && !p.finished ? null : p));
      notify("info", `${conn.name} was unplugged.`);
      void api.disconnect();
      return;
    }
    if (errorDevice && !devices.some((d) => d.id === errorDevice)) setError(null);
    if (conn || connecting || error) return;
    const first = devices.find((d) => d.id !== dismissedId);
    if (first) void connect(first.id, false);
  }, [devices, conn, connecting, error, errorDevice, dismissedId, connect, api, notify]);

  // A locked phone shows no storage; try again by itself once it is unlocked.
  useEffect(() => {
    if (error?.code !== "locked" || !errorDevice) return;
    const timer = setTimeout(() => void connect(errorDevice, false), 3000);
    return () => clearTimeout(timer);
  }, [error, errorDevice, connect]);

  // Back / Forward from the mouse's side buttons and the keyboard, the same
  // gestures that work in a browser or Finder.
  useEffect(() => {
    if (!conn) return;
    const offNative = api.onMouseNav?.((direction) => (direction === "back" ? goBack() : goForward()));

    // Where the web view does deliver side buttons (Windows, Linux): 3 is
    // back, 4 is forward. Cancelled so the web view does not act on them too.
    const onMouseUp = (e: MouseEvent) => {
      if (e.button !== 3 && e.button !== 4) return;
      e.preventDefault();
      if (e.button === 3) goBack();
      else goForward();
    };
    const swallow = (e: MouseEvent) => {
      if (e.button === 3 || e.button === 4) e.preventDefault();
    };

    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable]")) return;
      const back = (e.metaKey && e.key === "[") || (e.altKey && e.key === "ArrowLeft");
      const forward = (e.metaKey && e.key === "]") || (e.altKey && e.key === "ArrowRight");
      if (back) goBack();
      else if (forward) goForward();
      else if (e.metaKey && e.key === "ArrowUp" && path.length > 0) navigate(path.slice(0, -1));
      else return;
      e.preventDefault();
    };

    window.addEventListener("mouseup", onMouseUp);
    window.addEventListener("mousedown", swallow);
    window.addEventListener("auxclick", swallow);
    window.addEventListener("keydown", onKey);
    return () => {
      offNative?.();
      window.removeEventListener("mouseup", onMouseUp);
      window.removeEventListener("mousedown", swallow);
      window.removeEventListener("auxclick", swallow);
      window.removeEventListener("keydown", onKey);
    };
  }, [api, conn, goBack, goForward, navigate, path]);

  // ------------------------------------------------------------ listing

  const refresh = useCallback(async () => {
    if (!conn || !storage) return;
    setLoading(true);
    try {
      setEntries(await api.list(storage.id, parent, showHidden));
    } catch (err) {
      const e = asUsbError(err);
      notify("error", e.message);
      if (e.code === "disconnected" || e.code === "not-connected") setConn(null);
    } finally {
      setLoading(false);
    }
  }, [api, conn, storage, parent, showHidden, notify]);

  useEffect(() => {
    setSelected(new Set());
    setEntries([]);
    void refresh();
  }, [refresh]);

  // Thumbnails, one at a time, for the photos in view.
  useEffect(() => {
    if (!conn?.thumbnails || !storage) return;
    let cancelled = false;
    const wanted = entries
      .filter((e) => e.kind === "image" || e.kind === "video")
      .slice(0, MAX_THUMBNAILS)
      .filter((e) => !(e.handle in thumbs));
    (async () => {
      for (const entry of wanted) {
        if (cancelled || busy) return;
        try {
          const url = await api.thumbnail(storage.id, entry.handle);
          if (!cancelled) setThumbs((t) => ({ ...t, [entry.handle]: url }));
        } catch {
          if (!cancelled) setThumbs((t) => ({ ...t, [entry.handle]: "" }));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // `thumbs` is deliberately left out: it changes on every fetched image.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entries, conn, storage, api, busy]);

  // ---------------------------------------------------------- transfers

  useEffect(
    () =>
      api.onProgress((p) => {
        setProgress(p);
        if (!p.finished) return;
        if (p.error?.code === "cancelled") {
          notify("info", "Copy cancelled.");
        } else if (p.error) {
          notify("error", p.error.message);
        } else if (p.direction === "download") {
          notify("info", `Copied ${p.files_done} ${p.files_done === 1 ? "file" : "files"} to your Mac.`);
        } else {
          notify("info", `Copied ${p.files_done} ${p.files_done === 1 ? "file" : "files"} to the phone.`);
        }
      }),
    [api, notify],
  );

  // Refresh what the phone holds once a copy onto it finishes.
  const lastFinished = useRef<string | null>(null);
  useEffect(() => {
    if (!progress?.finished || lastFinished.current === progress.job) return;
    lastFinished.current = progress.job;
    if (progress.direction === "upload") void refresh();
    api
      .storages()
      .then((storages) => setConn((c) => (c ? { ...c, storages } : c)))
      .catch(() => {});
  }, [progress, refresh, api]);

  const start = async (run: () => Promise<string>) => {
    try {
      setProgress(null);
      await run();
    } catch (err) {
      notify("error", asUsbError(err).message);
    }
  };

  const uploadPaths = useCallback(
    (paths: string[]) => {
      if (!storage || paths.length === 0) return;
      void start(() => api.upload(storage.id, parent, paths));
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [api, storage, parent],
  );

  // Files dropped from Finder go into the folder on screen.
  useEffect(() => {
    if (!droppedPaths) return;
    onDropHandled();
    if (!conn || !storage) {
      notify("warn", "Connect a phone first, then drop files here to copy them onto it.");
      return;
    }
    if (busy) {
      notify("warn", "Wait for the current copy to finish.");
      return;
    }
    uploadPaths(droppedPaths);
  }, [droppedPaths, onDropHandled, conn, storage, busy, uploadPaths, notify]);

  const download = async (dest: string | null) => {
    if (!storage || selected.size === 0) return;
    await start(() => api.download(storage.id, [...selected], dest));
  };

  const remove = async () => {
    if (!storage) return;
    setConfirmDelete(false);
    try {
      await api.remove(storage.id, [...selected]);
      notify("info", `Deleted ${selected.size} ${selected.size === 1 ? "item" : "items"} from the phone.`);
      await refresh();
    } catch (err) {
      notify("error", asUsbError(err).message);
    }
  };

  const createFolder = async () => {
    if (!storage || !newFolder?.trim()) return;
    try {
      await api.createFolder(storage.id, parent, newFolder.trim());
      setNewFolder(null);
      await refresh();
    } catch (err) {
      notify("error", asUsbError(err).message);
    }
  };

  /**
   * Walk to a folder by names from the top, e.g. DCIM → Camera. Phone makers
   * disagree on where things live (TECNO and Xiaomi use `Pictures/Screenshot`,
   * Samsung `DCIM/Screenshots`), so several candidates are tried in order.
   */
  const goTo = async (label: string, candidates: string[][]) => {
    if (!storage) return;
    let best: Crumb[] = [];
    try {
      for (const names of candidates) {
        const trail: Crumb[] = [];
        let at: string | null = null;
        for (const name of names) {
          const items = await api.list(storage.id, at);
          const next = items.find((e) => e.folder && e.name.toLowerCase() === name.toLowerCase());
          if (!next) break;
          trail.push({ handle: next.handle, name: next.name });
          at = next.handle;
        }
        if (trail.length === names.length) {
          navigate(trail);
          return;
        }
        if (trail.length > best.length) best = trail;
      }
    } catch (err) {
      notify("error", asUsbError(err).message);
      return;
    }
    if (best.length) navigate(best);
    notify("info", `No ${label} folder on this phone yet.`);
  };

  const toggle = (handle: string) =>
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(handle)) next.delete(handle);
      else next.add(handle);
      return next;
    });

  const allSelected = entries.length > 0 && selected.size === entries.length;
  const selectedBytes = useMemo(
    () => entries.filter((e) => selected.has(e.handle)).reduce((sum, e) => sum + e.size, 0),
    [entries, selected],
  );
  const hasFolderSelected = entries.some((e) => e.folder && selected.has(e.handle));

  // ------------------------------------------------------------- render

  if (!conn) {
    return (
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-5 px-4 pb-8 sm:px-6">
        <NotConnected
          devices={devices}
          connecting={connecting}
          error={error}
          onConnect={(id, release) => void connect(id, release)}
        />
      </div>
    );
  }

  const storageUsed = storage ? storage.total - storage.free : 0;

  return (
    <div className="relative mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 pb-8 sm:px-6">
      {/* Phone header */}
      <section className="flex flex-wrap items-center gap-4 rounded-xl2 bg-white p-4 ring-1 ring-ink-200/70 dark:bg-ink-900 dark:ring-ink-800">
        <span className="grid size-11 place-items-center rounded-xl bg-emerald-100 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300">
          <Icon name="phone" className="size-5.5" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="flex items-center gap-2 truncate font-semibold">
            {conn.name}
            <Badge tone="online">USB</Badge>
            {devices?.find((d) => d.id === conn.id)?.speed ? (
              <span className="text-xs font-normal text-ink-500 dark:text-ink-400">
                {devices.find((d) => d.id === conn.id)!.speed}
              </span>
            ) : null}
          </p>
          {storage ? (
            <div className="mt-1.5 flex items-center gap-2.5">
              {conn.storages.length > 1 ? (
                <select
                  value={storage.id}
                  onChange={(e) => {
                    setStorageId(e.target.value);
                    resetNav();
                  }}
                  className="h-7 rounded-md bg-ink-100 px-1.5 text-xs font-medium dark:bg-ink-800"
                  aria-label="Storage"
                >
                  {conn.storages.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </select>
              ) : (
                <span className="text-xs text-ink-500 dark:text-ink-400">{storage.name}</span>
              )}
              <ProgressBar className="max-w-40" value={percent(storageUsed, storage.total)} />
              <span className="shrink-0 text-xs tabular-nums text-ink-500 dark:text-ink-400">
                {formatBytes(storage.free)} free
              </span>
            </div>
          ) : null}
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            setDismissedId(conn.id);
            setConn(null);
            setEntries([]);
            void api.disconnect();
          }}
        >
          Disconnect
        </Button>
      </section>

      {busy || progress?.finished ? (
        <TransferPanel
          progress={progress!}
          onCancel={() => void api.cancel()}
          onReveal={(p) => api.reveal(p)}
          onDismiss={() => setProgress(null)}
        />
      ) : null}

      {/* Toolbar */}
      <section>
        <SectionTitle
          action={
            <div className="flex items-center gap-1">
              <IconButton label="Refresh" icon="refresh" onClick={() => void refresh()} />
              {conn.can_upload ? (
                <IconButton label="New folder" icon="plus" onClick={() => setNewFolder("")} />
              ) : null}
            </div>
          }
        >
          On the phone
        </SectionTitle>

        <div className="mb-3 flex flex-wrap items-center gap-2">
          {conn.can_upload ? (
            <>
              <Button
                size="sm"
                variant="primary"
                icon="upload"
                disabled={busy}
                onClick={() => void api.pickFiles().then(uploadPaths)}
              >
                Copy files to phone
              </Button>
              <Button size="sm" icon="folder" disabled={busy} onClick={() => void api.pickFolder().then(uploadPaths)}>
                Copy a folder
              </Button>
            </>
          ) : null}
          <span className="mx-1 hidden h-5 w-px bg-ink-200 sm:block dark:bg-ink-700" />
          <Button size="sm" variant="ghost" icon="camera" onClick={() => void goTo("Camera", [["DCIM", "Camera"]])}>
            Camera
          </Button>
          <Button size="sm" variant="ghost" icon="image" onClick={() =>
              void goTo("Screenshots", [
                ["Pictures", "Screenshots"],
                ["Pictures", "Screenshot"],
                ["DCIM", "Screenshots"],
                ["DCIM", "Screenshot"],
              ])
            }>
            Screenshots
          </Button>
          <Button size="sm" variant="ghost" icon="download" onClick={() => void goTo("Download", [["Download"]])}>
            Download
          </Button>
        </div>

        {/* Breadcrumbs */}
        <nav aria-label="Folder" className="mb-2 flex min-w-0 flex-wrap items-center gap-1 text-sm">
          <IconButton
            label="Back (⌘[ or the mouse's back button)"
            icon="chevronLeft"
            disabled={nav.back.length === 0}
            onClick={goBack}
            className="size-8"
          />
          <IconButton
            label="Forward (⌘] or the mouse's forward button)"
            icon="chevron"
            disabled={nav.forward.length === 0}
            onClick={goForward}
            className="mr-1 size-8"
          />
          <button
            type="button"
            onClick={() => navigate([])}
            className={cx(
              "rounded-md px-1.5 py-0.5 font-medium",
              path.length === 0 ? "text-ink-900 dark:text-white" : "text-brand-600 hover:underline dark:text-brand-300",
            )}
          >
            {storage?.name ?? "Phone"}
          </button>
          {path.map((crumb, i) => (
            <span key={crumb.handle} className="flex min-w-0 items-center gap-1">
              <Icon name="chevron" className="size-3.5 shrink-0 text-ink-400" />
              <button
                type="button"
                onClick={() => navigate(path.slice(0, i + 1))}
                className={cx(
                  "truncate rounded-md px-1.5 py-0.5 font-medium",
                  i === path.length - 1
                    ? "text-ink-900 dark:text-white"
                    : "text-brand-600 hover:underline dark:text-brand-300",
                )}
              >
                {crumb.name}
              </button>
            </span>
          ))}
        </nav>

        {/* Selection bar */}
        {selected.size > 0 ? (
          <div className="sticky top-0 z-10 mb-2 flex flex-wrap items-center gap-2 rounded-xl bg-brand-50 px-3 py-2 ring-1 ring-brand-200 dark:bg-brand-500/10 dark:ring-brand-500/30">
            <span className="text-sm font-medium text-brand-800 dark:text-brand-200">
              {selected.size} selected
              {selectedBytes > 0 ? ` · ${formatBytes(selectedBytes)}${hasFolderSelected ? "+" : ""}` : ""}
            </span>
            <div className="ml-auto flex flex-wrap gap-2">
              <Button size="sm" variant="primary" icon="download" disabled={busy} onClick={() => void download(null)}>
                Copy to Downloads
              </Button>
              <Button
                size="sm"
                icon="folder"
                disabled={busy}
                onClick={() =>
                  void api.pickDestination().then((dest) => {
                    if (dest) void download(dest);
                  })
                }
              >
                Copy to…
              </Button>
              {conn.can_delete ? (
                <IconButton label="Delete from phone" icon="trash" disabled={busy} onClick={() => setConfirmDelete(true)} />
              ) : null}
              <IconButton label="Clear selection" icon="close" onClick={() => setSelected(new Set())} />
            </div>
          </div>
        ) : null}

        {/* Listing */}
        <div
          className={cx(
            "overflow-hidden rounded-xl2 bg-white ring-1 ring-ink-200/70 transition dark:bg-ink-900 dark:ring-ink-800",
            dropActive && "ring-2 ring-brand-500 dark:ring-brand-400",
          )}
        >
          <div className="flex items-center gap-3 border-b border-ink-100 px-3 py-2 text-xs font-medium text-ink-500 dark:border-ink-800 dark:text-ink-400">
            <input
              type="checkbox"
              aria-label="Select all"
              checked={allSelected}
              disabled={entries.length === 0}
              onChange={() => setSelected(allSelected ? new Set() : new Set(entries.map((e) => e.handle)))}
              className="size-4 accent-brand-600"
            />
            <span className="flex-1">Name</span>
            <span className="hidden w-28 text-right sm:block">Modified</span>
            <span className="w-20 text-right">Size</span>
          </div>

          {newFolder !== null ? (
            <form
              className="flex items-center gap-2 border-b border-ink-100 px-3 py-2 dark:border-ink-800"
              onSubmit={(e) => {
                e.preventDefault();
                void createFolder();
              }}
            >
              <Icon name="folder" className="size-5 shrink-0 text-brand-500" />
              <input
                autoFocus
                className={cx(inputClass, "h-9")}
                placeholder="Folder name"
                value={newFolder}
                onChange={(e) => setNewFolder(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setNewFolder(null);
                }}
              />
              <Button size="sm" variant="primary" type="submit" disabled={!newFolder.trim()}>
                Create
              </Button>
              <Button size="sm" variant="ghost" type="button" onClick={() => setNewFolder(null)}>
                Cancel
              </Button>
            </form>
          ) : null}

          {loading && entries.length === 0 ? (
            <div className="flex items-center justify-center gap-2 py-14 text-sm text-ink-500">
              <Spinner /> Reading the phone…
            </div>
          ) : entries.length === 0 ? (
            <div className="py-14 text-center text-sm text-ink-500 dark:text-ink-400">
              This folder is empty.
              {conn.can_upload ? " Drag files here from Finder to copy them onto the phone." : ""}
            </div>
          ) : (
            // No scroll box of its own: the page scrolls, once.
            <ul>
              {entries.map((entry) => (
                <EntryRow
                  key={entry.handle}
                  entry={entry}
                  thumb={thumbs[entry.handle]}
                  selected={selected.has(entry.handle)}
                  onToggle={() => toggle(entry.handle)}
                  onOpen={() => {
                    if (entry.folder) navigate([...path, { handle: entry.handle, name: entry.name }]);
                    else toggle(entry.handle);
                  }}
                />
              ))}
            </ul>
          )}
        </div>

        <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
        <p className="text-xs text-ink-500 dark:text-ink-400">
          {conn.can_upload ? "Drag files or folders from Finder onto this window to copy them here. " : ""}
          Double-click a folder to open it.
        </p>
          <label className="flex items-center gap-2 text-xs text-ink-600 dark:text-ink-300">
            <input
              type="checkbox"
              checked={showHidden}
              onChange={(e) => setShowHidden(e.target.checked)}
              className="size-3.5 accent-brand-600"
            />
            Show hidden files
          </label>
        </div>
      </section>

      {dropActive ? (
        <div className="pointer-events-none fixed inset-0 z-40 grid place-items-center bg-brand-600/10 backdrop-blur-[1px]">
          <div className="rounded-2xl bg-white px-6 py-4 text-center shadow-xl ring-1 ring-brand-300 dark:bg-ink-900">
            <Icon name="upload" className="mx-auto size-7 text-brand-600" />
            <p className="mt-2 font-semibold">Drop to copy onto {conn.name}</p>
            <p className="text-sm text-ink-500">into {path.length ? path[path.length - 1]!.name : storage?.name}</p>
          </div>
        </div>
      ) : null}

      <Modal
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title={`Delete ${selected.size} ${selected.size === 1 ? "item" : "items"} from the phone?`}
        description={
          hasFolderSelected
            ? "Folders are deleted with everything inside them. This cannot be undone."
            : "This cannot be undone."
        }
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmDelete(false)}>
              Cancel
            </Button>
            <Button variant="danger" icon="trash" onClick={() => void remove()}>
              Delete
            </Button>
          </>
        }
      />
    </div>
  );
}

function EntryRow({
  entry,
  thumb,
  selected,
  onToggle,
  onOpen,
}: {
  entry: UsbEntry;
  thumb: string | undefined;
  selected: boolean;
  onToggle: () => void;
  onOpen: () => void;
}) {
  // Dot-files are hidden on the phone; shown faded so they read as such.
  const hiddenFile = entry.name.startsWith(".");
  const fade = hiddenFile && "opacity-45";
  return (
    <li
      title={hiddenFile ? "Hidden" : undefined}
      className={cx(
        "flex cursor-default items-center gap-3 border-b border-ink-100 px-3 py-1.5 last:border-0 dark:border-ink-800",
        selected ? "bg-brand-50 dark:bg-brand-500/10" : "hover:bg-ink-50 dark:hover:bg-ink-800/50",
      )}
      onDoubleClick={entry.folder ? onOpen : undefined}
      onClick={(e) => {
        // Single click on a folder name opens it; elsewhere it selects.
        if ((e.target as HTMLElement).closest("[data-open]")) onOpen();
        else onToggle();
      }}
    >
      <input
        type="checkbox"
        aria-label={`Select ${entry.name}`}
        checked={selected}
        onChange={onToggle}
        onClick={(e) => e.stopPropagation()}
        className="size-4 accent-brand-600"
      />
      <span
        className={cx(
          "grid size-9 shrink-0 place-items-center overflow-hidden rounded-lg bg-ink-100 text-ink-500 dark:bg-ink-800 dark:text-ink-400",
          fade,
        )}
      >
        {thumb ? (
          <img src={thumb} alt="" className="size-full object-cover" />
        ) : (
          <Icon
            name={KIND_ICON[entry.kind]}
            className={cx("size-4.5", entry.folder && "text-brand-500 dark:text-brand-300")}
          />
        )}
      </span>
      <span className={cx("min-w-0 flex-1", fade)}>
        {entry.folder ? (
          <button
            type="button"
            data-open
            className="max-w-full truncate text-left text-sm font-medium hover:text-brand-600 hover:underline dark:hover:text-brand-300"
          >
            {entry.name}
          </button>
        ) : (
          <span className="block truncate text-sm">{entry.name}</span>
        )}
      </span>
      <span className={cx("hidden w-28 text-right text-xs tabular-nums text-ink-500 sm:block dark:text-ink-400", fade)}>
        {entry.modified_ms ? formatDate(entry.modified_ms) : "—"}
      </span>
      <span className={cx("w-20 text-right text-xs tabular-nums text-ink-500 dark:text-ink-400", fade)}>
        {entry.folder ? "—" : formatBytes(entry.size)}
      </span>
    </li>
  );
}

function samePath(a: Crumb[], b: Crumb[]): boolean {
  return a.length === b.length && a.every((crumb, i) => crumb.handle === b[i]!.handle);
}

const KIND_ICON: Record<UsbEntryKind, IconName> = {
  folder: "folder",
  image: "image",
  video: "video",
  audio: "music",
  file: "file",
};

function formatDate(ms: number): string {
  return new Date(ms).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

function TransferPanel({
  progress,
  onCancel,
  onReveal,
  onDismiss,
}: {
  progress: UsbProgress;
  onCancel: () => void;
  onReveal: (path: string) => void;
  onDismiss: () => void;
}) {
  const toMac = progress.direction === "download";
  const ok = progress.finished && !progress.error;
  const eta =
    progress.speed_bps > 0 ? (progress.bytes_total - progress.bytes_done) / progress.speed_bps : null;

  return (
    <section className="rounded-xl2 bg-white p-4 ring-1 ring-ink-200/70 animate-rise dark:bg-ink-900 dark:ring-ink-800">
      <div className="flex items-start gap-3">
        <span
          className={cx(
            "grid size-9 shrink-0 place-items-center rounded-lg",
            ok
              ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300"
              : progress.error && progress.error.code !== "cancelled"
                ? "bg-rose-100 text-rose-700 dark:bg-rose-500/15 dark:text-rose-300"
                : "bg-brand-100 text-brand-700 dark:bg-brand-500/15 dark:text-brand-300",
          )}
        >
          <Icon name={ok ? "check" : toMac ? "download" : "upload"} className="size-4.5" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-3">
            <p className="truncate font-medium">
              {progress.finished
                ? ok
                  ? `Copied ${progress.files_done} ${progress.files_done === 1 ? "file" : "files"} ${toMac ? "to this Mac" : "to the phone"}`
                  : progress.error?.code === "cancelled"
                    ? "Copy cancelled"
                    : "Copy failed"
                : toMac
                  ? "Copying to this Mac"
                  : "Copying to the phone"}
            </p>
            {!progress.finished ? (
              <span className="shrink-0 text-sm font-semibold tabular-nums text-brand-600 dark:text-brand-300">
                {formatPercent(progress.bytes_done, progress.bytes_total)}
              </span>
            ) : null}
          </div>
          <p className="mt-0.5 truncate text-[13px] text-ink-500 dark:text-ink-400">
            {progress.finished
              ? progress.error
                ? progress.error.message
                : formatBytes(progress.bytes_total)
              : `${progress.files_total ? `File ${Math.min(progress.files_done + 1, progress.files_total)} of ${progress.files_total} · ` : ""}${progress.current}`}
          </p>
        </div>
        {progress.finished ? (
          <IconButton label="Dismiss" icon="close" onClick={onDismiss} />
        ) : (
          <IconButton label="Cancel copy" icon="close" onClick={onCancel} />
        )}
      </div>

      {!progress.finished ? (
        <>
          <ProgressBar
            className="mt-3"
            value={percent(progress.bytes_done, progress.bytes_total)}
            indeterminate={progress.bytes_total === 0}
          />
          <div className="mt-2 flex flex-wrap gap-x-3 text-[13px] tabular-nums text-ink-500 dark:text-ink-400">
            <span>
              {formatBytes(progress.bytes_done)} / {formatBytes(progress.bytes_total)}
            </span>
            <span className="font-medium text-ink-700 dark:text-ink-200">{formatSpeed(progress.speed_bps)}</span>
            <span>{formatEta(eta)}</span>
          </div>
        </>
      ) : ok && toMac && progress.saved.length > 0 ? (
        <Button size="sm" icon="folder" className="mt-3" onClick={() => onReveal(progress.saved[0]!)}>
          Show in Finder
        </Button>
      ) : null}
    </section>
  );
}

/** Before a phone is open: what to do, or what went wrong and how to fix it. */
function NotConnected({
  devices,
  connecting,
  error,
  onConnect,
}: {
  devices: UsbDevice[] | null;
  connecting: boolean;
  error: (UsbError & { deviceId?: string }) | null;
  onConnect: (id: string, release: boolean) => void;
}) {
  const device = devices?.find((d) => d.id === error?.deviceId) ?? devices?.[0] ?? null;

  if (connecting) {
    return (
      <Panel icon="usb" tone="brand" title={`Opening ${device?.name ?? "the phone"}…`}>
        <Spinner className="size-5 text-brand-500" />
      </Panel>
    );
  }

  if (error && device) {
    if (error.code === "exclusive") {
      return (
        <Panel icon="alert" tone="warn" title="Another app has the phone">
          <p>
            {error.message} macOS opens Android phones in Image Capture as soon as they are plugged
            in, which locks every other app out.
          </p>
          <p className="mt-2">
            ShareLocal can close it for you (and Android File Transfer, if it is installed). Nothing is
            uninstalled — Photos and Image Capture work normally afterwards.
          </p>
          <div className="mt-4 flex flex-wrap gap-2">
            <Button variant="primary" icon="usb" onClick={() => onConnect(device.id, true)}>
              Free the phone and connect
            </Button>
            <Button onClick={() => onConnect(device.id, false)}>Try again</Button>
          </div>
        </Panel>
      );
    }
    if (error.code === "locked") {
      return (
        <Panel icon="lock" tone="warn" title={`Unlock ${device.name}`}>
          <p>{error.message}</p>
          <SetupSteps />
          <div className="mt-4 flex items-center gap-3">
            <Button icon="refresh" onClick={() => onConnect(device.id, false)}>
              Try again
            </Button>
            <span className="flex items-center gap-2 text-xs text-ink-500">
              <Spinner className="size-3.5" /> Checking again by itself…
            </span>
          </div>
        </Panel>
      );
    }
    return (
      <Panel icon="alert" tone="error" title={`Could not open ${device.name}`}>
        <p>{error.message}</p>
        <SetupSteps />
        <Button className="mt-4" icon="refresh" onClick={() => onConnect(device.id, false)}>
          Try again
        </Button>
      </Panel>
    );
  }

  return (
    <Panel icon="usb" tone="neutral" title="Plug in your Android phone">
      <p>
        Copy photos, videos and files between this Mac and an Android phone over a USB cable — no
        Wi-Fi needed, nothing to install on the phone.
      </p>
      <SetupSteps />
      <p className="mt-4 flex items-center gap-2 text-xs text-ink-500 dark:text-ink-400">
        <Spinner className="size-3.5" />
        {devices === null ? "Checking USB…" : "Waiting for a phone…"}
      </p>
    </Panel>
  );
}

function SetupSteps() {
  return (
    <ol className="mt-4 space-y-2 text-sm text-ink-700 dark:text-ink-300">
      {[
        "Connect the phone to this Mac with a USB cable that carries data (some charge-only cables do not).",
        "Unlock the phone.",
        "Swipe down from the top of the phone, tap the “USB” notification (“Charging this device via USB”), and choose “File transfer”.",
        "If the phone asks “Allow access to phone data?”, tap Allow.",
      ].map((step, i) => (
        <li key={step} className="flex gap-2.5">
          <span className="grid size-5 shrink-0 place-items-center rounded-full bg-brand-100 text-[11px] font-semibold text-brand-700 dark:bg-brand-500/15 dark:text-brand-300">
            {i + 1}
          </span>
          <span className="leading-relaxed">{step}</span>
        </li>
      ))}
    </ol>
  );
}

function Panel({
  icon,
  tone,
  title,
  children,
}: {
  icon: IconName;
  tone: "neutral" | "brand" | "warn" | "error";
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-xl2 bg-white p-6 ring-1 ring-ink-200/70 dark:bg-ink-900 dark:ring-ink-800">
      <div className="flex items-start gap-4">
        <span
          className={cx(
            "grid size-12 shrink-0 place-items-center rounded-2xl",
            tone === "warn"
              ? "bg-amber-100 text-amber-700 dark:bg-amber-500/15 dark:text-amber-300"
              : tone === "error"
                ? "bg-rose-100 text-rose-700 dark:bg-rose-500/15 dark:text-rose-300"
                : "bg-brand-100 text-brand-700 dark:bg-brand-500/15 dark:text-brand-300",
          )}
        >
          <Icon name={icon} className="size-6" />
        </span>
        <div className="min-w-0 flex-1 text-sm leading-relaxed text-ink-600 dark:text-ink-300">
          <h2 className="mb-1.5 text-base font-semibold text-ink-900 dark:text-white">{title}</h2>
          {children}
        </div>
      </div>
    </section>
  );
}
