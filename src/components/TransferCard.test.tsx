import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TransferCard } from "./TransferCard";
import { session } from "../test/fixtures";

describe("TransferCard", () => {
  it("shows every number the transfer screen promises", () => {
    render(<TransferCard session={session()} />);

    expect(screen.getByText("movie.mp4")).toBeInTheDocument();
    expect(screen.getByText("42%")).toBeInTheDocument();
    expect(screen.getByText("8.40 GB / 20.0 GB")).toBeInTheDocument();
    expect(screen.getByText("86.7 MB/s")).toBeInTheDocument();
    expect(screen.getByText("ETA 2m 10s")).toBeInTheDocument();
    expect(screen.getByText(/Sending to Windows PC/)).toBeInTheDocument();

    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "42");
  });

  it("can be paused and cancelled while it runs", async () => {
    const onPause = vi.fn();
    const onCancel = vi.fn();
    render(<TransferCard session={session()} onPause={onPause} onCancel={onCancel} />);

    await userEvent.click(screen.getByRole("button", { name: /pause transfer/i }));
    expect(onPause).toHaveBeenCalledWith("session-1", true);

    await userEvent.click(screen.getByRole("button", { name: /cancel transfer/i }));
    expect(onCancel).toHaveBeenCalledWith("session-1");
  });

  it("offers to resume once paused", async () => {
    const onPause = vi.fn();
    render(<TransferCard session={session({ status: "paused" })} onPause={onPause} />);
    await userEvent.click(screen.getByRole("button", { name: /resume transfer/i }));
    expect(onPause).toHaveBeenCalledWith("session-1", false);
  });

  it("shows where a finished file went", async () => {
    const onReveal = vi.fn();
    render(
      <TransferCard
        session={session({
          status: "completed",
          role: "receiving",
          saved_paths: ["/Users/me/Downloads/ShareLocal/movie.mp4"],
        })}
        onReveal={onReveal}
      />,
    );

    expect(screen.getByText("Complete")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /show in folder/i }));
    expect(onReveal).toHaveBeenCalledWith("/Users/me/Downloads/ShareLocal/movie.mp4");
  });

  it("explains a failure in words the user can act on", () => {
    render(
      <TransferCard
        session={session({
          status: "failed",
          error: { code: "not-enough-space", detail: "The other device is out of disk space" },
        })}
      />,
    );
    expect(screen.getByText("Failed")).toBeInTheDocument();
    expect(screen.getByText(/out of disk space/i)).toBeInTheDocument();
  });

  it("shows a received snippet with a copy button instead of file actions", async () => {
    const onCopy = vi.fn();
    const onOpen = vi.fn();
    render(
      <TransferCard
        session={session({
          status: "completed",
          role: "receiving",
          kind: "text",
          text: "https://example.com/from-my-phone",
          saved_paths: ["/tmp/shared-text-1.txt"],
        })}
        onCopy={onCopy}
        onOpen={onOpen}
      />,
    );

    // The heading labels it; the snippet itself appears once, in the preview.
    expect(screen.getByText("Shared text")).toBeInTheDocument();
    expect(screen.getByText("https://example.com/from-my-phone")).toBeInTheDocument();
    // A snippet is copied, not opened.
    expect(screen.queryByRole("button", { name: /^open$/i })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /copy/i }));
    expect(onCopy).toHaveBeenCalledWith("https://example.com/from-my-phone");
  });

  it("counts files through a multi-file transfer", () => {
    render(
      <TransferCard
        session={session({
          files: [
            { rel_path: "Trip/a.jpg", size: 10 },
            { rel_path: "Trip/b.jpg", size: 10 },
            { rel_path: "Trip/c.jpg", size: 10 },
          ],
          total_bytes: 30,
        })}
      />,
    );
    expect(screen.getByText("Trip + 2 more")).toBeInTheDocument();
    expect(screen.getByText(/file 1 of 3/)).toBeInTheDocument();
  });

  it("says a phone has to tap Download instead of looking stuck", async () => {
    const onCancel = vi.fn();
    render(
      <TransferCard
        session={session({
          direction: "pull",
          role: "sending",
          status: "accepted",
          peer: { ...session().peer, name: "iPhone · Safari" },
        })}
        onCancel={onCancel}
      />,
    );
    expect(screen.getByText(/Ready on iPhone · Safari/)).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /cancel transfer/i }));
    expect(onCancel).toHaveBeenCalledWith("session-1");
  });
});
