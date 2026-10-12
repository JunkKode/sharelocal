import { useEffect, useRef, useState } from "react";
import { MAX_TEXT_BYTES } from "../types";
import { Icon } from "./Icon";
import { Modal } from "./Modal";
import { Button, cx } from "./ui";
import { asLink, copyText } from "../lib/clipboard";

/**
 * Compose a snippet to send — a link, a code, a paragraph. Anything larger
 * than `MAX_TEXT_BYTES` belongs in a file, and the box says so rather than
 * failing after the send.
 */
export function TextComposer({
  open,
  onClose,
  onSend,
  onPasteClipboard,
  destinationName,
}: {
  open: boolean;
  onClose: () => void;
  onSend: (text: string) => void;
  onPasteClipboard?: () => Promise<string | null>;
  destinationName?: string;
}) {
  const [text, setText] = useState("");
  const area = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (open) {
      setText("");
      // Let the modal mount before stealing focus.
      const id = setTimeout(() => area.current?.focus(), 0);
      return () => clearTimeout(id);
    }
    return undefined;
  }, [open]);

  const bytes = new TextEncoder().encode(text).length;
  const tooLong = bytes > MAX_TEXT_BYTES;
  const empty = text.trim().length === 0;

  const send = () => {
    if (empty || tooLong) return;
    onSend(text);
    onClose();
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Send text"
      description={
        destinationName
          ? `A snippet goes straight to ${destinationName} — no file to open.`
          : "A snippet goes straight across — no file to open."
      }
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" icon="upload" disabled={empty || tooLong} onClick={send}>
            Send text
          </Button>
        </>
      }
    >
      <textarea
        ref={area}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // The usual "send" chord, so a short snippet never needs the mouse.
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            send();
          }
        }}
        rows={7}
        placeholder="Paste a link, a code, an address…"
        className="w-full resize-y rounded-xl bg-white p-3 text-sm leading-relaxed text-ink-900 ring-1 ring-ink-200 focus:ring-2 focus:ring-brand-500 focus:outline-none dark:bg-ink-800 dark:text-ink-100 dark:ring-ink-700"
      />

      <div className="mt-2 flex items-center justify-between gap-3">
        {onPasteClipboard ? (
          <Button
            size="sm"
            icon="copy"
            onClick={async () => {
              const clip = await onPasteClipboard();
              if (clip) setText(clip);
            }}
          >
            Paste clipboard
          </Button>
        ) : (
          <span />
        )}
        <span
          className={cx(
            "text-xs tabular-nums",
            tooLong ? "text-rose-600 dark:text-rose-400" : "text-ink-500 dark:text-ink-400",
          )}
        >
          {tooLong
            ? `${Math.round(bytes / 1024)} KB — too long, send it as a file`
            : `${text.length} characters`}
        </span>
      </div>

      <p className="mt-3 flex items-start gap-1.5 text-xs leading-relaxed text-ink-500 dark:text-ink-400">
        <Icon name="text" className="mt-0.5 size-3.5 shrink-0" />
        The other device can copy it straight to the clipboard, and it is also
        saved as a .txt file.
      </p>
    </Modal>
  );
}

/** A received snippet, with the one action people actually want: copy. */
export function TextPreview({
  text,
  onCopy,
  className,
}: {
  text: string;
  onCopy?: (text: string) => void;
  className?: string;
}) {
  const [copied, setCopied] = useState<"idle" | "ok" | "failed">("idle");
  const link = asLink(text);

  const copy = async () => {
    let ok = true;
    if (onCopy) onCopy(text);
    else ok = await copyText(text);
    setCopied(ok ? "ok" : "failed");
    setTimeout(() => setCopied("idle"), 1600);
  };

  return (
    <div className={className}>
      <pre className="max-h-44 overflow-auto whitespace-pre-wrap break-words rounded-xl bg-ink-50 p-3 text-sm leading-relaxed text-ink-800 select-text dark:bg-ink-800/60 dark:text-ink-200">
        {text}
      </pre>
      <div className="mt-2 flex flex-wrap gap-2">
        <Button
          size="sm"
          icon={copied === "ok" ? "check" : "copy"}
          onClick={copy}
          aria-live="polite"
        >
          {copied === "ok" ? "Copied" : copied === "failed" ? "Press and hold the text to copy" : "Copy"}
        </Button>
        {link ? (
          <a
            href={link}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex h-9 items-center justify-center gap-2 rounded-xl bg-white px-3 text-sm font-medium text-ink-800 ring-1 ring-ink-200 transition hover:bg-ink-50 dark:bg-ink-800 dark:text-ink-100 dark:ring-ink-700 dark:hover:bg-ink-700"
          >
            <Icon name="external" className="size-4.5" />
            Open link
          </a>
        ) : null}
      </div>
    </div>
  );
}
