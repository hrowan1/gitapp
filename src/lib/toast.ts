// Tiny global toast channel so any component can confirm an action.

export interface Toast {
  id: number;
  text: string;
  error?: boolean;
  action?: { label: string; run: () => void };
}

type Listener = (t: Toast) => void;
const listeners = new Set<Listener>();
let next = 1;

export function toast(text: string, error = false, action?: Toast["action"]) {
  const t = { id: next++, text, error, action };
  listeners.forEach((l) => l(t));
}

export function onToast(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}
