import type { Goal, Milestone, WorkspaceItem } from "../../domain/types";

interface GoalDetailProps {
  goal: Goal;
  milestones: Milestone[];
  linkedItems: WorkspaceItem[];
}

export function GoalDetail({ goal, milestones, linkedItems }: GoalDetailProps) {
  return (
    <section className="goal-detail" aria-label={`${goal.title} details`}>
      <div>
        <h3>Milestones</h3>
        <div className="goal-detail__list">
          {milestones.length > 0 ? milestones.map((milestone) => <p key={milestone.id}>{milestone.title}</p>) : <p>No milestones.</p>}
        </div>
      </div>
      <div>
        <h3>Linked items</h3>
        <div className="goal-detail__list">
          {linkedItems.length > 0 ? linkedItems.map((item) => <p key={item.id}>{item.title}</p>) : <p>No linked items.</p>}
        </div>
      </div>
    </section>
  );
}
