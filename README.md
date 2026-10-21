# Dev Inbox

Your engineering inbox: everything that needs your attention across every GitHub repository, plus a personal memory of what you've done and what you meant to come back to. See [VISION.md](VISION.md) for the product idea.

A Tauri 2 desktop app (Rust + React/TypeScript) for macOS. It is **local-first**: no backend; your tokens live in the macOS keychain and PR metadata (never source code) sits in a local SQLite database.

## Install

Needs an Apple Silicon Mac with macOS 14.2 or later (meeting transcription and Apple Intelligence features need macOS 26).

**One line** (recommended — installs into Applications and opens it):

```bash
curl -fsSL https://raw.githubusercontent.com/hrowan1/gitapp/main/install.sh | bash
```

**Or download** [Dev-Inbox-macOS-arm64.dmg](https://github.com/hrowan1/gitapp/releases/latest/download/Dev-Inbox-macOS-arm64.dmg) and drag Dev Inbox into Applications. Dev Inbox isn't notarised by Apple yet, so the first time macOS says it can't check it: open **System Settings → Privacy & Security**, scroll down and click **Open Anyway** (once). Or, in Terminal: `xattr -dr com.apple.quarantine "/Applications/Dev Inbox.app"`.

**Updates** are automatic: Dev Inbox checks GitHub a minute after it starts and every 6 hours, downloads a new version in the background (verified against the release signing key), and installs it — it opens the next time Dev Inbox starts, or click **Restart to update** in the sidebar. Settings → Updates shows the version, checks now, or turns automatic installs off.

## Run it

```bash
npm install
npm run tauri dev
```

Sign in with **Use my GitHub CLI login** (reuses `gh auth login`) or paste a classic token with `repo` + `read:org` scopes.

Demo data without a GitHub account (separate database, never syncs):

```bash
DEV_INBOX_DEMO=1 npm run tauri dev
```

Build a local `.app`/`.dmg`: `npm run tauri build` (needs the updater key in `TAURI_SIGNING_PRIVATE_KEY`, or pass `--config '{"bundle":{"createUpdaterArtifacts":false}}'`).

## Releasing

From an up-to-date `main` on an Apple Silicon Mac with Xcode:

```bash
npm run release -- patch        # or minor, major, or an exact version like 0.3.0
```

It bumps the version, runs the tests, builds and signs the app, checks the signature, then — after you confirm — commits "Release vX", tags it, pushes, and creates the GitHub release with the `.dmg`, the update archive, its signature and `latest.json` (the feed installed apps read). `npm run release -- patch --dry` does everything except publish. Release notes are the commit subjects since the last tag.

Two keys live in `~/.tauri` (never in the repo) — **back them both up**:

- `dev-inbox-updater.key` signs updates; apps only install updates signed with it (its public half is in `tauri.conf.json`). Lose it and installed copies can't update — everyone would have to reinstall.
- `dev-inbox-signing.*` is the self-signed "Dev Inbox Release" code-signing certificate. Run `npm run release:cert` once to create it (or restore it from the backup on a new Mac). Signing every release with it keeps the app's identity stable, so macOS keeps its microphone, audio-capture and keychain permissions across updates; without it releases are signed ad-hoc and those get asked for again after each update.

Moving to an Apple Developer ID later (notarised, no "Open Anyway") only changes the signing step; installed copies keep updating.


## What it does

- **Today** and **Inbox** are one list each, organised by what to do next — Reminders · Replies to you · Review requested · Fix (changes requested or failing) · Ready to merge · Assigned to you — then Waiting on others, Parked, Gone quiet and Done & FYI. Today shows what needs you (plus Jira and meeting action items) with the rest folded away; Inbox has everything, with search and a repository filter. No repo folders: long sections show 8 and "Show more". Click a row to open its details and actions right under it.
- **PRs, issues and branches**: PRs and issues you're involved in, plus the default branch of repos you're active in when its CI goes red (🔴, or 🔧 if your commit broke it; it clears itself when green).
- **Personal states**, derived automatically from GitHub, overridable by hand:
  - review requested → 👀 Need to review
  - you requested changes, then new commits landed → 🔄 Needs re-review
  - a reply in a review thread you're in, an @mention, or the author answering you → 💬 Replied to you (other people's chatter doesn't count)
  - an issue assigned to you that you haven't touched → 📋 Assigned to you
  - your PR: failing CI / merge conflicts / changes requested → 🔧 I need to make changes
  - your PR approved and green → 🚀 Ready to merge
  - you requested changes and nothing happened yet → ⏳ Waiting for the author
  - no activity for 30+ days → 🕸️ Gone quiet (kept out of "Needs you")
- **Memory**: manual states stick until something *new* happens on the PR, then the PR resurfaces automatically. "Come back later" takes a note and a time, and notifies you when it's due.
  - **Waits on other work**: a note that references a PR/issue (`#392`, `owner/repo#12`, a link) — or names one by title ("after the API migration") — brings the item back when that PR merges or closes.
  - **Reads your comments**: write "let's come back to this after #392" or "I'll take another look next week" on GitHub and the reminder is created for you.
- **History** (Time → History): a timeline of your reviews, comments, opened/merged PRs and the states you set.
- **New since you last looked**: a dot on things that need you and arrived after you last opened them; "mark seen" clears them all.
- **Following** (Settings → Following): everything is followed by default; untick any PR, issue, branch or whole repo to stop hearing about it (gone from Today, counts and notifications). Filter, then follow/unfollow everything shown in one go.
- **Noise control**: **Ignore people & bots** (Settings): their comments don't count as replies and their PRs go to FYI.
- **⌘K command bar**:
  - Questions: "what do I need to review?", "what's new since I last looked?", "show me Sarah's PRs", "what have I forgotten about?", "what did I work on yesterday?"
  - Actions: "remind me about the checkout PR tomorrow", "come back to auth middleware after the API migration", "mark sarah's checkout pr as done", "approve sarah's checkout pr", "reply to priya", "merge export reports", "check out the checkout refactor", "open web in my editor", "stop following …", "rerun checks on billing"
  - `Tab` on any PR lists everything you can do to it.
- **Act from the app** (each asks first, then posts to GitHub as you):
  - **Reply**: "Replied to you" rows quote what was said; reply goes into the right review thread, or as a comment.
  - **Approve / request changes / comment**, and **merge** using a method your repo allows.
  - **Open in editor**: finds your clones by scanning your home folder (or folders you choose) for GitHub remotes and opens the repo in your editor (VS Code, Cursor, Zed, JetBrains, Xcode… whatever's installed). **Check out & open** switches to the PR branch first, **only if the working tree is clean**. Not cloned? It offers to clone it (via `gh`) into `~/code`.
  - Re-run failed GitHub Actions workflows; copy `gh pr checkout`.
- **Jira in Today**: tickets where someone else did something since you last looked — assigned it to you, @mentioned you, commented on a ticket that's yours (or you reported), or moved one of yours (e.g. QA sent it back) — with a desktop notification for each (a burst becomes one). Opening the ticket, or ✓, clears it; so does doing something on it yourself. ⌘K finds tickets by key or title.
- **Tickets** (`4`, Jira Cloud): the two things you can change in Jira from here are a ticket's **status** (click it: the moves its workflow allows from where it is) and its **assignee** (click it: assign to me, find someone, or unassign); the ticket refreshes straight after. Everything else is read-only: every ticket that involves you — assigned to you now or before, reported by you, or moved, edited or commented on by you (last 30 days on first connect), its current status and description (collapsible), how it moved (status, assignment, resolution, your comments — who and when), and the PRs that mention its key. Click a movement to open that day. Connect in Settings with your site, Atlassian email and an API token (stored in the keychain). Apart from status and assignee changes you make, nothing is written to Jira.
- **Meetings** (`5`, macOS 14.2+): press **Record** to capture your microphone *and* the meeting's audio (Zoom, Meet, Teams — anything playing on the Mac) as two tracks, then **Stop** for an on-device transcript (macOS 26 Speech framework; nothing is uploaded). Lines are labelled *You* / *Others* by track, and echoes of the call in your mic are dropped. Find in the transcript, click a line to play from there, copy it, and link the meeting to a ticket (guessed from the title) so it shows as an exact block in the Day view. Recording only ever starts when you press Record (or "Start recording" in ⌘K); the sidebar shows **● REC** throughout. Audio is kept 30 days by default (Settings: 7/30/90 days, forever, or delete once transcribed); transcripts are kept. *You're responsible for telling people you're recording* — there's a one-click notice to paste into the chat.
  - **AI notes**: after each transcript, an overview, decisions and action items (yours vs. others') are written automatically — by **Apple Intelligence on this Mac** (default; nothing leaves the Mac, needs macOS 26 with Apple Intelligence on; long meetings are summarised in parts) or by **Claude** (with your own Anthropic API key, stored in the keychain; the transcript is sent to Anthropic), or **off** — Settings → Meetings. Until you choose, it's Claude when a key is saved and Apple Intelligence otherwise; Claude without a key falls back to Apple Intelligence. Claude model: **Haiku 5.5** by default (about $0.002 for a 30-minute meeting), or Sonnet 5.5 / Opus 5.5. Notes show above the transcript (copy, rewrite) and in the Meetings list and Day view.
  - **Search** (box at the top of Meetings, or ⌘K → "Search meetings for …"): every title, transcript line and note, prefix-matched and ranked, with highlighted snippets; click a line to open the meeting scrolled to it.
  - **Live captions** while recording (Settings → Meetings, on by default): what's being said, as You / Others, from Apple's live speech recognition. The full transcript is still made from the recording afterwards.
  - **Speakers**: after recording, the call audio is split by voice on this Mac ([FluidAudio](https://github.com/FluidInference/FluidAudio), pinned 0.17.5; ~20 MB of models downloaded once into the app's folder), so lines read Speaker 1, Speaker 2… Click a speaker to name them; names carry into notes, search and copies. Once named, a person's voice fingerprint is kept on this Mac and **recognised in later meetings** (marked ✓; a voice is only named when it clearly matches one person — measured: same person 0.83–0.93 similarity, different people ≤ 0.37, threshold 0.62). Settings → Meetings lists the voices it knows (forget any of them) and can turn either off.
  - **Suggested ticket**: when a meeting's notes are written and it isn't linked to a ticket, the AI (Settings → AI) picks the one open ticket it was clearly about — from your Jira tickets and ticket keys in open PRs — and the meeting shows "Looks like BILL-142 · Link · Not this". It answers "none" for standups and anything unclear, and never links by itself.
  - **Meetings on tickets**: a ticket's page lists the meetings linked to it (via "Counts towards" or "Link a recent meeting…"), with their notes and your action items. Links live only in Dev Inbox — nothing is added to Jira.
  - **Your action items in Today**: the ones the notes give to *you* show under "From your meetings" — tick them off (undo), snooze them (3 hours, tomorrow, next Monday), or open the meeting. They have checkboxes in the notes too, and rewriting the notes keeps what you've ticked.
- **Ask your history** (⌘K — type a question, e.g. "what did I do on BILL-142 this week?", "what did Sam say about VAT?", "what's still open from Tuesday?"): the app gathers the facts itself — tickets you name, keyword matches across meetings, PRs and tickets, and the period you mention (default the last two weeks): meetings with notes and action items, ticket moves, PR activity, what's open now — and the model answers from those alone, citing them; each citation opens the PR, ticket or meeting (at the line). Settings → **AI** picks Apple Intelligence (on this Mac) or Claude (Haiku by default) for this and the other AI features, separately from meeting notes; the Anthropic key is entered once there and used by both.
- **AI day summary & week review** (Day view): "Summarise this day" writes a few bullets — what you worked on and for how long, what moved, what meetings decided — and keeps them (rewrite any time); "Week review" gives Shipped / Time / Decided / Still open for the week. Both use Settings → AI.
- **Smarter reminders**: your own comments the reminder rules don't recognise but that might be a promise ("I'll look at this once Sam's PR is in", "will check tomorrow") are read once by the AI after each sync; real ones park the PR until then (default 3 days) with the same note and notification as rule-based reminders.
- **CI failure explainer** (a PR with failing checks → "Explain the failure"): fetches the failed checks on the latest commit — failing step, annotations, and for GitHub Actions the job log cut down to the error lines with what led up to them plus the end — and the AI says what broke, where, the likely cause and fix. Other CI providers get the check's own output.
- **Calendar** (Settings → Connections): your meetings from the Mac's Calendar app — whatever it syncs (Google, Outlook/Exchange, iCloud), so there's nothing to sign in to; macOS asks once for access, and it's read-only. Only real meetings are used: events with other people or a call link that you haven't declined (not all-day events, "Lunch" or focus blocks), from the calendars you choose.
  - **In your day**: meetings count in the Day view and timesheet even if you didn't record them — under their ticket when the title has a key ("BILL-142 sync"), otherwise as a meeting (Other work on the timesheet). Only the part that has happened counts, and a meeting you recorded counts once.
  - **Reminders**: 5 minutes before a meeting (1–15, or off) a notification shows its ticket and status, open PRs or what's still open from last time, with **Record** and **Join call** buttons. Recording is still only ever started by you.
  - **Up next & briefs** (Meetings): today's and tomorrow's meetings above your recordings; click one for its brief — the ticket and its open PRs, the last meeting in the series (same ticket or title) with its notes and open action items, invite notes, Record and Join. Recording during a calendar meeting names the recording after it and links its ticket.
- **First-run setup**: after signing in, a short guide connects Jira, Harvest and the calendar, sends a test notification and sets up AI — each step skippable; run it again from Settings → General.
- **Settings** are grouped: General, Connections (GitHub, Jira, Harvest, Calendar), Time & timesheet, Meetings, AI, Repositories, Following.
- **Day view** (`3` or `s`; Day, Timesheet and History are tabs under **Time**): pick any date to see what you worked on and for how long, the meetings you recorded (click for the transcript), and every **ticket move** that day on tickets that involve you — yours and other people's — a timeline of blocks grouped by **ticket** (Jira keys from branch names, PR titles or commit messages; otherwise PR, branch or repo), a week strip with daily totals, and per-ticket breakdowns. Time is estimated from your PR commits (authored time), pushes (captured every sync), reviews, comments, your own Jira moves and comments (tickets moved by others are shown but add no time), local `git log`, and — optionally — local editing activity sampled every 5 minutes from your clones (repo, branch and time only; never leaves your Mac). Edit, add or delete blocks (a day you edit stops auto-updating; reset any time), and copy a readable summary. Blocks are shaped like Jira worklogs, ready for a Jira integration.
- **Timesheet** (`t`, Harvest): the week as a grid — one row per ticket, plus **Other work** (meetings, reviews, anything not on a ticket) — filled from the Day view and rounded to the nearest 15 minutes. Type over any cell (`1.5`, `1:30`, `90m`; ↺ goes back to the estimate). Each row gets a Harvest **project and task**: choose it once and it's remembered for that ticket and its whole Jira project (Other work uses a default you choose); monthly projects like "Project Hair September" **roll over** to that month's project by themselves. **Submit** a day or the whole week: one entry per ticket per day, with "KEY Summary" as the notes and linked to the Jira ticket the same way Harvest's own Jira button links them (learnt from your existing entries when you connect). Time already logged for a ticket some other way (e.g. from Jira) is subtracted, not doubled. Resubmitting updates or removes only entries Dev Inbox made. On weekdays at 17:30 (configurable, or off) a notification reminds you if today hasn't been sent; clicking it opens the Timesheet. Connect in Settings with your Harvest account ID and a personal access token (stored in the keychain).
- **Menu bar**: a monochrome inbox icon that gets a dot when something needs you (the number too, if you turn it on in Settings), and "REC" while recording. Click it for the **panel** — a Liquid Glass window under the icon (the app's own UI): what needs you (PRs, Jira tickets, your meeting action items — each opens in the app, action items tick off in place), a recording strip with Stop, and Record / Ask or act / Settings. Right-click for Open, Sync now and Quit. Closing the main window keeps it running. Global shortcut **⌥⌘K** (Ctrl+Alt+K elsewhere).
- **Desktop notifications** when something new needs you (bursts are collapsed), through macOS's current notification API (UNUserNotificationCenter — macOS 27 refuses the old one): they show as Dev Inbox, appear even while the app is in front, and clicking one brings Dev Inbox forward with the PR or issue open in Today (or the ticket in Tickets) — "Open on GitHub" is one click from there. Optional **morning summary** on weekdays.
- **Filters** (All tab) by repo/org, author, state, CI, mine/others; hide repos in Settings.

Mouse: click a row for its actions menu (open, reply, approve, merge, editor, come back later, mark as…) · double-click opens it on GitHub · right-click also opens the menu.

Keyboard: `j`/`k` move · `↵`/`o` open (or expand a group) · `e` done · `l` come back later / more · `x` reset to automatic · `R` reply · `a` approve · `m` merge · `c` check out & open in editor · `s` Day view · `r` sync · `1`–`8` switch views · `?` all shortcuts · `⌘K` or `/` command bar.

## How syncing works

`src-tauri/src/sync.rs` polls every 2 minutes (configurable):

1. Four cheap GitHub searches (PRs: `review-requested:@me`, `involves:@me`, `reviewed-by:@me`; issues: `involves:@me`) return ids, `updatedAt` and CI rollup.
2. Full details (reviews, review threads, comments, review requests, pushes, checks) are fetched only for items that changed, plus a full refresh every 30 minutes.
3. `derive.rs` maps each PR/issue to a personal state, with no AI involved. Transitions into an actionable state are logged, clear stale manual overrides, and trigger notifications. `intent.rs` reads your new comments for "come back to this" intent.
4. Every 5 minutes, the default branch of up to 25 recently active repos is checked for red CI.
5. Items that drop out of search are re-checked by id to record merges and closes; snoozes waiting on a PR/issue are resolved when it merges or closes.

## Layout

```
src-tauri/src/
  github.rs   GraphQL client (search hits + batched detail fetch, retries)
  derive.rs   GitHub data → personal state (unit tested)
  intent.rs   references, time phrases, "come back to this" in comments (unit tested)
  editor.rs   finding local clones, safe branch checkout, launching the editor, local activity (unit tested)
  day.rs      Day view: events → ticket-grouped time blocks, corrections (unit tested)
  jira.rs     Jira Cloud: assigned-ever tickets, bulk changelogs, storage; status and assignee changes (unit tested)
  harvest.rs  Harvest API v2: projects, time entries, learning how entries link to Jira (unit tested)
  timesheet.rs  the week grid: proposals from the Day view, 15-minute rounding, remembered and monthly buckets (unit tested)
  meetings.rs recorder sidecar control, transcript merge/echo removal, storage, retention (unit tested)
  summary.rs  meeting notes: Apple Intelligence (via the sidecar) or Claude (Messages API, JSON schema output)
  search.rs   meeting search (SQLite FTS5 over titles, transcript lines with speakers, and notes)
  voices.rs   remembering named voices across meetings (fingerprints, matching, learning)
  ai.rs       the one way AI features run a prompt: Apple Intelligence (sidecar) or Claude (Messages API)
  ask.rs      "Ask your history": deterministic retrieval into cited facts, then one model call
  ci.rs       CI log excerpts (errors with context, the end) and the failure-explainer prompt
  suggest.rs  suggesting a ticket for an unlinked meeting
  reminders.rs AI reading of "I'll come back to this" comments the keyword rules miss
  notifications.rs macOS notifications: UNUserNotificationCenter in the app bundle, AppleScript in dev
src-tauri/recorder/                 the dev-inbox-recorder sidecar, a SwiftPM package built by build.rs: mic (AVAudioEngine)
                                    + system audio (Core Audio process tap) to AAC, live captions and transcription
                                    (SpeechAnalyzer), FoundationModels summaries, FluidAudio speaker separation,
                                    EventKit calendar events
  calendar.rs the Mac's calendars (via the helper): stored events, meetings as Day spans, reminders (unit tested)
  sync.rs     sync pass, memory updates, notifications
  db.rs       SQLite schema & queries
  demo.rs     fictional seed data for DEV_INBOX_DEMO=1
  lib.rs      Tauri commands, tray, poller, global shortcut
src/
  App.tsx, components/   sidebar shell, Today / Inbox (one list, details inline), Time (Day, Timesheet, History), Tickets,
                         Meetings, Settings (sections), first-run Setup, ⌘K
  lib/query.ts           command-bar intent parsing
  lib/day.ts             day boundaries, durations, colours, summary text
  lib/states.ts          state metadata & Today sections
```

## Tests

```bash
cd src-tauri
cargo test                                                                 # derivation rules
GITHUB_TOKEN=$(gh auth token) cargo test live -- --ignored --nocapture    # read-only, against your real account
```

## Known limitations / next steps

- Notifications need a real, signed `.app` to show as Dev Inbox (the first one asks for permission). `tauri dev` runs a bare binary, so there they go through AppleScript instead: they show as Script Editor and clicking them can't bring the item up. Settings → "send a test" says which way it went, and whether notifications are turned off for Dev Inbox in System Settings.
- In `tauri dev`, the Microphone and **Screen & System Audio Recording** prompts are attributed to Terminal (or your editor's terminal). If meeting audio stays silent, allow it in System Settings → Privacy & Security and restart the app. Transcription needs macOS 26+; on first use it downloads the speech model for your language.
- Speaker labels are per track, not per person: everyone on the call is "Others". Spoken ticket keys rarely transcribe cleanly, so set the ticket from the title or by hand.
- Each `cargo` rebuild changes the binary signature, so macOS may ask again for keychain access in development.
- Re-running checks only works for GitHub Actions; other CI providers are named so you can re-run them from their own dashboards.
- Reminders from comments only read comments written after you first installed the app, so old history doesn't flood in.
- Not notarised by Apple yet (no Developer ID), so a downloaded `.dmg` needs "Open Anyway" once; the install script avoids that. Apple Silicon only.
- Next: OAuth device flow instead of tokens, and Apple notarisation.
