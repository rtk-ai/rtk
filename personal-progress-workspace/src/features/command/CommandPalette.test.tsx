import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { CommandPalette } from "./CommandPalette";

describe("CommandPalette", () => {
  it("opens with Ctrl+K and runs selected actions", async () => {
    const onAction = vi.fn();
    render(<CommandPalette onAction={onAction} />);

    expect(screen.queryByPlaceholderText("Run a command...")).not.toBeInTheDocument();

    fireEvent.keyDown(window, { ctrlKey: true, key: "k" });
    expect(await screen.findByPlaceholderText("Run a command...")).toBeVisible();

    await userEvent.click(screen.getByText("Open goals"));

    expect(onAction).toHaveBeenCalledWith(expect.objectContaining({ id: "open-goals" }));
    expect(screen.queryByPlaceholderText("Run a command...")).not.toBeInTheDocument();
  });
});
