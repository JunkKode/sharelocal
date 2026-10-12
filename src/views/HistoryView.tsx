import { useMemo } from "react";
import type { HistoryEntry } from "../types";
import { dayLabel, formatBytes, formatSpeed, formatTime } from "../lib/format";
import { Icon } from "../components/Icon";
import { Button, EmptyState, IconButton, SectionTitle, cx } from "../components/ui";

export function HistoryView({
  entries,
  onClear,
  onRemove,
  onReveal,
}: {
  entries: HistoryEntry[];
  onClear: () => void;
  onRemove: (id: string) => void;
  onReveal: (path: string) => void;
}) {
  const groups = useMemo(() => groupByDay(entries), [entries]);

  if (entries.length === 0) {
    return (
      <div className="mx-auto w-full max-w-3xl px-4 sm:px-6">
        <div className="rounded-xl2 bg-white ring-1 ring-ink-200/70 dark:bg-ink-900 dark:ring-ink-800">
          <EmptyState icon="history" title="No history yet">
            Completed transfers are listed here. Only names, sizes and speeds are stored — never the
            files themselves.
          </EmptyState>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 pb-8 sm:px-6">
      <SectionTitle
        action={
          <Button size="sm" variant="ghost" onClick={onClear}>
            Clear history
          </Button>
        }
      >
        Transfer history
      </SectionTitle>

      {groups.map(([label, items]) => (
        <section key={label}>
          <h3 className="mb-2 text-xs font-semibold uppercase tracking-[0.08em] text-ink-400 dark:text-ink-500">
            {label}
          </h3>
          <div className="overflow-hidden rounded-xl2 bg-white ring-1 ring-ink-200/70 dark:bg-ink-900 dark:ring-ink-800">
            {items.map((entry, index) => (
              <HistoryRow
                key={entry.id}
                entry={entry}
                first={index === 0}
                onRemove={onRemove}
                onReveal={onReveal}
              />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

function HistoryRow({
  entry,
  first,
  onRemove,
  onReveal,
}: {
  entry: HistoryEntry;
  first: boolean;
  onRemove: (id: string) => void;
  onReveal: (path: string) => void;
}) {
  const sending = entry.role === "sending";
  const failed = entry.status !== "completed";
  const savedPath = entry.saved_paths?.[0];

  return (
    <div
      className={cx(
        "group flex items-center gap-3 px-4 py-3",
        !first && "border-t border-ink-100 dark:border-ink-800",
      )}
    >
      <div
        className={cx(
          "grid size-8 shrink-0 place-items-center rounded-lg",
          failed
            ? "bg-rose-100 text-rose-600 dark:bg-rose-500/15 dark:text-rose-300"
            : "bg-ink-100 text-ink-500 dark:bg-ink-800 dark:text-ink-400",
        )}
      >
        <Icon name={failed ? "alert" : sending ? "upload" : "download"} className="size-4" />
      </div>

      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium text-ink-900 dark:text-ink-100">
          {entry.display_name}
        </p>
        <p className="mt-0.5 truncate text-[13px] text-ink-500 dark:text-ink-400">
          {sending ? "Sent to" : "Received from"} {entry.peer_name} · {formatTime(entry.at_ms)}
          {failed ? ` · ${entry.status}` : ""}
        </p>
      </div>

      <div className="hidden shrink-0 text-right text-[13px] tabular-nums text-ink-500 sm:block dark:text-ink-400">
        <p>{formatBytes(entry.total_bytes)}</p>
        {entry.avg_speed_bps > 0 && !failed ? <p>{formatSpeed(entry.avg_speed_bps)}</p> : null}
      </div>

      <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition group-hover:opacity-100 focus-within:opacity-100">
        {savedPath ? (
          <IconButton label="Show in folder" icon="folder" onClick={() => onReveal(savedPath)} />
        ) : null}
        <IconButton label="Remove from history" icon="close" onClick={() => onRemove(entry.id)} />
      </div>
    </div>
  );
}

/** Group entries under Today / Yesterday / weekday / date headings. */
export function groupByDay(entries: HistoryEntry[], now = Date.now()): [string, HistoryEntry[]][] {
  const groups = new Map<string, HistoryEntry[]>();
  for (const entry of entries) {
    const label = dayLabel(entry.at_ms, now);
    const bucket = groups.get(label);
    if (bucket) bucket.push(entry);
    else groups.set(label, [entry]);
  }
  return Array.from(groups.entries());
}
