import { type FormEvent, useEffect, useState } from "react";
import type { WorkspaceItem } from "../../domain/types";

interface FocusLoggerProps {
  items: WorkspaceItem[];
  onLog: (item: WorkspaceItem, minutes: number) => void;
}

export function FocusLogger({ items, onLog }: FocusLoggerProps) {
  const [itemId, setItemId] = useState(items[0]?.id ?? "");
  const [minutes, setMinutes] = useState("25");

  useEffect(() => {
    if (!items.some((item) => item.id === itemId)) {
      setItemId(items[0]?.id ?? "");
    }
  }, [itemId, items]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const item = items.find((entry) => entry.id === itemId);
    const duration = Number(minutes);
    if (!item || duration <= 0) return;
    onLog(item, duration);
  }

  return (
    <form className="focus-logger" onSubmit={submit}>
      <label>
        Focus item
        <select value={itemId} onChange={(event) => setItemId(event.target.value)} disabled={items.length === 0}>
          {items.map((item) => (
            <option key={item.id} value={item.id}>
              {item.title}
            </option>
          ))}
        </select>
      </label>
      <label>
        Minutes
        <input min="1" type="number" value={minutes} onChange={(event) => setMinutes(event.target.value)} />
      </label>
      <button className="focus-logger__submit" type="submit" disabled={items.length === 0}>
        Log focus
      </button>
    </form>
  );
}
