import type { GoalStatus } from "../../domain/types";

export function summarizeGoal(input: { progress: number; status: GoalStatus }) {
  const statusLabel = input.status.charAt(0).toUpperCase() + input.status.slice(1);

  return {
    progressLabel: `${Math.round(input.progress)}%`,
    statusLabel,
  };
}
