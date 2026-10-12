/** Small presentational primitives shared by the desktop and browser UIs. */

import type { ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

export function cx(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

const BUTTON_BASE =
  "inline-flex items-center justify-center gap-2 rounded-xl font-medium transition " +
  "active:scale-[0.98] disabled:pointer-events-none disabled:opacity-45 select-none";

const VARIANTS = {
  primary:
    "bg-brand-600 text-white hover:bg-brand-500 shadow-sm shadow-brand-600/25 " +
    "dark:bg-brand-500 dark:hover:bg-brand-400",
  secondary:
    "bg-white text-ink-800 ring-1 ring-ink-200 hover:bg-ink-50 " +
    "dark:bg-ink-800 dark:text-ink-100 dark:ring-ink-700 dark:hover:bg-ink-700",
  ghost:
    "text-ink-600 hover:bg-ink-100 hover:text-ink-900 " +
    "dark:text-ink-300 dark:hover:bg-ink-800 dark:hover:text-white",
  danger:
    "bg-rose-600 text-white hover:bg-rose-500 shadow-sm shadow-rose-600/25",
} as const;

const SIZES = {
  sm: "h-9 px-3 text-sm",
  md: "h-11 px-4 text-sm",
  // Touch-friendly, for the phone client and the main actions.
  lg: "h-13 px-5 text-base",
} as const;

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: keyof typeof VARIANTS;
  size?: keyof typeof SIZES;
  icon?: IconName;
  children?: ReactNode;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  className,
  children,
  ...rest
}: ButtonProps) {
  return (
    <button className={cx(BUTTON_BASE, VARIANTS[variant], SIZES[size], className)} {...rest}>
      {icon ? <Icon name={icon} className="size-4.5 shrink-0" /> : null}
      {children}
    </button>
  );
}

export function IconButton({
  label,
  icon,
  className,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; icon: IconName }) {
  return (
    <button
      aria-label={label}
      title={label}
      className={cx(
        "inline-flex size-9 items-center justify-center rounded-lg text-ink-500 transition",
        "hover:bg-ink-100 hover:text-ink-900 active:scale-95",
        "dark:text-ink-400 dark:hover:bg-ink-800 dark:hover:text-white",
        "disabled:pointer-events-none disabled:opacity-40",
        className,
      )}
      {...rest}
    >
      <Icon name={icon} className="size-4.5" />
    </button>
  );
}

