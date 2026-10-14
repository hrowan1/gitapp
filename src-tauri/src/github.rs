//! Minimal GitHub GraphQL client: fetches the PRs and issues a user is involved in, plus
//! default-branch CI for their active repos, with just enough metadata to derive personal state.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

const ENDPOINT: &str = "https://api.github.com/graphql";
const DETAIL_BATCH: usize = 20;
const MAX_PAGES: u32 = 3;

/// Search queries that together cover everything that can need the user's attention.
/// (`review_requested` is tracked separately because it includes team requests.)
const REVIEW_REQUESTED_QUERY: &str = "is:pr is:open archived:false review-requested:@me sort:updated-desc";
const INVOLVES_QUERY: &str = "is:pr is:open archived:false involves:@me sort:updated-desc";
const REVIEWED_QUERY: &str = "is:pr is:open archived:false reviewed-by:@me sort:updated-desc";
const ISSUES_QUERY: &str = "is:issue is:open archived:false involves:@me sort:updated-desc";

const PR_FRAGMENT: &str = r#"
fragment PrFields on PullRequest {
  id number title url isDraft state createdAt updatedAt mergedAt closedAt mergeable reviewDecision
  headRefName isCrossRepository
  author { __typename login avatarUrl(size: 64) }
  repository { nameWithOwner }
  assignees(first: 10) { nodes { login } }
  reviewRequests(first: 20) {
    nodes { requestedReviewer { __typename ... on User { login } ... on Team { slug } } }
  }
  reviews(last: 30) { nodes { id state submittedAt body author { __typename login } } }
  comments(last: 20) { nodes { id createdAt body author { __typename login } } }
  reviewThreads(last: 30) { nodes { id comments(last: 10) { nodes { id createdAt body author { __typename login } } } } }
  commits(last: 1) { nodes { commit { committedDate statusCheckRollup { state } } } }
  timelineItems(last: 10, itemTypes: [HEAD_REF_FORCE_PUSHED_EVENT, REVIEW_REQUESTED_EVENT]) {
    nodes {
      __typename
      ... on HeadRefForcePushedEvent { createdAt }
      ... on ReviewRequestedEvent { createdAt requestedReviewer { __typename ... on User { login } } }
    }
  }
}
"#;

const ISSUE_FRAGMENT: &str = r#"
fragment IssueFields on Issue {
  id number title url state createdAt updatedAt closedAt
  author { __typename login avatarUrl(size: 64) }
  repository { nameWithOwner }
  assignees(first: 10) { nodes { login } }
  comments(last: 30) { nodes { id createdAt body author { __typename login } } }
  timelineItems(last: 10, itemTypes: [ASSIGNED_EVENT]) {
    nodes { __typename ... on AssignedEvent { createdAt assignee { __typename ... on User { login } } } }
  }
}
"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    #[serde(rename = "__typename")]
    pub typename: Option<String>,
    pub login: String,
    pub avatar_url: Option<String>,
}

