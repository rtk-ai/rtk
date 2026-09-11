import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { GoalBundle } from "./goalApi";
import { createGoal, loadGoals, updateGoal } from "./goalApi";
import { GoalsPage } from "./GoalsPage";

vi.mock("./goalApi", async () => {
  const actual = await vi.importActual<typeof import("./goalApi")>("./goalApi");

  return {
    ...actual,
    createGoal: vi.fn(),
    loadGoals: vi.fn(),
    updateGoal: vi.fn(),
  };
});

describe("GoalsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(loadGoals).mockResolvedValue(goalBundle());
    vi.mocked(createGoal).mockResolvedValue(goalBundle().goals[0]);
    vi.mocked(updateGoal).mockResolvedValue(goalBundle().goals[0]);
  });

  it("renders goal progress summaries", async () => {
    renderWithQueryClient(
      <GoalsPage workspace={{ id: "workspace-1", name: "Personal Workspace", owner_user_id: "user-1" }} />,
    );

    expect(await screen.findByRole("heading", { name: "Launch channel" })).toBeVisible();
    expect(screen.getByText("Active")).toBeVisible();
    expect(screen.getByText("72%")).toBeVisible();
    expect(screen.getByText("First draft")).toBeVisible();
    expect(screen.getByText("Write episode outline")).toBeVisible();
  });

  it("creates a goal from the Goals tab", async () => {
    renderWithQueryClient(
      <GoalsPage workspace={{ id: "workspace-1", name: "Personal Workspace", owner_user_id: "user-1" }} />,
    );

    expect(await screen.findByRole("heading", { name: "Launch channel" })).toBeVisible();

    await userEvent.type(screen.getByLabelText("Goal title"), "Build portfolio");
    await userEvent.type(screen.getByLabelText("Goal description"), "Ship the first version");
    await userEvent.type(screen.getByLabelText("Target date"), "2026-06-20");
    await userEvent.selectOptions(screen.getByLabelText("Goal status"), "active");
    await userEvent.click(screen.getByRole("button", { name: "Create goal" }));

    await waitFor(() =>
      expect(createGoal).toHaveBeenCalledWith({
        workspaceId: "workspace-1",
        title: "Build portfolio",
        description: "Ship the first version",
        status: "active",
        targetDate: "2026-06-20",
      }),
    );
  });

  it("edits an existing goal inline", async () => {
    renderWithQueryClient(
      <GoalsPage workspace={{ id: "workspace-1", name: "Personal Workspace", owner_user_id: "user-1" }} />,
    );

    expect(await screen.findByRole("heading", { name: "Launch channel" })).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Edit goal Launch channel" }));
    await userEvent.clear(screen.getByLabelText("Edit title"));
    await userEvent.type(screen.getByLabelText("Edit title"), "Launch portfolio");
    await userEvent.clear(screen.getByLabelText("Edit description"));
    await userEvent.clear(screen.getByLabelText("Edit target date"));
    await userEvent.selectOptions(screen.getByLabelText("Edit status"), "paused");
    await userEvent.click(screen.getByRole("button", { name: "Save goal" }));

    await waitFor(() =>
      expect(updateGoal).toHaveBeenCalledWith("goal-1", {
        workspaceId: "workspace-1",
        title: "Launch portfolio",
        description: null,
        status: "paused",
        targetDate: null,
      }),
    );
  });
});

function renderWithQueryClient(children: ReactNode) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  });

  return render(<QueryClientProvider client={queryClient}>{children}</QueryClientProvider>);
}

function goalBundle(): GoalBundle {
  return {
    goals: [
      {
        id: "goal-1",
        workspaceId: "workspace-1",
        title: "Launch channel",
        description: "Publish consistently",
        status: "active",
        targetDate: "2026-06-01",
        progressMode: "manual",
        manualProgressValue: 72,
      },
    ],
    milestones: [
      {
        id: "milestone-1",
        goalId: "goal-1",
        title: "First draft",
        status: "done",
        dueDate: "2026-05-10",
        sortOrder: 0,
      },
    ],
    linkedItems: [
      {
        id: "item-1",
        workspaceId: "workspace-1",
        boardId: "board-1",
        columnId: "doing",
        goalId: "goal-1",
        title: "Write episode outline",
        description: null,
        type: "content",
        tags: [],
        priority: "high",
        status: "Doing",
        scheduledDate: null,
        dueDate: null,
        estimateMinutes: 90,
        focusMinutesTotal: 45,
        progressMode: "focus_time",
        progressValue: null,
        sortOrder: 0,
      },
    ],
  };
}