export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div
      className={cx(
        "rounded-xl2 bg-white ring-1 ring-ink-200/70 dark:bg-ink-900 dark:ring-ink-800",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function SectionTitle({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return (
    <div className="mb-3 flex items-center justify-between gap-3">
      <h2 className="text-xs font-semibold uppercase tracking-[0.08em] text-ink-500 dark:text-ink-400">
        {children}
      </h2>
      {action}
    </div>
  );
}

type Tone = "neutral" | "online" | "warn" | "error" | "brand";

const TONES: Record<Tone, string> = {
  neutral: "bg-ink-100 text-ink-600 dark:bg-ink-800 dark:text-ink-300",
  online: "bg-emerald-100 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300",
  warn: "bg-amber-100 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300",
  error: "bg-rose-100 text-rose-700 dark:bg-rose-500/15 dark:text-rose-300",
  brand: "bg-brand-100 text-brand-700 dark:bg-brand-500/15 dark:text-brand-300",
};

export function Badge({ tone = "neutral", children }: { tone?: Tone; children: ReactNode }) {
  return (
    <span
      className={cx(
        "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium",
        TONES[tone],
      )}
    >
      {children}
    </span>
  );
}

/** A small live dot: green when online, amber when stale, grey when gone. */
export function StatusDot({ status }: { status: "online" | "stale" | "offline" }) {
  const colour =
    status === "online" ? "bg-emerald-500" : status === "stale" ? "bg-amber-500" : "bg-ink-400";
  return (
    <span className="relative inline-flex size-2">
      <span className={cx("absolute inset-0 rounded-full", colour)} />
      {status === "online" ? (
        <span className={cx("absolute inset-0 rounded-full animate-pulse-ring", colour)} />
      ) : null}
    </span>
  );
}

export function ProgressBar({
  value,
  tone = "brand",
  indeterminate = false,
  className,
}: {
  value: number;
  tone?: "brand" | "success" | "error";
  indeterminate?: boolean;
  className?: string;
}) {
  const fill =
    tone === "success" ? "bg-emerald-500" : tone === "error" ? "bg-rose-500" : "bg-brand-500";
  return (
    <div
      role="progressbar"
      aria-valuenow={indeterminate ? undefined : Math.round(value)}
      aria-valuemin={0}
      aria-valuemax={100}
      className={cx(
        "relative h-1.5 w-full overflow-hidden rounded-full bg-ink-200/80 dark:bg-ink-800",
        className,
      )}
    >
      <div
        className={cx("h-full rounded-full transition-[width] duration-300 ease-out", fill)}
        style={{ width: `${indeterminate ? 35 : Math.max(0, Math.min(100, value))}%` }}
      />
      {indeterminate ? (
        // Same reason as the toggle knob: pin the left edge rather than
        // relying on a static position.
        <div
          className="absolute inset-y-0 left-0 w-1/4 bg-white/40"
          style={{ animation: "sheen 1.4s linear infinite" }}
        />
      ) : null}
    </div>
  );
}

export function Spinner({ className = "size-4" }: { className?: string }) {
  return <Icon name="spinner" className={cx("animate-spin", className)} strokeWidth={2.2} />;
}

export function EmptyState({
  icon,
  title,
  children,
}: {
  icon: IconName;
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center gap-3 px-6 py-12 text-center">
      <div className="grid size-12 place-items-center rounded-2xl bg-ink-100 text-ink-400 dark:bg-ink-800 dark:text-ink-500">
        <Icon name={icon} className="size-6" />
      </div>
      <p className="font-medium text-ink-800 dark:text-ink-200">{title}</p>
      {children ? (
        <p className="max-w-sm text-sm leading-relaxed text-ink-500 dark:text-ink-400">{children}</p>
      ) : null}
    </div>
  );
}

export function Field({
  label,
  hint,
  children,
  htmlFor,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
  htmlFor?: string;
}) {
  return (
    <div className="flex flex-col gap-1.5 py-3">
      <label htmlFor={htmlFor} className="text-sm font-medium text-ink-800 dark:text-ink-200">
        {label}
      </label>
      {children}
      {hint ? <p className="text-xs leading-relaxed text-ink-500 dark:text-ink-400">{hint}</p> : null}
    </div>
  );
}

export const inputClass =
  "h-11 w-full rounded-xl bg-white px-3 text-sm text-ink-900 ring-1 ring-ink-200 " +
  "placeholder:text-ink-400 focus:ring-2 focus:ring-brand-500 focus:outline-none " +
  "dark:bg-ink-800 dark:text-ink-100 dark:ring-ink-700";

export function Toggle({
  checked,
  onChange,
  label,
  description,
  id,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  description?: string;
  id?: string;
}) {
  return (
    <div className="flex items-start justify-between gap-6 py-3">
      <div className="min-w-0">
        <label htmlFor={id} className="block text-sm font-medium text-ink-800 dark:text-ink-200">
          {label}
        </label>
        {description ? (
          <p className="mt-0.5 text-xs leading-relaxed text-ink-500 dark:text-ink-400">
            {description}
          </p>
        ) : null}
      </div>
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        onClick={() => onChange(!checked)}
        className={cx(
          "relative mt-0.5 h-6.5 w-11 shrink-0 rounded-full transition-colors",
          checked ? "bg-brand-600 dark:bg-brand-500" : "bg-ink-300 dark:bg-ink-700",
        )}
      >
        {/*
          `left-0` is load-bearing. Without it the knob is positioned from its
          static position inside the button, and a button's default
          `text-align: center` puts that at the middle of the track — so the
          knob renders 22px too far right and hangs off the end.
          Track 44 − knob 22 − 2px inset = 20px of travel.
        */}
        <span
          className={cx(
            "absolute top-0.5 left-0 size-5.5 rounded-full bg-white shadow transition-transform",
            checked ? "translate-x-5" : "translate-x-0.5",
          )}
        />
      </button>
    </div>
  );
}