impl Actor {
    pub fn is_bot(&self) -> bool {
        // Also catch machine users (regular accounts used for automation), e.g. "acme-bot", "fftbot".
        self.typename.as_deref() == Some("Bot") || self.login.to_ascii_lowercase().ends_with("bot") || self.login.ends_with("[bot]")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Nodes<T> {
    pub nodes: Vec<Option<T>>,
}

impl<T> Nodes<T> {
    pub fn empty() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.nodes.iter().flatten()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Login {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRequest {
    pub requested_reviewer: Option<RequestedReviewer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequestedReviewer {
    pub login: Option<String>,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub id: String,
    pub state: String,
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub body: String,
    pub author: Option<Actor>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub created_at: String,
    #[serde(default)]
    pub body: String,
    pub author: Option<Actor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewThread {
    #[serde(default)]
    pub id: String,
    pub comments: Nodes<Comment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitNode {
    pub commit: Commit,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub committed_date: Option<String>,
    pub status_check_rollup: Option<Rollup>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rollup {
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    #[serde(rename = "__typename")]
    pub typename: String,
    pub created_at: Option<String>,
    pub requested_reviewer: Option<RequestedReviewer>,
    pub assignee: Option<RequestedReviewer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub is_draft: bool,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub merged_at: Option<String>,
    pub closed_at: Option<String>,
    pub mergeable: Option<String>,
    pub review_decision: Option<String>,
    #[serde(default)]
    pub head_ref_name: Option<String>,
    #[serde(default)]
    pub is_cross_repository: Option<bool>,
    pub author: Option<Actor>,
    pub repository: Repository,
    pub assignees: Nodes<Login>,
    pub review_requests: Nodes<ReviewRequest>,
    pub reviews: Nodes<Review>,
    pub comments: Nodes<Comment>,
    #[serde(default = "Nodes::empty")]
    pub review_threads: Nodes<ReviewThread>,
    pub commits: Nodes<CommitNode>,
    pub timeline_items: Nodes<TimelineItem>,
    /// Set locally from the search hit: the PR was returned by the `review-requested:@me` search
    /// (covers team review requests, which `reviewRequests` can't attribute to the user).
    #[serde(skip)]
    pub matched_review_requested: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub author: Option<Actor>,
    pub repository: Repository,
    pub assignees: Nodes<Login>,
    pub comments: Nodes<Comment>,
    pub timeline_items: Nodes<TimelineItem>,
}

/// Full details fetched by node id.
#[derive(Default)]
pub struct Fetched {
    pub prs: Vec<PullRequest>,
    pub issues: Vec<Issue>,
}

/// A search result: enough to decide whether full details need refetching.
#[derive(Debug, Clone)]
pub struct Hit {
    pub id: String,
    pub updated_at: String,
    pub ci: String,
    pub review_requested: bool,
}

/// The latest commit on a repo's default branch and how its CI went.
#[derive(Debug, Clone)]
pub struct BranchStatus {
    pub repo: String,
    pub branch: String,
    pub oid: String,
    pub committed_date: String,
    pub commit_url: String,
    pub headline: String,
    pub author_login: Option<String>,
    pub author_avatar: Option<String>,
    pub ci: String,
    /// (workflow or app name, run url) for each failed suite.
    pub failures: Vec<(String, Option<String>)>,
}

/// Where a referenced PR or issue (e.g. "after #123 merges") currently stands.
#[derive(Debug, Clone)]
pub struct RefState {
    pub state: String,
    pub title: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeOptions {
    pub methods: Vec<String>,
    pub default: String,
    /// MERGEABLE | CONFLICTING | UNKNOWN
    pub mergeable: String,
    /// CLEAN | BLOCKED | BEHIND | DIRTY | UNSTABLE | HAS_HOOKS | UNKNOWN …
    pub state: String,
}

pub struct Rerun {
    pub rerun: usize,
    pub external: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Viewer {
    pub login: String,
    pub avatar_url: String,
}

/// An error worth retrying (network blip, 5xx, truncated body).
#[derive(Debug)]
struct Transient(String);

impl std::fmt::Display for Transient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Transient {}

pub struct Client {
    http: reqwest::Client,
    token: String,
}

impl Client {
    pub fn new(token: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("dev-inbox")
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("http client");
        Self { http, token: token.to_string() }
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        // GitHub occasionally answers heavy queries with a 502/timeout page; retry those.
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.graphql_once(query, &variables).await {
                Err(e) if attempt < 3 && e.downcast_ref::<Transient>().is_some() => {
                    tokio::time::sleep(std::time::Duration::from_millis(800 * attempt)).await;
                }
                other => return other,
            }
        }
    }

    async fn graphql_once(&self, query: &str, variables: &Value) -> Result<Value> {
        let res = self
            .http
            .post(ENDPOINT)
            .bearer_auth(&self.token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| anyhow!(Transient(format!("GitHub request failed: {e}"))))?;
        let status = res.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("GitHub rejected the token (401). Sign in again.");
        }
        let text = res.text().await.map_err(|e| anyhow!(Transient(format!("GitHub response interrupted: {e}"))))?;
        if status.is_server_error() {
            return Err(anyhow!(Transient(format!("GitHub returned {status}"))));
        }
        let body: Value = serde_json::from_str(&text)
            .map_err(|_| anyhow!(Transient(format!("GitHub returned an unreadable response ({status})"))))?;
        if !status.is_success() {
            bail!("GitHub returned {status}: {}", body["message"].as_str().unwrap_or(""));
        }
        if let Some(errors) = body.get("errors").and_then(|e| e.as_array()) {
            // Partial data is still useful (e.g. one inaccessible repo); only fail if nothing came back.
            if body.get("data").map_or(true, |d| d.is_null()) {
                let msg = errors
                    .iter()
                    .filter_map(|e| e["message"].as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                bail!("GitHub GraphQL error: {msg}");
            }
        }
        Ok(body["data"].clone())
    }

    pub async fn viewer(&self) -> Result<Viewer> {
        let data = self.graphql("query { viewer { login avatarUrl(size: 64) } }", json!({})).await?;
        Ok(Viewer {
            login: data["viewer"]["login"].as_str().ok_or_else(|| anyhow!("no viewer"))?.to_string(),
            avatar_url: data["viewer"]["avatarUrl"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// Cheap search: just enough to tell whether a PR changed since the last sync.
    async fn search_hits(&self, q: &str) -> Result<Vec<Hit>> {
        let query = "query($q: String!, $cursor: String) {
              search(query: $q, type: ISSUE, first: 100, after: $cursor) {
                pageInfo { hasNextPage endCursor }
                nodes {
                  ... on PullRequest { id updatedAt commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } }
                  ... on Issue { id updatedAt }
                }
              }
            }";
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let data = self.graphql(query, json!({ "q": q, "cursor": cursor })).await?;
            let search = &data["search"];
            for node in search["nodes"].as_array().into_iter().flatten() {
                let (Some(id), Some(updated_at)) = (node["id"].as_str(), node["updatedAt"].as_str()) else { continue };
                let rollup = node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str();
                out.push(Hit {
                    id: id.to_string(),
                    updated_at: updated_at.to_string(),
                    ci: crate::derive::Ci::from_rollup(rollup).as_str().to_string(),
                    review_requested: false,
                });
            }
            if search["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                break;
            }
            cursor = search["pageInfo"]["endCursor"].as_str().map(String::from);
        }
        Ok(out)
    }

    /// Every open PR and issue the viewer is involved in, deduplicated.
    pub async fn involved_hits(&self) -> Result<Vec<Hit>> {
        let (requested, involves, reviewed, issues) = tokio::try_join!(
            self.search_hits(REVIEW_REQUESTED_QUERY),
            self.search_hits(INVOLVES_QUERY),
            self.search_hits(REVIEWED_QUERY),
            self.search_hits(ISSUES_QUERY),
        )?;
        let mut by_id: std::collections::HashMap<String, Hit> = Default::default();
        for mut h in requested {
            h.review_requested = true;
            by_id.insert(h.id.clone(), h);
        }
        for h in involves.into_iter().chain(reviewed).chain(issues) {
            by_id.entry(h.id.clone()).or_insert(h);
        }
        Ok(by_id.into_values().collect())
    }

    /// Re-run the failed GitHub Actions workflows on a PR's latest commit.
    /// Checks from other CI providers can't be re-run through the API; they're reported back by name.
    pub async fn rerun_failed_checks(&self, pr_id: &str) -> Result<Rerun> {
        let data = self
            .graphql(
                "query($id: ID!) { node(id: $id) { ... on PullRequest {
                    repository { nameWithOwner }
                    commits(last: 1) { nodes { commit { checkSuites(first: 50) { nodes {
                      conclusion app { name } workflowRun { databaseId }
                    } } } } }
                } } }",
                json!({ "id": pr_id }),
            )
            .await?;
        let node = &data["node"];
        let repo = node["repository"]["nameWithOwner"].as_str().ok_or_else(|| anyhow!("PR not found"))?;
        let suites = node["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"].as_array().cloned().unwrap_or_default();
        let mut out = Rerun { rerun: 0, external: Vec::new() };
        for suite in suites {
            let failed = matches!(
                suite["conclusion"].as_str(),
                Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "STARTUP_FAILURE")
            );
            if !failed {
                continue;
            }
            match suite["workflowRun"]["databaseId"].as_i64() {
                Some(run_id) => {
                    self.rest_post(&format!("/repos/{repo}/actions/runs/{run_id}/rerun-failed-jobs")).await?;
                    out.rerun += 1;
                }
                None => out.external.push(suite["app"]["name"].as_str().unwrap_or("external CI").to_string()),
            }
        }
        Ok(out)
    }

    /// Run a mutation; unlike queries, any GraphQL error means it didn't happen.
    async fn mutate(&self, query: &str, variables: Value) -> Result<Value> {
        let res = self
            .http
            .post(ENDPOINT)
            .bearer_auth(&self.token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .context("GitHub request failed")?;
        let status = res.status();
        let body: Value = res.json().await.context("invalid GitHub response")?;
        if let Some(errors) = body.get("errors").and_then(|e| e.as_array()).filter(|e| !e.is_empty()) {
            let msg = errors.iter().filter_map(|e| e["message"].as_str()).collect::<Vec<_>>().join("; ");
            bail!("{msg}");
        }
        if !status.is_success() {
            bail!("GitHub returned {status}: {}", body["message"].as_str().unwrap_or(""));
        }
        Ok(body["data"].clone())
    }

    /// Submit a review: event is APPROVE, REQUEST_CHANGES or COMMENT.
    pub async fn submit_review(&self, pr_id: &str, event: &str, body: Option<&str>) -> Result<()> {
        self.mutate(
            "mutation($id: ID!, $event: PullRequestReviewEvent!, $body: String) {
                addPullRequestReview(input: { pullRequestId: $id, event: $event, body: $body }) { pullRequestReview { id } }
            }",
            json!({ "id": pr_id, "event": event, "body": body }),
        )
        .await?;
        Ok(())
    }

    /// A conversation comment on a PR or issue.
    pub async fn add_comment(&self, subject_id: &str, body: &str) -> Result<()> {
        self.mutate(
            "mutation($id: ID!, $body: String!) { addComment(input: { subjectId: $id, body: $body }) { clientMutationId } }",
            json!({ "id": subject_id, "body": body }),
        )
        .await?;
        Ok(())
    }

    /// Reply inside a review thread.
    pub async fn reply_in_thread(&self, thread_id: &str, body: &str) -> Result<()> {
        self.mutate(
            "mutation($id: ID!, $body: String!) {
                addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $id, body: $body }) { comment { id } }
            }",
            json!({ "id": thread_id, "body": body }),
        )
        .await?;
        Ok(())
    }

    /// Which merge methods the repo allows, its default, and whether the PR can merge now.
    pub async fn merge_options(&self, pr_id: &str) -> Result<MergeOptions> {
        let data = self
            .graphql(
                "query($id: ID!) { node(id: $id) { ... on PullRequest {
                    mergeable mergeStateStatus viewerCanUpdate
                    repository { mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed viewerDefaultMergeMethod }
                } } }",
                json!({ "id": pr_id }),
            )
            .await?;
        let n = &data["node"];
        let r = &n["repository"];
        let mut methods = Vec::new();
        for (flag, method) in [("squashMergeAllowed", "SQUASH"), ("mergeCommitAllowed", "MERGE"), ("rebaseMergeAllowed", "REBASE")] {
            if r[flag].as_bool() == Some(true) {
                methods.push(method.to_string());
            }
        }
        Ok(MergeOptions {
            methods,
            default: r["viewerDefaultMergeMethod"].as_str().unwrap_or("MERGE").to_string(),
            mergeable: n["mergeable"].as_str().unwrap_or("UNKNOWN").to_string(),
            state: n["mergeStateStatus"].as_str().unwrap_or("UNKNOWN").to_string(),
        })
    }

