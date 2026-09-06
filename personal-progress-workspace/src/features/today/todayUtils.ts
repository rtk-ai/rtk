import type { WorkspaceItem } from "../../domain/types";

export function splitTodayItems(items: WorkspaceItem[], today: string) {
  return {
    today: items.filter((item) => item.status !== "Done" && item.scheduledDate === today),
    overdue: items.filter((item) => item.status !== "Done" && item.dueDate !== null && item.dueDate < today),
  };
}
