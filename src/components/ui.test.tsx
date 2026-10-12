import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ProgressBar, Toggle } from "./ui";

describe("Toggle", () => {
  it("reports its state to assistive technology", () => {
    render(<Toggle checked={false} onChange={vi.fn()} label="Auto accept" />);
    const toggle = screen.getByRole("switch", { name: "Auto accept" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });

  it("flips when clicked, and when activated from the keyboard", async () => {
    const onChange = vi.fn();
    render(<Toggle checked={false} onChange={onChange} label="Auto accept" />);
    const toggle = screen.getByRole("switch");

    // Reachable by keyboard: it is the only focusable control in the row.
    await userEvent.tab();
    expect(toggle).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith(true);

    await userEvent.click(toggle);
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it("pins the knob to the left edge of the track", () => {
    // Regression guard. The knob is absolutely positioned; without `left-0` it
    // falls back to its static position, and a button's default
    // `text-align: center` puts that in the middle of the track — the knob then
    // renders 22px too far right and hangs off the end. jsdom does not lay out,
    // so the class itself is what gets asserted.
    const { container } = render(
      <Toggle checked onChange={vi.fn()} label="Auto accept" />,
    );
    const knob = container.querySelector('[role="switch"] > span');
    expect(knob).toBeTruthy();
    expect(knob!.className).toContain("left-0");
  });

  it("shows the description alongside the label", () => {
    render(
      <Toggle
        checked
        onChange={vi.fn()}
        label="Desktop notifications"
        description="Tell me about incoming requests."
      />,
    );
    expect(screen.getByText("Tell me about incoming requests.")).toBeInTheDocument();
  });
});

describe("ProgressBar", () => {
  it("exposes the percentage", () => {
    render(<ProgressBar value={42} />);
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "42");
    expect(bar).toHaveAttribute("aria-valuemin", "0");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
  });

  it("clamps out-of-range values instead of overflowing", () => {
    const { container: over } = render(<ProgressBar value={180} />);
    const overFill = over.querySelector('[role="progressbar"] > div') as HTMLElement;
    expect(overFill.style.width).toBe("100%");

    const { container: under } = render(<ProgressBar value={-20} />);
    const underFill = under.querySelector('[role="progressbar"] > div') as HTMLElement;
    expect(underFill.style.width).toBe("0%");
  });

  it("omits a value while it is indeterminate", () => {
    render(<ProgressBar value={0} indeterminate />);
    expect(screen.getByRole("progressbar")).not.toHaveAttribute("aria-valuenow");
  });
});
