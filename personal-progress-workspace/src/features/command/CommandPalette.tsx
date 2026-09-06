import { Command } from "cmdk";
import { useEffect, useState } from "react";
import { commandActions, type CommandAction } from "./commandActions";

interface CommandPaletteProps {
  onAction: (action: CommandAction) => void;
}

export function CommandPalette({ onAction }: CommandPaletteProps) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen((value) => !value);
      }

      if (event.key === "Escape") {
        setOpen(false);
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  if (!open) return null;

  return (
    <div className="command-overlay" role="presentation" onMouseDown={() => setOpen(false)}>
      <Command className="command-menu" label="Command palette" onMouseDown={(event) => event.stopPropagation()}>
        <Command.Input placeholder="Run a command..." autoFocus />
        <Command.List>
          <Command.Empty>No command found.</Command.Empty>
          {commandActions.map((action) => (
            <Command.Item
              key={action.id}
              value={`${action.label} ${action.keywords.join(" ")}`}
              onSelect={() => {
                onAction(action);
                setOpen(false);
              }}
            >
              {action.label}
            </Command.Item>
          ))}
        </Command.List>
      </Command>
    </div>
  );
}
