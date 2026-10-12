import { describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { UsbView, type UsbApi } from "./UsbView";
import type { UsbEntry, UsbProgress } from "../types";

const pixel = { id: "1234", name: "Google Pixel 9", serial: "abc", speed: "USB 3 (5 Gbps)" };

const ROOT: UsbEntry[] = [
  { handle: "10", name: "DCIM", size: 0, folder: true, modified_ms: null, kind: "folder" },
  { handle: "11", name: "Download", size: 0, folder: true, modified_ms: null, kind: "folder" },
  { handle: "12", name: "notes.txt", size: 20, folder: false, modified_ms: 1_700_000_000_000, kind: "file" },
];
ROOT.push({ handle: "13", name: ".thumbnails", size: 0, folder: true, modified_ms: null, kind: "folder" });
const DCIM: UsbEntry[] = [
  { handle: "20", name: "Camera", size: 0, folder: true, modified_ms: null, kind: "folder" },
];

function fakeApi(overrides: Partial<UsbApi> = {}) {
  let progressHandler: ((p: UsbProgress) => void) | null = null;
  let navHandler: ((d: "back" | "forward") => void) | null = null;
  const api: UsbApi = {
    devices: vi.fn().mockResolvedValue([pixel]),
    connect: vi.fn().mockResolvedValue({
      id: pixel.id,
      name: pixel.name,
      storages: [{ id: "65537", name: "Internal shared storage", total: 128e9, free: 40e9, writable: true }],
      can_upload: true,
      can_delete: true,
      thumbnails: false,
    }),
    disconnect: vi.fn().mockResolvedValue(undefined),
    storages: vi.fn().mockResolvedValue([]),
    list: vi.fn(async (_storage: string, parent: string | null) => (parent === "10" ? DCIM : parent ? [] : ROOT)),
    thumbnail: vi.fn().mockRejectedValue(new Error("none")),
    createFolder: vi.fn().mockResolvedValue("99"),
    remove: vi.fn().mockResolvedValue(undefined),
    download: vi.fn().mockResolvedValue("job-1"),
    upload: vi.fn().mockResolvedValue("job-2"),
    cancel: vi.fn().mockResolvedValue(undefined),
    onProgress: (handler) => {
      progressHandler = handler;
      return () => {
        progressHandler = null;
      };
    },
    onMouseNav: (handler) => {
      navHandler = handler;
      return () => {
        navHandler = null;
      };
    },
    pickFiles: vi.fn().mockResolvedValue(["/Users/me/a.jpg"]),
    pickFolder: vi.fn().mockResolvedValue([]),
    pickDestination: vi.fn().mockResolvedValue("/Users/me/Desktop"),
    reveal: vi.fn(),
    ...overrides,
  };
  return {
    api,
    emit: (p: UsbProgress) => progressHandler?.(p),
    mouseNav: (d: "back" | "forward") => navHandler?.(d),
  };
}

function renderUsb(api: UsbApi, props: Partial<Parameters<typeof UsbView>[0]> = {}) {
  const notify = vi.fn();
  const utils = render(
    <UsbView api={api} notify={notify} droppedPaths={null} dropActive={false} onDropHandled={vi.fn()} {...props} />,
  );
  return { notify, ...utils };
}

describe("UsbView", () => {
  it("explains how to plug a phone in when none is found", async () => {
    const { api } = fakeApi({ devices: vi.fn().mockResolvedValue([]) });
    renderUsb(api);
    expect(await screen.findByText("Plug in your Android phone")).toBeInTheDocument();
    expect(screen.getByText(/choose “File transfer”/)).toBeInTheDocument();
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("opens a plugged-in phone by itself and browses into folders", async () => {
    const { api } = fakeApi();
    renderUsb(api);
    expect(await screen.findByText("notes.txt")).toBeInTheDocument();
    expect(api.connect).toHaveBeenCalledWith("1234", false);
    expect(screen.getByText("Google Pixel 9")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "DCIM" }));
    expect(await screen.findByLabelText("Select Camera")).toBeInTheDocument();
    expect(api.list).toHaveBeenLastCalledWith("65537", "10", false);

    await userEvent.click(screen.getByLabelText("Show hidden files"));
    await waitFor(() => expect(api.list).toHaveBeenLastCalledWith("65537", "10", true));
  });

  it("copies the selection to the Mac", async () => {
    const { api } = fakeApi();
    renderUsb(api);
    await userEvent.click(await screen.findByLabelText("Select notes.txt"));
    await userEvent.click(screen.getByRole("button", { name: /copy to downloads/i }));
    expect(api.download).toHaveBeenCalledWith("65537", ["12"], null);

    await userEvent.click(screen.getByRole("button", { name: /copy to…/i }));
    await waitFor(() => expect(api.download).toHaveBeenLastCalledWith("65537", ["12"], "/Users/me/Desktop"));
  });

  it("offers to free a phone macOS is holding", async () => {
    const { api } = fakeApi({
      connect: vi.fn().mockRejectedValue({ code: "exclusive", message: "macOS Image Capture is using the phone." }),
    });
    renderUsb(api);
    expect(await screen.findByText("Another app has the phone")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /free the phone and connect/i }));
    expect(api.connect).toHaveBeenLastCalledWith("1234", true);
  });

  it("says to unlock a locked phone", async () => {
    const { api } = fakeApi({
      connect: vi.fn().mockRejectedValue({ code: "locked", message: "Unlock the phone." }),
    });
    renderUsb(api);
    expect(await screen.findByText("Unlock Google Pixel 9")).toBeInTheDocument();
  });

  it("copies files dropped from Finder into the open folder", async () => {
    const { api } = fakeApi();
    const onDropHandled = vi.fn();
    const { rerender, notify } = renderUsb(api);
    await screen.findByText("notes.txt");
    rerender(
      <UsbView
        api={api}
        notify={notify}
        droppedPaths={["/Users/me/trip"]}
        dropActive={false}
        onDropHandled={onDropHandled}
      />,
    );
    await waitFor(() => expect(api.upload).toHaveBeenCalledWith("65537", null, ["/Users/me/trip"]));
    expect(onDropHandled).toHaveBeenCalled();
  });

  it("shows progress and lets the copy be cancelled", async () => {
    const { api, emit } = fakeApi();
    const { notify } = renderUsb(api);
    await screen.findByText("notes.txt");
    const running: UsbProgress = {
      job: "job-1",
      direction: "download",
      current: "IMG_0001.jpg",
      files_done: 1,
      files_total: 4,
      bytes_done: 50,
      bytes_total: 100,
      speed_bps: 30 * 1024 * 1024,
      finished: false,
      error: null,
      saved: ["/Users/me/Downloads/DCIM"],
    };
    act(() => emit(running));
    expect(screen.getByText("Copying to this Mac")).toBeInTheDocument();
    expect(screen.getByText("50%")).toBeInTheDocument();
    expect(screen.getByText(/File 2 of 4 · IMG_0001.jpg/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /cancel copy/i }));
    expect(api.cancel).toHaveBeenCalled();

    act(() => emit({ ...running, files_done: 4, bytes_done: 100, finished: true }));
    expect(notify).toHaveBeenCalledWith("info", "Copied 4 files to your Mac.");
    await userEvent.click(screen.getByRole("button", { name: /show in finder/i }));
    expect(api.reveal).toHaveBeenCalledWith("/Users/me/Downloads/DCIM");
  });

  it("goes back and forward through folders like a browser", async () => {
    const { api, mouseNav } = fakeApi();
    renderUsb(api);
    await screen.findByText("notes.txt");
    const back = screen.getByRole("button", { name: /^back/i });
    const forward = screen.getByRole("button", { name: /^forward/i });
    expect(back).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: "DCIM" }));
    await screen.findByLabelText("Select Camera");

    // The toolbar button.
    await userEvent.click(back);
    await screen.findByText("notes.txt");
    await userEvent.click(forward);
    await screen.findByLabelText("Select Camera");

    // The mouse's back button, as forwarded natively on macOS.
    act(() => mouseNav("back"));
    await screen.findByText("notes.txt");
    act(() => mouseNav("forward"));
    await screen.findByLabelText("Select Camera");

    // The mouse's back button where the web view delivers it (button 3).
    act(() => {
      window.dispatchEvent(new MouseEvent("mouseup", { button: 3 }));
    });
    await screen.findByText("notes.txt");

    // ⌘] goes forward again, ⌘↑ goes up a level.
    await userEvent.keyboard("{Meta>}]{/Meta}");
    await screen.findByLabelText("Select Camera");
    await userEvent.keyboard("{Meta>}{ArrowUp}{/Meta}");
    await screen.findByText("notes.txt");
  });

  it("fades hidden files so they read as hidden", async () => {
    const { api } = fakeApi();
    renderUsb(api);
    const hiddenName = await screen.findByRole("button", { name: ".thumbnails" });
    expect(hiddenName.closest("span")).toHaveClass("opacity-45");
    expect(screen.getByRole("button", { name: "DCIM" }).closest("span")).not.toHaveClass("opacity-45");
  });
});
