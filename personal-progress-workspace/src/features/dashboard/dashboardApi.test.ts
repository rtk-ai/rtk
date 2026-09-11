import { beforeEach, describe, expect, it, vi } from "vitest";
import { loadPrimaryBoard } from "../boards/boardApi";
import { loadGoals } from "../goals/goalApi";
import { loadDashboard } from "./dashboardApi";

vi.mock("../boards/boardApi", () => ({
  loadPrimaryBoard: vi.fn(),
}));

vi.mock("../goals/goalApi", () => ({
  loadGoals: vi.fn(),
}));

describe("loadDashboard", () => {
  beforeEach(() => {
    vi.mocked(loadPrimaryBoard).mockReset();
    vi.mocked(loadGoals).mockReset();
  });

  it("loads board and goal data for one workspace", async () => {
    const board = { boardId: "board-1", columns: [], items: [] };
    const goals = { goals: [], milestones: [], linkedItems: [] };
    vi.mocked(loadPrimaryBoard).mockResolvedValue(board);
    vi.mocked(loadGoals).mockResolvedValue(goals);

    const dashboard = await loadDashboard("workspace-1");

    expect(loadPrimaryBoard).toHaveBeenCalledWith("workspace-1");
    expect(loadGoals).toHaveBeenCalledWith("workspace-1");
    expect(dashboard).toEqual({ board, goals });
  });
});
