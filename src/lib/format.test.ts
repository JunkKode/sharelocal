import { describe, expect, it } from "vitest";
import {
  baseName,
  dayLabel,
  describeFiles,
  formatBytes,
  formatDuration,
  formatEta,
  formatPercent,
  formatSpeed,
  percent,
} from "./format";

describe("formatBytes", () => {
  it("matches the Rust core's formatting", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1024)).toBe("1.00 KB");
    expect(formatBytes(1536)).toBe("1.50 KB");
    expect(formatBytes(20 * 1024 ** 3)).toBe("20.0 GB");
    expect(formatBytes(150 * 1024 ** 2)).toBe("150 MB");
    expect(formatBytes(1024 ** 4)).toBe("1.00 TB");
  });

  it("copes with nonsense input", () => {
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("—");
  });
});

describe("formatSpeed", () => {
  it("uses MB/s for the range people care about", () => {
    expect(formatSpeed(86.7 * 1024 ** 2)).toBe("86.7 MB/s");
    expect(formatSpeed(220 * 1024 ** 2)).toBe("220 MB/s");
    expect(formatSpeed(12 * 1024 ** 2)).toBe("12.0 MB/s");
    expect(formatSpeed(1.5 * 1024 ** 2)).toBe("1.5 MB/s");
  });

  it("drops to smaller units on a slow link", () => {
    expect(formatSpeed(400 * 1024)).toBe("400 KB/s");
    expect(formatSpeed(500)).toBe("500 B/s");
  });

  it("shows a placeholder before any data has moved", () => {
    expect(formatSpeed(0)).toBe("—");
    expect(formatSpeed(Number.NaN)).toBe("—");
  });
});

describe("formatDuration / formatEta", () => {
  it("formats the ranges a transfer actually spans", () => {
    expect(formatDuration(12)).toBe("12s");
    expect(formatDuration(130)).toBe("2m 10s");
    expect(formatDuration(3900)).toBe("1h 05m");
    expect(formatEta(130)).toBe("ETA 2m 10s");
  });

  it("renders an unknown ETA rather than NaN", () => {
    expect(formatDuration(null)).toBe("—");
    expect(formatEta(null)).toBe("—");
    expect(formatDuration(-5)).toBe("—");
  });
});

describe("percentages", () => {
  it("clamps to 0-100", () => {
    expect(formatPercent(0, 100)).toBe("0%");
    expect(formatPercent(42, 100)).toBe("42%");
    expect(formatPercent(200, 100)).toBe("100%");
    expect(formatPercent(1, 0)).toBe("0%");
    expect(percent(50, 200)).toBe(25);
    expect(percent(5, 0)).toBe(0);
  });

  it("floors rather than rounds, so 99.9% never reads as 100%", () => {
    expect(formatPercent(999, 1000)).toBe("99%");
  });
});

describe("names", () => {
  it("takes the last path segment", () => {
    expect(baseName("Trip/day1/a.jpg")).toBe("a.jpg");
    expect(baseName("movie.mp4")).toBe("movie.mp4");
    expect(baseName("trailing/")).toBe("trailing");
  });

  it("summarises a multi-file transfer", () => {
    expect(describeFiles([])).toBe("No files");
    expect(describeFiles([{ rel_path: "a.txt" }])).toBe("a.txt");
    expect(describeFiles([{ rel_path: "Trip/a.jpg" }, { rel_path: "Trip/b.jpg" }])).toBe(
      "Trip + 1 more",
    );
  });
});

describe("dayLabel", () => {
  const now = new Date(2026, 2, 15, 12, 0, 0).getTime();

  it("labels recent days in words", () => {
    expect(dayLabel(new Date(2026, 2, 15, 9, 0, 0).getTime(), now)).toBe("Today");
    expect(dayLabel(new Date(2026, 2, 14, 23, 0, 0).getTime(), now)).toBe("Yesterday");
  });

  it("falls back to a date for anything older than a week", () => {
    const label = dayLabel(new Date(2025, 10, 2).getTime(), now);
    expect(label).not.toBe("Today");
    expect(label).toMatch(/2025/);
  });
});
