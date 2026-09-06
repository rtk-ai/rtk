import type {
  BoardColumn,
  ItemPriority,
  ItemStatus,
  ItemType,
  ProgressMode,
  WorkspaceItem,
} from "../../domain/types";
import { supabase } from "../../lib/supabase";

interface BoardRow {
  id: string;
}

interface BoardColumnRow {
  id: string;
  board_id: string;
  name: string;
  status_key: ItemStatus;
  sort_order: number | string;
}

interface ItemRow {
  id: string;
  workspace_id: string;
  board_id: string;
  column_id: string;
  goal_id: string | null;
  title: string;
  description: string | null;
  type: ItemType;
  tags: string[] | null;
  priority: ItemPriority;
  status: ItemStatus;
  scheduled_date: string | null;
  due_date: string | null;
  estimate_minutes: number | null;
  focus_minutes_total: number;
  progress_mode: ProgressMode;
  progress_value: number | null;
  sort_order: number | string;
}

interface ItemMutationPayload {
  workspace_id?: string;
  board_id?: string;
  column_id?: string;
  title?: string;
  description?: string | null;
  type?: ItemType;
  tags?: string[];
  priority?: ItemPriority;
  status?: ItemStatus;
  scheduled_date?: string | null;
  due_date?: string | null;
  estimate_minutes?: number | null;
  progress_mode?: ProgressMode;
  progress_value?: number | null;
  created_by?: string;
}

export interface BoardData {
  boardId: string;
  columns: BoardColumn[];
  items: WorkspaceItem[];
}

export interface ItemInput {
  workspaceId: string;
  boardId: string;
  columnId: string;
  title: string;
  description: string | null;
  type: ItemType;
  tags: string[];
  priority: ItemPriority;
  status: ItemStatus;
  scheduledDate: string | null;
  dueDate: string | null;
  estimateMinutes: number | null;
  progressMode: ProgressMode;
  progressValue: number | null;
  createdBy: string;
}

export interface FocusSessionInput {
  workspaceId: string;
  itemId: string;
  goalId: string | null;
  startedAt: string;
  endedAt: string;
  durationMinutes: number;
}

export function mapBoardColumn(row: BoardColumnRow): BoardColumn {
  return {
    id: row.id,
    boardId: row.board_id,
    name: row.name,
    statusKey: row.status_key,
    sortOrder: Number(row.sort_order),
  };
}

export function mapWorkspaceItem(row: ItemRow): WorkspaceItem {
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

function toItemPayload(input: Partial<ItemInput>, includeCreatedBy: boolean): ItemMutationPayload {
  const payload: ItemMutationPayload = {};

  if (input.workspaceId !== undefined) payload.workspace_id = input.workspaceId;
  if (input.boardId !== undefined) payload.board_id = input.boardId;
  if (input.columnId !== undefined) payload.column_id = input.columnId;
  if (input.title !== undefined) payload.title = input.title;
  if (input.description !== undefined) payload.description = input.description;
  if (input.type !== undefined) payload.type = input.type;
  if (input.tags !== undefined) payload.tags = input.tags;
  if (input.priority !== undefined) payload.priority = input.priority;
  if (input.status !== undefined) payload.status = input.status;
  if (input.scheduledDate !== undefined) payload.scheduled_date = input.scheduledDate;
  if (input.dueDate !== undefined) payload.due_date = input.dueDate;
  if (input.estimateMinutes !== undefined) payload.estimate_minutes = input.estimateMinutes;
  if (input.progressMode !== undefined) payload.progress_mode = input.progressMode;
  if (input.progressValue !== undefined) payload.progress_value = input.progressValue;
  if (includeCreatedBy && input.createdBy !== undefined) payload.created_by = input.createdBy;

  return payload;
}

export async function loadPrimaryBoard(workspaceId: string): Promise<BoardData> {
  const { data: board, error: boardError } = await supabase
    .from("boards")
    .select("id")
    .eq("workspace_id", workspaceId)
    .order("sort_order", { ascending: true })
    .limit(1)
    .maybeSingle();

  if (boardError) throw boardError;
  if (!board) throw new Error("No board found for workspace.");

  const boardId = (board as BoardRow).id;
  const [
    { data: columns, error: columnsError },
    { data: items, error: itemsError },
  ] = await Promise.all([
    supabase
      .from("board_columns")
      .select("id, board_id, name, status_key, sort_order")
      .eq("board_id", boardId)
      .order("sort_order", { ascending: true }),
    supabase
      .from("items")
      .select(
        "id, workspace_id, board_id, column_id, goal_id, title, description, type, tags, priority, status, scheduled_date, due_date, estimate_minutes, focus_minutes_total, progress_mode, progress_value, sort_order",
      )
      .eq("board_id", boardId)
      .order("sort_order", { ascending: true }),
  ]);

  if (columnsError) throw columnsError;
  if (itemsError) throw itemsError;

  return {
    boardId,
    columns: ((columns as BoardColumnRow[] | null) ?? []).map(mapBoardColumn),
    items: ((items as ItemRow[] | null) ?? []).map(mapWorkspaceItem),
  };
}

export async function createItem(input: ItemInput): Promise<WorkspaceItem> {
  const { data, error } = await supabase
    .from("items")
    .insert(toItemPayload(input, true))
    .select("*")
    .single();

  if (error) throw error;
  return mapWorkspaceItem(data as ItemRow);
}

export async function updateItem(itemId: string, input: Partial<ItemInput>): Promise<WorkspaceItem> {
  let query = supabase.from("items").update(toItemPayload(input, false)).eq("id", itemId);

  if (input.workspaceId) {
    query = query.eq("workspace_id", input.workspaceId);
  }

  if (input.boardId) {
    query = query.eq("board_id", input.boardId);
  }

  const { data, error } = await query.select("*").single();

  if (error) throw error;
  return mapWorkspaceItem(data as ItemRow);
}

export async function logFocusSession(input: FocusSessionInput): Promise<void> {
  const { error: sessionError } = await supabase.from("focus_sessions").insert({
    workspace_id: input.workspaceId,
    item_id: input.itemId,
    goal_id: input.goalId,
    started_at: input.startedAt,
    ended_at: input.endedAt,
    duration_minutes: input.durationMinutes,
  });

  if (sessionError) throw sessionError;

  const { error: itemError } = await supabase.rpc("increment_item_focus_minutes", {
    target_item_id: input.itemId,
    minutes_to_add: input.durationMinutes,
  });

  if (!itemError) return;

  const { data: item, error: loadError } = await supabase
    .from("items")
    .select("focus_minutes_total")
    .eq("id", input.itemId)
    .eq("workspace_id", input.workspaceId)
    .single();

  if (loadError) throw loadError;

  const currentMinutes = Number(
    (item as { focus_minutes_total?: number | string | null } | null)?.focus_minutes_total ?? 0,
  );
  const { error: updateError } = await supabase
    .from("items")
    .update({ focus_minutes_total: currentMinutes + input.durationMinutes })
    .eq("id", input.itemId)
    .eq("workspace_id", input.workspaceId);

  if (updateError) throw updateError;
}

export async function updateItemPosition(item: WorkspaceItem): Promise<void> {
  await updateItemPositions([item]);
}

export async function updateItemPositions(items: WorkspaceItem[]): Promise<void> {
  const { error } = await supabase.rpc("update_item_positions", {
    position_updates: items.map((item) => ({
      id: item.id,
      workspace_id: item.workspaceId,
      board_id: item.boardId,
      column_id: item.columnId,
      status: item.status,
      sort_order: item.sortOrder,
    })),
  });

  if (error) throw error;
}
