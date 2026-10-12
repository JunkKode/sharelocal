import type { Peer } from "../types";
import { formatBytes } from "../lib/format";
import { DeviceCard } from "./DeviceCard";
import { Modal } from "./Modal";
import { Button, EmptyState } from "./ui";

/** Asked after files are chosen but before a destination is known. */
export function DevicePicker({
  open,
  peers,
  fileCount,
  totalBytes,
  onPick,
  onClose,
}: {
  open: boolean;
  peers: Peer[];
  fileCount: number;
  totalBytes: number | null;
  onPick: (peer: Peer) => void;
  onClose: () => void;
}) {
  const available = peers.filter((p) => p.status !== "offline");

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Send to which device?"
      description={
        fileCount > 0
          ? `${fileCount} ${fileCount === 1 ? "item" : "items"}${
              totalBytes != null ? ` · ${formatBytes(totalBytes)}` : ""
            }`
          : undefined
      }
      footer={<Button onClick={onClose}>Cancel</Button>}
    >
      {available.length === 0 ? (
        <EmptyState icon="wifi" title="No devices nearby">
          Open ShareLocal on another device on the same network, or connect a phone with the QR
          code.
        </EmptyState>
      ) : (
        <div className="flex max-h-80 flex-col gap-2 overflow-y-auto">
          {available.map((peer) => (
            <DeviceCard key={peer.id} peer={peer} onSelect={onPick} />
          ))}
        </div>
      )}
    </Modal>
  );
}
