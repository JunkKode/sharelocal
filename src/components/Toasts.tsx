import { useEffect } from "react";
import type { NoticeLevel } from "../types";
import { Icon } from "./Icon";
import { cx } from "./ui";

export interface Toast {
  id: string;
  level: NoticeLevel;
  message: string;
}

const TONE: Record<NoticeLevel, string> = {
  info: "bg-ink-900 text-white dark:bg-ink-100 dark:text-ink-900",
  warn: "bg-amber-500 text-amber-950",
  error: "bg-rose-600 text-white",
};

export function Toasts({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: string) => void }) {
  return (
    <div
      className="pointer-events-none fixed inset-x-0 bottom-0 z-60 flex flex-col items-center gap-2 p-4"
      role="status"
      aria-live="polite"
    >
      {toasts.map((toast) => (
        <ToastRow key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>
  );
}

function ToastRow({ toast, onDismiss }: { toast: Toast; onDismiss: (id: string) => void }) {
  useEffect(() => {
    // Errors stay a little longer, because they usually need reading twice.
    const ms = toast.level === "error" ? 9000 : 5000;
    const timer = setTimeout(() => onDismiss(toast.id), ms);
    return () => clearTimeout(timer);
  }, [toast.id, toast.level, onDismiss]);

  return (
    <div
      className={cx(
        "pointer-events-auto flex max-w-lg items-start gap-2.5 rounded-xl px-4 py-3 text-sm shadow-lg animate-rise",
        TONE[toast.level],
      )}
    >
      <Icon
        name={toast.level === "error" ? "alert" : toast.level === "warn" ? "alert" : "check"}
        className="mt-0.5 size-4 shrink-0"
      />
      <span className="flex-1 leading-snug">{toast.message}</span>
      <button
        type="button"
        aria-label="Dismiss"
        onClick={() => onDismiss(toast.id)}
        className="-mr-1 rounded p-0.5 opacity-70 transition hover:opacity-100"
      >
        <Icon name="close" className="size-4" />
      </button>
    </div>
  );
}
