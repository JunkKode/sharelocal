import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  desktop,
  subscribeToFileDrops,
  subscribeToMouseNav,
  subscribeToUsbProgress,
} from "../platform/desktop";
import { useDesktopState, useTheme } from "../useDesktopState";
import type { ConflictPolicy, Peer, WifiInfo } from "../types";
import { HomeView } from "./HomeView";
import { TransfersView } from "./TransfersView";
import { HistoryView } from "./HistoryView";
import { SettingsView } from "./SettingsView";
import { UsbView, type UsbApi } from "./UsbView";
import { DevicePicker } from "../components/DevicePicker";
import { TextComposer } from "../components/TextComposer";
import { OfferPrompt } from "../components/OfferPrompt";
import { QrPanel } from "../components/QrPanel";
import { WifiQrPanel } from "../components/WifiQrPanel";
import { Toasts } from "../components/Toasts";
import { Icon, type IconName } from "../components/Icon";
import { Modal } from "../components/Modal";
import { Button, Spinner, cx, inputClass } from "../components/ui";
import { baseName } from "../lib/format";

type Tab = "home" | "transfers" | "usb" | "history" | "settings";

const TABS: { id: Tab; label: string; icon: IconName }[] = [
  { id: "home", label: "Devices", icon: "wifi" },
  { id: "transfers", label: "Transfers", icon: "upload" },
  { id: "usb", label: "USB", icon: "usb" },
  { id: "history", label: "History", icon: "history" },
  { id: "settings", label: "Settings", icon: "settings" },
];

