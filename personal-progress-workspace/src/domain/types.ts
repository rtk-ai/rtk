export type ItemType = "task" | "content" | "learning" | "habit" | "personal";
export type ItemStatus = "Inbox" | "Planned" | "Doing" | "Review" | "Done";
export type ItemPriority = "low" | "medium" | "high" | "urgent";
export type ProgressMode = "tasks" | "milestones" | "focus_time" | "streak" | "manual";
export type GoalStatus = "active" | "paused" | "completed";
export type MilestoneStatus = "active" | "done";

export interface BoardColumn {
  id: string;
  boardId: string;
  name: string;
  statusKey: ItemStatus;
  sortOrder: number;
}

export interface WorkspaceItem {
  id: string;
  workspaceId: string;
  boardId: string;
  columnId: string;
  goalId: string | null;
  title: string;
  description: string | null;
  type: ItemType;
  tags: string[];
  priority: ItemPriority;
  status: ItemStatus;
  scheduledDate: string | null;
  dueDate: string | null;
  estimateMinutes: number | null;
  focusMinutesTotal: number;
  progressMode: ProgressMode;
  progressValue: number | null;
  sortOrder: number;
}

export interface Goal {
  id: string;
  workspaceId: string;
  title: string;
  description: string | null;
  status: GoalStatus;
  targetDate: string | null;
  progressMode: ProgressMode;
  manualProgressValue: number | null;
}

export interface Milestone {
  id: string;
  goalId: string;
  title: string;
  status: MilestoneStatus;
  dueDate: string | null;
  sortOrder: number;
}

export interface ItemProgressInput {
  mode: ProgressMode;
  status?: ItemStatus;
  progressValue?: number | null;
  estimateMinutes?: number | null;
  focusMinutesTotal?: number | null;
  streakCompletedToday?: boolean;
}

export interface GoalProgressInput {
  mode: ProgressMode;
  milestones: Array<{ status: MilestoneStatus }>;
  linkedItems: Array<{ status: ItemStatus }>;
  focusMinutesTotal: number;
  focusTargetMinutes: number;
  manualProgressValue: number | null;
}
