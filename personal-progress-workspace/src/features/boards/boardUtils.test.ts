import { describe, expect, it } from "vitest";
import type { BoardColumn, WorkspaceItem } from "../../domain/types";
import { groupItemsByColumn, reorderAfterMove } from "./boardUtils";

const columns: BoardColumn[] = [
  { id: "doing", boardId: "board-1", name: "Doing", statusKey: "Doing", sortOrder: 2 },
  { id: "inbox", boardId: "board-1", name: "Inbox", statusKey: "Inbox", sortOrder: 1 },
];

function item(overrides: Partial<WorkspaceItem>): WorkspaceItem {
  return {
    id: "item-1",
    workspaceId: "workspace-1",
    boardId: "board-1",
    columnId: "inbox",
    goalId: null,
    title: "Item",
    description: null,
    type: "task",
    tags: [],
    priority: "medium",
    status: "Inbox",
    scheduledDate: null,
    dueDate: null,
    estimateMinutes: null,
    focusMinutesTotal: 0,
    progressMode: "tasks",
    progressValue: null,
    sortOrder: 0,
    ...overrides,
  };
}

describe("groupItemsByColumn", () => {
  it("returns every column with items sorted by sort order", () => {
    const grouped = groupItemsByColumn(columns, [
      item({ id: "b", columnId: "inbox", title: "Second", sortOrder: 20 }),
      item({ id: "a", columnId: "inbox", title: "First", sortOrder: 10 }),
      item({ id: "c", columnId: "unknown", title: "Hidden", sortOrder: 0 }),
    ]);

    expect(grouped.map((column) => column.id)).toEqual(["inbox", "doing"]);
    expect(grouped[0].items.map((entry) => entry.id)).toEqual(["a", "b"]);
    expect(grouped[1].items).toEqual([]);
  });
});

describe("reorderAfterMove", () => {
  it("moves an item to the target column status and assigns sort order", () => {
    const moved = reorderAfterMove({
      items: [
        item({ id: "a", columnId: "inbox", status: "Inbox", sortOrder: 0 }),
        item({ id: "b", columnId: "doing", status: "Doing", sortOrder: 0 }),
      ],
      activeItemId: "a",
      targetColumn: columns[0],
      targetIndex: 1,
    });

    expect(moved.find((entry) => entry.id === "a")).toMatchObject({
      columnId: "doing",
      status: "Doing",
      sortOrder: 1,
    });
    expect(moved.find((entry) => entry.id === "b")).toMatchObject({ sortOrder: 0 });
  });

  it("inserts into a populated target column with sequential sort orders", () => {
    const moved = reorderAfterMove({
      items: [
        item({ id: "a", columnId: "inbox", status: "Inbox", sortOrder: 0 }),
        item({ id: "b", columnId: "doing", status: "Doing", sortOrder: 0 }),
        item({ id: "c", columnId: "doing", status: "Doing", sortOrder: 1 }),
      ],
      activeItemId: "a",
      targetColumn: columns[0],
      targetIndex: 1,
    });

    const targetItems = moved
      .filter((entry) => entry.columnId === "doing")
      .sort((left, right) => left.sortOrder - right.sortOrder);

    expect(targetItems.map((entry) => [entry.id, entry.status, entry.sortOrder])).toEqual([
      ["b", "Doing", 0],
      ["a", "Doing", 1],
      ["c", "Doing", 2],
    ]);
    expect(new Set(targetItems.map((entry) => entry.sortOrder)).size).toBe(targetItems.length);
  });

  it("clamps same-column reorder indexes and renumbers the target column", () => {
    const moved = reorderAfterMove({
      items: [
        item({ id: "a", columnId: "inbox", sortOrder: 0 }),
        item({ id: "b", columnId: "inbox", sortOrder: 1 }),
        item({ id: "c", columnId: "inbox", sortOrder: 2 }),
      ],
      activeItemId: "a",
      targetColumn: columns[1],
      targetIndex: 99,
    });

    expect(
      moved
        .filter((entry) => entry.columnId === "inbox")
        .sort((left, right) => left.sortOrder - right.sortOrder)
        .map((entry) => [entry.id, entry.sortOrder]),
    ).toEqual([
      ["b", 0],
      ["c", 1],
      ["a", 2],
    ]);
  });
});
