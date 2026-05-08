import { todayIsoDate } from "../../domain/dates";
import { calculateGoalProgress } from "../../domain/progress";
import type { Goal, WorkspaceItem } from "../../domain/types";
import type { BoardData } from "../boards/boardApi";
import type { GoalBundle } from "../goals/goalApi";
import { summarizeGoal } from "../goals/goalUtils";

const priorityWeight: Record<WorkspaceItem["priority"], number> = {
  urgent: 4,
  high: 3,
  medium: 2,
  low: 1,
};

export interface DashboardGoalHighlight {
  id: string;
  title: string;
  status: Goal["status"];
  statusLabel: string;
  targetDate: string | null;
  progress: number;
  progressLabel: string;
  openItems: number;
  overdueItems: number;
}

export interface DashboardSummary {
  stats: {
    totalItems: number;
    openItems: number;
    doneItems: number;
    doingItems: number;
    todayItems: number;
    overdueItems: number;
    urgentItems: number;
    focusMinutes: number;
    focusHoursLabel: string;
  };
  today: {
    scheduled: WorkspaceItem[];
    overdue: WorkspaceItem[];
  };
  focus: {
    totalMinutes: number;
    estimatedMinutes: number;
    completionPercent: number;
  };
  priorityItems: WorkspaceItem[];
  goalHighlights: DashboardGoalHighlight[];
}

interface DashboardSummaryInput {
  board: BoardData;
  goals: GoalBundle;
  today?: string;
}

function isOpen(item: WorkspaceItem) {
  return item.status !== "Done";
}

function isOverdue(item: WorkspaceItem, today: string) {
  return isOpen(item) && Boolean(item.dueDate && item.dueDate < today);
}

function formatHours(minutes: number) {
  const hours = minutes / 60;
  return `${Number.isInteger(hours) ? hours : hours.toFixed(1)}h`;
}

function sortByAttention(left: WorkspaceItem, right: WorkspaceItem) {
  const dueLeft = left.dueDate ?? left.scheduledDate ?? "9999-12-31";
  const dueRight = right.dueDate ?? right.scheduledDate ?? "9999-12-31";

  if (dueLeft !== dueRight) return dueLeft.localeCompare(dueRight);
  if (priorityWeight[left.priority] !== priorityWeight[right.priority]) {
    return priorityWeight[right.priority] - priorityWeight[left.priority];
  }

  return left.title.localeCompare(right.title);
}

function buildGoalHighlight(goal: Goal, goals: GoalBundle, today: string): DashboardGoalHighlight {
  const milestones = goals.milestones.filter((milestone) => milestone.goalId === goal.id);
  const linkedItems = goals.linkedItems.filter((item) => item.goalId === goal.id);
  const progress = calculateGoalProgress({
    mode: goal.progressMode,
    milestones,
    linkedItems,
    focusMinutesTotal: linkedItems.reduce((sum, item) => sum + item.focusMinutesTotal, 0),
    focusTargetMinutes: linkedItems.reduce((sum, item) => sum + (item.estimateMinutes ?? 0), 0),
    manualProgressValue: goal.manualProgressValue,
  });
  const summary = summarizeGoal({ progress, status: goal.status });

  return {
    id: goal.id,
    title: goal.title,
    status: goal.status,
    statusLabel: summary.statusLabel,
    targetDate: goal.targetDate,
    progress,
    progressLabel: summary.progressLabel,
    openItems: linkedItems.filter(isOpen).length,
    overdueItems: linkedItems.filter((item) => isOverdue(item, today)).length,
  };
}

export function buildDashboardSummary({ board, goals, today = todayIsoDate() }: DashboardSummaryInput): DashboardSummary {
  const openItems = board.items.filter(isOpen);
  const doneItems = board.items.filter((item) => item.status === "Done");
  const scheduledToday = openItems.filter((item) => item.scheduledDate === today);
  const overdue = openItems.filter((item) => isOverdue(item, today)).sort(sortByAttention);
  const focusMinutes = board.items.reduce((sum, item) => sum + item.focusMinutesTotal, 0);
  const estimatedMinutes = board.items.reduce((sum, item) => sum + (item.estimateMinutes ?? 0), 0);
  const focusCompletion =
    estimatedMinutes > 0 ? Math.min(100, Math.round((focusMinutes / estimatedMinutes) * 100)) : 0;

  return {
    stats: {
      totalItems: board.items.length,
      openItems: openItems.length,
      doneItems: doneItems.length,
      doingItems: board.items.filter((item) => item.status === "Doing").length,
      todayItems: scheduledToday.length,
      overdueItems: overdue.length,
      urgentItems: openItems.filter((item) => item.priority === "urgent").length,
      focusMinutes,
      focusHoursLabel: formatHours(focusMinutes),
    },
    today: {
      scheduled: scheduledToday.sort(sortByAttention),
      overdue,
    },
    focus: {
      totalMinutes: focusMinutes,
      estimatedMinutes,
      completionPercent: focusCompletion,
    },
    priorityItems: openItems
      .filter((item) => item.priority === "urgent" || item.priority === "high")
      .sort(sortByAttention)
      .slice(0, 5),
    goalHighlights: goals.goals
      .map((goal) => buildGoalHighlight(goal, goals, today))
      .sort((left, right) => {
        if (left.overdueItems !== right.overdueItems) return right.overdueItems - left.overdueItems;
        if (left.progress !== right.progress) return left.progress - right.progress;
        return (left.targetDate ?? "9999-12-31").localeCompare(right.targetDate ?? "9999-12-31");
      })
      .slice(0, 4),
  };
}
