import { useQuery } from "@tanstack/react-query";
import { calculateGoalProgress } from "../../domain/progress";
import type { WorkspaceRecord } from "../workspaces/workspaceApi";
import { GoalDetail } from "./GoalDetail";
import { loadGoals } from "./goalApi";
import { summarizeGoal } from "./goalUtils";

export function GoalsPage({ workspace }: { workspace: WorkspaceRecord }) {
  const goalsQuery = useQuery({
    queryKey: ["goals", workspace.id],
    queryFn: () => loadGoals(workspace.id),
  });

  if (goalsQuery.isLoading) return <section className="board-state">Loading goals...</section>;
  if (goalsQuery.isError || !goalsQuery.data) return <section className="board-state">Goals failed to load.</section>;

  return (
    <section className="goals-page">
      <header className="goals-page__header">
        <p className="eyebrow">{workspace.name}</p>
        <h1>Progress dashboard</h1>
      </header>
      <div className="goal-grid">
        {goalsQuery.data.goals.map((goal) => {
          const milestones = goalsQuery.data.milestones.filter((milestone) => milestone.goalId === goal.id);
          const linkedItems = goalsQuery.data.linkedItems.filter((item) => item.goalId === goal.id);
          const progress = calculateGoalProgress({
            mode: goal.progressMode,
            milestones,
            linkedItems,
            focusMinutesTotal: linkedItems.reduce((sum, item) => sum + item.focusMinutesTotal, 0),
            focusTargetMinutes: linkedItems.reduce((sum, item) => sum + (item.estimateMinutes ?? 0), 0),
            manualProgressValue: goal.manualProgressValue,
          });
          const summary = summarizeGoal({ progress, status: goal.status });

          return (
            <article className="goal-card" key={goal.id}>
              <div className="goal-card__header">
                <span>{summary.statusLabel}</span>
                {goal.targetDate ? <time dateTime={goal.targetDate}>{goal.targetDate}</time> : null}
              </div>
              <h2>{goal.title}</h2>
              <strong>{summary.progressLabel}</strong>
              <div className="progress-bar" aria-label={`${summary.progressLabel} progress`}>
                <span style={{ width: `${progress}%` }} />
              </div>
              <GoalDetail goal={goal} milestones={milestones} linkedItems={linkedItems} />
            </article>
          );
        })}
      </div>
    </section>
  );
}
