import { todayIsoDate } from "../../domain/dates";
import type { WorkspaceItem } from "../../domain/types";
import { FocusLogger } from "./FocusLogger";
import { splitTodayItems } from "./todayUtils";

interface TodayPanelProps {
  items: WorkspaceItem[];
  today?: string;
  onLogFocus: (item: WorkspaceItem, minutes: number) => void;
}

export function TodayPanel({ items, today = todayIsoDate(), onLogFocus }: TodayPanelProps) {
  const groups = splitTodayItems(items, today);
  const openItems = items.filter((item) => item.status !== "Done");

  return (
    <aside className="today-panel">
      <p className="eyebrow">Today</p>
      <h2>{groups.today.length} planned</h2>
      <section className="today-panel__section">
        <h3>Scheduled</h3>
        <div className="today-panel__list">
          {groups.today.length > 0 ? (
            groups.today.map((item) => <p key={item.id}>{item.title}</p>)
          ) : (
            <p>Nothing scheduled.</p>
          )}
        </div>
      </section>
      <section className="today-panel__section">
        <h3>Overdue</h3>
        <div className="today-panel__list today-panel__list--overdue">
          {groups.overdue.length > 0 ? (
            groups.overdue.map((item) => <p key={item.id}>{item.title}</p>)
          ) : (
            <p>No overdue items.</p>
          )}
        </div>
      </section>
      <FocusLogger items={openItems} onLog={onLogFocus} />
    </aside>
  );
}
