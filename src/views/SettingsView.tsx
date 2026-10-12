import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { ConflictPolicy, NetworkInterface, NetworkStatus, Settings } from "../types";
import { Button, Card, Field, inputClass, SectionTitle, Toggle, cx } from "../components/ui";

const CONFLICT_OPTIONS: { value: ConflictPolicy; label: string }[] = [
  { value: "keepboth", label: "Keep both" },
  { value: "replace", label: "Replace" },
  { value: "cancel", label: "Skip the file" },
];

export function SettingsView({
  settings,
  network,
  onSave,
  onSetDownloadDir,
  onOpenDownloadDir,
  listInterfaces,
}: {
  settings: Settings;
  network: NetworkStatus | null;
  onSave: (next: Settings) => void;
  onSetDownloadDir: (path: string) => Promise<string | null>;
  onOpenDownloadDir: () => void;
  listInterfaces: () => Promise<NetworkInterface[]>;
}) {
  const [draft, setDraft] = useState<Settings>(settings);
  const [interfaces, setInterfaces] = useState<NetworkInterface[]>([]);
  const [pinError, setPinError] = useState<string | null>(null);

  // Adopt external changes (another window, the tray) without clobbering an
  // edit in progress on a different field.
  useEffect(() => setDraft(settings), [settings]);

  useEffect(() => {
    listInterfaces().then(setInterfaces).catch(() => setInterfaces([]));
  }, [listInterfaces]);

  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  const patch = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));

  const chooseFolder = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Choose download folder" });
    if (typeof picked === "string") {
      const applied = await onSetDownloadDir(picked);
      if (applied) patch("download_dir", applied);
    }
  };

  const save = () => {
    const pin = draft.pin?.trim() ?? "";
    if (pin && !/^\d{4,10}$/.test(pin)) {
      setPinError("A PIN must be 4 to 10 digits.");
      return;
    }
    setPinError(null);
    onSave({ ...draft, pin: pin || null });
  };

  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-6 px-4 pb-24 sm:px-6">
      <section>
        <SectionTitle>General</SectionTitle>
        <Card className="px-4 divide-y divide-ink-100 dark:divide-ink-800">
          <Field label="Device name" htmlFor="device-name" hint="How this computer appears to others.">
            <input
              id="device-name"
              className={inputClass}
              value={draft.device_name}
              maxLength={64}
              onChange={(e) => patch("device_name", e.target.value)}
            />
          </Field>

          <Field
            label="Download folder"
            hint="Where received files are saved."
          >
            <div className="flex gap-2">
              <input className={cx(inputClass, "flex-1")} value={draft.download_dir} readOnly />
              <Button onClick={chooseFolder}>Change</Button>
              <Button icon="folder" onClick={onOpenDownloadDir} aria-label="Open download folder" />
            </div>
          </Field>

          <Field label="Appearance">
            <div className="grid grid-cols-3 gap-2">
              {(["system", "light", "dark"] as const).map((theme) => (
                <button
                  key={theme}
                  type="button"
                  onClick={() => patch("theme", theme)}
                  aria-pressed={draft.theme === theme}
                  className={cx(
                    "h-11 rounded-xl text-sm font-medium capitalize ring-1 transition",
                    draft.theme === theme
                      ? "bg-brand-50 text-brand-700 ring-brand-500 dark:bg-brand-500/10 dark:text-brand-300 dark:ring-brand-400"
                      : "bg-white text-ink-700 ring-ink-200 hover:ring-ink-300 dark:bg-ink-800 dark:text-ink-200 dark:ring-ink-700",
                  )}
                >
                  {theme}
                </button>
              ))}
            </div>
          </Field>

          <Toggle
            id="autostart"
            label="Start with the computer"
            description="Launch ShareLocal at login so it is ready to receive."
            checked={draft.autostart}
            onChange={(v) => patch("autostart", v)}
          />
          <Toggle
            id="start-minimized"
            label="Start minimised"
            description="Open into the tray instead of showing the window."
            checked={draft.start_minimized}
            onChange={(v) => patch("start_minimized", v)}
          />
          <Toggle
            id="notifications"
            label="Desktop notifications"
            description="Tell me about incoming requests and finished transfers."
            checked={draft.notifications}
            onChange={(v) => patch("notifications", v)}
          />
        </Card>
      </section>

      <section>
        <SectionTitle>Transfers</SectionTitle>
        <Card className="px-4 divide-y divide-ink-100 dark:divide-ink-800">
          <Toggle
            id="auto-accept"
            label="Accept transfers automatically"
            description="Off is safer: you are asked before anything is written to disk."
            checked={draft.auto_accept}
            onChange={(v) => patch("auto_accept", v)}
          />

          <Field
            label="Transfer PIN"
            htmlFor="pin"
            hint={
              pinError ? (
                <span className="text-rose-600 dark:text-rose-400">{pinError}</span>
              ) : (
                "Optional. Other devices must enter this before they can send you anything."
              )
            }
          >
            <input
              id="pin"
              className={inputClass}
              inputMode="numeric"
              autoComplete="off"
              placeholder="No PIN"
              value={draft.pin ?? ""}
              maxLength={10}
              onChange={(e) => patch("pin", e.target.value.replace(/\D/g, "") || null)}
            />
          </Field>

          <Field label="If a file already exists" htmlFor="conflict">
            <select
              id="conflict"
              className={inputClass}
              value={draft.conflict_policy}
              onChange={(e) => patch("conflict_policy", e.target.value as ConflictPolicy)}
            >
              {CONFLICT_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </Field>

          <Field
            label={`Simultaneous transfers: ${draft.max_concurrent}`}
            htmlFor="max-concurrent"
            hint="More at once is not always faster — one at a time usually wins on Wi-Fi."
          >
            <input
              id="max-concurrent"
              type="range"
              min={1}
              max={8}
              value={draft.max_concurrent}
              onChange={(e) => patch("max_concurrent", Number(e.target.value))}
              className="h-11 w-full accent-brand-600"
            />
          </Field>
        </Card>
      </section>

      <section>
        <SectionTitle>Network</SectionTitle>
        <Card className="px-4 divide-y divide-ink-100 dark:divide-ink-800">
          <Field label="Status">
            <dl className="grid grid-cols-2 gap-y-1.5 rounded-xl bg-ink-50 p-3 text-sm dark:bg-ink-800/60">
              <dt className="text-ink-500 dark:text-ink-400">Local IP</dt>
              <dd className="text-right font-medium tabular-nums">{network?.ipv4 ?? "—"}</dd>
              <dt className="text-ink-500 dark:text-ink-400">IPv6</dt>
              <dd className="truncate text-right font-medium tabular-nums">{network?.ipv6 ?? "—"}</dd>
              <dt className="text-ink-500 dark:text-ink-400">Connection</dt>
              <dd className="text-right font-medium capitalize">{network?.link ?? "unknown"}</dd>
              <dt className="text-ink-500 dark:text-ink-400">Discovery</dt>
              <dd className="text-right font-medium">
                {network?.mdns_ok ? "mDNS" : null}
                {network?.mdns_ok && network?.udp_ok ? " + " : null}
                {network?.udp_ok ? "LAN broadcast" : null}
                {!network?.mdns_ok && !network?.udp_ok ? "unavailable" : null}
              </dd>
            </dl>
          </Field>

          <Field
            label="Network interface"
            htmlFor="interface"
            hint="Automatic picks Ethernet over Wi-Fi when both are available."
          >
            <select
              id="interface"
              className={inputClass}
              value={draft.preferred_interface ?? ""}
              onChange={(e) => patch("preferred_interface", e.target.value || null)}
            >
              <option value="">Automatic</option>
              {interfaces.map((i) => (
                <option key={`${i.name}-${i.ip}`} value={i.name}>
                  {i.name} — {i.ip}
                </option>
              ))}
            </select>
          </Field>

          <Field
            label="Transfer port"
            htmlFor="port"
            hint="Changing this restarts the local server. Peers discover the new port automatically."
          >
            <input
              id="port"
              type="number"
              min={1024}
              max={65535}
              className={inputClass}
              value={draft.port}
              onChange={(e) => patch("port", Number(e.target.value) || draft.port)}
            />
          </Field>

          <Toggle
            id="mdns"
            label="Discover with mDNS / Bonjour"
            description="The standard way devices find each other on a LAN."
            checked={draft.discovery_mdns}
            onChange={(v) => patch("discovery_mdns", v)}
          />
          <Toggle
            id="udp"
            label="Local network fallback"
            description="A small UDP broadcast for networks where mDNS is filtered."
            checked={draft.discovery_udp}
            onChange={(v) => patch("discovery_udp", v)}
          />
          <Toggle
            id="tls"
            label="Encrypt transfers (HTTPS)"
            description="Uses a certificate this device generates itself. Browsers will warn about it, because there is no authority on a LAN to vouch for it."
            checked={draft.tls}
            onChange={(v) => patch("tls", v)}
          />
        </Card>
      </section>

      <section>
        <SectionTitle>Privacy</SectionTitle>
        <Card className="p-4">
          <ul className="space-y-1.5 text-sm text-ink-600 dark:text-ink-300">
            <li>Files go straight from device to device. Nothing is uploaded anywhere.</li>
            <li>No account, no sign-in, no external database.</li>
            <li>No analytics and no tracking of any kind.</li>
            <li>History records names and sizes only — never file contents.</li>
            <li>The file server refuses any connection from outside your local network.</li>
          </ul>
        </Card>
      </section>

      {dirty ? (
        <div className="fixed inset-x-0 bottom-0 z-40 border-t border-ink-200 bg-white/90 p-3 backdrop-blur dark:border-ink-800 dark:bg-ink-950/90">
          <div className="mx-auto flex max-w-2xl items-center justify-between gap-3">
            <p className="text-sm text-ink-500 dark:text-ink-400">You have unsaved changes.</p>
            <div className="flex gap-2">
              <Button onClick={() => setDraft(settings)}>Discard</Button>
              <Button variant="primary" icon="check" onClick={save}>
                Save changes
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
