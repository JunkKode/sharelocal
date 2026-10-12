import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { OfferPrompt } from "./OfferPrompt";
import { session } from "../test/fixtures";

const incoming = session({
  role: "receiving",
  status: "pending",
  files: [
    { rel_path: "Trip/a.jpg", size: 1024 },
    { rel_path: "Trip/b.jpg", size: 2048 },
  ],
  total_bytes: 3072,
});

describe("OfferPrompt", () => {
  it("renders nothing when there is no offer", () => {
    const { container } = render(
      <OfferPrompt session={null} onAccept={vi.fn()} onReject={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows who is sending and exactly what", () => {
    render(<OfferPrompt session={incoming} onAccept={vi.fn()} onReject={vi.fn()} />);
    expect(screen.getByText(/Windows PC wants to send you files/)).toBeInTheDocument();
    expect(screen.getByText(/2 files · 3.00 KB/)).toBeInTheDocument();
    expect(screen.getByText("Trip/a.jpg")).toBeInTheDocument();
    expect(screen.getByText("Trip/b.jpg")).toBeInTheDocument();
  });

  it("accepts with the default keep-both policy", async () => {
    const onAccept = vi.fn();
    render(<OfferPrompt session={incoming} onAccept={onAccept} onReject={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: /^accept$/i }));
    expect(onAccept).toHaveBeenCalledWith("session-1", "keepboth");
  });

  it("lets the user choose replace when a file would be overwritten", async () => {
    const onAccept = vi.fn();
    const clashing = { ...incoming, conflicts: ["Trip/a.jpg"] };
    render(<OfferPrompt session={clashing} onAccept={onAccept} onReject={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: /replace/i }));
    await userEvent.click(screen.getByRole("button", { name: /^accept$/i }));
    expect(onAccept).toHaveBeenCalledWith("session-1", "replace");
  });

  it("declines on the button and on Escape", async () => {
    const onReject = vi.fn();
    render(<OfferPrompt session={incoming} onAccept={vi.fn()} onReject={onReject} />);

    await userEvent.click(screen.getByRole("button", { name: /decline/i }));
    expect(onReject).toHaveBeenCalledWith("session-1");

    await userEvent.keyboard("{Escape}");
    expect(onReject).toHaveBeenCalledTimes(2);
  });

  it("does not ask about conflicts when there are none", () => {
    // The receiver reports which files already exist. An empty list means
    // nothing would be overwritten, so the question is noise.
    render(<OfferPrompt session={incoming} onAccept={vi.fn()} onReject={vi.fn()} />);
    expect(screen.queryByText(/already in your download folder/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /replace/i })).toBeNull();
    expect(screen.queryByText("already here")).toBeNull();
  });

  it("asks about conflicts, and marks which files clash", async () => {
    const clashing = { ...incoming, conflicts: ["Trip/a.jpg"] };
    const onAccept = vi.fn();
    render(<OfferPrompt session={clashing} onAccept={onAccept} onReject={vi.fn()} />);

    expect(screen.getByText(/One file is already in your download folder/i)).toBeInTheDocument();
    expect(screen.getByText("already here")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /replace/i }));
    await userEvent.click(screen.getByRole("button", { name: /^accept$/i }));
    expect(onAccept).toHaveBeenCalledWith("session-1", "replace");
  });

  it("is a modal dialog for assistive technology", () => {
    render(<OfferPrompt session={incoming} onAccept={vi.fn()} onReject={vi.fn()} />);
    expect(screen.getByRole("dialog")).toHaveAttribute("aria-modal", "true");
  });
});