export function DesktopApp() {
  const { state, actions, dismissToast, notify } = useDesktopState();
  const [tab, setTab] = useState<Tab>("home");
  const [pendingPaths, setPendingPaths] = useState<string[] | null>(null);
  const [pinFor, setPinFor] = useState<{ peer: Peer; paths: string[] } | null>(null);
  const [pin, setPin] = useState("");
  const [qrOpen, setQrOpen] = useState(false);
  const [wifiOpen, setWifiOpen] = useState(false);
  const [usbDrop, setUsbDrop] = useState<string[] | null>(null);
  const [wifiInfo, setWifiInfo] = useState<WifiInfo | null>(null);
  const [wifiDetecting, setWifiDetecting] = useState(false);
  /** Wi-Fi passwords unlocked this session, so reopening the panel is instant. */
  const wifiPasswords = useRef(new Map<string, string>());
  const [textFor, setTextFor] = useState<Peer | "choose" | null>(null);
  const [pairing, setPairing] = useState<{ url: string; expires_in_secs: number } | null>(null);
  const [drag, setDrag] = useState<{ active: boolean; targetId: string | null }>({
    active: false,
    targetId: null,
  });

  useTheme(state.settings?.theme);

  const busyPeerIds = useMemo(
    () => new Set(state.activeSessions.filter((s) => s.role === "sending").map((s) => s.peer.id)),
    [state.activeSessions],
  );

  /** Start a send, asking for a PIN first when the peer requires one. */
  const sendTo = useCallback(
    (peer: Peer, paths: string[]) => {
      if (paths.length === 0) return;
      if (peer.pin_required) {
        setPin("");
        setPinFor({ peer, paths });
        return;
      }
      void actions.send(peer.id, paths);
      setTab("transfers");
    },
    [actions],
  );

  // Native drag-and-drop from Finder / Explorer / a Linux file manager.
  useEffect(() => {
    return subscribeToFileDrops((event) => {
      if (event.phase === "over") {
        // Tauri reports physical pixels; the DOM works in CSS pixels.
        const ratio = window.devicePixelRatio || 1;
        const element = document.elementFromPoint(event.x / ratio, event.y / ratio);
        const card = element?.closest<HTMLElement>("[data-device-id]");
        setDrag({ active: true, targetId: card?.dataset.deviceId ?? null });
        return;
      }
      if (event.phase === "drop") {
        const targetId = drag.targetId;
        setDrag({ active: false, targetId: null });
        if (event.paths.length === 0) return;
        // On the USB tab a drop means "copy onto the phone".
        if (tab === "usb") {
          setUsbDrop(event.paths);
          return;
        }

        const peer = targetId ? state.peers.find((p) => p.id === targetId) : undefined;
        if (peer) sendTo(peer, event.paths);
        else setPendingPaths(event.paths);
        return;
      }
      setDrag({ active: false, targetId: null });
    });
  }, [drag.targetId, state.peers, sendTo, tab]);

  const usbApi = useMemo<UsbApi>(
    () => ({
      devices: desktop.usbDevices,
      connect: desktop.usbConnect,
      disconnect: desktop.usbDisconnect,
      storages: desktop.usbStorages,
      list: desktop.usbList,
      thumbnail: desktop.usbThumbnail,
      createFolder: desktop.usbCreateFolder,
      remove: desktop.usbDelete,
      download: desktop.usbDownload,
      upload: desktop.usbUpload,
      cancel: desktop.usbCancel,
      onProgress: subscribeToUsbProgress,
      onMouseNav: subscribeToMouseNav,
      pickFiles: async () =>
        toPaths(await open({ multiple: true, title: "Choose files to copy to the phone" })),
      pickFolder: async () =>
        toPaths(await open({ directory: true, multiple: false, title: "Choose a folder to copy to the phone" })),
      pickDestination: async () =>
        toPaths(await open({ directory: true, multiple: false, title: "Copy to which folder?" }))[0] ?? null,
      reveal: (path) => void actions.revealPath(path),
    }),
    [actions],
  );
  const clearUsbDrop = useCallback(() => setUsbDrop(null), []);

  /// Pick files. With a `peer`, send straight there; otherwise queue them and
  /// ask which device.
  const pickFiles = async (peer?: Peer) => {
    const picked = await open({ multiple: true, title: "Select files to send" });
    const paths = toPaths(picked);
    if (!paths.length) return;
    if (peer) sendTo(peer, paths);
    else setPendingPaths(paths);
  };

  const pickFolder = async (peer?: Peer) => {
    const picked = await open({ directory: true, multiple: false, title: "Select a folder to send" });
    const paths = toPaths(picked);
    if (!paths.length) return;
    if (peer) sendTo(peer, paths);
    else setPendingPaths(paths);
  };

  const sendText = useCallback(
    (peer: Peer, text: string) => {
      void (async () => {
        try {
          await desktop.sendText(peer.id, text, peer.pin_required ? pin : null);
          setTab("transfers");
        } catch (err) {
          notify("error", err instanceof Error ? err.message : String(err));
        }
      })();
    },
    [notify, pin],
  );

  const copyText = useCallback(
    (text: string) => {
      void desktop
        .copyToClipboard(text)
        .then(() => notify("info", "Copied to the clipboard."))
        .catch(() => notify("error", "Could not write to the clipboard."));
    },
    [notify],
  );

  const copyLabelled = useCallback(
    (text: string, what: string) => {
      void desktop
        .copyToClipboard(text)
        .then(() => notify("info", `${what} copied.`))
        .catch(() => notify("error", "Could not write to the clipboard."));
    },
    [notify],
  );

  const saveWifiPng = useCallback(
    (png: string, name: string) => {
      void desktop
        .savePng(name, png)
        .then((path) => {
          notify("info", `Saved ${baseName(path)} to your download folder.`);
          void actions.revealPath(path);
        })
        .catch((e) => notify("error", e instanceof Error ? e.message : String(e)));
    },
    [notify, actions],
  );

  const copyWifiImage = useCallback(
    (png: string) => {
      void desktop
        .copyImage(png)
        .then(() => notify("info", "QR image copied — paste it into a chat."))
        .catch(() => notify("error", "Could not copy the image."));
    },
    [notify],
  );

  // Look the network up in the background — at start and whenever the
  // network changes — so "Share Wi-Fi" opens with everything already filled
  // in. The macOS lookup can take several seconds on a cold start.
  const networkKey = `${state.network?.interface ?? ""}|${state.network?.ipv4 ?? ""}`;
  useEffect(() => {
    if (!state.ready) return;
    let cancelled = false;
    setWifiDetecting(true);
    desktop
      .wifiInfo()
      .then((info) => {
        if (!cancelled) setWifiInfo(info);
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setWifiDetecting(false);
      });
    return () => {
      cancelled = true;
    };
  }, [state.ready, networkKey]);

  const passwordFor = useCallback((ssid: string) => wifiPasswords.current.get(ssid), []);
  const rememberWifi = useCallback((ssid: string, password: string) => {
    if (wifiPasswords.current.get(ssid) === password) return;
    wifiPasswords.current.set(ssid, password);
    void desktop.wifiRemember(ssid, password).catch(() => {});
  }, []);

  const openWifi = () => {
    setQrOpen(false);
    setWifiOpen(true);
  };

  const openQr = async () => {
    setQrOpen(true);
    try {
      setPairing(await actions.createPairing());
    } catch {
      notify("error", "Could not create a pairing code.");
    }
  };

  if (!state.ready) {
    return (
      <div className="grid h-full place-items-center">
        <Spinner className="size-6 text-ink-400" />
      </div>
    );
  }

  if (state.error) {
    return (
      <div className="grid h-full place-items-center p-8 text-center">
        <div>
          <Icon name="alert" className="mx-auto size-8 text-rose-500" />
          <p className="mt-3 font-medium">ShareLocal could not start</p>
          <p className="mt-1 max-w-sm text-sm text-ink-500 dark:text-ink-400">{state.error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <Header tab={tab} onTab={setTab} activeCount={state.activeSessions.length} />

      <main className="flex-1 overflow-y-auto pt-5">
        {tab === "home" ? (
          <HomeView
            device={state.device}
            network={state.network}
            peers={state.peers}
            activeSessions={state.activeSessions}
            dropActive={drag.active}
            dropTargetId={drag.targetId}
            busyPeerIds={busyPeerIds}
            onSelectPeer={(peer) => {
              if (pendingPaths) {
                sendTo(peer, pendingPaths);
                setPendingPaths(null);
              } else {
                void pickFiles(peer);
              }
            }}
            onPickFiles={() => void pickFiles()}
            onPickFolder={() => void pickFolder()}
            onSendText={() => setTextFor("choose")}
            onRefreshPeers={() => void actions.refreshPeers()}
            onOpenQr={openQr}
            onOpenWifi={openWifi}
            onCancel={(id) => void actions.cancel(id)}
            onPause={(id, paused) => void actions.pause(id, paused)}
            onOpenPath={(p) => void actions.openPath(p)}
            onRevealPath={(p) => void actions.revealPath(p)}
            onCopyText={copyText}
          />
        ) : null}

        {tab === "transfers" ? (
          <TransfersView
            active={state.activeSessions}
            finished={state.finishedSessions}
            onCancel={(id) => void actions.cancel(id)}
            onPause={(id, paused) => void actions.pause(id, paused)}
            onOpenPath={(p) => void actions.openPath(p)}
            onRevealPath={(p) => void actions.revealPath(p)}
            onCopyText={copyText}
            onClearFinished={() => void actions.clearFinished()}
          />
        ) : null}

        {tab === "usb" ? (
          <UsbView
            api={usbApi}
            notify={notify}
            droppedPaths={usbDrop}
            dropActive={drag.active}
            onDropHandled={clearUsbDrop}
          />
        ) : null}

        {tab === "history" ? (
          <HistoryView
            entries={state.history}
            onClear={() => void actions.clearHistory()}
            onRemove={(id) => void actions.removeHistoryEntry(id)}
            onReveal={(p) => void actions.revealPath(p)}
          />
        ) : null}

        {tab === "settings" && state.settings ? (
          <SettingsView
            settings={state.settings}
            network={state.network}
            onSave={(next) => void actions.saveSettings(next)}
            onSetDownloadDir={actions.setDownloadDir}
            onOpenDownloadDir={() => void actions.openDownloadDir()}
            listInterfaces={actions.listInterfaces}
          />
        ) : null}
      </main>

      <DevicePicker
        open={pendingPaths !== null || textFor === "choose"}
        peers={state.peers}
        fileCount={pendingPaths?.length ?? 0}
        totalBytes={null}
        onPick={(peer) => {
          if (pendingPaths) {
            sendTo(peer, pendingPaths);
            setPendingPaths(null);
          } else {
            setTextFor(peer);
          }
        }}
        onClose={() => {
          setPendingPaths(null);
          setTextFor(null);
        }}
      />

      <TextComposer
        open={textFor !== null && textFor !== "choose"}
        destinationName={typeof textFor === "object" ? textFor?.name : undefined}
        onClose={() => setTextFor(null)}
        onSend={(text) => {
          if (textFor && textFor !== "choose") sendText(textFor, text);
        }}
        onPasteClipboard={async () => {
          try {
            return await navigator.clipboard.readText();
          } catch {
            notify("warn", "Let the browser read the clipboard, or paste with ⌘V.");
            return null;
          }
        }}
      />

      <OfferPrompt
        session={state.pendingOffers[0] ?? null}
        onAccept={(id, conflict: ConflictPolicy) => void actions.accept(id, conflict)}
        onReject={(id) => void actions.reject(id)}
      />

      <QrPanel
        open={qrOpen}
        url={pairing?.url ?? null}
        expiresInSecs={pairing?.expires_in_secs ?? 600}
        onClose={() => setQrOpen(false)}
        onRefresh={() => void openQr()}
        onRevoke={() => void actions.revokePairing()}
        peers={state.peers}
        onCopyLink={(url) => copyLabelled(url, "Pairing link")}
        onShareWifi={openWifi}
      />

      <WifiQrPanel
        open={wifiOpen}
        onClose={() => setWifiOpen(false)}
        info={wifiInfo}
        detecting={wifiDetecting}
        passwordFor={passwordFor}
        loadAccess={desktop.wifiPasswordAccess}
        loadPassword={desktop.wifiPassword}
        onRemember={rememberWifi}
        onDownload={saveWifiPng}
        onCopyImage={copyWifiImage}
        onCopyText={copyLabelled}
      />

      <Modal
        open={pinFor !== null}
        onClose={() => setPinFor(null)}
        title={`${pinFor?.peer.name ?? "That device"} needs a PIN`}
        description="Ask the person at the other device for their transfer PIN."
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPinFor(null)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={pin.length < 4}
              onClick={() => {
                if (!pinFor) return;
                void actions.send(pinFor.peer.id, pinFor.paths, pin);
                setPinFor(null);
                setTab("transfers");
              }}
            >
              Send
            </Button>
          </>
        }
      >
        <input
          className={cx(inputClass, "text-center text-lg tracking-[0.4em]")}
          inputMode="numeric"
          autoFocus
          maxLength={10}
          placeholder="••••"
          value={pin}
          onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))}
        />
      </Modal>

      <Toasts toasts={state.toasts} onDismiss={dismissToast} />
    </div>
  );
}

