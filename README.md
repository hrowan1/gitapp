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
- **Personal states**, derived automatically from GitHub, overridable by hand:
  - review requested → 👀 Need to review
  - you requested changes, then new commits landed → 🔄 Needs re-review
  - someone commented after your last comment/review → 💬 Replied to you
  - your PR: failing CI / merge conflicts / changes requested → 🔧 I need to make changes
  - your PR approved and green → 🚀 Ready to merge
  - you requested changes and nothing happened yet → ⏳ Waiting for the author
  - no activity for 30+ days → 🕸️ Gone quiet (kept out of "Needs you")
- **Memory**: manual states stick until something *new* happens on the PR, then the PR resurfaces automatically. "Come back later" takes a note ("after the API migration") and a time, and notifies you when it's due.
- **History**: a timeline of your reviews, comments, opened/merged PRs and the states you set.
- **New since you last looked**: a dot on things that need you and arrived after you last opened them; "mark seen" clears them all.
- **Noise control**: a repo with 4+ PRs in one section collapses into a single group row with bulk "✓ all" / snooze / mute. **Mute** hides a PR for good (until it closes). **Ignore people & bots** (Settings): their comments don't count as replies and their PRs go to FYI.
- **⌘K command bar**:
  - Questions: "what do I need to review?", "what's new since I last looked?", "show me Sarah's PRs", "what have I forgotten about?", "what did I work on yesterday?"
  - Actions: "remind me about the checkout PR tomorrow", "come back to auth middleware after the API migration", "mark sarah's checkout pr as done", "mute …", "rerun checks on billing", "open …", "checkout …"
  - `Tab` on any PR lists everything you can do to it.
- **PR actions**: re-run failed GitHub Actions workflows; copy `gh pr checkout` to the clipboard.
- **Menu bar / tray** with a monochrome icon and needs-you count; closing the window keeps it running. Global shortcut **⌥⌘K** (Ctrl+Alt+K elsewhere).
- **Desktop notifications** when something new needs you (bursts are collapsed), plus an optional **morning summary** on weekdays.
- **Filters** (All tab) by repo/org, author, state, CI, mine/others; hide repos in Settings.

Keyboard: `j`/`k` move · `↵`/`o` open (or expand a group) · `e` done · `l` come back later / more · `x` reset to automatic · `r` sync · `1`–`4` switch tabs · `⌘K` or `/` command bar.

## How syncing works

`src-tauri/src/sync.rs` polls every 2 minutes (configurable):

1. Three cheap GitHub searches (`review-requested:@me`, `involves:@me`, `reviewed-by:@me`) return PR ids, `updatedAt` and CI rollup.
2. Full details (reviews, comments, review requests, pushes, checks) are fetched only for PRs that changed, plus a full refresh every 30 minutes.
3. `derive.rs` maps each PR to a personal state, with no AI involved. Transitions into an actionable state are logged, clear stale manual overrides, and trigger notifications.
4. PRs that drop out of search are re-checked by id to record merges and closes.

## Layout

```
src-tauri/src/
  github.rs   GraphQL client (search hits + batched detail fetch, retries)
  derive.rs   GitHub data → personal state (unit tested)
  sync.rs     sync pass, memory updates, notifications
  db.rs       SQLite schema & queries
  demo.rs     fictional seed data for DEV_INBOX_DEMO=1
  lib.rs      Tauri commands, tray, poller, global shortcut
src/
  App.tsx, components/   Today / All / History / Settings / ⌘K palette
  lib/query.ts           command-bar intent parsing
  lib/states.ts          state metadata & Today sections
```

## Tests

```bash
cd src-tauri
cargo test                                                                 # derivation rules
GITHUB_TOKEN=$(gh auth token) cargo test live -- --ignored --nocapture    # read-only, against your real account
```

## Known limitations / next steps

- Notifications in `tauri dev` may be attributed to the terminal on macOS; they behave properly in a built `.app`.
- Each `cargo` rebuild changes the binary signature, so macOS may ask again for keychain access in development.
- Re-running checks only works for GitHub Actions; other CI providers are named so you can re-run them from their own dashboards.
- Next: OAuth device flow instead of tokens, per-thread reply tracking, "come back after X" that resolves itself (e.g. when a named PR merges), and AI intent extraction.