    pub async fn merge(&self, pr_id: &str, method: &str) -> Result<()> {
        self.mutate(
            "mutation($id: ID!, $method: PullRequestMergeMethod!) {
                mergePullRequest(input: { pullRequestId: $id, mergeMethod: $method }) { pullRequest { state } }
            }",
            json!({ "id": pr_id, "method": method }),
        )
        .await?;
        Ok(())
    }

    async fn rest_post(&self, path: &str) -> Result<()> {
        let res = self
            .http
            .post(format!("https://api.github.com{path}"))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        let status = res.status();
        if !status.is_success() {
            let body: Value = res.json().await.unwrap_or_default();
            let hint = if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
                " (your token may need the `repo`/`workflow` scope)"
            } else {
                ""
            };
            bail!("GitHub returned {status}: {}{hint}", body["message"].as_str().unwrap_or(""));
        }
        Ok(())
    }

    /// Full details for specific PRs/issues by node id, fetched in parallel batches.
    pub async fn items_by_id(&self, ids: &[String]) -> Result<Fetched> {
        let query = format!(
            "{PR_FRAGMENT} {ISSUE_FRAGMENT}
            query($ids: [ID!]!) {{ nodes(ids: $ids) {{ __typename ... on PullRequest {{ ...PrFields }} ... on Issue {{ ...IssueFields }} }} }}"
        );
        let batches: Vec<&[String]> = ids.chunks(DETAIL_BATCH).collect();
        let mut out = Fetched::default();
        // A few batches at a time keeps us well inside GitHub's secondary rate limits.
        for group in batches.chunks(4) {
            let calls = group.iter().map(|chunk| self.graphql(&query, json!({ "ids": chunk })));
            let results = futures::future::try_join_all(calls).await?;
            for data in results {
                for node in data["nodes"].as_array().into_iter().flatten() {
                    match node["__typename"].as_str() {
                        Some("PullRequest") => {
                            if let Ok(pr) = serde_json::from_value::<PullRequest>(node.clone()) {
                                out.prs.push(pr);
                            }
                        }
                        Some("Issue") => {
                            if let Ok(issue) = serde_json::from_value::<Issue>(node.clone()) {
                                out.issues.push(issue);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(out)
    }

    /// Latest default-branch commit and CI result for each repo (batched with aliases).
    pub async fn default_branches(&self, repos: &[String]) -> Result<Vec<BranchStatus>> {
        let mut out = Vec::new();
        for chunk in repos.chunks(20) {
            let mut vars = serde_json::Map::new();
            let mut decls = Vec::new();
            let mut fields = Vec::new();
            for (i, repo) in chunk.iter().enumerate() {
                let Some((owner, name)) = repo.split_once('/') else { continue };
                vars.insert(format!("o{i}"), json!(owner));
                vars.insert(format!("n{i}"), json!(name));
                decls.push(format!("$o{i}: String!, $n{i}: String!"));
                fields.push(format!(
                    "r{i}: repository(owner: $o{i}, name: $n{i}) {{ nameWithOwner defaultBranchRef {{ name target {{ ... on Commit {{
                        oid committedDate url messageHeadline
                        author {{ user {{ login avatarUrl(size: 64) }} }}
                        statusCheckRollup {{ state }}
                        checkSuites(last: 20) {{ nodes {{ conclusion app {{ name }} workflowRun {{ url workflow {{ name }} }} }} }}
                    }} }} }} }}"
                ));
            }
            if fields.is_empty() {
                continue;
            }
            let query = format!("query({}) {{ {} }}", decls.join(", "), fields.join("\n"));
            let data = self.graphql(&query, Value::Object(vars)).await?;
            for i in 0..chunk.len() {
                let r = &data[format!("r{i}")];
                let c = &r["defaultBranchRef"]["target"];
                let (Some(repo), Some(oid)) = (r["nameWithOwner"].as_str(), c["oid"].as_str()) else { continue };
                let mut failures = Vec::new();
                for suite in c["checkSuites"]["nodes"].as_array().into_iter().flatten() {
                    if matches!(suite["conclusion"].as_str(), Some("FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE")) {
                        let name = suite["workflowRun"]["workflow"]["name"]
                            .as_str()
                            .or(suite["app"]["name"].as_str())
                            .unwrap_or("CI")
                            .to_string();
                        if !failures.iter().any(|(n, _): &(String, Option<String>)| *n == name) {
                            failures.push((name, suite["workflowRun"]["url"].as_str().map(String::from)));
                        }
                    }
                }
                out.push(BranchStatus {
                    repo: repo.to_string(),
                    branch: r["defaultBranchRef"]["name"].as_str().unwrap_or("main").to_string(),
                    oid: oid.to_string(),
                    committed_date: c["committedDate"].as_str().unwrap_or_default().to_string(),
                    commit_url: c["url"].as_str().unwrap_or_default().to_string(),
                    headline: c["messageHeadline"].as_str().unwrap_or_default().to_string(),
                    author_login: c["author"]["user"]["login"].as_str().map(String::from),
                    author_avatar: c["author"]["user"]["avatarUrl"].as_str().map(String::from),
                    ci: crate::derive::Ci::from_rollup(c["statusCheckRollup"]["state"].as_str()).as_str().to_string(),
                    failures,
                });
            }
        }
        Ok(out)
    }

    /// Current state of referenced PRs/issues ("owner/repo#123" keys), batched with aliases.
    pub async fn ref_states(&self, refs: &[String]) -> Result<std::collections::HashMap<String, RefState>> {
        let mut out = std::collections::HashMap::new();
        for chunk in refs.chunks(30) {
            let mut vars = serde_json::Map::new();
            let mut decls = Vec::new();
            let mut fields = Vec::new();
            for (i, key) in chunk.iter().enumerate() {
                let Some((repo, num)) = key.split_once('#') else { continue };
                let (Some((owner, name)), Ok(num)) = (repo.split_once('/'), num.parse::<i64>()) else { continue };
                vars.insert(format!("o{i}"), json!(owner));
                vars.insert(format!("n{i}"), json!(name));
                vars.insert(format!("x{i}"), json!(num));
                decls.push(format!("$o{i}: String!, $n{i}: String!, $x{i}: Int!"));
                fields.push(format!(
                    "w{i}: repository(owner: $o{i}, name: $n{i}) {{ issueOrPullRequest(number: $x{i}) {{
                        ... on PullRequest {{ state title }} ... on Issue {{ state title }}
                    }} }}"
                ));
            }
            if fields.is_empty() {
                continue;
            }
            let query = format!("query({}) {{ {} }}", decls.join(", "), fields.join("\n"));
            let data = self.graphql(&query, Value::Object(vars)).await?;
            for (i, key) in chunk.iter().enumerate() {
                let node = &data[format!("w{i}")]["issueOrPullRequest"];
                if let Some(state) = node["state"].as_str() {
                    out.insert(
                        key.clone(),
                        RefState { state: state.to_string(), title: node["title"].as_str().unwrap_or_default().to_string() },
                    );
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod live_tests {
    /// Read-only smoke test against the real API: `GITHUB_TOKEN=$(gh auth token) cargo test live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_fetch() {
        let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN");
        let client = super::Client::new(&token);
        let viewer = client.viewer().await.unwrap();
        let t = std::time::Instant::now();
        let hits = client.involved_hits().await.unwrap();
        println!("hits={} in {:?}", hits.len(), t.elapsed());
        let ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
        let t = std::time::Instant::now();
        let fetched = client.items_by_id(&ids).await.unwrap();
        println!("details in {:?}", t.elapsed());
        println!("viewer={} prs={} issues={}", viewer.login, fetched.prs.len(), fetched.issues.len());
        let repos: Vec<String> = fetched.prs.iter().map(|p| p.repository.name_with_owner.clone()).take(5).collect();
        for b in client.default_branches(&repos).await.unwrap() {
            println!("branch {} {} ci={} failures={:?}", b.repo, b.branch, b.ci, b.failures);
        }
        let prs = &fetched.prs;
        if let Some(pr) = prs.first() {
            let threads_with_ids = prs.iter().flat_map(|p| p.review_threads.iter()).filter(|t| !t.id.is_empty()).count();
            println!("branch of {}#{} = {:?} cross={:?}; threads with ids={threads_with_ids}", pr.repository.name_with_owner, pr.number, pr.head_ref_name, pr.is_cross_repository);
            let m = client.merge_options(&pr.id).await.unwrap();
            println!("merge options: {m:?}");
        }
        for pr in prs.iter().take(15) {
            let d = crate::derive::derive(pr, &crate::derive::Ctx::new(&viewer.login, &[]));
            println!("{:>16} | {}#{} | {} | {}", d.state, pr.repository.name_with_owner, pr.number, d.reason, d.ci.as_str());
        }
    }
}
