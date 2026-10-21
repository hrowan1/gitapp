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
  /** Who writes meeting summaries: apple (on this Mac) | claude | off */
  summaryProvider: string;
  /** Which Claude model writes them, when it's Claude (default Haiku 5.5). */
  claudeModel: string;
  /** The other AI features: apple | claude | off, and their Claude model. */
  aiProvider: string;
  aiModel: string;
  /** Show what's being said while recording. */
  liveCaptions: boolean;
  /** Tell the voices on the call apart after recording (on this Mac). */
  separateSpeakers: boolean;
  /** Remember voices you name and recognise them in later meetings. */
  recogniseVoices: boolean;
  /** Show the number of things that need you in the menu bar (otherwise just a dot on the icon). */
  menuBarCount: boolean;
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
  /** AI notes, once made. */
  summary?: MeetingSummary | null;
  /** apple | claude */
  summaryBy?: string | null;
  /** summarising | done | failed */
  summaryStatus?: string | null;
  summaryError?: string | null;
  /** A ticket the AI thinks this unlinked meeting is about. */
  suggestedTicket?: string | null;
  suggestedTitle?: string | null;
}

export interface MeetingSummary {
  overview: string;
  decisions: string[];
  actionItems: { owner: "you" | "others" | "unclear"; text: string }[];
}

/** An action item from a meeting's notes, with your progress on it. */
export interface MeetingAction {
  id: number;
  meetingId: string;
  meetingTitle: string;
  /** When the meeting started. */
  meetingAt: string;
  ticket?: string | null;
  owner: "you" | "others" | "unclear";
  text: string;
  done: boolean;
  snoozedUntil?: string | null;
}

export interface SummaryStatus {
  /** apple | claude | off */
  provider: string;
  appleReady: boolean;
  appleReason?: string | null;
  claudeKey: boolean;
  /** The engine the other AI features will actually use. */
  aiProvider: string;
  /** The Claude models on offer. */
  claudeModels: { id: string; name: string }[];
}

export interface Segment {
  start: number;
  end: number;
  speaker: "you" | "others";
  text: string;
  /** For "others": which voice on the call ("1", "2"…), when they could be told apart. */
  voice?: string | null;
}

/** A move a ticket can make in Jira. */
export interface JiraTransition {
  id: string;
  /** The transition's name ("Start progress"). */
  name: string;
  /** The status it moves to ("In Progress"). */
  to: string;
  /** new | indeterminate | done */
  category: string;
}

/** Someone a ticket can be assigned to. */
export interface JiraUser {
  accountId: string;
  name: string;
  avatar?: string | null;
}

/** Why a PR's checks failed, in words. */
export interface CiExplanation {
  text: string;
  /** "apple" or the Claude model id. */
  by: string;
  /** The failed checks looked at: [name, link]. */
  checks: [string, string | null][];
}

/** An AI-written summary of a day or a week. */
export interface AiSummary {
  text: string;
  /** "apple" or the Claude model id. */
  by: string;
  createdAt: string;
}

/** Something an answer can cite. */
export interface AskSource {
  /** "S1", "S2"… as cited in the answer */
  id: string;
  /** pr | ticket | meeting */
  kind: string;
  label: string;
  url?: string | null;
  /** A ticket key or meeting id to open in the app. */
  target?: string | null;
  /** For a meeting: seconds into it. */
  at?: number | null;
}

export interface AskAnswer {
  /** Text with [S1]-style citations. */
  answer: string;
  sources: AskSource[];
  /** "apple" or the Claude model id. */
  by: string;
  /** The period the facts came from, in words. */
  period: string;
}

/** Someone whose voice the app recognises (you named them in a meeting). */
export interface KnownVoice {
  id: number;
  name: string;
  /** How many meetings you've named them in. */
  samples: number;
  updatedAt: string;
}

