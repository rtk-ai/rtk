import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { BoardColumn, WorkspaceItem } from "../../domain/types";
import { ItemForm } from "./ItemForm";
import type { ItemInput } from "./boardApi";

const defaults: Pick<ItemInput, "workspaceId" | "boardId" | "columnId" | "status" | "createdBy"> = {
  workspaceId: "workspace-1",
  boardId: "board-1",
  columnId: "planned",
  status: "Planned",
  createdBy: "user-1",
};

const columns: BoardColumn[] = [
  { id: "planned", boardId: "board-1", name: "Planned", statusKey: "Planned", sortOrder: 0 },
  { id: "doing", boardId: "board-1", name: "Doing", statusKey: "Doing", sortOrder: 1 },
  { id: "done", boardId: "board-1", name: "Done", statusKey: "Done", sortOrder: 2 },
];

describe("ItemForm", () => {
  it("submits normalized item input from user-entered fields", async () => {
    const onSubmit = vi.fn();
    render(<ItemForm columns={columns} defaults={defaults} onSubmit={onSubmit} />);

    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Plan launch" } });
    fireEvent.change(screen.getByLabelText("Description"), { target: { value: "Draft the rollout notes" } });
    await userEvent.selectOptions(screen.getByLabelText("Type"), "content");
    await userEvent.selectOptions(screen.getByLabelText("Priority"), "high");
    fireEvent.change(screen.getByLabelText("Tags"), { target: { value: "launch, writing,  " } });
    fireEvent.change(screen.getByLabelText("Scheduled date"), { target: { value: " 2026-05-08 " } });
    fireEvent.change(screen.getByLabelText("Due date"), { target: { value: "2026-05-10" } });
    fireEvent.change(screen.getByLabelText("Estimate time"), { target: { value: "1.5h" } });
    await userEvent.selectOptions(screen.getByLabelText("Progress status"), "Doing");
    await userEvent.click(screen.getByRole("button", { name: "Create item" }));

    expect(onSubmit).toHaveBeenCalledWith({
      ...defaults,
      columnId: "doing",
      title: "Plan launch",
      description: "Draft the rollout notes",
      type: "content",
      tags: ["launch", "writing"],
      priority: "high",
      scheduledDate: "2026-05-08",
      dueDate: "2026-05-10",
      estimateMinutes: 90,
      status: "Doing",
      progressMode: "manual",
      progressValue: null,
    });
  });

  it("maps estimate presets to minutes", async () => {
    const onSubmit = vi.fn();
    render(<ItemForm columns={columns} defaults={defaults} onSubmit={onSubmit} />);

    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Deep work block" } });
    await userEvent.click(screen.getByRole("button", { name: "2 hours" }));
    await userEvent.click(screen.getByRole("button", { name: "Create item" }));

    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ estimateMinutes: 120 }));
  });

  it("blocks submit for invalid estimate time input", async () => {
    const onSubmit = vi.fn();
    render(<ItemForm columns={columns} defaults={defaults} onSubmit={onSubmit} />);

    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Sketch notes" } });
    fireEvent.change(screen.getByLabelText("Estimate time"), { target: { value: "abc" } });
    await userEvent.click(screen.getByRole("button", { name: "Create item" }));

    expect(screen.getByText("Use minutes or hours, for example 30 mins or 1.5h.")).toBeVisible();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("prefills editable values for an existing item", async () => {
    const onSubmit = vi.fn();
    render(<ItemForm columns={columns} defaults={defaults} initialItem={item()} onSubmit={onSubmit} />);

    expect(screen.getByDisplayValue("Review weekly routine")).toBeVisible();
    expect(screen.getByDisplayValue("habit, routine")).toBeVisible();
    expect(screen.getByLabelText("Progress status")).toHaveValue("Doing");

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
        status: "Doing",
        columnId: "doing",
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
