/**
 * All desktop UI state in one hook: the snapshot from Rust plus the live event
 * stream that keeps it current. Views stay presentational.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { desktop, subscribeToEvents } from "./platform/desktop";
import type {
  AppEvent,
  ConflictPolicy,
  HistoryEntry,
  NetworkStatus,
  Peer,
  SessionView,
  Settings,
  DeviceInfo,
} from "./types";
import type { Toast } from "./components/Toasts";

export interface DesktopState {
  ready: boolean;
  device: DeviceInfo | null;
  network: NetworkStatus | null;
  peers: Peer[];
  sessions: SessionView[];
  settings: Settings | null;
  history: HistoryEntry[];
  toasts: Toast[];
  /** Offers waiting for the user to accept or decline, oldest first. */
  pendingOffers: SessionView[];
  activeSessions: SessionView[];
  finishedSessions: SessionView[];
  error: string | null;
}

export function useDesktopState() {
  const [device, setDevice] = useState<DeviceInfo | null>(null);
  const [network, setNetwork] = useState<NetworkStatus | null>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [sessions, setSessions] = useState<SessionView[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const notify = useCallback((level: Toast["level"], message: string) => {
    setToasts((current) => [
      ...current.slice(-4),
      { id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, level, message },
    ]);
  }, []);

  const dismissToast = useCallback((id: string) => {
    setToasts((current) => current.filter((t) => t.id !== id));
  }, []);

  const refreshHistory = useCallback(async () => {
    try {
      setHistory(await desktop.history());
    } catch {
      // History is a nicety; a failure here must not break the app.
    }
  }, []);

  // Initial load.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const snapshot = await desktop.snapshot();
        if (cancelled) return;
        setDevice(snapshot.device);
        setNetwork(snapshot.network);
        setPeers(snapshot.peers);
        setSessions(snapshot.sessions);
        setSettings(snapshot.settings);
        setReady(true);
        await refreshHistory();
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setReady(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [refreshHistory]);

  // Live updates.
  useEffect(() => {
    const unsubscribe = subscribeToEvents((event: AppEvent) => {
      switch (event.type) {
        case "peers-changed":
          setPeers(event.peers);
          break;
        case "network-changed":
          setNetwork(event.network);
          break;
        case "incoming-offer":
        case "session-updated":
          setSessions((current) => {
            const next = current.filter((s) => s.id !== event.session.id);
            next.unshift(event.session);
            return next;
          });
          break;
        case "session-removed":
          setSessions((current) => current.filter((s) => s.id !== event.id));
          break;
        case "history-changed":
          void refreshHistory();
          break;
        case "settings-changed":
          void desktop.getSettings().then(setSettings).catch(() => {});
          void desktop.snapshot().then((s) => setDevice(s.device)).catch(() => {});
          break;
        case "notice":
          notify(event.level, event.message);
          break;
      }
    });
    return unsubscribe;
  }, [notify, refreshHistory]);

  const pendingOffers = useMemo(
    () => sessions.filter((s) => s.role === "receiving" && s.status === "pending"),
    [sessions],
  );

  const activeSessions = useMemo(
    () =>
      sessions.filter((s) =>
        ["accepted", "transferring", "paused"].includes(s.status),
      ),
    [sessions],
  );

  const finishedSessions = useMemo(
    () =>
      sessions.filter((s) =>
        ["completed", "failed", "cancelled", "rejected"].includes(s.status),
      ),
    [sessions],
  );

  const state: DesktopState = {
    ready,
    device,
    network,
    peers,
    sessions,
    settings,
    history,
    toasts,
    pendingOffers,
    activeSessions,
    finishedSessions,
    error,
  };

  const actions = useDesktopActions({ notify, setSettings, refreshHistory });

  return { state, actions, dismissToast, notify } as const;
}

function useDesktopActions({
  notify,
  setSettings,
  refreshHistory,
}: {
  notify: (level: Toast["level"], message: string) => void;
  setSettings: (settings: Settings) => void;
  refreshHistory: () => Promise<void>;
}) {
  // `notify` is stable; keep the action object stable too so children do not
  // re-render on every tick of a transfer.
  const ref = useRef({ notify, setSettings, refreshHistory });
  ref.current = { notify, setSettings, refreshHistory };

  return useMemo(
    () => ({
      async send(peerId: string, paths: string[], pin?: string | null) {
        try {
          return await desktop.sendFiles(peerId, paths, pin ?? null);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
          return null;
        }
      },
      async accept(sessionId: string, conflict: ConflictPolicy) {
        try {
          await desktop.acceptOffer(sessionId, conflict);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async reject(sessionId: string) {
        try {
          await desktop.rejectOffer(sessionId);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async cancel(sessionId: string) {
        try {
          await desktop.cancelTransfer(sessionId);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async pause(sessionId: string, paused: boolean) {
        try {
          await desktop.pauseTransfer(sessionId, paused);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async clearFinished() {
        try {
          await desktop.clearFinished();
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async refreshPeers() {
        try {
          await desktop.refreshPeers();
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async saveSettings(next: Settings) {
        try {
          ref.current.setSettings(await desktop.updateSettings(next));
          ref.current.notify("info", "Settings saved.");
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async setDownloadDir(path: string) {
        try {
          return await desktop.setDownloadDir(path);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
          return null;
        }
      },
      async openPath(path: string) {
        try {
          await desktop.openPath(path);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async revealPath(path: string) {
        try {
          await desktop.revealPath(path);
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async openDownloadDir() {
        try {
          await desktop.openDownloadDir();
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async clearHistory() {
        try {
          await desktop.clearHistory();
          await ref.current.refreshHistory();
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      async removeHistoryEntry(id: string) {
        try {
          await desktop.removeHistoryEntry(id);
          await ref.current.refreshHistory();
        } catch (err) {
          ref.current.notify("error", messageOf(err));
        }
      },
      createPairing: () => desktop.createPairing(),
      revokePairing: () => desktop.revokePairing(),
      listInterfaces: () => desktop.listInterfaces(),
    }),
    [],
  );
}

export type DesktopActions = ReturnType<typeof useDesktopActions>;

function messageOf(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Something went wrong.";
}

/** Apply the chosen theme to <html>. "system" leaves it to the media query. */
export function useTheme(theme: Settings["theme"] | undefined) {
  useEffect(() => {
    const root = document.documentElement;
    root.classList.remove("dark", "light");
    if (theme === "dark") root.classList.add("dark");
    if (theme === "light") root.classList.add("light");
  }, [theme]);
}
