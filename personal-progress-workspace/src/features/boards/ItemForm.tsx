import { type FormEvent, useState } from "react";
import type { BoardColumn, ItemPriority, ItemStatus, ItemType, WorkspaceItem } from "../../domain/types";
import type { ItemInput } from "./boardApi";
import { parseEstimateTimeInput } from "./itemFormUtils";

const itemTypes: ItemType[] = ["task", "content", "learning", "habit", "personal"];
const priorities: ItemPriority[] = ["low", "medium", "high", "urgent"];
const visibleStatusOptions: Array<{ value: ItemStatus; label: string }> = [
  { value: "Planned", label: "Chưa hoàn thành" },
  { value: "Doing", label: "Đang làm" },
  { value: "Done", label: "Đã hoàn thành" },
];
const estimatePresets = [
  { label: "30 mins", value: "30" },
  { label: "1 hour", value: "60" },
  { label: "2 hours", value: "120" },
  { label: "4 hours", value: "240" },
];

interface ItemFormProps {
  initialItem?: WorkspaceItem | null;
  columns: BoardColumn[];
  defaults: Pick<ItemInput, "workspaceId" | "boardId" | "columnId" | "status" | "createdBy">;
  onSubmit: (input: ItemInput) => void;
}

export function ItemForm({ initialItem = null, columns, defaults, onSubmit }: ItemFormProps) {
  const [title, setTitle] = useState(initialItem?.title ?? "");
  const [description, setDescription] = useState(initialItem?.description ?? "");
  const [type, setType] = useState<ItemType>(initialItem?.type ?? "task");
  const [priority, setPriority] = useState<ItemPriority>(initialItem?.priority ?? "medium");
  const [tags, setTags] = useState((initialItem?.tags ?? []).join(", "));
  const [scheduledDate, setScheduledDate] = useState(initialItem?.scheduledDate ?? "");
  const [dueDate, setDueDate] = useState(initialItem?.dueDate ?? "");
  const [estimateTime, setEstimateTime] = useState(initialItem?.estimateMinutes?.toString() ?? "");
  const [estimateError, setEstimateError] = useState<string | null>(null);
  const [progressStatus, setProgressStatus] = useState<ItemStatus>(visibleStatusFor(initialItem?.status ?? defaults.status));

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedDescription = description.trim();
    const parsedEstimate = parseEstimateTimeInput(estimateTime);

    if (parsedEstimate.error) {
      setEstimateError(parsedEstimate.error);
      return;
    }

    setEstimateError(null);

    const selectedColumn = columns.find((column) => column.statusKey === progressStatus);
    const trimmedScheduledDate = scheduledDate.trim();
    const trimmedDueDate = dueDate.trim();

    onSubmit({
      ...defaults,
      columnId: selectedColumn?.id ?? defaults.columnId,
      status: selectedColumn?.statusKey ?? defaults.status,
      title: title.trim(),
      description: trimmedDescription ? trimmedDescription : null,
      type,
      tags: tags
        .split(",")
        .map((tag) => tag.trim())
        .filter(Boolean),
      priority,
      scheduledDate: trimmedScheduledDate ? trimmedScheduledDate : null,
      dueDate: trimmedDueDate ? trimmedDueDate : null,
      estimateMinutes: parsedEstimate.minutes,
      progressMode: initialItem?.progressMode ?? "manual",
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
          <input
            value={scheduledDate}
            onChange={(event) => setScheduledDate(event.target.value)}
            placeholder="YYYY-MM-DD"
          />
        </label>
        <label>
          Due date
          <input value={dueDate} onChange={(event) => setDueDate(event.target.value)} placeholder="YYYY-MM-DD" />
        </label>
      </div>
      <div className="item-form__grid">
        <div className="item-form__estimate">
          <label>
            Estimate time
            <input
              value={estimateTime}
              onChange={(event) => {
                setEstimateTime(event.target.value);
                setEstimateError(null);
              }}
              placeholder="30 mins or 1.5h"
            />
          </label>
          <div className="estimate-presets" aria-label="Estimate presets">
            {estimatePresets.map((preset) => (
              <button key={preset.value} type="button" onClick={() => setEstimateTime(preset.value)}>
                {preset.label}
              </button>
            ))}
          </div>
          {estimateError ? <p className="form-error">{estimateError}</p> : null}
        </div>
        <label>
          Progress status
          <select value={progressStatus} onChange={(event) => setProgressStatus(event.target.value as ItemStatus)}>
            {visibleStatusOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
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

function visibleStatusFor(status: ItemStatus): ItemStatus {
  if (status === "Doing" || status === "Done") return status;
  return "Planned";
}
