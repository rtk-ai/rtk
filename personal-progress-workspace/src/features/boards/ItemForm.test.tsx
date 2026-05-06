import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { WorkspaceItem } from "../../domain/types";
import { ItemForm } from "./ItemForm";
import type { ItemInput } from "./boardApi";

const defaults: Pick<ItemInput, "workspaceId" | "boardId" | "columnId" | "status" | "createdBy"> = {
  workspaceId: "workspace-1",
  boardId: "board-1",
  columnId: "inbox",
  status: "Inbox",
  createdBy: "user-1",
};

describe("ItemForm", () => {
  it("submits normalized item input from user-entered fields", async () => {
    const onSubmit = vi.fn();
    render(<ItemForm defaults={defaults} onSubmit={onSubmit} />);

    await userEvent.type(screen.getByLabelText("Title"), "Plan launch");
    await userEvent.type(screen.getByLabelText("Description"), "Draft the rollout notes");
    await userEvent.selectOptions(screen.getByLabelText("Type"), "content");
    await userEvent.selectOptions(screen.getByLabelText("Priority"), "high");
    await userEvent.type(screen.getByLabelText("Tags"), "launch, writing,  ");
    await userEvent.type(screen.getByLabelText("Scheduled date"), "2026-05-08");
    await userEvent.type(screen.getByLabelText("Due date"), "2026-05-10");
    await userEvent.type(screen.getByLabelText("Estimate minutes"), "75");
    await userEvent.selectOptions(screen.getByLabelText("Progress mode"), "focus_time");
    await userEvent.click(screen.getByRole("button", { name: "Create item" }));

    expect(onSubmit).toHaveBeenCalledWith({
      ...defaults,
      title: "Plan launch",
      description: "Draft the rollout notes",
      type: "content",
      tags: ["launch", "writing"],
      priority: "high",
      scheduledDate: "2026-05-08",
      dueDate: "2026-05-10",
      estimateMinutes: 75,
      progressMode: "focus_time",
      progressValue: null,
    });
  });

  it("prefills editable values for an existing item", async () => {
    const onSubmit = vi.fn();
    render(<ItemForm defaults={defaults} initialItem={item()} onSubmit={onSubmit} />);

    expect(screen.getByDisplayValue("Review weekly routine")).toBeVisible();
    expect(screen.getByDisplayValue("habit, routine")).toBeVisible();

    await userEvent.clear(screen.getByLabelText("Description"));
    await userEvent.click(screen.getByRole("button", { name: "Save item" }));

    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Review weekly routine",
        description: null,
        type: "habit",
        priority: "medium",
        tags: ["habit", "routine"],
        scheduledDate: "2026-05-09",
        dueDate: null,
        estimateMinutes: 30,
        progressMode: "streak",
        progressValue: 40,
      }),
    );
  });
});

function item(): WorkspaceItem {
  return {
    id: "item-1",
    workspaceId: "workspace-1",
    boardId: "board-1",
    columnId: "doing",
    goalId: null,
    title: "Review weekly routine",
    description: "Keep the habit honest",
    type: "habit",
    tags: ["habit", "routine"],
    priority: "medium",
    status: "Doing",
    scheduledDate: "2026-05-09",
    dueDate: null,
    estimateMinutes: 30,
    focusMinutesTotal: 0,
    progressMode: "streak",
    progressValue: 40,
    sortOrder: 0,
  };
}
