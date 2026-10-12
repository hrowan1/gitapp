// Actions on PRs, shared by rows, groups and the command bar.

import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type Item, type StateId } from "./api";
import { STATES } from "./states";
import { toast } from "./toast";
import { untilLabel } from "./time";

export function openItem(item: Item) {
  api.markSeen([item.id]).catch(() => {});
  openUrl(item.url);
}

const plural = (n: number) => (n === 1 ? "1 item" : `${n} items`);

export async function setStates(
  items: Item[],
  state: StateId | null,
  opts: { until?: string | null; note?: string | null; quiet?: boolean } = {},
) {
  if (!items.length) return;
  try {
    await api.setState(
      items.map((i) => i.id),
      state,
      opts.until,
      opts.note,
    );
    if (opts.quiet) return;
    const what = items.length === 1 ? `“${items[0].title}”` : plural(items.length);
    if (state === null) toast(`${what} back to automatic`);
    else if (state === "later") {
      const ref = opts.note?.match(/[\w.-]+\/[\w.-]+#\d+|#\d+/)?.[0];
      const when = opts.until ? `until ${untilLabel(opts.until)}` : ref ? `until ${ref} merges or closes` : "until something changes";
      toast(`${what} parked ${when}`);
    }
    else toast(`${what} → ${STATES[state].emoji} ${STATES[state].label}`);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function unfollow(items: Item[]) {
  if (!items.length) return;
  try {
    await api.setFollowed(
      items.map((i) => i.id),
      false,
    );
    const what = items.length === 1 ? `“${items[0].title}”` : plural(items.length);
    toast(`Stopped following ${what} · undo in the Following tab`);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function rerunChecks(item: Item) {
  toast(`Re-running failed checks on #${item.number}…`);
  try {
    toast(await api.rerunChecks(item.id));
  } catch (e) {
    toast(String(e), true);
  }
}

export async function copyCheckout(item: Item) {
  const cmd = `gh pr checkout ${item.number} -R ${item.repo}`;
  try {
    await navigator.clipboard.writeText(cmd);
    toast(`Copied: ${cmd}`);
  } catch {
    toast(cmd);
  }
}
