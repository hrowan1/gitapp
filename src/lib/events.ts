// Small in-app signals between distant components (e.g. the list asking the detail pane
// to focus its snooze box when you press `l`).

type Signal = "focus-snooze";
const listeners = new Map<Signal, Set<() => void>>();

export function emit(signal: Signal) {
  listeners.get(signal)?.forEach((l) => l());
}

export function on(signal: Signal, l: () => void): () => void {
  if (!listeners.has(signal)) listeners.set(signal, new Set());
  listeners.get(signal)!.add(l);
  return () => listeners.get(signal)!.delete(l);
}
