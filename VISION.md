# Dev Inbox — Product Vision

> Your engineering inbox. Everything that needs your attention, across every repository, in one place.

## The core idea

A desktop application that acts as a developer's personal engineering inbox. It connects to GitHub and keeps track of everything that requires a developer's attention across multiple repositories:

- Pull requests needing review
- Pull requests they have reviewed
- PRs where someone has responded to their comments
- PRs where they requested changes and need to re-review
- Their own PRs waiting for review
- Failed CI/checks
- PRs waiting for someone else
- PRs they deliberately want to come back to
- Stale work
- GitHub Actions
- Issues and other relevant activity

**The problem:** "I work across lots of repositories and constantly lose track of what I need to do."

GitHub contains the information, but it doesn't provide a good personal workflow/memory layer across everything. The product is the place a developer opens first to answer: **"What needs my attention?"**

## Key differentiator: personal memory

The app doesn't just mirror GitHub. It remembers what you have done and what you intend to do.

- You review a PR and request changes → the app records **Waiting for author**. The author pushes new commits → state changes to **Needs re-review** and you're notified.
- You comment "Let's come back to this after the API migration" → the app remembers the intent (**Come back after API migration**) and resurfaces the PR later.

## Personal states

GitHub has Open / Closed / Draft / Merged. The app adds a second, personal layer:

| State | Meaning |
|---|---|
| 👀 Need to review | |
| 🔧 I need to make changes | |
| 💬 Waiting for response | |
| 🔄 Needs re-review | |
| ⏳ Waiting for CI | |
| 📌 Come back later | |
| ✅ Done | |

A PR can be *Open* on GitHub while simultaneously being *Waiting for Sarah* in the app.

## The homepage: "Today"

Not a GitHub clone. The primary screen answers: *What do I need to care about today?*

```
┌─────────────────────────────────────────────┐
│  Dev Inbox                         🔔 3     │
│                                             │
│  What needs my attention?            ⌘ K   │
├─────────────────────────────────────────────┤
│  🔴 NEEDS YOU                               │
│  Sarah · Checkout refactor           14h    │
│  Review requested · 2 checks failing        │
│  James · Auth middleware              2d    │
│  Review requested · Ready                   │
│                                             │
│  💬 COME BACK TO                            │
│  API pagination                       3h    │
│  Sarah replied to your comment              │
│                                             │
│  ⚠️ YOUR WORK                               │
│  Billing · CI failing                       │
│  Dashboard · Waiting for review             │
└─────────────────────────────────────────────┘
```

The goal is not maximum information. The goal is **minimum cognitive load**.

## Natural-language interface

Global command bar (⌘K / Ctrl+K). Example queries:

- "What do I need to review?" / "What am I waiting on?"
- "Which PRs have I reviewed but haven't finished?"
- "Show me PRs where someone replied to me."
- "What did I work on yesterday?" / "What's blocking me?"
- "What PRs are failing checks?" / "Show me Sarah's PRs."
- "What have I forgotten about?" / "Show me things I commented on that changed."
- "What should I focus on today?"

### Natural-language actions (later)

- "Open Sarah's checkout PR." / "Approve it."
- "Retry the failed workflow."
- "Remind me about this tomorrow."
- "Mark this as waiting for Sarah." / "Put this in come back later."
- "Open this in my IDE."

## AI enhances the product, it isn't the product

The core must work without AI. Most state is deterministic:

| GitHub signal | Personal state |
|---|---|
| Review requested from user | Need review |
| Author pushed commits after user's review | Needs re-review |
| Check failed | Failing checks |
| User requested changes | Waiting for author |

AI is for ambiguous human intent — "Come back to this after Mike replies", "Revisit once the API migration is done" — converting it into structured memory. Also: NL search & commands, summarising activity, explaining CI failures, prioritising work, answering questions about personal history.

### AI architecture

Users shouldn't need their own API key; the SaaS provides AI via its backend.

```
Desktop App → Backend → GitHub API → Personal Workflow DB → AI Layer
```

The AI receives only the minimal context needed, e.g.:

```json
{
  "reviews_needed": 4,
  "waiting_for_me": 2,
  "waiting_for_others": 3,
  "failing_checks": 2,
  "recently_changed": 5
}
```

## Pricing (to validate)

