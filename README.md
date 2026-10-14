# Dev Inbox

Your engineering inbox: everything that needs your attention across every GitHub repository, plus a personal memory of what you've done and what you meant to come back to. See [VISION.md](VISION.md) for the product idea.

This is the Phase 1 MVP: a Tauri 2 desktop app (Rust + React/TypeScript). It is **local-first**: no backend; the GitHub token lives in the OS keychain and PR metadata (never source code) sits in a local SQLite database.

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

Build a release `.app`/`.dmg`: `npm run tauri build`.

## What it does

- **Today**: Needs you · Come back to · Your work · Waiting on others · Gone quiet · Done & FYI
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
- **History**: a timeline of your reviews, comments, opened/merged PRs and the states you set.
- **New since you last looked**: a dot on things that need you and arrived after you last opened them; "mark seen" clears them all.
- **Following** tab: everything is followed by default; untick any PR, issue, branch or whole repo to stop hearing about it (gone from Today, counts and notifications). Filter, then follow/unfollow everything shown in one go.
- **Noise control**: a repo with 4+ items in one section collapses into a single group row with bulk "✓ all" / snooze / stop following. **Ignore people & bots** (Settings): their comments don't count as replies and their PRs go to FYI.
- **⌘K command bar**:
  - Questions: "what do I need to review?", "what's new since I last looked?", "show me Sarah's PRs", "what have I forgotten about?", "what did I work on yesterday?"
  - Actions: "remind me about the checkout PR tomorrow", "come back to auth middleware after the API migration", "mark sarah's checkout pr as done", "approve sarah's checkout pr", "reply to priya", "merge export reports", "check out the checkout refactor", "open web in my editor", "stop following …", "rerun checks on billing"
  - `Tab` on any PR lists everything you can do to it.
- **Act from the app** (each asks first, then posts to GitHub as you):
  - **Reply**: "Replied to you" rows quote what was said; reply goes into the right review thread, or as a comment.
  - **Approve / request changes / comment**, and **merge** using a method your repo allows.
  - **Open in editor**: finds your clones by scanning your home folder (or folders you choose) for GitHub remotes and opens the repo in your editor (VS Code, Cursor, Zed, JetBrains, Xcode… whatever's installed). **Check out & open** switches to the PR branch first, **only if the working tree is clean**. Not cloned? It offers to clone it (via `gh`) into `~/code`.
  - Re-run failed GitHub Actions workflows; copy `gh pr checkout`.
- **Standup** (`s`, History → "Write my standup", or ⌘K "standup"): Yesterday / Today / Blocked from your history and inbox; since the last working day, today, this week or 7 days; Markdown with links or plain text; leave repos out; edit, then copy.
- **Menu bar / tray** with a monochrome icon and needs-you count; closing the window keeps it running. Global shortcut **⌥⌘K** (Ctrl+Alt+K elsewhere).
- **Desktop notifications** when something new needs you (bursts are collapsed); on macOS, clicking one opens the PR/issue. Optional **morning summary** on weekdays.
- **Filters** (All tab) by repo/org, author, state, CI, mine/others; hide repos in Settings.

Keyboard: `j`/`k` move · `↵`/`o` open (or expand a group) · `e` done · `l` come back later / more · `x` reset to automatic · `R` reply · `a` approve · `m` merge · `c` check out & open in editor · `s` standup · `r` sync · `1`–`5` switch tabs · `⌘K` or `/` command bar.

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
  editor.rs   finding local clones, safe branch checkout, launching the editor (unit tested)
  sync.rs     sync pass, memory updates, notifications
  db.rs       SQLite schema & queries
  demo.rs     fictional seed data for DEV_INBOX_DEMO=1
  lib.rs      Tauri commands, tray, poller, global shortcut
src/
  App.tsx, components/   Today / All / History / Following / Settings / ⌘K palette
  lib/query.ts           command-bar intent parsing
  lib/standup.ts         standup text from history + inbox
  lib/states.ts          state metadata & Today sections
```

## Tests

```bash
cd src-tauri
cargo test                                                                 # derivation rules
GITHUB_TOKEN=$(gh auth token) cargo test live -- --ignored --nocapture    # read-only, against your real account
```

## Known limitations / next steps

- Notifications in `tauri dev` are attributed to Terminal on macOS (so Terminal's notification settings apply); a built `.app` uses its own identity. Settings → "send a test" checks they arrive.
- Each `cargo` rebuild changes the binary signature, so macOS may ask again for keychain access in development.
- Re-running checks only works for GitHub Actions; other CI providers are named so you can re-run them from their own dashboards.
- Reminders from comments only read comments written after you first installed the app, so old history doesn't flood in.
- Next: OAuth device flow instead of tokens, packaging (signed `.dmg`, auto-update), approve/merge/open-in-editor actions, and AI for intent the keyword rules miss.
