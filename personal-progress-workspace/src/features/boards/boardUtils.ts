import type { BoardColumn, WorkspaceItem } from "../../domain/types";

export interface ColumnWithItems extends BoardColumn {
  items: WorkspaceItem[];
}

export function groupItemsByColumn(columns: BoardColumn[], items: WorkspaceItem[]): ColumnWithItems[] {
  return [...columns]
    .sort((left, right) => left.sortOrder - right.sortOrder)
    .map((column) => ({
      ...column,
      items: items
        .filter((item) => item.columnId === column.id)
        .sort((left, right) => left.sortOrder - right.sortOrder),
    }));
}

export function reorderAfterMove({
  items,
  activeItemId,
  targetColumn,
  targetIndex,
}: {
  items: WorkspaceItem[];
  activeItemId: string;
  targetColumn: BoardColumn;
  targetIndex: number;
}): WorkspaceItem[] {
  const activeItem = items.find((item) => item.id === activeItemId);
  if (!activeItem) return items;

  const targetItems = items
    .filter((item) => item.columnId === targetColumn.id && item.id !== activeItemId)
    .sort((left, right) => left.sortOrder - right.sortOrder);
  const insertIndex = Math.max(0, Math.min(targetIndex, targetItems.length));
  const updatedActiveItem: WorkspaceItem = {
    ...activeItem,
    columnId: targetColumn.id,
    status: targetColumn.statusKey,
  };
  const renumberedTargetItems = [
    ...targetItems.slice(0, insertIndex),
    updatedActiveItem,
    ...targetItems.slice(insertIndex),
  ].map((item, sortOrder) => ({ ...item, sortOrder }));
  const updatedById = new Map(renumberedTargetItems.map((item) => [item.id, item]));

  return items.map((item) => updatedById.get(item.id) ?? item);
}
