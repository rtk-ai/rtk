export type CommandAction =
  | { id: "open-dashboard"; label: "Open dashboard"; keywords: string[] }
  | { id: "create-item"; label: "Create item"; keywords: string[] }
  | { id: "open-board"; label: "Open board"; keywords: string[] }
  | { id: "open-goals"; label: "Open goals"; keywords: string[] }
  | { id: "log-focus"; label: "Log focus"; keywords: string[] };

export const commandActions: CommandAction[] = [
  { id: "open-dashboard", label: "Open dashboard", keywords: ["home", "overview", "summary"] },
  { id: "create-item", label: "Create item", keywords: ["new", "task", "content", "learning"] },
  { id: "open-board", label: "Open board", keywords: ["workspace", "kanban"] },
  { id: "open-goals", label: "Open goals", keywords: ["milestone", "progress"] },
  { id: "log-focus", label: "Log focus", keywords: ["time", "deep work"] },
];
