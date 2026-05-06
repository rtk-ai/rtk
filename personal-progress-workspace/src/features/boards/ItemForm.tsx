import { type FormEvent, useState } from "react";
import type { ItemPriority, ItemType, ProgressMode, WorkspaceItem } from "../../domain/types";
import type { ItemInput } from "./boardApi";

const itemTypes: ItemType[] = ["task", "content", "learning", "habit", "personal"];
const priorities: ItemPriority[] = ["low", "medium", "high", "urgent"];
const progressModes: ProgressMode[] = ["tasks", "milestones", "focus_time", "streak", "manual"];

interface ItemFormProps {
  initialItem?: WorkspaceItem | null;
  defaults: Pick<ItemInput, "workspaceId" | "boardId" | "columnId" | "status" | "createdBy">;
  onSubmit: (input: ItemInput) => void;
}

export function ItemForm({ initialItem = null, defaults, onSubmit }: ItemFormProps) {
  const [title, setTitle] = useState(initialItem?.title ?? "");
  const [description, setDescription] = useState(initialItem?.description ?? "");
  const [type, setType] = useState<ItemType>(initialItem?.type ?? "task");
  const [priority, setPriority] = useState<ItemPriority>(initialItem?.priority ?? "medium");
  const [tags, setTags] = useState((initialItem?.tags ?? []).join(", "));
  const [scheduledDate, setScheduledDate] = useState(initialItem?.scheduledDate ?? "");
  const [dueDate, setDueDate] = useState(initialItem?.dueDate ?? "");
  const [estimateMinutes, setEstimateMinutes] = useState(initialItem?.estimateMinutes?.toString() ?? "");
  const [progressMode, setProgressMode] = useState<ProgressMode>(initialItem?.progressMode ?? "tasks");

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedDescription = description.trim();

    onSubmit({
      ...defaults,
      title: title.trim(),
      description: trimmedDescription ? trimmedDescription : null,
      type,
      tags: tags
        .split(",")
        .map((tag) => tag.trim())
        .filter(Boolean),
      priority,
      scheduledDate: scheduledDate || null,
      dueDate: dueDate || null,
      estimateMinutes: estimateMinutes ? Number(estimateMinutes) : null,
      progressMode,
      progressValue: initialItem?.progressValue ?? null,
    });
  }

  return (
    <form className="item-form" onSubmit={handleSubmit}>
      <label>
        Title
        <input value={title} onChange={(event) => setTitle(event.target.value)} required />
      </label>
      <label>
        Description
        <textarea value={description} onChange={(event) => setDescription(event.target.value)} rows={4} />
      </label>
      <div className="item-form__grid">
        <label>
          Type
          <select value={type} onChange={(event) => setType(event.target.value as ItemType)}>
            {itemTypes.map((itemType) => (
              <option key={itemType} value={itemType}>
                {itemType}
              </option>
            ))}
          </select>
        </label>
        <label>
          Priority
          <select value={priority} onChange={(event) => setPriority(event.target.value as ItemPriority)}>
            {priorities.map((itemPriority) => (
              <option key={itemPriority} value={itemPriority}>
                {itemPriority}
              </option>
            ))}
          </select>
        </label>
      </div>
      <label>
        Tags
        <input value={tags} onChange={(event) => setTags(event.target.value)} placeholder="content, learning" />
      </label>
      <div className="item-form__grid">
        <label>
          Scheduled date
          <input type="date" value={scheduledDate} onChange={(event) => setScheduledDate(event.target.value)} />
        </label>
        <label>
          Due date
          <input type="date" value={dueDate} onChange={(event) => setDueDate(event.target.value)} />
        </label>
      </div>
      <div className="item-form__grid">
        <label>
          Estimate minutes
          <input
            min="0"
            step="5"
            type="number"
            value={estimateMinutes}
            onChange={(event) => setEstimateMinutes(event.target.value)}
          />
        </label>
        <label>
          Progress mode
          <select value={progressMode} onChange={(event) => setProgressMode(event.target.value as ProgressMode)}>
            {progressModes.map((mode) => (
              <option key={mode} value={mode}>
                {mode}
              </option>
            ))}
          </select>
        </label>
      </div>
      <button className="item-form__submit" type="submit">
        {initialItem ? "Save item" : "Create item"}
      </button>
    </form>
  );
}
