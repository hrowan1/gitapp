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
  | "muted"
  | "assigned"
  | "broken";

export type Kind = "pr" | "issue" | "run";

export interface Personal {
  state: string;
  note?: string | null;
  until?: string | null;
  setAt: string;
  /** "owner/repo#123": comes back when that PR/issue merges or closes. */
  waitsOn?: string | null;
}

export interface Item {
  id: string;
  kind: Kind;
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
  branch?: string | null;
  /** What the latest reply said, for "Replied to you". */
  context?: ReplyContext | null;
}

export interface ReplyContext {
  who: string;
  body: string;
  /** Reply goes into this review thread; otherwise it's a normal comment. */
  threadId?: string | null;
}

export interface MergeOptions {
  methods: ("SQUASH" | "MERGE" | "REBASE")[];
  default: string;
  mergeable: string;
  state: string;
}

export interface EditorSettings {
  editors: string[];
  editor?: string | null;
  roots: string[];
  clones: Record<string, string>;
  cloneDir: string;
}

export interface Inbox {
  items: Item[];
  lastSync?: string | null;
  lastError?: string | null;
  syncing: boolean;
}

export interface Activity {
  at: string;
  kind: "reviewed" | "commented" | "opened" | "merged" | "closed" | "marked" | "update" | "committed" | "pushed";
  prId?: string | null;
  repo?: string | null;
  number?: number | null;
  title?: string | null;
  url?: string | null;
  detail: string;
  isMine: boolean;
  itemKind?: Kind | null;
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
  trackLocal: boolean;
  /** Days to keep meeting audio: 0 forever, -1 delete once transcribed. */
  audioRetentionDays: number;
}

export interface RecordingStatus {
  recording: boolean;
  meetingId?: string | null;
  startedAt?: string | null;
}

export interface Meeting {
  id: string;
  title: string;
  startedAt: string;
  endedAt?: string | null;
  seconds?: number | null;
  /** recording | transcribing | done | failed */
  status: string;
  error?: string | null;
  ticket?: string | null;
  hasAudio: boolean;
  audioDir?: string | null;
  sources?: string | null;
  preview?: string | null;
}

export interface Segment {
  start: number;
  end: number;
  speaker: "you" | "others";
  text: string;
}

export interface TimeBlock {
  id?: number | null;
  /** "T:IO-2641" (ticket), "PR:owner/repo#12", "B:repo:branch", "R:repo", or "M:…" (added by hand) */
  key: string;
  label: string;
  start: string;
  end: string;
  minutes: number;
  note?: string | null;
}

export interface DayEvent {
  at: string;
  kind: string;
  text: string;
  url?: string | null;
  repo: string;
  branch?: string | null;
}

export interface DayGroup {
  key: string;
  label: string;
  url?: string | null;
  /** Jira status, for known tickets. */
  status?: string | null;
  minutes: number;
  events: DayEvent[];
}

export interface DayReport {
  day: string;
  frozen: boolean;
  totalMinutes: number;
  blocks: TimeBlock[];
  groups: DayGroup[];
  /** Every movement that day on Jira tickets that involve you. */
  ticketMoves: TicketMove[];
  /** Meetings recorded that day. */
  meetings: Meeting[];
}

export interface TicketMove {
  key: string;
  summary: string;
  /** The ticket's status now. */
  status: string;
  category: string;
  url: string;
  at: string;
  /** status | assignee | resolution | comment | created */
  kind: string;
  detail: string;
  actor?: string | null;
  actorIsMe: boolean;
  /** An assignee change that gave the ticket to you. */
  toMe: boolean;
}

export interface FollowItem {
  id: string;
  kind: Kind;
  repo: string;
  number: number;
  title: string;
  author: string;
  isMine: boolean;
  state: StateId;
  updatedAt: string;
  followed: boolean;
}

export interface JiraStatus {
  connected: boolean;
  site?: string | null;
  email?: string | null;
  name?: string | null;
  lastSync?: string | null;
  error?: string | null;
  tickets: number;
}

