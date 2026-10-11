// Tiny global toast channel so any component can confirm an action.

export interface Toast {
  id: number;
  text: string;
  error?: boolean;
}

type Listener = (t: Toast) => void;
const listeners = new Set<Listener>();
let next = 1;

export function toast(text: string, error = false) {
  const t = { id: next++, text, error };
  listeners.forEach((l) => l(t));
}

export function onToast(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}
