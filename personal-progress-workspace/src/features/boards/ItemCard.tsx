import { useDraggable } from "@dnd-kit/core";
import { CSS } from "@dnd-kit/utilities";
import { CalendarDays, Clock3, Flag, GripVertical } from "lucide-react";
import { calculateItemProgress } from "../../domain/progress";
import type { WorkspaceItem } from "../../domain/types";

export function ItemCard({
  item,
  onOpen,
}: {
  item: WorkspaceItem;
  onOpen: (item: WorkspaceItem) => void;
}) {
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, isDragging } = useDraggable({
    id: item.id,
  });
  const progress = calculateItemProgress({
    mode: item.progressMode,
    status: item.status,
    progressValue: item.progressValue,
    estimateMinutes: item.estimateMinutes,
    focusMinutesTotal: item.focusMinutesTotal,
  });
  const style = transform
    ? {
        transform: CSS.Translate.toString(transform),
      }
    : undefined;

  return (
    <article
      ref={setNodeRef}
      className={`item-card ${isDragging ? "item-card--dragging" : ""}`}
      style={style}
    >
      <div className="item-card__topline">
        <span className={`type-pill type-pill--${item.type}`}>{item.type}</span>
        <div className="item-card__actions">
          <span className={`priority-dot priority-dot--${item.priority}`} aria-label={`${item.priority} priority`} />
          <button
            ref={setActivatorNodeRef}
            className="item-card__drag-handle"
            type="button"
            aria-label={`Drag ${item.title}`}
            onClick={(event) => event.stopPropagation()}
            {...listeners}
            {...attributes}
          >
            <GripVertical size={16} aria-hidden="true" />
          </button>
        </div>
      </div>
      <button className="item-card__open" type="button" aria-label={`Open ${item.title}`} onClick={() => onOpen(item)}>
        <h3>{item.title}</h3>
      </button>
      <div className="item-card__meta">
        {item.dueDate ? (
          <span>
            <CalendarDays size={14} /> {item.dueDate}
          </span>
        ) : null}
        {item.estimateMinutes ? (
          <span>
            <Clock3 size={14} /> {item.estimateMinutes}m
          </span>
        ) : null}
        {item.goalId ? (
          <span>
            <Flag size={14} /> Goal
          </span>
        ) : null}
      </div>
      <div className="progress-bar" aria-label={`${progress}% progress`}>
        <span style={{ width: `${progress}%` }} />
      </div>
    </article>
  );
}
