import { DndContext } from "@dnd-kit/core";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { WorkspaceItem } from "../../domain/types";
import { ItemCard } from "./ItemCard";

function item(overrides: Partial<WorkspaceItem> = {}): WorkspaceItem {
  return {
    id: "item-1",
    workspaceId: "workspace-1",
    boardId: "board-1",
    columnId: "doing",
    goalId: "goal-1",
    title: "Draft launch plan",
    description: null,
    type: "content",
    tags: [],
    priority: "high",
    status: "Doing",
    scheduledDate: null,
    dueDate: "2026-05-12",
    estimateMinutes: 90,
    focusMinutesTotal: 45,
    progressMode: "focus_time",
    progressValue: null,
    sortOrder: 0,
    ...overrides,
  };
}

describe("ItemCard", () => {
  it("renders item metadata and calculated progress", () => {
    render(
      <DndContext>
        <ItemCard item={item()} onOpen={vi.fn()} />
      </DndContext>,
    );

    expect(screen.getByRole("button", { name: "Open Draft launch plan" })).toBeVisible();
    expect(screen.getByText("content")).toBeVisible();
    expect(screen.getByLabelText("high priority")).toBeVisible();
    expect(screen.getByText("2026-05-12")).toBeVisible();
    expect(screen.getByText("90m")).toBeVisible();
    expect(screen.getByText("Goal")).toBeVisible();
    expect(screen.getByLabelText("50% progress")).toBeVisible();
  });

  it("opens from the open control without opening from the drag handle", async () => {
    const onOpen = vi.fn();

    render(
      <DndContext>
        <ItemCard item={item()} onOpen={onOpen} />
      </DndContext>,
    );

    await userEvent.click(screen.getByRole("button", { name: "Open Draft launch plan" }));
    expect(onOpen).toHaveBeenCalledOnce();
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ id: "item-1" }));

    onOpen.mockClear();
    await userEvent.click(screen.getByRole("button", { name: "Drag Draft launch plan" }));
    expect(onOpen).not.toHaveBeenCalled();
  });
});