export interface Ticket {
  key: string;
  summary: string;
  status: string;
  /** new | indeterminate | done */
  category: string;
  issueType: string;
  priority?: string | null;
  project: string;
  assignee?: string | null;
  assignedToMe: boolean;
  /** How it involves you: assigned | was_assigned | reported | updated */
  involvement: string;
  url: string;
  updated?: string | null;
  resolved?: string | null;
  lastMove?: string | null;
  lastMoveAt?: string | null;
}

export interface TicketEvent {
  at: string;
  /** status | assignee | resolution | comment | created */
  kind: string;
  detail: string;
  actor?: string | null;
  actorIsMe: boolean;
  toMe: boolean;
  fromMe: boolean;
}

export interface TicketDetail {
  events: TicketEvent[];
  prs: { id: string; repo: string; number: number; title: string; url: string; open: boolean }[];
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
  testNotification: () => invoke<void>("test_notification"),
  submitReview: (prId: string, event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body?: string) =>
    invoke<string>("submit_review", { prId, event, body: body ?? null }),
  sendComment: (itemId: string, body: string, threadId?: string | null) =>
    invoke<string>("send_comment", { itemId, body, threadId: threadId ?? null }),
  mergeOptions: (prId: string) => invoke<MergeOptions>("merge_options", { prId }),
  mergePr: (prId: string, method: string) => invoke<string>("merge_pr", { prId, method }),
  editorSettings: () => invoke<EditorSettings>("editor_settings"),
  updateEditorSettings: (editor: string | null, roots: string[], cloneDir: string) =>
    invoke<number>("update_editor_settings", { editor, roots, cloneDir }),
  setRepoPath: (repo: string, path: string | null) => invoke<void>("set_repo_path", { repo, path }),
  openLocally: (itemId: string, checkout: boolean) => invoke<string>("open_locally", { itemId, checkout }),
  cloneAndOpen: (itemId: string, checkout: boolean) => invoke<string>("clone_and_open", { itemId, checkout }),
  getDay: (day: string, start: string, end: string) => invoke<DayReport>("get_day", { day, start, end }),
  saveDay: (day: string, blocks: TimeBlock[]) => invoke<void>("save_day", { day, blocks }),
  resetDay: (day: string) => invoke<void>("reset_day", { day }),
  jiraStatus: () => invoke<JiraStatus>("jira_status"),
  jiraConnect: (site: string, email: string, token: string) =>
    invoke<JiraStatus>("jira_connect", { site, email, token }),
  jiraDisconnect: () => invoke<void>("jira_disconnect"),
  jiraSyncNow: () => invoke<void>("jira_sync_now"),
  getTickets: () => invoke<Ticket[]>("get_tickets"),
  getTicket: (key: string) => invoke<TicketDetail>("get_ticket", { key }),
  recordingStatus: () => invoke<RecordingStatus>("recording_status"),
  startRecording: (title?: string) => invoke<RecordingStatus>("start_recording", { title: title ?? null }),
  stopRecording: () => invoke<void>("stop_recording"),
  listMeetings: () => invoke<Meeting[]>("list_meetings"),
  getMeeting: (id: string) => invoke<{ meeting: Meeting; segments: Segment[] }>("get_meeting", { id }),
  updateMeeting: (id: string, title: string, ticket: string | null) => invoke<void>("update_meeting", { id, title, ticket }),
  deleteMeeting: (id: string) => invoke<void>("delete_meeting", { id }),
  retranscribeMeeting: (id: string) => invoke<void>("retranscribe_meeting", { id }),
  getFollowing: () => invoke<FollowItem[]>("get_following"),
  setFollowed: (ids: string[], followed: boolean) => invoke<void>("set_followed", { ids, followed }),
  getActivity: (days: number) => invoke<Activity[]>("get_activity", { days }),
  getItemActivity: (itemId: string) => invoke<Activity[]>("get_item_activity", { itemId }),
  getRepos: () => invoke<RepoInfo[]>("get_repos"),
  setRepoHidden: (repo: string, hidden: boolean) => invoke<void>("set_repo_hidden", { repo, hidden }),
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (settings: Settings) => invoke<void>("update_settings", { settings }),
};
