import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, ArrowRight, CalendarClock, Flame, Gauge, ListChecks, Sparkles, Target } from "lucide-react";
import type { WorkspaceItem } from "../../domain/types";
import type { WorkspaceRecord } from "../workspaces/workspaceApi";
import { loadDashboard } from "./dashboardApi";
import type { DashboardGoalHighlight } from "./dashboardUtils";
import { buildDashboardSummary } from "./dashboardUtils";

interface DashboardPageProps {
  workspace: WorkspaceRecord;
  onOpenBoard: () => void;
  onOpenGoals: () => void;
}

function itemMeta(item: WorkspaceItem) {
  const due = item.dueDate ? `Due ${item.dueDate}` : "No due date";
  return `${item.priority} priority - ${due}`;
}

function DashboardMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone: "blue" | "green" | "amber" | "rose";
}) {
  return (
    <article className={`dashboard-metric dashboard-metric--${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </article>
  );
}

function ItemList({ empty, items }: { empty: string; items: WorkspaceItem[] }) {
  if (items.length === 0) return <p className="dashboard-empty">{empty}</p>;

  return (
    <div className="dashboard-list">
      {items.slice(0, 4).map((item) => (
        <article className="dashboard-list-item" key={item.id}>
          <div>
            <h3>{item.title}</h3>
            <p>{itemMeta(item)}</p>
          </div>
          <span>{item.status}</span>
        </article>
      ))}
    </div>
  );
}

function GoalHighlight({ goal }: { goal: DashboardGoalHighlight }) {
  return (
    <article className="dashboard-goal">
      <div className="dashboard-goal__topline">
        <span>{goal.statusLabel}</span>
        {goal.targetDate ? <time dateTime={goal.targetDate}>{goal.targetDate}</time> : null}
      </div>
      <h3>{goal.title}</h3>
      <div className="dashboard-goal__progress">
        <strong>{goal.progressLabel}</strong>
        <span>{goal.openItems} open</span>
      </div>
      <div className="progress-bar" aria-label={`${goal.progressLabel} progress`}>
        <span style={{ width: `${goal.progress}%` }} />
      </div>
      {goal.overdueItems > 0 ? <p>{goal.overdueItems} overdue item needs attention.</p> : <p>On track for now.</p>}
    </article>
  );
}

export function DashboardPage({ workspace, onOpenBoard, onOpenGoals }: DashboardPageProps) {
  const dashboardQuery = useQuery({
    queryKey: ["dashboard", workspace.id],
    queryFn: () => loadDashboard(workspace.id),
  });

  if (dashboardQuery.isLoading) return <section className="board-state">Loading dashboard...</section>;
  if (dashboardQuery.isError || !dashboardQuery.data) {
    return <section className="board-state">Dashboard failed to load.</section>;
  }

  const summary = buildDashboardSummary(dashboardQuery.data);

  return (
    <main className="dashboard-page">
      <section className="dashboard-hero">
        <div className="dashboard-hero__copy">
          <p className="eyebrow">{workspace.name}</p>
          <h1>Command dashboard</h1>
          <p>
            A live read on your work: what needs attention, what is moving today, and where your goals stand.
          </p>
          <div className="dashboard-actions" aria-label="Dashboard quick actions">
            <button type="button" onClick={onOpenBoard}>
              <ListChecks size={16} aria-hidden="true" />
              Open board
            </button>
            <button type="button" onClick={onOpenGoals}>
              <Target size={16} aria-hidden="true" />
              Review goals
            </button>
          </div>
        </div>
        <div className="dashboard-hero__focus" aria-label="Focus progress">
          <Sparkles size={24} aria-hidden="true" />
          <span>Focus logged</span>
          <strong>{summary.stats.focusHoursLabel}</strong>
          <div className="progress-bar" aria-label={`${summary.focus.completionPercent}% focus progress`}>
            <span style={{ width: `${summary.focus.completionPercent}%` }} />
          </div>
          <p>{summary.focus.completionPercent}% of estimated work has logged focus.</p>
        </div>
      </section>

      <section className="dashboard-metrics" aria-label="Workspace metrics">
        <DashboardMetric label="Open work" value={`${summary.stats.openItems} open`} tone="blue" />
        <DashboardMetric label="Today" value={`${summary.stats.todayItems} planned`} tone="green" />
        <DashboardMetric label="Due pressure" value={`${summary.stats.overdueItems} due now`} tone="rose" />
        <DashboardMetric label="In motion" value={`${summary.stats.doingItems} doing`} tone="amber" />
      </section>

      <section className="dashboard-grid">
        <section className="dashboard-panel dashboard-panel--wide" aria-labelledby="today-heading">
          <div className="dashboard-panel__header">
            <div>
              <p className="eyebrow">Today</p>
              <h2 id="today-heading">Planned momentum</h2>
            </div>
            <CalendarClock size={20} aria-hidden="true" />
          </div>
          <ItemList empty="Nothing scheduled for today." items={summary.today.scheduled} />
        </section>

        <section className="dashboard-panel" aria-labelledby="attention-heading">
          <div className="dashboard-panel__header">
            <div>
              <p className="eyebrow">Attention</p>
              <h2 id="attention-heading">Priority lane</h2>
            </div>
            <Flame size={20} aria-hidden="true" />
          </div>
          <ItemList empty="No urgent or high-priority items." items={summary.priorityItems} />
        </section>

        <section className="dashboard-panel" aria-labelledby="overdue-heading">
          <div className="dashboard-panel__header">
            <div>
              <p className="eyebrow">Risk</p>
              <h2 id="overdue-heading">Overdue</h2>
            </div>
            <AlertTriangle size={20} aria-hidden="true" />
          </div>
          <ItemList empty="No overdue items." items={summary.today.overdue} />
        </section>

        <section className="dashboard-panel dashboard-panel--wide" aria-labelledby="goals-heading">
          <div className="dashboard-panel__header">
            <div>
              <p className="eyebrow">Goals</p>
              <h2 id="goals-heading">Active outcomes</h2>
            </div>
            <button className="dashboard-panel__link" type="button" onClick={onOpenGoals}>
              See all
              <ArrowRight size={16} aria-hidden="true" />
            </button>
          </div>
          <div className="dashboard-goal-grid">
            {summary.goalHighlights.length > 0 ? (
              summary.goalHighlights.map((goal) => <GoalHighlight goal={goal} key={goal.id} />)
            ) : (
              <p className="dashboard-empty">No active goals yet.</p>
            )}
          </div>
        </section>

        <section className="dashboard-panel dashboard-panel--focus" aria-labelledby="focus-heading">
          <div className="dashboard-panel__header">
            <div>
              <p className="eyebrow">Pulse</p>
              <h2 id="focus-heading">Workload</h2>
            </div>
            <Gauge size={20} aria-hidden="true" />
          </div>
          <div className="dashboard-workload">
            <span>{summary.stats.doneItems} done</span>
            <strong>{summary.stats.totalItems} total items</strong>
            <p>{summary.stats.urgentItems} urgent item{summary.stats.urgentItems === 1 ? "" : "s"} in open work.</p>
          </div>
        </section>
      </section>
    </main>
  );
}
