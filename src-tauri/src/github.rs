//! Minimal GitHub GraphQL client: fetches the PRs a user is involved in,
//! with just enough metadata to derive personal workflow state.

use anyhow::{anyhow, bail, Result};
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

const PR_FRAGMENT: &str = r#"
fragment PrFields on PullRequest {
  id number title url isDraft state createdAt updatedAt mergedAt closedAt mergeable reviewDecision
  author { __typename login avatarUrl(size: 64) }
  repository { nameWithOwner }
  assignees(first: 10) { nodes { login } }
  reviewRequests(first: 20) {
    nodes { requestedReviewer { __typename ... on User { login } ... on Team { slug } } }
  }
  reviews(last: 30) { nodes { id state submittedAt author { __typename login } } }
  comments(last: 20) { nodes { id createdAt author { __typename login } } }
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
    pub author: Option<Actor>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub created_at: String,
    pub author: Option<Actor>,
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
    pub author: Option<Actor>,
    pub repository: Repository,
    pub assignees: Nodes<Login>,
    pub review_requests: Nodes<ReviewRequest>,
    pub reviews: Nodes<Review>,
    pub comments: Nodes<Comment>,
    pub commits: Nodes<CommitNode>,
    pub timeline_items: Nodes<TimelineItem>,
    /// Set locally from the search hit: the PR was returned by the `review-requested:@me` search
    /// (covers team review requests, which `reviewRequests` can't attribute to the user).
    #[serde(skip)]
    pub matched_review_requested: bool,
}

/// A search result: enough to decide whether full details need refetching.
#[derive(Debug, Clone)]
pub struct Hit {
    pub id: String,
    pub updated_at: String,
    pub ci: String,
    pub review_requested: bool,
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
                nodes { ... on PullRequest {
                  id updatedAt
                  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
                } }
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

    /// Every open PR the viewer is involved in, deduplicated.
    pub async fn involved_hits(&self) -> Result<Vec<Hit>> {
        let (requested, involves, reviewed) = tokio::try_join!(
            self.search_hits(REVIEW_REQUESTED_QUERY),
            self.search_hits(INVOLVES_QUERY),
            self.search_hits(REVIEWED_QUERY),
        )?;
        let mut by_id: std::collections::HashMap<String, Hit> = Default::default();
        for mut h in requested {
            h.review_requested = true;
            by_id.insert(h.id.clone(), h);
        }
        for h in involves.into_iter().chain(reviewed) {
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

    /// Full details for specific PRs by node id, fetched in parallel batches.
    pub async fn prs_by_id(&self, ids: &[String]) -> Result<Vec<PullRequest>> {
        let query = format!(
            "{PR_FRAGMENT}
            query($ids: [ID!]!) {{ nodes(ids: $ids) {{ ... on PullRequest {{ ...PrFields }} }} }}"
        );
        let batches: Vec<&[String]> = ids.chunks(DETAIL_BATCH).collect();
        let mut out = Vec::new();
        // A few batches at a time keeps us well inside GitHub's secondary rate limits.
        for group in batches.chunks(4) {
            let calls = group.iter().map(|chunk| self.graphql(&query, json!({ "ids": chunk })));
            let results = futures::future::try_join_all(calls).await?;
            for data in results {
                for node in data["nodes"].as_array().into_iter().flatten() {
                    if let Ok(pr) = serde_json::from_value::<PullRequest>(node.clone()) {
                        out.push(pr);
                    }
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
        let prs = client.prs_by_id(&ids).await.unwrap();
        println!("details in {:?}", t.elapsed());
        println!("viewer={} prs={}", viewer.login, prs.len());
        for pr in prs.iter().take(15) {
            let d = crate::derive::derive(pr, &crate::derive::Ctx::new(&viewer.login, &[]));
            println!("{:>16} | {}#{} | {} | {}", d.state, pr.repository.name_with_owner, pr.number, d.reason, d.ci.as_str());
        }
    }
}
