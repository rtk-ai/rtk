import type { Goal, Milestone, WorkspaceItem } from "../../domain/types";
import { supabase } from "../../lib/supabase";

interface GoalRow {
  id: string;
  workspace_id: string;
  title: string;
  description: string | null;
  status: Goal["status"];
  target_date: string | null;
  progress_mode: Goal["progressMode"];
  manual_progress_value: number | null;
}

interface MilestoneRow {
  id: string;
  goal_id: string;
  title: string;
  status: Milestone["status"];
  due_date: string | null;
  sort_order: number | string;
}

interface LinkedItemRow {
  id: string;
  workspace_id: string;
  board_id: string;
  column_id: string;
  goal_id: string | null;
  title: string;
  description: string | null;
  type: WorkspaceItem["type"];
  tags: string[] | null;
  priority: WorkspaceItem["priority"];
  status: WorkspaceItem["status"];
  scheduled_date: string | null;
  due_date: string | null;
  estimate_minutes: number | null;
  focus_minutes_total: number;
  progress_mode: WorkspaceItem["progressMode"];
  progress_value: number | null;
  sort_order: number | string;
}

export interface GoalBundle {
  goals: Goal[];
  milestones: Milestone[];
  linkedItems: WorkspaceItem[];
}

export async function loadGoals(workspaceId: string): Promise<GoalBundle> {
  const [
    { data: goals, error: goalsError },
    { data: milestones, error: milestonesError },
    { data: items, error: itemsError },
  ] = await Promise.all([
    supabase.from("goals").select("*").eq("workspace_id", workspaceId).order("created_at", { ascending: false }),
    supabase.from("milestones").select("*").eq("workspace_id", workspaceId).order("sort_order", { ascending: true }),
    supabase.from("items").select("*").eq("workspace_id", workspaceId).not("goal_id", "is", null),
  ]);

  if (goalsError) throw goalsError;
  if (milestonesError) throw milestonesError;
  if (itemsError) throw itemsError;

  return {
    goals: ((goals as GoalRow[] | null) ?? []).map(mapGoal),
    milestones: ((milestones as MilestoneRow[] | null) ?? []).map(mapMilestone),
    linkedItems: ((items as LinkedItemRow[] | null) ?? []).map(mapLinkedItem),
  };
}

function mapGoal(row: GoalRow): Goal {
  return {
    id: row.id,
    workspaceId: row.workspace_id,
    title: row.title,
    description: row.description,
    status: row.status,
    targetDate: row.target_date,
    progressMode: row.progress_mode,
    manualProgressValue: row.manual_progress_value,
  };
}

function mapMilestone(row: MilestoneRow): Milestone {
  return {
    id: row.id,
    goalId: row.goal_id,
    title: row.title,
    status: row.status,
    dueDate: row.due_date,
    sortOrder: Number(row.sort_order),
  };
}

function mapLinkedItem(row: LinkedItemRow): WorkspaceItem {
  return {
    id: row.id,
    workspaceId: row.workspace_id,
    boardId: row.board_id,
    columnId: row.column_id,
    goalId: row.goal_id,
    title: row.title,
    description: row.description,
    type: row.type,
    tags: row.tags ?? [],
    priority: row.priority,
    status: row.status,
    scheduledDate: row.scheduled_date,
    dueDate: row.due_date,
    estimateMinutes: row.estimate_minutes,
    focusMinutesTotal: row.focus_minutes_total,
    progressMode: row.progress_mode,
    progressValue: row.progress_value,
    sortOrder: Number(row.sort_order),
  };
}
