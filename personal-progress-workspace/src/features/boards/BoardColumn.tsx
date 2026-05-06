import { useDroppable } from "@dnd-kit/core";
import type { WorkspaceItem } from "../../domain/types";
import type { ColumnWithItems } from "./boardUtils";
import { ItemCard } from "./ItemCard";

export function BoardColumn({
  column,
  onOpenItem,
}: {
  column: ColumnWithItems;
  onOpenItem: (item: WorkspaceItem) => void;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: column.id });

  return (
    <section ref={setNodeRef} className={`board-column ${isOver ? "board-column--over" : ""}`}>
      <header>
        <h2>{column.name}</h2>
        <span aria-label={`${column.items.length} items`}>{column.items.length}</span>
      </header>
      <div className="board-column__cards">
        {column.items.map((item) => (
          <ItemCard key={item.id} item={item} onOpen={onOpenItem} />
        ))}
      </div>
    </section>
  );
}