function Header({
  tab,
  onTab,
  activeCount,
}: {
  tab: Tab;
  onTab: (tab: Tab) => void;
  activeCount: number;
}) {
  return (
    <header className="drag-region sticky top-0 z-30 border-b border-ink-200/70 bg-ink-50/80 px-4 pt-3 backdrop-blur-lg sm:px-6 dark:border-ink-800 dark:bg-ink-950/80">
      <div className="mx-auto flex max-w-3xl items-center gap-2">
        <div className="flex items-center gap-2 pb-3">
          <span className="grid size-7 place-items-center rounded-lg bg-brand-600 text-white">
            <Icon name="upload" className="size-4" strokeWidth={2.2} />
          </span>
          <span className="text-[15px] font-semibold tracking-tight">ShareLocal</span>
        </div>

        <nav className="ml-auto flex items-end gap-0.5" aria-label="Sections">
          {TABS.map((item) => (
            <button
              key={item.id}
              type="button"
              onClick={() => onTab(item.id)}
              aria-current={tab === item.id ? "page" : undefined}
              className={cx(
                "relative flex items-center gap-1.5 rounded-t-lg px-3 py-2 text-sm font-medium transition",
                tab === item.id
                  ? "text-brand-700 dark:text-brand-300"
                  : "text-ink-500 hover:text-ink-800 dark:text-ink-400 dark:hover:text-ink-100",
              )}
            >
              <Icon name={item.icon} className="size-4" />
              <span className="hidden sm:inline">{item.label}</span>
              {item.id === "transfers" && activeCount > 0 ? (
                <span className="ml-0.5 grid size-4.5 place-items-center rounded-full bg-brand-600 text-[10px] font-semibold text-white">
                  {activeCount}
                </span>
              ) : null}
              {tab === item.id ? (
                <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-brand-600 dark:bg-brand-400" />
              ) : null}
            </button>
          ))}
        </nav>
      </div>
    </header>
  );
}

function toPaths(picked: unknown): string[] {
  if (typeof picked === "string") return [picked];
  if (Array.isArray(picked)) return picked.filter((p): p is string => typeof p === "string");
  return [];
}
