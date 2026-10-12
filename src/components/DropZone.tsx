import type { DragEvent, ReactNode } from "react";
import { Icon } from "./Icon";
import { Button, cx } from "./ui";

/**
 * The large drop target on the home screen.
 *
 * On the desktop, files arrive through Tauri's native drag-drop events (real
 * paths). In a browser, the HTML5 events are used instead. Both paths end up
 * calling the same handlers.
 */
export function DropZone({
  active,
  onPickFiles,
  onPickFolder,
  onSendText,
  onBrowserDrop,
  hint,
  children,
}: {
  active: boolean;
  onPickFiles: () => void;
  onPickFolder?: () => void;
  onSendText?: () => void;
  onBrowserDrop?: (files: File[]) => void;
  hint?: ReactNode;
  children?: ReactNode;
}) {
  const handleDrop = (event: DragEvent<HTMLDivElement>) => {
    if (!onBrowserDrop) return;
    event.preventDefault();
    const files = Array.from(event.dataTransfer?.files ?? []);
    if (files.length) onBrowserDrop(files);
  };

  return (
    <div
      onDragOver={onBrowserDrop ? (e) => e.preventDefault() : undefined}
      onDrop={handleDrop}
      className={cx(
        "relative flex flex-col items-center justify-center gap-4 rounded-xl2 border-2 border-dashed px-6 py-10 text-center transition",
        active
          ? "border-brand-500 bg-brand-50 dark:bg-brand-500/10"
          : "border-ink-200 bg-white/60 hover:border-ink-300 dark:border-ink-800 dark:bg-ink-900/50 dark:hover:border-ink-700",
      )}
    >
      <div
        className={cx(
          "grid size-14 place-items-center rounded-2xl transition",
          active
            ? "bg-brand-500 text-white scale-110"
            : "bg-ink-100 text-ink-400 dark:bg-ink-800 dark:text-ink-500",
        )}
      >
        <Icon name="upload" className="size-6.5" />
      </div>

      <div>
        <p className="text-[15px] font-medium text-ink-900 dark:text-ink-100">
          {active ? "Release to send" : "Drop files here"}
        </p>
        {hint ? (
          <p className="mt-1 text-sm text-ink-500 dark:text-ink-400">{hint}</p>
        ) : (
          <p className="mt-1 text-sm text-ink-500 dark:text-ink-400">
            or drop them straight onto a device above
          </p>
        )}
      </div>

      <div className="flex flex-wrap items-center justify-center gap-2">
        <Button variant="primary" size="lg" icon="file" onClick={onPickFiles}>
          Select Files
        </Button>
        {onPickFolder ? (
          <Button variant="secondary" size="lg" icon="folder" onClick={onPickFolder}>
            Select Folder
          </Button>
        ) : null}
        {onSendText ? (
          <Button variant="secondary" size="lg" icon="text" onClick={onSendText}>
            Send Text
          </Button>
        ) : null}
      </div>

      {children}
    </div>
  );
}