/** A meeting that matches a search, with its best matching lines. */
export interface MeetingSearchResult {
  meetingId: string;
  title: string;
  startedAt: string;
  ticket?: string | null;
  hits: {
    /** title | line | notes */
    kind: string;
    start?: number | null;
    speaker: string;
    /** Matches are wrapped in \u0002 … \u0003. */
    snippet: string;
  }[];
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

/** A Jira ticket where someone else did something since you last looked. */
export interface TicketAttention {
  key: string;
  summary: string;
  status: string;
  category: string;
  url: string;
  at: string;
  /** assignee | mention | comment | status | resolution */
  kind: string;
  /** "Sam assigned it to you", "Sam moved it In QA → In Progress", "Sam mentioned you: …" */
  reason: string;
  count: number;
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
  /** Plain text: paragraphs separated by blank lines, list items as "• " lines. */
  description?: string | null;
  /** Meetings linked to this ticket in the app (not in Jira), newest first, and their action items. */
  meetings: Meeting[];
  actions: MeetingAction[];
  /** Recent meetings with no ticket, to link one. */
  linkable: Meeting[];
  events: TicketEvent[];
  prs: { id: string; repo: string; number: number; title: string; url: string; open: boolean }[];
}

export interface CalendarInfo {
  id: string;
  title: string;
  source: string;
  color: string;
}

export interface CalendarStatus {
  /** full | notDetermined | denied | restricted | writeOnly */
  access: string;
  enabled: boolean;
  calendars: CalendarInfo[];
  /** Calendar ids to read; empty means all. */
  selected: string[];
  /** Minutes before a meeting to remind you; 0 is off. */
  remind: number;
  countTime: boolean;
  lastSync: string | null;
  error: string | null;
}

export interface CalendarEvent {
  uid: string;
  calendarId: string;
  calendar: string;
  title: string;
  start: string;
  end: string;
  allDay: boolean;
  location: string;
  notes: string;
  videoUrl: string | null;
  attendees: number;
  attendeeNames: string[];
  organiser: string | null;
  myStatus: string;
  free: boolean;
}

export interface MeetingBrief {
  event: CalendarEvent;
  ticket: {
    key: string;
    summary: string | null;
    status: string | null;
    category: string | null;
    url: string | null;
    prs: { id: string; repo: string; number: number; title: string; url: string; open: boolean }[];
  } | null;
  /** The last meeting in the series you recorded, with its notes. */
  last: Meeting | null;
  openActions: MeetingAction[];
}

export interface UpdateStatus {
  current: string;
  /** A newer version on GitHub. */
  available: string | null;
  notes: string | null;
  /** Installed on disk, waiting for a restart. */
  installed: string | null;
  busy: boolean;
  lastCheck: string | null;
  error: string | null;
  automatic: boolean;
  /** A built app that can replace itself (not `tauri dev`). */
  canInstall: boolean;
}

export interface HarvestStatus {
  connected: boolean;
  name: string | null;
  accountId: string | null;
  /** Entries are linked to Jira the same way Harvest's own Jira integration does. */
  linkLearnt: boolean;
  /** "HH:MM", or null when the end-of-day reminder is off. */
  nudge: string | null;
}

export interface HarvestProject {
  id: number;
  name: string;
  code: string | null;
  client: string | null;
  tasks: { id: number; name: string }[];
}

export interface HarvestBucket {
  projectId: number;
  projectName: string;
  taskId: number;
  taskName: string;
  /** "ticket" | "project" | "default" | "rollover" */
  from: string;
}

export interface TimesheetCell {
  day: string;
  proposed: number;
  /** Already in Harvest, logged some other way. */
  logged: number;
  minutes: number;
  edited: boolean;
  submitted: boolean;
}

export interface TimesheetRow {
  /** A Jira key, or "__other". */
  key: string;
  label: string;
  bucket: HarvestBucket | null;
  cells: TimesheetCell[];
}

export interface TimesheetWeek {
  days: string[];
  rows: TimesheetRow[];
  harvest: boolean;
  harvestError: string | null;
}

export interface SubmitResult {
  created: number;
  updated: number;
  removed: number;
  skipped: string[];
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
  testNotification: () => invoke<string>("test_notification"),
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
  getTicketAttention: () => invoke<TicketAttention[]>("get_ticket_attention"),
  markTicketSeen: (key: string) => invoke<void>("mark_ticket_seen", { key }),
  recordingStatus: () => invoke<RecordingStatus>("recording_status"),
  startRecording: (title?: string) => invoke<RecordingStatus>("start_recording", { title: title ?? null }),
  stopRecording: () => invoke<void>("stop_recording"),
  listMeetings: () => invoke<Meeting[]>("list_meetings"),
  getMeeting: (id: string) =>
    invoke<{
      meeting: Meeting;
      segments: Segment[];
      actions: MeetingAction[];
      speakers: Record<string, string>;
      /** Voices named automatically from someone you named before. */
      recognised: string[];
    }>(
      "get_meeting",
      { id },
    ),
  searchMeetings: (query: string) => invoke<MeetingSearchResult[]>("search_meetings", { query }),
  linkMeeting: (id: string, ticket: string | null) => invoke<void>("link_meeting", { id, ticket }),
  dismissSuggestion: (id: string) => invoke<void>("dismiss_suggestion", { id }),
  jiraTransitions: (key: string) => invoke<JiraTransition[]>("jira_transitions", { key }),
  jiraMove: (key: string, transitionId: string) => invoke<void>("jira_move", { key, transitionId }),
  jiraAssignable: (key: string, query: string) => invoke<JiraUser[]>("jira_assignable", { key, query }),
  /** accountId "me" assigns it to you; null unassigns. */
  jiraAssign: (key: string, accountId: string | null) => invoke<void>("jira_assign", { key, accountId }),
  knownVoices: () => invoke<KnownVoice[]>("known_voices"),
  askHistory: (question: string) => invoke<AskAnswer>("ask_history", { question }),
  explainCi: (prId: string) => invoke<CiExplanation>("explain_ci", { prId }),
  getAiSummary: (kind: "day" | "week", day: string) => invoke<AiSummary | null>("get_ai_summary", { kind, day }),
  writeAiSummary: (kind: "day" | "week", day: string) => invoke<AiSummary>("write_ai_summary", { kind, day }),
  forgetVoice: (id: number) => invoke<void>("forget_voice", { id }),
  nameSpeaker: (id: string, voice: string, name: string) => invoke<void>("name_speaker", { id, voice, name }),
  getMeetingActions: () => invoke<MeetingAction[]>("get_meeting_actions"),
  setActionDone: (id: number, done: boolean) => invoke<void>("set_action_done", { id, done }),
  snoozeAction: (id: number, until: string | null) => invoke<void>("snooze_action", { id, until }),
  updateMeeting: (id: string, title: string, ticket: string | null) => invoke<void>("update_meeting", { id, title, ticket }),
  deleteMeeting: (id: string) => invoke<void>("delete_meeting", { id }),
  retranscribeMeeting: (id: string) => invoke<void>("retranscribe_meeting", { id }),
  summariseMeeting: (id: string) => invoke<void>("summarise_meeting_now", { id }),
  summaryStatus: () => invoke<SummaryStatus>("summary_status"),
  setAnthropicKey: (key: string) => invoke<void>("set_anthropic_key", { key }),
  clearAnthropicKey: () => invoke<void>("clear_anthropic_key"),
  getFollowing: () => invoke<FollowItem[]>("get_following"),
  setFollowed: (ids: string[], followed: boolean) => invoke<void>("set_followed", { ids, followed }),
  getActivity: (days: number) => invoke<Activity[]>("get_activity", { days }),
  getItemActivity: (itemId: string) => invoke<Activity[]>("get_item_activity", { itemId }),
  getRepos: () => invoke<RepoInfo[]>("get_repos"),
  setRepoHidden: (repo: string, hidden: boolean) => invoke<void>("set_repo_hidden", { repo, hidden }),
  calendarStatus: () => invoke<CalendarStatus>("calendar_status"),
  calendarConnect: () => invoke<CalendarStatus>("calendar_connect"),
  calendarDisconnect: () => invoke<void>("calendar_disconnect"),
  calendarSave: (selected: string[], remind: number, countTime: boolean) =>
    invoke<CalendarStatus>("calendar_save", { selected, remind, countTime }),
  calendarUpcoming: () => invoke<MeetingBrief[]>("calendar_upcoming"),
  calendarBrief: (uid: string) => invoke<MeetingBrief | null>("calendar_brief", { uid }),
  calendarSyncNow: () => invoke<CalendarStatus>("calendar_sync_now"),
  calendarRecord: (uid: string) => invoke<RecordingStatus>("calendar_record", { uid }),
  setupNeeded: () => invoke<boolean>("setup_needed"),
  finishSetup: () => invoke<void>("finish_setup"),
  updateStatus: () => invoke<UpdateStatus>("update_status"),
  /** install: download and install even when automatic updates are off. */
  checkForUpdates: (install: boolean) => invoke<UpdateStatus>("check_for_updates", { install }),
  setAutoUpdate: (on: boolean) => invoke<void>("set_auto_update", { on }),
  restartToUpdate: () => invoke<void>("restart_to_update"),
  harvestStatus: () => invoke<HarvestStatus>("harvest_status"),
  harvestConnect: (accountId: string, token: string) => invoke<HarvestStatus>("harvest_connect", { accountId, token }),
  harvestDisconnect: () => invoke<void>("harvest_disconnect"),
  harvestProjects: () => invoke<HarvestProject[]>("harvest_projects"),
  setTimesheetNudge: (at: string | null) => invoke<void>("set_timesheet_nudge", { at }),
  timesheetWeek: (day: string) => invoke<TimesheetWeek>("timesheet_week", { day }),
  /** null goes back to the Day view's estimate. */
  timesheetSet: (day: string, row: string, minutes: number | null) => invoke<void>("timesheet_set", { day, row, minutes }),
  timesheetBucket: (row: string, projectId: number, projectName: string, taskId: number, taskName: string) =>
    invoke<void>("timesheet_bucket", { row, projectId, projectName, taskId, taskName }),
  timesheetSubmit: (days: string[]) => invoke<SubmitResult>("timesheet_submit", { days }),
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (settings: Settings) => invoke<void>("update_settings", { settings }),
};
