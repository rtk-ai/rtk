import { X } from "lucide-react";
import type { WorkspaceItem } from "../../domain/types";
import type { BoardData, ItemInput } from "./boardApi";
import { ItemForm } from "./ItemForm";

interface ItemDrawerProps {
  user: { id: string };
  workspaceId: string;
  board: BoardData;
  item: WorkspaceItem | null;
  onClose: () => void;
  onSubmit: (itemId: string | null, input: ItemInput) => void;
}

export function ItemDrawer({ user, workspaceId, board, item, onClose, onSubmit }: ItemDrawerProps) {
  const defaultColumn = item
    ? (board.columns.find((column) => column.id === item.columnId) ?? board.columns[0])
    : board.columns[0];

  return (
    <aside className="detail-drawer" aria-label="Item details">
      <header className="detail-drawer__header">
        <div>
          <p className="eyebrow">{item ? item.status : (defaultColumn?.statusKey ?? "Inbox")}</p>
          <h2>{item ? "Edit item" : "Create item"}</h2>
        </div>
        <button className="icon-button" type="button" aria-label="Close item drawer" onClick={onClose}>
          <X size={18} aria-hidden="true" />
        </button>
      </header>
      {defaultColumn ? (
        <ItemForm
          columns={board.columns}
          initialItem={item}
          defaults={{
            workspaceId,
            boardId: board.boardId,
            columnId: item?.columnId ?? defaultColumn.id,
            status: item?.status ?? defaultColumn.statusKey,
            createdBy: user.id,
          }}
          onSubmit={(input) => onSubmit(item?.id ?? null, input)}
        />
      ) : (
        <p className="drawer-empty">No columns available yet.</p>
      )}
    </aside>
  );
}
