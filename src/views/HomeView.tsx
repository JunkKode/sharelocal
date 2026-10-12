import type { Peer, SessionView, NetworkStatus, DeviceInfo } from "../types";
import { DeviceCard } from "../components/DeviceCard";
import { DropZone } from "../components/DropZone";
import { TransferCard } from "../components/TransferCard";
import { Icon } from "../components/Icon";
import { Badge, Button, EmptyState, IconButton, SectionTitle } from "../components/ui";

export function HomeView({
  device,
  network,
  peers,
  activeSessions,
  dropActive,
  dropTargetId,
  busyPeerIds,
  onSelectPeer,
  onPickFiles,
  onPickFolder,
  onSendText,
  onRefreshPeers,
  onOpenQr,
  onOpenWifi,
  onCancel,
  onPause,
  onOpenPath,
  onRevealPath,
  onCopyText,
}: {
  device: DeviceInfo | null;
  network: NetworkStatus | null;
  peers: Peer[];
  activeSessions: SessionView[];
  dropActive: boolean;
  dropTargetId: string | null;
  busyPeerIds: Set<string>;
  onSelectPeer: (peer: Peer) => void;
  onPickFiles: () => void;
  onPickFolder: () => void;
  onSendText: () => void;
  onRefreshPeers: () => void;
  onOpenQr: () => void;
  onOpenWifi: () => void;
  onCancel: (id: string) => void;
  onPause: (id: string, paused: boolean) => void;
  onOpenPath: (path: string) => void;
  onRevealPath: (path: string) => void;
  onCopyText: (text: string) => void;
}) {
  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 pb-8 sm:px-6">
      <ThisDevice device={device} network={network} onOpenQr={onOpenQr} onOpenWifi={onOpenWifi} />

      <section>
        <SectionTitle
          action={
            <div className="flex items-center gap-1">
              <IconButton label="Look for devices again" icon="refresh" onClick={onRefreshPeers} />
              <IconButton label="Connect a phone with a QR code" icon="qr" onClick={onOpenQr} />
            </div>
          }
        >
          Nearby devices {peers.length > 0 ? `(${peers.length})` : ""}
        </SectionTitle>

        {peers.length === 0 ? (
          <div className="rounded-xl2 bg-white ring-1 ring-ink-200/70 dark:bg-ink-900 dark:ring-ink-800">
            <EmptyState icon="wifi" title="No other devices yet">
              <span className="block">
                Devices appear here once they are running ShareLocal too. Open the app on another
                computer, or{" "}
                <button
                  type="button"
                  onClick={onOpenQr}
                  className="font-medium text-brand-600 underline underline-offset-2 dark:text-brand-300"
                >
                  show a QR code
                </button>{" "}
                and scan it with a phone — phones do not need to install anything.
              </span>
              <span className="mt-2 block">
                If another computer is running it and still does not show up, allow ShareLocal
                through the firewall on both machines.
              </span>
            </EmptyState>
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {peers.map((peer) => (
              <DeviceCard
                key={peer.id}
                peer={peer}
                dropTarget={dropTargetId === peer.id}
                busy={busyPeerIds.has(peer.id)}
                onSelect={onSelectPeer}
              />
            ))}
          </div>
        )}
      </section>

      <DropZone
        active={dropActive && !dropTargetId}
        onPickFiles={onPickFiles}
        onPickFolder={onPickFolder}
        onSendText={onSendText}
      />

      {activeSessions.length > 0 ? (
        <section>
          <SectionTitle>In progress</SectionTitle>
          <div className="flex flex-col gap-2">
            {activeSessions.map((session) => (
              <TransferCard
                key={session.id}
                session={session}
                onCancel={onCancel}
                onPause={onPause}
                onOpen={onOpenPath}
                onReveal={onRevealPath}
                onCopy={onCopyText}
              />
            ))}
          </div>
        </section>
      ) : null}
    </div>
  );
}

const HERO_BUTTON =
  "border-0 bg-white/15 text-white ring-0 backdrop-blur hover:bg-white/25 " +
  "dark:bg-white/15 dark:text-white dark:hover:bg-white/25";

function ThisDevice({
  device,
  network,
  onOpenQr,
  onOpenWifi,
}: {
  device: DeviceInfo | null;
  network: NetworkStatus | null;
  onOpenQr: () => void;
  onOpenWifi: () => void;
}) {
  const offline = network ? !network.online : false;
  const discoveryDown = network ? !network.mdns_ok && !network.udp_ok : false;

  return (
    <section className="rounded-xl2 bg-gradient-to-br from-brand-600 to-brand-500 p-5 text-white shadow-sm shadow-brand-600/20">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <p className="text-[11px] font-semibold uppercase tracking-[0.1em] text-white/70">
            This device
          </p>
          <h1 className="mt-1 truncate text-xl font-semibold">{device?.name ?? "ShareLocal"}</h1>
          <div className="mt-2 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[13px] text-white/85">
            {offline ? (
              <span className="inline-flex items-center gap-1.5">
                <Icon name="alert" className="size-3.5" />
                No local network
              </span>
            ) : (
              <>
                <span className="inline-flex items-center gap-1.5">
                  <Icon name={network?.link === "ethernet" ? "ethernet" : "wifi"} className="size-3.5" />
                  {network?.ipv4 ?? "…"}
                </span>
                <span aria-hidden>·</span>
                <span>port {network?.port}</span>
                {network?.interface ? (
                  <>
                    <span aria-hidden>·</span>
                    <span>{network.interface}</span>
                  </>
                ) : null}
              </>
            )}
            {network?.tls ? <Badge tone="brand">TLS on</Badge> : null}
          </div>
        </div>

        <div className="flex shrink-0 flex-col gap-2 sm:flex-row">
          <Button
            variant="secondary"
            icon="qr"
            onClick={onOpenQr}
            title="Connect a phone with a QR code"
            className={HERO_BUTTON}
          >
            <span className="hidden sm:inline">Connect phone</span>
          </Button>
          <Button
            variant="secondary"
            icon="wifi"
            onClick={onOpenWifi}
            title="Share this Wi-Fi network with a QR code"
            className={HERO_BUTTON}
          >
            <span className="hidden sm:inline">Share Wi-Fi</span>
          </Button>
        </div>
      </div>

      {network?.isolation_suspected ? (
        <p className="mt-3 rounded-lg bg-white/15 px-3 py-2 text-[13px] leading-snug">
          Devices are visible but not reachable. This network probably has client (AP) isolation
          switched on — try a different Wi-Fi network or connect over Ethernet.
        </p>
      ) : null}

      {discoveryDown && !offline ? (
        <p className="mt-3 rounded-lg bg-white/15 px-3 py-2 text-[13px] leading-snug">
          Device discovery could not start. Check that ShareLocal is allowed through your firewall.
        </p>
      ) : null}
    </section>
  );
}
