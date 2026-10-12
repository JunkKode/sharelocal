import type { SessionView } from "../types";
import {
  baseName,
  describeFiles,
  formatBytes,
  formatEta,
  formatPercent,
  formatSpeed,
  percent,
} from "../lib/format";
import { Icon } from "./Icon";
import { TextPreview } from "./TextComposer";
import { Badge, Button, cx, IconButton, ProgressBar } from "./ui";

const STATUS_COPY: Record<string, string> = {
  pending: "Waiting for the other device",
  accepted: "Starting…",
  transferring: "In progress",
  paused: "Paused",
  completed: "Complete",
  cancelled: "Cancelled",
  rejected: "Declined",
  failed: "Failed",
};

/**
 * One active or finished transfer. Shows everything the spec asks for: name,
 * size, speed, percentage, bar, ETA and direction.
 */
export function TransferCard({
  session,
  onCancel,
  onPause,
  onOpen,
  onReveal,
  onCopy,
}: {
  session: SessionView;
  onCancel?: (id: string) => void;
  onPause?: (id: string, paused: boolean) => void;
  onOpen?: (path: string) => void;
  onReveal?: (path: string) => void;
  onCopy?: (text: string) => void;
}) {
  const { progress } = session;
  const sending = session.role === "sending";
  const done = progress.bytes_done;
  // Sending to a phone turns the transfer around: the files are published and
  // nothing moves until someone taps Download over there. Showing that as
  // "Starting…" with a spinning bar made it look stuck.
  const waitingForPhone =
    session.direction === "pull" && sending && session.status === "accepted";
  const running =
    session.status === "transferring" || (session.status === "accepted" && !waitingForPhone);
  const finished = session.status === "completed";
  const failed = session.status === "failed" || session.status === "cancelled" || session.status === "rejected";

  const isText = session.kind === "text";
  const currentFile = session.files[progress.file_index];
  // A finished text share renders the snippet in full below, so the heading
  // stays a plain label rather than repeating the first line.
  const title = isText
    ? "Shared text"
    : session.files.length > 1
      ? describeFiles(session.files)
      : baseName(session.files[0]?.rel_path ?? "");

  return (
    <div className="rounded-xl2 bg-white p-4 ring-1 ring-ink-200/70 animate-rise dark:bg-ink-900 dark:ring-ink-800">
      <div className="flex items-start gap-3">
        <div
          className={cx(
            "grid size-9 shrink-0 place-items-center rounded-lg",
            finished
              ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300"
              : failed
                ? "bg-rose-100 text-rose-700 dark:bg-rose-500/15 dark:text-rose-300"
                : "bg-brand-100 text-brand-700 dark:bg-brand-500/15 dark:text-brand-300",
          )}
        >
          <Icon
            name={
              finished ? "check" : failed ? "alert" : isText ? "text" : sending ? "upload" : "download"
            }
            className="size-4.5"
          />
        </div>

        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-3">
            <p className="truncate font-medium text-ink-900 dark:text-ink-100">{title}</p>
            {running ? (
              <span className="shrink-0 text-sm font-semibold tabular-nums text-brand-600 dark:text-brand-300">
                {formatPercent(done, session.total_bytes)}
              </span>
            ) : null}
          </div>

          <p className="mt-0.5 truncate text-[13px] text-ink-500 dark:text-ink-400">
            {sending ? "Sending to" : "Receiving from"} {session.peer.name}
            {session.files.length > 1 && running ? (
              <>
                {" · "}
                file {progress.file_index + 1} of {session.files.length}
                {currentFile ? ` · ${baseName(currentFile.rel_path)}` : ""}
              </>
            ) : null}
          </p>
        </div>

        <div className="flex shrink-0 items-center gap-1">
          {running && onPause ? (
            <IconButton label="Pause transfer" icon="pause" onClick={() => onPause(session.id, true)} />
          ) : null}
          {session.status === "paused" && onPause ? (
            <IconButton label="Resume transfer" icon="play" onClick={() => onPause(session.id, false)} />
          ) : null}
          {(running || waitingForPhone || session.status === "paused" || session.status === "pending") &&
          onCancel ? (
            <IconButton label="Cancel transfer" icon="close" onClick={() => onCancel(session.id)} />
          ) : null}
        </div>
      </div>

      {waitingForPhone ? (
        <div className="mt-3 flex items-start gap-2.5 rounded-xl bg-brand-50 px-3 py-2.5 text-[13px] leading-snug text-brand-800 dark:bg-brand-500/10 dark:text-brand-200">
          <Icon name="phone" className="mt-px size-4 shrink-0" />
          <span>
            Ready on {session.peer.name}. It downloads when someone taps{" "}
            <strong className="font-semibold">Download</strong> on the phone.
          </span>
        </div>
      ) : running || session.status === "paused" ? (
        <>
          <ProgressBar
            className="mt-3"
            value={percent(done, session.total_bytes)}
            indeterminate={session.status === "accepted"}
          />
          <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] tabular-nums text-ink-500 dark:text-ink-400">
            <span>
              {formatBytes(done)} / {formatBytes(session.total_bytes)}
            </span>
            <span aria-hidden>·</span>
            <span className="font-medium text-ink-700 dark:text-ink-200">
              {formatSpeed(progress.speed_bps)}
            </span>
            <span aria-hidden>·</span>
            <span>{formatEta(progress.eta_secs)}</span>
          </div>
        </>
      ) : (
        <div className="mt-2.5 flex flex-wrap items-center gap-2 text-[13px] text-ink-500 dark:text-ink-400">
          <Badge tone={finished ? "online" : failed ? "error" : "neutral"}>
            {STATUS_COPY[session.status] ?? session.status}
          </Badge>
          <span>{isText ? "text" : formatBytes(session.total_bytes)}</span>
          {session.error ? (
            <span className="w-full text-rose-600 dark:text-rose-400">{session.error.detail}</span>
          ) : null}
        </div>
      )}

      {finished && isText && session.text ? (
        <TextPreview text={session.text} onCopy={onCopy} className="mt-3" />
      ) : null}

      {finished && !isText && session.saved_paths?.length && (onOpen || onReveal) ? (
        <div className="mt-3 flex flex-wrap gap-2">
          {onOpen ? (
            <Button size="sm" icon="external" onClick={() => onOpen(session.saved_paths![0]!)}>
              Open
            </Button>
          ) : null}
          {onReveal ? (
            <Button size="sm" icon="folder" onClick={() => onReveal(session.saved_paths![0]!)}>
              Show in folder
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
