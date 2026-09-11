import { type FormEvent, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { calculateGoalProgress } from "../../domain/progress";
import type { Goal, GoalStatus } from "../../domain/types";
import type { WorkspaceRecord } from "../workspaces/workspaceApi";
import { GoalDetail } from "./GoalDetail";
import { createGoal, loadGoals, updateGoal, type GoalInput } from "./goalApi";
import { summarizeGoal } from "./goalUtils";

interface GoalFormState {
  title: string;
  description: string;
  targetDate: string;
  status: GoalStatus;
}

const emptyGoalForm: GoalFormState = {
  title: "",
  description: "",
  targetDate: "",
  status: "active",
};

const goalStatusOptions: Array<{ value: GoalStatus; label: string }> = [
  { value: "active", label: "Đang hoạt động" },
  { value: "paused", label: "Tạm dừng" },
  { value: "completed", label: "Hoàn thành" },
];

export function GoalsPage({ workspace }: { workspace: WorkspaceRecord }) {
  const queryClient = useQueryClient();
  const [createForm, setCreateForm] = useState<GoalFormState>(emptyGoalForm);
  const [editingGoalId, setEditingGoalId] = useState<string | null>(null);
  const [editForm, setEditForm] = useState<GoalFormState>(emptyGoalForm);
  const [createError, setCreateError] = useState<string | null>(null);
  const [editError, setEditError] = useState<string | null>(null);

  const goalsQuery = useQuery({
    queryKey: ["goals", workspace.id],
    queryFn: () => loadGoals(workspace.id),
  });

  const createMutation = useMutation({
    mutationFn: (input: GoalInput) => createGoal(input),
    onSuccess: () => {
      setCreateForm(emptyGoalForm);
      setCreateError(null);
      void queryClient.invalidateQueries({ queryKey: ["goals", workspace.id] });
    },
    onError: (error) => setCreateError(errorMessage(error, "Goal could not be created.")),
  });

  const updateMutation = useMutation({
    mutationFn: ({ goalId, input }: { goalId: string; input: GoalInput }) => updateGoal(goalId, input),
    onSuccess: () => {
      setEditingGoalId(null);
      setEditError(null);
      void queryClient.invalidateQueries({ queryKey: ["goals", workspace.id] });
    },
    onError: (error) => setEditError(errorMessage(error, "Goal could not be updated.")),
  });

  function handleCreateSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    createMutation.mutate(toGoalInput(workspace.id, createForm));
  }

  function startEditing(goal: Goal) {
    setEditingGoalId(goal.id);
    setEditForm(goalToFormState(goal));
    setEditError(null);
  }

  function handleEditSubmit(event: FormEvent<HTMLFormElement>, goalId: string) {
    event.preventDefault();
    updateMutation.mutate({ goalId, input: toGoalInput(workspace.id, editForm) });
  }

  if (goalsQuery.isLoading) return <section className="board-state">Loading goals...</section>;
  if (goalsQuery.isError || !goalsQuery.data) return <section className="board-state">Goals failed to load.</section>;

  return (
    <section className="goals-page">
      <header className="goals-page__header">
        <p className="eyebrow">{workspace.name}</p>
        <h1>Progress dashboard</h1>
      </header>
      <form className="goal-compose goal-form" onSubmit={handleCreateSubmit}>
        <div>
          <p className="eyebrow">Goals</p>
          <h2>Create goal</h2>
        </div>
        <div className="goal-form__grid">
          <label>
            Goal title
            <input
              value={createForm.title}
              onChange={(event) => setCreateForm((form) => ({ ...form, title: event.target.value }))}
              required
            />
          </label>
          <label>
            Goal status
            <select
              value={createForm.status}
              onChange={(event) => setCreateForm((form) => ({ ...form, status: event.target.value as GoalStatus }))}
            >
              {goalStatusOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
        </div>
        <label>
          Goal description
          <textarea
            value={createForm.description}
            onChange={(event) => setCreateForm((form) => ({ ...form, description: event.target.value }))}
            rows={3}
          />
        </label>
        <label>
          Target date
          <input
            value={createForm.targetDate}
            onChange={(event) => setCreateForm((form) => ({ ...form, targetDate: event.target.value }))}
            placeholder="YYYY-MM-DD"
          />
        </label>
        {createError ? <p className="form-error">{createError}</p> : null}
        <div className="goal-form__actions">
          <button className="goal-card__button goal-card__button--primary" type="submit" disabled={createMutation.isPending}>
            Create goal
          </button>
        </div>
      </form>
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
                <div className="goal-card__actions">
                  {goal.targetDate ? <time dateTime={goal.targetDate}>{goal.targetDate}</time> : null}
                  <button
                    className="goal-card__button"
                    type="button"
                    aria-label={`Edit goal ${goal.title}`}
                    onClick={() => startEditing(goal)}
                  >
                    Edit goal
                  </button>
                </div>
              </div>
              {editingGoalId === goal.id ? (
                <form className="goal-edit-form" onSubmit={(event) => handleEditSubmit(event, goal.id)}>
                  <label>
                    Edit title
                    <input
                      value={editForm.title}
                      onChange={(event) => setEditForm((form) => ({ ...form, title: event.target.value }))}
                      required
                    />
                  </label>
                  <label>
                    Edit description
                    <textarea
                      value={editForm.description}
                      onChange={(event) => setEditForm((form) => ({ ...form, description: event.target.value }))}
                      rows={3}
                    />
                  </label>
                  <div className="goal-form__grid">
                    <label>
                      Edit target date
                      <input
                        value={editForm.targetDate}
                        onChange={(event) => setEditForm((form) => ({ ...form, targetDate: event.target.value }))}
                        placeholder="YYYY-MM-DD"
                      />
                    </label>
                    <label>
                      Edit status
                      <select
                        value={editForm.status}
                        onChange={(event) => setEditForm((form) => ({ ...form, status: event.target.value as GoalStatus }))}
                      >
                        {goalStatusOptions.map((option) => (
                          <option key={option.value} value={option.value}>
                            {option.label}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                  {editError ? <p className="form-error">{editError}</p> : null}
                  <div className="goal-card__actions">
                    <button
                      className="goal-card__button goal-card__button--primary"
                      type="submit"
                      disabled={updateMutation.isPending}
                    >
                      Save goal
                    </button>
                    <button className="goal-card__button" type="button" onClick={() => setEditingGoalId(null)}>
                      Cancel edit
                    </button>
                  </div>
                </form>
              ) : (
                <>
                  <h2>{goal.title}</h2>
                  <strong>{summary.progressLabel}</strong>
                  <div className="progress-bar" aria-label={`${summary.progressLabel} progress`}>
                    <span style={{ width: `${progress}%` }} />
                  </div>
                  <GoalDetail goal={goal} milestones={milestones} linkedItems={linkedItems} />
                </>
              )}
            </article>
          );
        })}
      </div>
    </section>
  );
}

function goalToFormState(goal: Goal): GoalFormState {
  return {
    title: goal.title,
    description: goal.description ?? "",
    targetDate: goal.targetDate ?? "",
    status: goal.status,
  };
}

function toGoalInput(workspaceId: string, form: GoalFormState): GoalInput {
  const description = form.description.trim();
  const targetDate = form.targetDate.trim();

  return {
    workspaceId,
    title: form.title.trim(),
    description: description ? description : null,
    status: form.status,
    targetDate: targetDate ? targetDate : null,
  };
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}
