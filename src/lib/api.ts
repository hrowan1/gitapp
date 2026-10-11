import { invoke } from "@tauri-apps/api/core";

export type StateId =
  | "needs_review"
  | "needs_rereview"
  | "needs_changes"
  | "waiting"
  | "waiting_ci"
  | "later"
  | "done"
  | "replied"
  | "ready_to_merge"
  | "draft"
  | "fyi"
  | "stale"
  | "muted";

export interface Personal {
  state: string;
  note?: string | null;
  until?: string | null;
  setAt: string;
}

export interface Item {
  id: string;
  repo: string;
  number: number;
  title: string;
  url: string;
  author: string;
  authorAvatar?: string | null;
  isDraft: boolean;
  isMine: boolean;
  ci: "success" | "failure" | "pending" | "none";
  autoState: StateId;
  reason: string;
  since: string;
  updatedAt: string;
  personal?: Personal | null;
  state: StateId;
  reminderDue: boolean;
  isNew: boolean;
}

export interface Inbox {
  items: Item[];
  lastSync?: string | null;
  lastError?: string | null;
  syncing: boolean;
}

export interface Activity {
  at: string;
  kind: "reviewed" | "commented" | "opened" | "merged" | "closed" | "marked" | "update";
  prId?: string | null;
  repo?: string | null;
  number?: number | null;
  title?: string | null;
  url?: string | null;
  detail: string;
}

export interface RepoInfo {
  repo: string;
  openPrs: number;
  hidden: boolean;
}

export interface Settings {
  pollSeconds: number;
  notifications: boolean;
  ignoredUsers: string[];
  digest: boolean;
  digestHour: number;
}

export interface Session {
  login: string;
  avatarUrl: string;
}

export const api = {
  getSession: () => invoke<Session | null>("get_session"),
  signIn: (token: string) => invoke<Session>("sign_in", { token }),
  importGhToken: () => invoke<Session>("import_gh_token"),
  signOut: (deleteData: boolean) => invoke<void>("sign_out", { deleteData }),
  getInbox: () => invoke<Inbox>("get_inbox"),
  syncNow: () => invoke<void>("sync_now"),
  setState: (prIds: string[], state: StateId | null, until?: string | null, note?: string | null) =>
    invoke<void>("set_state", { prIds, state, until: until ?? null, note: note ?? null }),
  markSeen: (prIds: string[]) => invoke<void>("mark_seen", { prIds }),
  rerunChecks: (prId: string) => invoke<string>("rerun_checks", { prId }),
  getActivity: (days: number) => invoke<Activity[]>("get_activity", { days }),
  getRepos: () => invoke<RepoInfo[]>("get_repos"),
  setRepoHidden: (repo: string, hidden: boolean) => invoke<void>("set_repo_hidden", { repo, hidden }),
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (settings: Settings) => invoke<void>("update_settings", { settings }),
};
