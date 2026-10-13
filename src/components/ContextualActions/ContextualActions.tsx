export interface QuickAction {
  label: string;
  prompt: string;
}

export const QUICK_ACTIONS: QuickAction[] = [
  { label: "Explain", prompt: "Explain what this shows." },
  { label: "Summarize", prompt: "Summarize the content of this selection." },
  { label: "Solve", prompt: "Solve the problem shown, step by step." },
  { label: "Debug", prompt: "Find the bug or error shown here and explain how to fix it." },
  { label: "Describe", prompt: "Describe this image in detail." },
  { label: "Translate", prompt: "Translate all visible text into English. If it is already English, translate it into Spanish." },
];

interface Props {
  disabled: boolean;
  onAction: (prompt: string) => void;
}

export function ContextualActions({ disabled, onAction }: Props) {
  return (
    <div className="contextual-actions" role="group" aria-label="Quick actions">
      {QUICK_ACTIONS.map((a) => (
        <button key={a.label} type="button" disabled={disabled} onClick={() => onAction(a.prompt)}>
          {a.label}
        </button>
      ))}
    </div>
  );
}
