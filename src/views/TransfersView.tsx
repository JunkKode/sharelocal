import type { SessionView } from "../types";
import { TransferCard } from "../components/TransferCard";
import { Button, EmptyState, SectionTitle } from "../components/ui";

export function TransfersView({
  active,
  finished,
  onCancel,
  onPause,
  onOpenPath,
  onRevealPath,
  onCopyText,
  onClearFinished,
}: {
  active: SessionView[];
  finished: SessionView[];
  onCancel: (id: string) => void;
  onPause: (id: string, paused: boolean) => void;
  onOpenPath: (path: string) => void;
  onRevealPath: (path: string) => void;
  onCopyText: (text: string) => void;
  onClearFinished: () => void;
}) {
  if (active.length === 0 && finished.length === 0) {
    return (
      <div className="mx-auto w-full max-w-3xl px-4 sm:px-6">
        <div className="rounded-xl2 bg-white ring-1 ring-ink-200/70 dark:bg-ink-900 dark:ring-ink-800">
          <EmptyState icon="upload" title="No transfers yet">
            Send something from the home screen and it will show up here with live speed and
            progress.
          </EmptyState>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 pb-8 sm:px-6">
      {active.length > 0 ? (
        <section>
          <SectionTitle>Active ({active.length})</SectionTitle>
          <div className="flex flex-col gap-2">
            {active.map((session) => (
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

      {finished.length > 0 ? (
        <section>
          <SectionTitle
            action={
              <Button size="sm" variant="ghost" onClick={onClearFinished}>
                Clear
              </Button>
            }
          >
            Finished
          </SectionTitle>
          <div className="flex flex-col gap-2">
            {finished.map((session) => (
              <TransferCard
                key={session.id}
                session={session}
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
