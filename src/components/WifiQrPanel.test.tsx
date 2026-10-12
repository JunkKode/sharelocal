import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WifiQrPanel } from "./WifiQrPanel";

type Props = Parameters<typeof WifiQrPanel>[0];

function renderPanel(overrides: Partial<Props> = {}) {
  const props: Props = {
    open: true,
    onClose: vi.fn(),
    info: { ssid: "Home Net", security: "WPA" },
    detecting: false,
    passwordFor: () => undefined,
    loadAccess: vi.fn().mockResolvedValue({ remembered: false, biometrics: true, user_name: "mac" }),
    loadPassword: vi.fn().mockResolvedValue("hunter2"),
    onRemember: vi.fn(),
    onDownload: vi.fn(),
    onCopyImage: vi.fn(),
    onCopyText: vi.fn(),
    ...overrides,
  };
  const utils = render(<WifiQrPanel {...props} />);
  return { props, ...utils };
}

describe("WifiQrPanel", () => {
  it("opens with the network looked up ahead of time", () => {
    renderPanel();
    expect(screen.getByLabelText("Network name")).toHaveValue("Home Net");
    expect(screen.getByRole("button", { name: /copy text/i })).toBeDisabled();
  });

  it("tells the user which name to type the first time", async () => {
    renderPanel();
    expect(await screen.findByText("mac")).toBeInTheDocument();
    expect(screen.getByText(/the first time, macOS asks/i)).toBeInTheDocument();
  });

  it("is ready at once when the password was unlocked earlier this session", async () => {
    const { props } = renderPanel({ passwordFor: (ssid) => (ssid === "Home Net" ? "hunter2" : undefined) });
    expect(screen.getByLabelText("Password")).toHaveValue("hunter2");
    await waitFor(() => expect(screen.getByRole("button", { name: /copy text/i })).toBeEnabled());
    expect(props.loadPassword).not.toHaveBeenCalled();
  });

  it("offers Touch ID for a remembered password, and keeps what it unlocks", async () => {
    const { props } = renderPanel({
      loadAccess: vi.fn().mockResolvedValue({ remembered: true, biometrics: true, user_name: "mac" }),
    });
    await userEvent.click(await screen.findByRole("button", { name: /unlock with touch id/i }));
    expect(props.loadPassword).toHaveBeenCalledWith("Home Net");
    await waitFor(() => expect(screen.getByLabelText("Password")).toHaveValue("hunter2"));
    expect(props.onRemember).toHaveBeenCalledWith("Home Net", "hunter2");

    await userEvent.click(screen.getByRole("button", { name: /copy text/i }));
    expect(props.onCopyText).toHaveBeenCalledWith("Wi-Fi: Home Net\nPassword: hunter2", "Wi-Fi details");
  });

  it("stays quiet when the system sheet is dismissed, but explains real failures", async () => {
    const loadPassword = vi.fn().mockRejectedValueOnce("cancelled").mockRejectedValueOnce("No saved password.");
    renderPanel({ loadPassword });
    await userEvent.click(await screen.findByRole("button", { name: /get password/i }));
    await waitFor(() => expect(loadPassword).toHaveBeenCalledTimes(1));
    expect(screen.queryByText("cancelled")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /get password/i }));
    expect(await screen.findByText("No saved password.")).toBeInTheDocument();
  });

  it("remembers a typed password once it is used", async () => {
    const { props } = renderPanel();
    await userEvent.type(screen.getByLabelText("Password"), "typed-pw");
    await userEvent.click(screen.getByRole("button", { name: "Copy password" }));
    expect(props.onRemember).toHaveBeenCalledWith("Home Net", "typed-pw");
    expect(props.onCopyText).toHaveBeenCalledWith("typed-pw", "Password");
  });

  it("fills in the network when the background lookup finishes after opening", () => {
    const { props, rerender } = renderPanel({ info: null, detecting: true });
    expect(screen.getByText(/finding your network/i)).toBeInTheDocument();
    rerender(<WifiQrPanel {...props} info={{ ssid: "Late Net", security: "WPA" }} detecting={false} />);
    expect(screen.getByLabelText("Network name")).toHaveValue("Late Net");
  });

  it("needs no password for an open network", () => {
    renderPanel({ info: { ssid: "Cafe", security: "nopass" } });
    expect(screen.queryByLabelText("Password")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /copy text/i })).toBeEnabled();
  });
});
