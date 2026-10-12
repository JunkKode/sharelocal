import type { Peer } from "../types";
import { formatSpeed } from "../lib/format";
import { deviceIcon, Icon } from "./Icon";
import { Badge, cx, StatusDot } from "./ui";

const LINK_LABEL: Record<string, string> = {
  wifi: "Wi-Fi",
  ethernet: "Ethernet",
  unknown: "Local network",
};

/**
 * One row in "Nearby devices". Carries `data-device-id` so a native file drop
 * can be hit-tested against it with `document.elementFromPoint`.
 */
export function DeviceCard({
  peer,
  selected,
  dropTarget,
  busy,
  onSelect,
}: {
  peer: Peer;
  selected?: boolean;
  dropTarget?: boolean;
  busy?: boolean;
  onSelect: (peer: Peer) => void;
}) {
  const offline = peer.status === "offline";
  const speed = peer.last_speed_bps ? formatSpeed(peer.last_speed_bps) : null;
  const statusText = offline
    ? "Offline"
    : peer.status === "stale"
      ? "Not responding"
      : "Online";

  return (
    <button
      type="button"
      data-device-id={peer.id}
      onClick={() => onSelect(peer)}
      disabled={offline}
      aria-pressed={selected}
      className={cx(
        "group relative flex w-full items-center gap-3.5 rounded-xl2 p-3.5 text-left transition",
        "ring-1 ring-ink-200/70 bg-white hover:ring-ink-300",
        "dark:bg-ink-900 dark:ring-ink-800 dark:hover:ring-ink-700",
        selected && "ring-2 ring-brand-500 dark:ring-brand-400",
        dropTarget && "ring-2 ring-brand-500 bg-brand-50 scale-[1.015] dark:bg-brand-500/10",
        offline && "opacity-55",
      )}
    >
      <div
        className={cx(
          "grid size-11 shrink-0 place-items-center rounded-xl transition",
          dropTarget
            ? "bg-brand-500 text-white"
            : "bg-ink-100 text-ink-600 group-hover:bg-brand-100 group-hover:text-brand-700 dark:bg-ink-800 dark:text-ink-300 dark:group-hover:bg-brand-500/15 dark:group-hover:text-brand-300",
        )}
      >
        <Icon name={deviceIcon(peer.kind, peer.os)} className="size-5.5" />
      </div>

      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate font-medium text-ink-900 dark:text-ink-100">{peer.name}</span>
          {peer.pin_required ? (
            <Icon name="lock" className="size-3.5 shrink-0 text-ink-400" title="Requires a PIN" />
          ) : null}
          {peer.tls ? <Badge tone="brand">TLS</Badge> : null}
        </div>

        <div className="mt-0.5 flex items-center gap-1.5 text-[13px] text-ink-500 dark:text-ink-400">
          <StatusDot status={peer.status} />
          <span>{statusText}</span>
          <span aria-hidden>·</span>
          <span className="inline-flex items-center gap-1">
            <Icon name={peer.link === "ethernet" ? "ethernet" : "wifi"} className="size-3.5" />
            {LINK_LABEL[peer.link] ?? "Local network"}
          </span>
          {speed ? (
            <>
              <span aria-hidden>·</span>
              <span>{speed}</span>
            </>
          ) : null}
        </div>

        {!peer.same_subnet ? (
          <p className="mt-1.5 text-[12px] leading-snug text-amber-700 dark:text-amber-300">
            On a different subnet — your router may block device-to-device traffic.
          </p>
        ) : null}
      </div>

      {busy ? (
        <span className="text-xs font-medium text-brand-600 dark:text-brand-300">Sending…</span>
      ) : (
        <Icon
          name="chevron"
          className="size-4 shrink-0 text-ink-300 transition group-hover:translate-x-0.5 dark:text-ink-600"
        />
      )}
    </button>
  );
}
