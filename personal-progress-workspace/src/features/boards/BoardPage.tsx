import { DndContext } from "@dnd-kit/core";
import type { DragEndEvent } from "@dnd-kit/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import { useMemo, useState } from "react";
import type { WorkspaceItem } from "../../domain/types";
import { useAuth } from "../auth/AuthProvider";
import { TodayPanel } from "../today/TodayPanel";
import type { WorkspaceRecord } from "../workspaces/workspaceApi";
import { BoardColumn } from "./BoardColumn";
import {
  type BoardData,
  createItem,
  type ItemInput,
  loadPrimaryBoard,
  logFocusSession,
  updateItem,
  updateItemPositions,
} from "./boardApi";
import { groupItemsByColumn, reorderAfterMove } from "./boardUtils";
import { ItemDrawer } from "./ItemDrawer";

function findChangedItems(previousItems: WorkspaceItem[], nextItems: WorkspaceItem[]) {
  const previousById = new Map(previousItems.map((item) => [item.id, item]));

  return nextItems.filter((item) => {
    const previous = previousById.get(item.id);
    return (
      previous &&
      (previous.columnId !== item.columnId || previous.status !== item.status || previous.sortOrder !== item.sortOrder)
    );
  });
}

interface MoveItemsVariables {
  changedItems: WorkspaceItem[];
  nextItems: WorkspaceItem[];
}

interface MoveItemsContext {
  previousData?: BoardData;
}

interface SaveItemVariables {
  itemId: string | null;
  input: ItemInput;
}

interface LogFocusVariables {
  item: WorkspaceItem;
  minutes: number;
}

export function BoardPage({ workspace }: { workspace: WorkspaceRecord }) {
  const { user } = useAuth();
  const queryClient = useQueryClient();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [openItem, setOpenItem] = useState<WorkspaceItem | null>(null);
  const queryKey = ["board", workspace.id] as const;

  const boardQuery = useQuery({
    queryKey,
    queryFn: () => loadPrimaryBoard(workspace.id),
  });

  const moveMutation = useMutation({
    mutationFn: ({ changedItems }: MoveItemsVariables) => updateItemPositions(changedItems),
    onMutate: async ({ nextItems }): Promise<MoveItemsContext> => {
      await queryClient.cancelQueries({ queryKey });
      const previousData = queryClient.getQueryData<BoardData>(queryKey);
      if (previousData) {
        queryClient.setQueryData(queryKey, { ...previousData, items: nextItems });
      }
      return { previousData };
    },
    onError: (_error, _variables, context) => {
      if (context?.previousData) {
        queryClient.setQueryData(queryKey, context.previousData);
      }
      queryClient.invalidateQueries({ queryKey });
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey }),
  });

  const saveMutation = useMutation({
    mutationFn: ({ itemId, input }: SaveItemVariables) => (itemId ? updateItem(itemId, input) : createItem(input)),
    onSuccess: () => {
      closeDrawer();
      queryClient.invalidateQueries({ queryKey });
    },
  });

  const focusMutation = useMutation({
    mutationFn: ({ item, minutes }: LogFocusVariables) => {
      const endedAtMs = Date.now();
      const endedAt = new Date(endedAtMs);
      const startedAt = new Date(endedAtMs - minutes * 60_000);

      return logFocusSession({
        workspaceId: workspace.id,
        itemId: item.id,
        goalId: item.goalId,
        startedAt: startedAt.toISOString(),
        endedAt: endedAt.toISOString(),
        durationMinutes: minutes,
      });
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey }),
  });

  const columns = useMemo(() => {
    if (!boardQuery.data) return [];
    return groupItemsByColumn(boardQuery.data.columns, boardQuery.data.items);
  }, [boardQuery.data]);

  function handleDragEnd(event: DragEndEvent) {
    if (moveMutation.isPending || !boardQuery.data || !event.over) return;

    const activeItemId = String(event.active.id);
    const targetColumn = boardQuery.data.columns.find((column) => column.id === String(event.over?.id));
    if (!targetColumn) return;

    const currentTargetItems = boardQuery.data.items.filter((item) => item.columnId === targetColumn.id);
    const nextItems = reorderAfterMove({
      items: boardQuery.data.items,
      activeItemId,
      targetColumn,
      targetIndex: currentTargetItems.length,
    });
    const changedItems = findChangedItems(boardQuery.data.items, nextItems);
    if (changedItems.length === 0) return;

    moveMutation.mutate({ changedItems, nextItems });
  }

  function openCreateDrawer() {
    setOpenItem(null);
    setDrawerOpen(true);
  }

  function openEditDrawer(item: WorkspaceItem) {
    setOpenItem(item);
    setDrawerOpen(true);
  }

  function closeDrawer() {
    setOpenItem(null);
    setDrawerOpen(false);
  }

  if (boardQuery.isLoading) return <section className="board-state">Loading board...</section>;
  if (boardQuery.isError) return <section className="board-state">Board failed to load.</section>;

  return (
    <section className="workspace-layout">
      <aside className="workspace-sidebar">
        <p className="eyebrow">{workspace.name}</p>
        <h1>Main Board</h1>
        <div className="workspace-sidebar__actions">
          <button className="sidebar-action" type="button" onClick={openCreateDrawer} disabled={!user}>
            <Plus size={16} aria-hidden="true" />
            New item
          </button>
        </div>
      </aside>
      <DndContext onDragEnd={handleDragEnd}>
        <main className="workspace-board">
          <div className="board-grid">
            {columns.map((column) => (
              <BoardColumn key={column.id} column={column} onOpenItem={openEditDrawer} />
            ))}
          </div>
        </main>
      </DndContext>
      {boardQuery.data ? (
        <TodayPanel
          items={boardQuery.data.items}
          onLogFocus={(item, minutes) => focusMutation.mutate({ item, minutes })}
        />
      ) : null}
      {drawerOpen && boardQuery.data && user ? (
        <ItemDrawer
          user={user}
          workspaceId={workspace.id}
          board={boardQuery.data}
          item={openItem}
          onClose={closeDrawer}
          onSubmit={(itemId, input) => saveMutation.mutate({ itemId, input })}
        />
      ) : null}
    </section>
  );
}
