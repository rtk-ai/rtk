import { type BoardData, loadPrimaryBoard } from "../boards/boardApi";
import { type GoalBundle, loadGoals } from "../goals/goalApi";

export interface DashboardData {
  board: BoardData;
  goals: GoalBundle;
}

export async function loadDashboard(workspaceId: string): Promise<DashboardData> {
  const [board, goals] = await Promise.all([loadPrimaryBoard(workspaceId), loadGoals(workspaceId)]);

  return { board, goals };
}
