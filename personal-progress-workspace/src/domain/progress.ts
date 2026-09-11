import type { GoalProgressInput, ItemProgressInput } from "./types";

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, Math.round(value)));
}

function ratioPercent(done: number, total: number): number {
  if (total <= 0) return 0;
  return clampPercent((done / total) * 100);
}

export function calculateItemProgress(input: ItemProgressInput): number {
  if (input.mode === "manual") {
    return clampPercent(input.progressValue ?? 0);
  }

  if (input.mode === "focus_time") {
    return ratioPercent(input.focusMinutesTotal ?? 0, input.estimateMinutes ?? 0);
  }

  if (input.mode === "streak") {
    return input.streakCompletedToday ? 100 : 0;
  }

  if (input.mode === "tasks" || input.mode === "milestones") {
    return input.status === "Done" ? 100 : 0;
  }

  return 0;
}

export function calculateGoalProgress(input: GoalProgressInput): number {
  if (input.mode === "manual") {
    return clampPercent(input.manualProgressValue ?? 0);
  }

  if (input.mode === "focus_time") {
    return ratioPercent(input.focusMinutesTotal, input.focusTargetMinutes);
  }

  if (input.mode === "milestones") {
    const done = input.milestones.filter((milestone) => milestone.status === "done").length;
    return ratioPercent(done, input.milestones.length);
  }

  if (input.mode === "tasks") {
    const done = input.linkedItems.filter((item) => item.status === "Done").length;
    return ratioPercent(done, input.linkedItems.length);
  }

  if (input.mode === "streak") {
    const done = input.linkedItems.filter((item) => item.status === "Done").length;
    return ratioPercent(done, input.linkedItems.length);
  }

  return 0;
}
