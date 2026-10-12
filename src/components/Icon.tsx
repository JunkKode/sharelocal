import type { JSX } from "react";

/**
 * A small inline icon set. Inline SVG rather than an icon font keeps the
 * bundle self-contained — the browser client must load with no external
 * requests, which the server's CSP also enforces.
 */

export type IconName =
  | "laptop"
  | "desktop"
  | "phone"
  | "tablet"
  | "browser"
  | "wifi"
  | "ethernet"
  | "upload"
  | "download"
  | "folder"
  | "file"
  | "qr"
  | "settings"
  | "history"
  | "close"
  | "check"
  | "alert"
  | "pause"
  | "play"
  | "refresh"
  | "chevron"
  | "external"
  | "lock"
  | "text"
  | "copy"
  | "camera"
  | "image"
  | "plus"
  | "eye"
  | "eyeOff"
  | "usb"
  | "trash"
  | "music"
  | "video"
  | "fingerprint"
  | "chevronLeft"
  | "spinner";

const PATHS: Record<IconName, JSX.Element> = {
  laptop: (
    <>
      <rect x="4" y="5" width="16" height="11" rx="2" />
      <path d="M2 19h20" />
    </>
  ),
  desktop: (
    <>
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8M12 16v4" />
    </>
  ),
  phone: (
    <>
      <rect x="7" y="2" width="10" height="20" rx="2.5" />
      <path d="M11 18.5h2" />
    </>
  ),
  tablet: (
    <>
      <rect x="4" y="2.5" width="16" height="19" rx="2.5" />
      <path d="M11 18.5h2" />
    </>
  ),
  browser: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2.5" />
      <path d="M3 9h18M7 6.5h.01M9.5 6.5h.01" />
    </>
  ),
  wifi: (
    <>
      <path d="M2.5 9a15 15 0 0 1 19 0M5.5 12.5a10 10 0 0 1 13 0M8.5 16a5.5 5.5 0 0 1 7 0" />
      <circle cx="12" cy="19.5" r="0.9" fill="currentColor" stroke="none" />
    </>
  ),
  ethernet: (
    <>
      <rect x="3" y="9" width="18" height="10" rx="2" />
      <path d="M7 9V6a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v3M8 19v-3M12 19v-3M16 19v-3" />
    </>
  ),
  upload: (
    <>
      <path d="M12 16V4M7.5 8.5 12 4l4.5 4.5" />
      <path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3" />
    </>
  ),
  download: (
    <>
      <path d="M12 4v12M7.5 11.5 12 16l4.5-4.5" />
      <path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3" />
    </>
  ),
  folder: <path d="M3 7.5A2 2 0 0 1 5 5.5h3.6a2 2 0 0 1 1.5.7l1 1.2H19a2 2 0 0 1 2 2v7.1a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />,
  file: (
    <>
      <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
      <path d="M14 3v5h5" />
    </>
  ),
  qr: (
    <>
      <rect x="3.5" y="3.5" width="6" height="6" rx="1.2" />
      <rect x="14.5" y="3.5" width="6" height="6" rx="1.2" />
      <rect x="3.5" y="14.5" width="6" height="6" rx="1.2" />
      <path d="M14.5 14.5h3v3h-3zM20.5 17.5v3h-3M17.5 20.5h-0.01" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1 1.56V21a2 2 0 0 1-4 0v-.11a1.7 1.7 0 0 0-1.11-1.56 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.56-1H3a2 2 0 0 1 0-4h.11a1.7 1.7 0 0 0 1.56-1.11 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34H9a1.7 1.7 0 0 0 1-1.56V3a2 2 0 0 1 4 0v.11a1.7 1.7 0 0 0 1 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87V9a1.7 1.7 0 0 0 1.56 1H21a2 2 0 0 1 0 4h-.11a1.7 1.7 0 0 0-1.49 1z" />
    </>
  ),
  history: (
    <>
      <path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1M3.5 5v4h4" />
      <path d="M12 7.5V12l3 1.8" />
    </>
  ),
  close: <path d="M6 6l12 12M18 6L6 18" />,
  check: <path d="M4.5 12.5 9 17l10.5-10.5" />,
  alert: (
    <>
      <path d="M12 3.5 21.5 20h-19z" />
      <path d="M12 9.5v4.2M12 17h.01" />
    </>
  ),
  pause: <path d="M9 5v14M15 5v14" />,
  play: <path d="M7 4.5 19 12 7 19.5z" />,
  refresh: (
    <>
      <path d="M20 11a8 8 0 1 0-1.8 6.1" />
      <path d="M20.5 5.5V11h-5.5" />
    </>
  ),
  chevron: <path d="M9 5l7 7-7 7" />,
  chevronLeft: <path d="M15 5l-7 7 7 7" />,
  external: (
    <>
      <path d="M14 4h6v6" />
      <path d="M20 4l-9 9" />
      <path d="M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4" />
    </>
  ),
  lock: (
    <>
      <rect x="4.5" y="10.5" width="15" height="10" rx="2.2" />
      <path d="M8 10.5V7.5a4 4 0 0 1 8 0v3" />
    </>
  ),
  camera: (
    <>
      <path d="M3 8.5A2 2 0 0 1 5 6.5h2.2l1-2h5.6l1 2H19a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      <circle cx="12" cy="13" r="3.5" />
    </>
  ),
  text: (
    <>
      <path d="M4 6.5V5h16v1.5" />
      <path d="M12 5v14M9 19h6" />
    </>
  ),
  copy: (
    <>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1" />
    </>
  ),
  image: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2.5" />
      <circle cx="8.5" cy="9.5" r="1.5" />
      <path d="m21 15.5-4.6-4.6a1.5 1.5 0 0 0-2.1 0L5 20" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  eye: (
    <>
      <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" />
      <circle cx="12" cy="12" r="2.8" />
    </>
  ),
  eyeOff: (
    <>
      <path d="M10.6 5.6A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a17 17 0 0 1-2.9 3.7M6.6 6.7C3.9 8.4 2.5 12 2.5 12S6 18.5 12 18.5c1.7 0 3.2-.5 4.4-1.2" />
      <path d="M9.9 10a2.8 2.8 0 0 0 4 4M3 3l18 18" />
    </>
  ),
  usb: (
    <>
      <path d="M12 2.5v15M12 2.5 9.5 5.5M12 2.5l2.5 3" />
      <path d="M12 13.5 7 11V8.5M12 15.5l5-2.5V10" />
      <rect x="5.5" y="6.5" width="3" height="2" rx=".5" />
      <circle cx="17" cy="9" r="1.2" />
      <circle cx="12" cy="19.5" r="2" />
    </>
  ),
  trash: (
    <>
      <path d="M4 6.5h16M9.5 6.5V4.5h5v2M6 6.5l1 13a1.5 1.5 0 0 0 1.5 1.4h7a1.5 1.5 0 0 0 1.5-1.4l1-13" />
      <path d="M10 10.5v6M14 10.5v6" />
    </>
  ),
  music: (
    <>
      <path d="M9 18V5.5l11-2V16" />
      <circle cx="6.5" cy="18" r="2.5" />
      <circle cx="17.5" cy="16" r="2.5" />
    </>
  ),
  video: (
    <>
      <rect x="2.5" y="6" width="13" height="12" rx="2" />
      <path d="m15.5 10.5 6-3.5v10l-6-3.5" />
    </>
  ),
  fingerprint: (
    <>
      <path d="M6.5 5.5A8 8 0 0 1 19.5 9" />
      <path d="M4.5 9.5a8 8 0 0 0-.3 4.5M20 13a12 12 0 0 1-.8 4" />
      <path d="M8 18.5c1-1.8 1.5-3.9 1.5-6a2.5 2.5 0 0 1 5 0c0 1 0 2-.2 3" />
      <path d="M7 11.5a5 5 0 0 1 10 1c0 2.6-.5 5.2-1.6 7.5" />
      <path d="M12 12.5c0 3-.8 5.9-2.3 8.5" />
    </>
  ),
  spinner: <path d="M12 3a9 9 0 0 1 9 9" />,
};

interface IconProps {
  name: IconName;
  className?: string;
  strokeWidth?: number;
  title?: string;
}

export function Icon({ name, className = "size-5", strokeWidth = 1.7, title }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
    >
      {title ? <title>{title}</title> : null}
      {PATHS[name]}
    </svg>
  );
}

/** Map a peer's declared kind onto an icon. */
export function deviceIcon(kind: string, os: string): IconName {
  switch (kind) {
    case "phone":
      return "phone";
    case "tablet":
      return "tablet";
    case "browser":
      return "browser";
    case "laptop":
      return "laptop";
    case "desktop":
      return "desktop";
    default:
      return os === "ios" || os === "android" ? "phone" : "desktop";
  }
}
