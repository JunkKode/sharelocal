import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TextComposer, TextPreview } from "./TextComposer";
import { MAX_TEXT_BYTES } from "../types";

describe("TextComposer", () => {
  it("sends what was typed", async () => {
    const onSend = vi.fn();
    const onClose = vi.fn();
    render(<TextComposer open onClose={onClose} onSend={onSend} destinationName="Windows PC" />);

    expect(screen.getByText(/straight to Windows PC/i)).toBeInTheDocument();
    await userEvent.type(screen.getByRole("textbox"), "https://example.com");
    await userEvent.click(screen.getByRole("button", { name: /send text/i }));

    expect(onSend).toHaveBeenCalledWith("https://example.com");
    expect(onClose).toHaveBeenCalled();
  });

  it("will not send nothing", async () => {
    const onSend = vi.fn();
    render(<TextComposer open onClose={vi.fn()} onSend={onSend} />);

    expect(screen.getByRole("button", { name: /send text/i })).toBeDisabled();
    await userEvent.type(screen.getByRole("textbox"), "   ");
    expect(screen.getByRole("button", { name: /send text/i })).toBeDisabled();
    expect(onSend).not.toHaveBeenCalled();
  });

  it("refuses a snippet that belongs in a file, and says why", async () => {
    const onSend = vi.fn();
    render(<TextComposer open onClose={vi.fn()} onSend={onSend} />);

    const area = screen.getByRole("textbox");
    // Typing 64KB would take forever; set the value directly.
    await userEvent.click(area);
    await userEvent.paste("x".repeat(MAX_TEXT_BYTES + 10));

    expect(screen.getByText(/too long, send it as a file/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /send text/i })).toBeDisabled();
  });

  it("can pull the clipboard in", async () => {
    const onPasteClipboard = vi.fn().mockResolvedValue("pasted value");
    render(
      <TextComposer open onClose={vi.fn()} onSend={vi.fn()} onPasteClipboard={onPasteClipboard} />,
    );
    await userEvent.click(screen.getByRole("button", { name: /paste clipboard/i }));
    expect(await screen.findByDisplayValue("pasted value")).toBeInTheDocument();
  });

  it("sends on the usual keyboard chord", async () => {
    const onSend = vi.fn();
    render(<TextComposer open onClose={vi.fn()} onSend={onSend} />);
    const area = screen.getByRole("textbox");
    await userEvent.type(area, "quick note");
    await userEvent.keyboard("{Meta>}{Enter}{/Meta}");
    expect(onSend).toHaveBeenCalledWith("quick note");
  });
});

describe("TextPreview", () => {
  it("shows the snippet and copies it", async () => {
    const onCopy = vi.fn();
    render(<TextPreview text="wifi: hunter2" onCopy={onCopy} />);

    expect(screen.getByText("wifi: hunter2")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /copy/i }));
    expect(onCopy).toHaveBeenCalledWith("wifi: hunter2");
    expect(await screen.findByRole("button", { name: /copied/i })).toBeInTheDocument();
  });
});
