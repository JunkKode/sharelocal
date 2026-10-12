import { useState } from "react";
import type { ConflictPolicy, SessionView } from "../types";
import { formatBytes } from "../lib/format";
import { deviceIcon, Icon } from "./Icon";
import { Modal } from "./Modal";
import { Button, cx } from "./ui";

const POLICIES: { value: ConflictPolicy; label: string; hint: string }[] = [
  { value: "keepboth", label: "Keep both", hint: "Saves as “name (2)”" },
  { value: "replace", label: "Replace", hint: "Overwrites the existing file" },
  { value: "cancel", label: "Skip", hint: "Refuses files that already exist" },
];

/** The accept/decline prompt for an incoming transfer. */
export function OfferPrompt({
  session,
  onAccept,
  onReject,
}: {
  session: SessionView | null;
  onAccept: (id: string, conflict: ConflictPolicy) => void;
  onReject: (id: string) => void;
}) {
  const [policy, setPolicy] = useState<ConflictPolicy>("keepboth");
  if (!session) return null;

  const preview = session.files.slice(0, 4);
  const extra = session.files.length - preview.length;
  // The receiver has already checked the download folder, so this question is
  // only worth asking when something would actually be overwritten.
  const conflicts = session.conflicts ?? [];
  const isText = session.kind === "text";
  // A text share is saved under a name this device generates, so it can never
  // collide with anything.
  const hasConflicts = !isText && conflicts.length > 0;

  return (
    <Modal
      open
      onClose={() => onReject(session.id)}
      title={
        isText
          ? `${session.peer.name} sent you some text`
          : `${session.peer.name} wants to send you files`
      }
      description={
        <span className="inline-flex items-center gap-1.5">
          <Icon name={deviceIcon(session.peer.kind, session.peer.os)} className="size-4" />
          {isText
            ? `${session.text?.length ?? 0} characters`
            : `${session.files.length} ${session.files.length === 1 ? "file" : "files"} · ${formatBytes(
                session.total_bytes,
              )}`}
        </span>
      }
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={() => onReject(session.id)}>
            Decline
          </Button>
          <Button
            variant="primary"
            size="lg"
            icon={isText ? "check" : "download"}
            onClick={() => onAccept(session.id, policy)}
          >
            Accept
          </Button>
        </>
      }
    >
      {isText ? (
        <div className="mb-4">
          <pre className="max-h-52 overflow-auto whitespace-pre-wrap break-words rounded-xl bg-ink-50 p-3 text-sm leading-relaxed text-ink-800 dark:bg-ink-800/60 dark:text-ink-200">
            {session.text}
          </pre>
        </div>
      ) : (
      <ul className="mb-4 max-h-52 space-y-1 overflow-y-auto rounded-xl bg-ink-50 p-2 dark:bg-ink-800/60">
        {preview.map((file) => (
          <li
            key={file.rel_path}
            className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm text-ink-700 dark:text-ink-300"
          >
            <Icon name="file" className="size-4 shrink-0 text-ink-400" />
            <span className="min-w-0 flex-1 truncate" title={file.rel_path}>
              {file.rel_path}
            </span>
            {conflicts.includes(file.rel_path) ? (
              <span className="shrink-0 text-[11px] font-medium text-amber-600 dark:text-amber-400">
                already here
              </span>
            ) : null}
            <span className="shrink-0 tabular-nums text-ink-500 dark:text-ink-400">
              {formatBytes(file.size)}
            </span>
          </li>
        ))}
        {extra > 0 ? (
          <li className="px-2 py-1.5 text-sm text-ink-500 dark:text-ink-400">
            + {extra} more {extra === 1 ? "file" : "files"}
          </li>
        ) : null}
      </ul>
      )}

      {hasConflicts ? (
      <fieldset>
        <legend className="mb-2 text-xs font-semibold uppercase tracking-[0.08em] text-ink-500 dark:text-ink-400">
          {conflicts.length === 1
            ? "One file is already in your download folder"
            : `${conflicts.length} files are already in your download folder`}
        </legend>
        <div className="grid grid-cols-3 gap-2">
          {POLICIES.map((option) => (
            <button
              key={option.value}
              type="button"
              onClick={() => setPolicy(option.value)}
              aria-pressed={policy === option.value}
              className={cx(
                "rounded-xl px-2 py-2.5 text-center transition ring-1",
                policy === option.value
                  ? "bg-brand-50 ring-brand-500 dark:bg-brand-500/10 dark:ring-brand-400"
                  : "bg-white ring-ink-200 hover:ring-ink-300 dark:bg-ink-800 dark:ring-ink-700",
              )}
            >
              <span className="block text-sm font-medium text-ink-900 dark:text-ink-100">
                {option.label}
              </span>
              <span className="mt-0.5 block text-[11px] leading-tight text-ink-500 dark:text-ink-400">
                {option.hint}
              </span>
            </button>
          ))}
        </div>
      </fieldset>
      ) : null}
    </Modal>
  );
}