- **Free** — basic GitHub integration, limited repos, PR dashboard, personal states, basic notifications
- **Pro (~£8–15/mo)** — unlimited repos, multiple orgs, advanced filters, personal memory, smart notifications, NL search, AI summaries, custom views, workflow actions
- **Team (~£10–20/dev/mo)** — team dashboards, shared views, review workload, bottleneck detection, Slack integration, team analytics

## Notifications

- 🔔 Sarah responded to the PR you were waiting on.
- 🔔 PR #421 has new commits after your requested changes.
- 🔴 Your PR's CI just failed.
- 👀 James requested your review.
- ⏰ Your "come back later" reminder is due.

## Desktop-first

Lives in the macOS menu bar / Windows & Linux tray. Global ⌘K / Ctrl+K opens the command bar instantly:

```
┌────────────────────────────────────────┐
│ What do you need?                      │
│ > what needs my attention?             │
├────────────────────────────────────────┤
│ 4 PRs need review                      │
│ 2 conversations need a response        │
│ 1 CI failure                           │
│ 2 items you postponed                  │
└────────────────────────────────────────┘
```

Feels like: GitHub + personal task manager + command palette + memory.

## Multiple repositories

The fundamental use case — aggregating across orgs/clients/personal projects. Filters: repository, organisation, author, assignee, PR status, review status, CI status, personal state, date, labels, milestone. Long-term, filters become unnecessary: "Show me PRs from Client A that need my review."

## Personal history

A timeline of the developer's activity:

```
Yesterday
09:12 — Reviewed PR #421
09:24 — Requested changes
10:03 — Commented on PR #418
11:42 — Opened PR #437
14:15 — Marked PR #402 as "Come back later"
16:32 — Reviewed Sarah's PR
```

Enables: "What did I do yesterday?", "Which PRs did I review this week?", "What did I leave unfinished?"

## The memory system

The app knows what you've worked on, reviewed, commented on, are waiting for, postponed, completed; what changed since you last looked; which conversations need follow-up; which repos you frequent.

The killer query: **"What am I forgetting?"**

```
You may have forgotten about:
1. PR #421 — You requested changes 4 days ago. The author responded yesterday.
2. PR #392 — You said you'd revisit after the API migration. The migration merged today.
3. PR #440 — You marked this as "come back later" last week.
```

## Integrations (later)

Start with GitHub. Eventually: GitLab, Bitbucket, Linear, Jira, Slack, Sentry, Vercel, CircleCI and other CI/CD — evolving from *GitHub inbox* to *engineering inbox*.

## Positioning

Avoid "a better GitHub desktop client" and "AI-powered GitHub dashboard". Instead:

- *Your engineering inbox.*
- *Stop losing track of your PRs.*
- *Your personal memory for software development.*

GitHub doesn't lack information — it has too much. The app turns hundreds of events → personal context → the 4 things you need to deal with.

## Moat

A dashboard is easy to copy. The moat is accumulated personal workflow memory: "reviewed this 3 days ago, requested changes, told Sarah to revisit after the API migration, marked another as low priority, waiting on James." After months of use, that creates switching costs.

## Privacy

- Minimal GitHub permissions
- Encrypt tokens
- Don't store source code; store only workflow metadata
- Clear retention controls, easy deletion
- Transparent AI data policy
- Potential local-first functionality; optional BYOK AI

Positioning: **We remember your workflow, not your source code.**

## Roadmap

**Phase 1 — Core GitHub inbox:** GitHub OAuth · repo selection · PR aggregation · review-request detection · CI/check status · author/assignee/repo filters · personal states · desktop notifications · "come back later" · personal activity history. Useful without AI.

**Phase 2 — Memory:** detect responses to your review · detect new commits after requested changes · auto-update personal state · follow-up reminders · personal timeline · "things you haven't finished".

**Phase 3 — AI:** NL search & commands · summaries · "what needs my attention / am I waiting on / am I forgetting?" · CI failure explanations · prioritisation.

**Phase 4 — Engineering inbox:** Linear/Jira · Slack · Sentry · deployments · other Git providers.

## The product loop

```
Open app → "What needs my attention?" → 3–5 important things → act
→ app remembers → something changes → app notices → notified → return
```

The goal isn't more time in the app; it's less time thinking about what to do.

## Long-term vision

Not a GitHub client — a personal operating system for software development. Ask "What should I be doing right now?" and get an answer grounded in GitHub, PR history, personal states, conversations, tasks, CI, deployments, incidents, previous decisions, and things you've asked it to remember.
