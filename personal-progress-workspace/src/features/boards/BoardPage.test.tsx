import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BoardData } from "./boardApi";
import { createItem, loadPrimaryBoard, logFocusSession, updateItem, updateItemPositions } from "./boardApi";
import { BoardPage } from "./BoardPage";

vi.mock("@dnd-kit/core", async () => {
  const actual = await vi.importActual<typeof import("@dnd-kit/core")>("@dnd-kit/core");

  return {
    ...actual,
    DndContext: ({
      children,
      onDragEnd,
    }: {
      children: ReactNode;
      onDragEnd: (event: { active: { id: string }; over: { id: string } }) => void;
    }) => (
      <>
        <button type="button" onClick={() => onDragEnd({ active: { id: "item-a" }, over: { id: "doing" } })}>
          Move item
        </button>
        {children}
      </>
    ),
    useDroppable: () => ({ setNodeRef: vi.fn(), isOver: false }),
    useDraggable: () => ({
      attributes: {},
      listeners: {},
      setNodeRef: vi.fn(),
      setActivatorNodeRef: vi.fn(),
      transform: null,
      isDragging: false,
    }),
  };
});

vi.mock("./boardApi", async () => {
  const actual = await vi.importActual<typeof import("./boardApi")>("./boardApi");

  return {
    ...actual,
    createItem: vi.fn(),
    loadPrimaryBoard: vi.fn(),
    logFocusSession: vi.fn(),
    updateItem: vi.fn(),
    updateItemPositions: vi.fn(),
  };
});

vi.mock("../auth/AuthProvider", () => ({
  useAuth: () => ({
    loading: false,
    session: null,
    user: { id: "user-1" },
  }),
}));

const boardData: BoardData = {
  boardId: "board-1",
  columns: [
    { id: "inbox", boardId: "board-1", name: "Inbox", statusKey: "Inbox", sortOrder: 0 },
    { id: "doing", boardId: "board-1", name: "Doing", statusKey: "Doing", sortOrder: 1 },
  ],
  items: [
    {
      id: "item-a",
      workspaceId: "workspace-1",
      boardId: "board-1",
      columnId: "inbox",
      goalId: null,
      title: "Draft plan",
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
    },
    {
      id: "item-b",
      workspaceId: "workspace-1",
      boardId: "board-1",
      columnId: "doing",
      goalId: null,
      title: "Existing work",
      description: null,
      type: "task",
      tags: [],
      priority: "medium",
      status: "Doing",
      scheduledDate: null,
      dueDate: null,
      estimateMinutes: null,
      focusMinutesTotal: 0,
      progressMode: "tasks",
      progressValue: null,
      sortOrder: 0,
    },
  ],
};

function renderBoardPage() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });

  const view = render(
    <QueryClientProvider client={queryClient}>
      <BoardPage workspace={{ id: "workspace-1", name: "Personal Workspace", owner_user_id: "user-1" }} />
    </QueryClientProvider>,
  );

  return { queryClient, ...view };
}

describe("BoardPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(loadPrimaryBoard).mockResolvedValue(structuredClone(boardData));
    vi.mocked(createItem).mockResolvedValue({ ...boardData.items[0], id: "created-item", title: "New launch note" });
    vi.mocked(logFocusSession).mockResolvedValue(undefined);
    vi.mocked(updateItem).mockResolvedValue({ ...boardData.items[0], title: "Updated plan" });
    vi.mocked(updateItemPositions).mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("persists drag moves with the batch position helper", async () => {
    const { queryClient } = renderBoardPage();
    const cancelQueries = vi.spyOn(queryClient, "cancelQueries");

    expect(await screen.findByRole("button", { name: "Open Draft plan" })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Move item" }));

    await waitFor(() => expect(updateItemPositions).toHaveBeenCalledTimes(1));
    expect(cancelQueries).toHaveBeenCalledWith({ queryKey: ["board", "workspace-1"] });
    expect(updateItemPositions).toHaveBeenCalledWith([
      expect.objectContaining({ id: "item-a", columnId: "doing", status: "Doing", sortOrder: 1 }),
    ]);
  });

  it("ignores drag moves while a previous move is pending", async () => {
    vi.mocked(updateItemPositions).mockImplementationOnce(() => new Promise(() => undefined));
    renderBoardPage();

    expect(await screen.findByRole("button", { name: "Open Draft plan" })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Move item" }));
    await userEvent.click(screen.getByRole("button", { name: "Move item" }));

    await waitFor(() => expect(updateItemPositions).toHaveBeenCalledTimes(1));
  });

  it("refetches the board after a failed drag move rollback", async () => {
    vi.mocked(updateItemPositions).mockRejectedValueOnce(new Error("Could not persist move"));
    renderBoardPage();

    expect(await screen.findByRole("button", { name: "Open Draft plan" })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Move item" }));

    await waitFor(() => expect(updateItemPositions).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(vi.mocked(loadPrimaryBoard).mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it("creates a new item from the board drawer with the first column defaults", async () => {
    renderBoardPage();

    await userEvent.click(await screen.findByRole("button", { name: "New item" }));
    await userEvent.type(screen.getByLabelText("Title"), "New launch note");
    await userEvent.click(screen.getByRole("button", { name: "Create item" }));

    await waitFor(() => expect(createItem).toHaveBeenCalledTimes(1));
    expect(createItem).toHaveBeenCalledWith(
      expect.objectContaining({
        workspaceId: "workspace-1",
        boardId: "board-1",
        columnId: "inbox",
        status: "Inbox",
        createdBy: "user-1",
        title: "New launch note",
        description: null,
        tags: [],
        priority: "medium",
      }),
    );
    await waitFor(() => expect(vi.mocked(loadPrimaryBoard).mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it("updates an opened item from the board drawer", async () => {
    renderBoardPage();

    await userEvent.click(await screen.findByRole("button", { name: "Open Draft plan" }));
    expect(screen.getByRole("heading", { name: "Edit item" })).toBeVisible();

    await userEvent.clear(screen.getByLabelText("Title"));
    await userEvent.type(screen.getByLabelText("Title"), "Updated plan");
    await userEvent.click(screen.getByRole("button", { name: "Save item" }));

    await waitFor(() => expect(updateItem).toHaveBeenCalledTimes(1));
    expect(updateItem).toHaveBeenCalledWith(
      "item-a",
      expect.objectContaining({
        workspaceId: "workspace-1",
        boardId: "board-1",
        columnId: "inbox",
        status: "Inbox",
        createdBy: "user-1",
        title: "Updated plan",
      }),
    );
  });

  it("logs focus from the today panel and refetches the board", async () => {
    renderBoardPage();

    await screen.findByRole("button", { name: "Log focus" });
    const now = new Date("2026-05-06T10:30:00.000Z").getTime();
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(now);

    try {
      await userEvent.click(screen.getByRole("button", { name: "Log focus" }));
    } finally {
      nowSpy.mockRestore();
    }

    await waitFor(() => expect(logFocusSession).toHaveBeenCalledTimes(1));
    expect(logFocusSession).toHaveBeenCalledWith({
      workspaceId: "workspace-1",
      itemId: "item-a",
      goalId: null,
      startedAt: "2026-05-06T10:05:00.000Z",
      endedAt: "2026-05-06T10:30:00.000Z",
      durationMinutes: 25,
    });
    await waitFor(() => expect(vi.mocked(loadPrimaryBoard).mock.calls.length).toBeGreaterThanOrEqual(2));
  });
});
