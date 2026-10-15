import type { StateId } from "../lib/api";
import { STATES, STATE_STYLE } from "../lib/states";
import { Icon } from "./Icon";

/** The state's icon in its tone colour. */
export function StateIcon({ state, size = 16 }: { state: StateId; size?: number }) {
  const s = STATE_STYLE[state] ?? STATE_STYLE.fyi;
  return (
    <span className={`state-icon tone-${s.tone}`} title={STATES[state]?.label}>
      <Icon name={s.icon} size={size} />
    </span>
  );
}

/** A small pill: icon + label, tinted by tone. */
export function StateChip({ state, label }: { state: StateId; label?: string }) {
  const s = STATE_STYLE[state] ?? STATE_STYLE.fyi;
  return (
    <span className={`chip-state tone-${s.tone}`}>
      <Icon name={s.icon} size={13} />
      {label ?? STATES[state]?.label}
    </span>
  );
}
