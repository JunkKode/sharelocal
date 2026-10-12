import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HomeView } from "./HomeView";
import { peer, session } from "../test/fixtures";
import type { NetworkStatus } from "../types";

const network: NetworkStatus = {
  online: true,
  interface: "en0",
  ipv4: "192.168.1.10",
  ipv6: null,
  link: "wifi",
  port: 47654,
  tls: false,
  mdns_ok: true,
  udp_ok: true,
  isolation_suspected: false,
};

const device = {
  id: "self",
  name: "MacBook Pro",
  os: "macos",
  kind: "laptop",
  version: 1,
  port: 47654,
  tls: false,
  pin_required: false,
};

function renderHome(overrides: Partial<Parameters<typeof HomeView>[0]> = {}) {
  const props = {
    device,
    network,
    peers: [peer()],
    activeSessions: [],
    dropActive: false,
    dropTargetId: null,
    busyPeerIds: new Set<string>(),
    onSelectPeer: vi.fn(),
    onPickFiles: vi.fn(),
    onPickFolder: vi.fn(),
    onSendText: vi.fn(),
    onRefreshPeers: vi.fn(),
    onOpenQr: vi.fn(),
    onOpenWifi: vi.fn(),
    onCancel: vi.fn(),
    onPause: vi.fn(),
    onOpenPath: vi.fn(),
    onRevealPath: vi.fn(),
    onCopyText: vi.fn(),
    ...overrides,
  };
  render(<HomeView {...props} />);
  return props;
}

describe("HomeView", () => {
  it("shows this device, its address and the nearby devices", () => {
    renderHome();
    expect(screen.getByText("MacBook Pro")).toBeInTheDocument();
    expect(screen.getByText("192.168.1.10")).toBeInTheDocument();
    expect(screen.getByText("Windows PC")).toBeInTheDocument();
    expect(screen.getByText("Ethernet")).toBeInTheDocument();
    expect(screen.getByText("Online")).toBeInTheDocument();
  });

  it("offers the whole workflow from one screen", () => {
    renderHome();
    expect(screen.getByRole("button", { name: /select files/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /select folder/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /send text/i })).toBeInTheDocument();
    expect(screen.getByText(/drop files here/i)).toBeInTheDocument();
  });

  it("sends when a device is chosen", async () => {
    const props = renderHome();
    await userEvent.click(screen.getByText("Windows PC"));
    expect(props.onSelectPeer).toHaveBeenCalledWith(expect.objectContaining({ id: "peer-1" }));
  });

  it("explains an empty device list instead of just spinning", async () => {
    const props = renderHome({ peers: [] });
    expect(screen.getByText(/no other devices yet/i)).toBeInTheDocument();
    expect(screen.getByText(/running ShareLocal too/i)).toBeInTheDocument();
    expect(screen.getByText(/firewall/i)).toBeInTheDocument();

    // The QR route out of the empty state is one click away.
    await userEvent.click(screen.getByRole("button", { name: /show a qr code/i }));
    expect(props.onOpenQr).toHaveBeenCalled();
  });

  it("explains client isolation rather than failing silently", () => {
    renderHome({ network: { ...network, isolation_suspected: true } });
    expect(screen.getByText(/client \(AP\) isolation/i)).toBeInTheDocument();
  });

  it("warns when a peer is on another subnet", () => {
    renderHome({ peers: [peer({ same_subnet: false })] });
    expect(screen.getByText(/different subnet/i)).toBeInTheDocument();
  });

  it("says when there is no network at all", () => {
    renderHome({ network: { ...network, online: false } });
    expect(screen.getByText(/no local network/i)).toBeInTheDocument();
  });

  it("marks the device a drag is hovering over", () => {
    renderHome({ dropActive: true, dropTargetId: "peer-1" });
    // The dropzone must not also claim the drop when a device is targeted.
    expect(screen.getByText(/drop files here/i)).toBeInTheDocument();
    expect(screen.queryByText(/release to send/i)).not.toBeInTheDocument();
  });

  it("prompts to release when dragging over the empty area", () => {
    renderHome({ dropActive: true, dropTargetId: null });
    expect(screen.getByText(/release to send/i)).toBeInTheDocument();
  });

  it("lists a transfer in progress", () => {
    renderHome({ activeSessions: [session()] });
    expect(screen.getByText("In progress")).toBeInTheDocument();
    expect(screen.getByText("movie.mp4")).toBeInTheDocument();
  });

  it("does not offer an offline device as a destination", () => {
    renderHome({ peers: [peer({ status: "offline" })] });
    const button = screen.getByText("Windows PC").closest("button");
    expect(button).toBeDisabled();
  });
});
