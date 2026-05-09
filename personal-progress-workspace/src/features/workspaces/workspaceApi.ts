import type { User } from "@supabase/supabase-js";
import { supabase } from "../../lib/supabase";

export interface WorkspaceRecord {
  id: string;
  name: string;
  owner_user_id: string;
}

interface BoardRecord {
  id: string;
}

interface ColumnRecord {
  status_key: string;
}

const DEFAULT_COLUMNS = ["Inbox", "Planned", "Doing", "Review", "Done"] as const;

export async function getOrCreatePersonalWorkspace(user: User): Promise<WorkspaceRecord> {
  const { data: existing, error: existingError } = await supabase
    .from("workspace_members")
    .select("workspaces(id, name, owner_user_id)")
    .eq("user_id", user.id)
    .limit(1)
    .maybeSingle();

  if (existingError) throw existingError;

  const joinedWorkspace = existing?.workspaces as unknown as WorkspaceRecord | WorkspaceRecord[] | null;
  const workspace = Array.isArray(joinedWorkspace) ? (joinedWorkspace[0] ?? null) : joinedWorkspace;
  if (workspace) {
    await ensureDefaultBoard(workspace.id);
    return workspace;
  }

  const { data: ownedWorkspace, error: ownedWorkspaceError } = await supabase
    .from("workspaces")
    .select("id, name, owner_user_id")
    .eq("owner_user_id", user.id)
    .maybeSingle();

  if (ownedWorkspaceError) throw ownedWorkspaceError;

  if (ownedWorkspace) {
    await ensureOwnerMembership((ownedWorkspace as WorkspaceRecord).id, user.id);
    await ensureDefaultBoard((ownedWorkspace as WorkspaceRecord).id);
    return ownedWorkspace as WorkspaceRecord;
  }

  const { data: createdWorkspace, error: workspaceError } = await supabase
    .from("workspaces")
    .insert({ name: "Personal Workspace", owner_user_id: user.id })
    .select("id, name, owner_user_id")
    .single();

  if (workspaceError) throw workspaceError;

  await ensureOwnerMembership(createdWorkspace.id, user.id);

  await ensureDefaultBoard(createdWorkspace.id);

  return createdWorkspace;
}

async function ensureOwnerMembership(workspaceId: string, userId: string) {
  const { error: memberError } = await supabase
    .from("workspace_members")
    .upsert(
      { workspace_id: workspaceId, user_id: userId, role: "owner" },
      { ignoreDuplicates: true, onConflict: "workspace_id,user_id" },
    );

  if (memberError) throw memberError;
}

async function ensureDefaultBoard(workspaceId: string) {
  const { data: existingBoard, error: existingBoardError } = await supabase
    .from("boards")
    .select("id")
    .eq("workspace_id", workspaceId)
    .order("sort_order", { ascending: true })
    .limit(1)
    .maybeSingle();

  if (existingBoardError) throw existingBoardError;

  const boardId = existingBoard ? (existingBoard as BoardRecord).id : await createDefaultBoard(workspaceId);

  const { data: existingColumns, error: columnsQueryError } = await supabase
    .from("board_columns")
    .select("status_key")
    .eq("board_id", boardId);

  if (columnsQueryError) throw columnsQueryError;

  const existingStatusKeys = new Set(
    ((existingColumns as ColumnRecord[] | null) ?? []).map((column) => column.status_key),
  );
  const missingColumns = DEFAULT_COLUMNS.flatMap((column, index) =>
    existingStatusKeys.has(column)
      ? []
      : [
          {
            board_id: boardId,
            name: column,
            status_key: column,
            sort_order: index,
          },
        ],
  );

  if (missingColumns.length === 0) return;

  const { error: columnsError } = await supabase.from("board_columns").insert(missingColumns);

  if (columnsError) throw columnsError;
}

async function createDefaultBoard(workspaceId: string) {
  const { data: board, error: boardError } = await supabase
    .from("boards")
    .insert({ workspace_id: workspaceId, name: "Main Board", sort_order: 0 })
    .select("id")
    .single();

  if (boardError) throw boardError;

  return (board as BoardRecord).id;
}
