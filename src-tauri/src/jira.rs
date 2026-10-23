//! Jira Cloud, read-only: tickets that involve the user — assigned to them now or before, reported by
//! them, or moved, edited or commented on by them — and how they moved: status changes, assignment
//! changes, resolutions, the user's comments and other people's comments and @mentions. Also works
//! out which tickets need the user (assigned to them, mentioned, commented on or moved by someone
//! else since they last looked) for Today and desktop notifications. Feeds the Tickets
//! page and the Day view. Authenticates with the user's email + API token (token in the keychain).

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};

use crate::derive::{fmt_ts, Ts};

/// How far back the first sync looks.
pub const FIRST_SYNC_DAYS: i64 = 30;

pub struct Client {
    http: reqwest::Client,
    base: String,
    email: String,
    token: String,
}

/// Jira answered with an error status.
#[derive(Debug)]
pub struct HttpError {
    pub status: reqwest::StatusCode,
    pub text: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if matches!(self.status, reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN) {
            write!(f, "Jira rejected the email or API token ({})", self.status)
        } else {
            write!(f, "Jira returned {} {}", self.status, self.text)
        }
    }
}

impl std::error::Error for HttpError {}

fn status_of(e: &anyhow::Error) -> Option<reqwest::StatusCode> {
    e.downcast_ref::<HttpError>().map(|h| h.status)
}

/// Jira refused the email and API token (an expired or revoked token, say).
pub fn login_rejected(e: &anyhow::Error) -> bool {
    matches!(status_of(e), Some(reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN))
}

/// "Issue does not exist or you do not have permission to see it." Jira says the same to a login
/// it no longer accepts — it answers that as nobody — so check `myself` before believing it.
pub fn not_found(e: &anyhow::Error) -> bool {
    status_of(e) == Some(reqwest::StatusCode::NOT_FOUND)
}

/// "fullfatthings.atlassian.net", "https://x.atlassian.net/jira/software/…" → "https://x.atlassian.net"
pub fn normalise_site(site: &str) -> Result<String> {
    let s = site.trim().trim_start_matches("https://").trim_start_matches("http://");
    let host = s.split('/').next().unwrap_or_default().to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') {
        bail!("Enter your Jira site, e.g. yourcompany.atlassian.net");
    }
    Ok(format!("https://{host}"))
}

/// Jira timestamps look like "2026-10-14T10:12:33.123+0100".
pub fn parse_jira_ts(s: &str) -> Option<Ts> {
    DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z")
        .or_else(|_| DateTime::parse_from_rfc3339(s))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Plain text from an Atlassian Document Format body.
pub fn adf_text(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        if let Some(t) = v.get("text").and_then(Value::as_str) {
            out.push_str(t);
        }
        if let Some(children) = v.get("content").and_then(Value::as_array) {
            for c in children {
                walk(c, out);
            }
            if matches!(v.get("type").and_then(Value::as_str), Some("paragraph" | "heading" | "listItem")) {
                out.push(' ');
            }
        }
    }
    let mut out = String::new();
    match v {
        Value::String(s) => out.push_str(s),
        _ => walk(v, &mut out),
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone)]
pub struct Me {
    pub account_id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub id: String,
    pub key: String,
    pub summary: String,
    /// Plain text, see `adf_plain`.
    pub description: Option<String>,
    pub status: String,
    /// new | indeterminate | done
    pub category: String,
    pub issue_type: String,
    pub priority: Option<String>,
    pub project: String,
    pub assignee: Option<(String, String)>,
    pub reporter: Option<String>,
    pub created: Option<Ts>,
    pub updated: Option<Ts>,
    pub resolved: Option<Ts>,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub id: String,
    pub author_id: String,
    pub author: String,
    pub at: Ts,
    pub text: String,
    /// Account ids @mentioned in it.
    pub mentions: Vec<String>,
}

/// An Atlassian Document Format body as readable plain text: blocks separated by blank lines, list
/// items as "• " / "1. " lines, mentions and emoji as their text, links as their URL.
pub fn adf_plain(v: &Value) -> String {
    fn inline(v: &Value, out: &mut String) {
        match v.get("type").and_then(Value::as_str) {
            Some("text") => out.push_str(v["text"].as_str().unwrap_or_default()),
            Some("hardBreak") => out.push('\n'),
            Some("mention" | "emoji") => out.push_str(v["attrs"]["text"].as_str().or(v["attrs"]["shortName"].as_str()).unwrap_or_default()),
            Some("inlineCard" | "blockCard") => out.push_str(v["attrs"]["url"].as_str().unwrap_or_default()),
            _ => {
                for c in v.get("content").and_then(Value::as_array).into_iter().flatten() {
                    inline(c, out);
                }
            }
        }
    }
    fn block(v: &Value, indent: &str, out: &mut String) {
        let children = || v.get("content").and_then(Value::as_array).into_iter().flatten();
        match v.get("type").and_then(Value::as_str) {
            Some("doc" | "panel" | "blockquote" | "expand" | "nestedExpand" | "layoutSection" | "layoutColumn") => {
                for c in children() {
                    block(c, indent, out);
                }
            }
            Some(list @ ("bulletList" | "orderedList")) => {
                for (n, item) in children().enumerate() {
                    let marker = if list == "bulletList" { "• ".to_string() } else { format!("{}. ", n + 1) };
                    out.push_str(indent);
                    out.push_str(&marker);
                    let mut first = true;
                    for c in item.get("content").and_then(Value::as_array).into_iter().flatten() {
                        if matches!(c["type"].as_str(), Some("bulletList" | "orderedList")) {
                            out.push('\n');
                            block(c, &format!("{indent}   "), out);
                            first = true;
                            continue;
                        }
                        if !first {
                            out.push(' ');
                        }
                        inline(c, out);
                        first = false;
                    }
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                if indent.is_empty() {
                    out.push('\n');
                }
            }
            Some("rule") => out.push_str("———\n\n"),
            Some("table") => {
                for row in children() {
                    let cells: Vec<String> = row
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|cell| {
                            let mut t = String::new();
                            inline(cell, &mut t);
                            t.trim().to_string()
                        })
                        .collect();
                    out.push_str(&cells.join(" | "));
                    out.push('\n');
                }
                out.push('\n');
            }
            _ => {
                // paragraph, heading, codeBlock, mediaGroup…
                let mut t = String::new();
                inline(v, &mut t);
                if !t.trim().is_empty() {
                    out.push_str(t.trim_end());
                    out.push_str("\n\n");
                }
            }
        }
    }
    let mut out = String::new();
    match v {
        Value::String(s) => out.push_str(s),
        Value::Null => {}
        _ => block(v, "", &mut out),
    }
    out.trim().to_string()
}

/// Account ids @mentioned in an Atlassian Document Format body.
pub fn adf_mentions(v: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        if v.get("type").and_then(Value::as_str) == Some("mention") {
            if let Some(id) = v["attrs"]["id"].as_str() {
                out.push(id.to_string());
            }
        }
        for c in v.get("content").and_then(Value::as_array).into_iter().flatten() {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out
}

/// One movement on a ticket.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub id: String,
    pub issue_key: String,
    pub at: Ts,
    /// status | assignee | resolution | comment | created
    pub kind: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub actor_id: Option<String>,
    pub actor: Option<String>,
    pub detail: String,
}

impl Client {
    pub fn new(site: &str, email: &str, token: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("dev-inbox")
                .timeout(std::time::Duration::from_secs(60))
                .build()?,
            base: normalise_site(site)?,
            email: email.trim().to_string(),
            token: token.trim().to_string(),
        })
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value> {
        let res = req
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .send()
            .await
            .context("Couldn't reach Jira")?;
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(HttpError { status, text: error_text(&body) }.into());
        }
        Ok(body)
    }

    // ---------- the two things Dev Inbox changes in Jira: status and assignee ----------

    /// The moves available for a ticket from its current status (as the user's own permissions allow).
    pub async fn transitions(&self, key: &str) -> Result<Vec<Transition>> {
        let v = self.send(self.http.get(format!("{}/rest/api/3/issue/{key}/transitions", self.base))).await?;
        Ok(v["transitions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["isAvailable"].as_bool() != Some(false))
            .filter_map(|t| {
                Some(Transition {
                    id: t["id"].as_str()?.to_string(),
                    name: t["name"].as_str().unwrap_or_default().to_string(),
                    to: t["to"]["name"].as_str().unwrap_or_default().to_string(),
                    category: t["to"]["statusCategory"]["key"].as_str().unwrap_or("new").to_string(),
                })
            })
            .collect())
    }

    /// Move a ticket. Transitions that need fields filled in (a resolution, a comment…) are refused by
    /// Jira with a message saying which; that message is passed on.
    pub async fn transition(&self, key: &str, transition_id: &str) -> Result<()> {
        let body = json!({ "transition": { "id": transition_id } });
        self.send(self.http.post(format!("{}/rest/api/3/issue/{key}/transitions", self.base)).json(&body)).await?;
        Ok(())
    }

    /// People who can be assigned the ticket, matching `query` (name or email); all of them if empty.
    pub async fn assignable(&self, key: &str, query: &str) -> Result<Vec<JiraUser>> {
        let url = reqwest::Url::parse_with_params(
            &format!("{}/rest/api/3/user/assignable/search", self.base),
            &[("issueKey", key), ("query", query), ("maxResults", "20")],
        )?;
        let v = self.send(self.http.get(url)).await?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter(|u| u["active"].as_bool() != Some(false))
            .filter_map(|u| {
                Some(JiraUser {
                    account_id: u["accountId"].as_str()?.to_string(),
                    name: u["displayName"].as_str().unwrap_or_default().to_string(),
                    avatar: u["avatarUrls"]["24x24"].as_str().map(String::from),
                })
            })
            .collect())
    }

    /// Assign a ticket to someone (None: unassign).
    pub async fn assign(&self, key: &str, account_id: Option<&str>) -> Result<()> {
        let body = json!({ "accountId": account_id });
        self.send(self.http.put(format!("{}/rest/api/3/issue/{key}/assignee", self.base)).json(&body)).await?;
        Ok(())
    }

    /// One ticket and its history, fresh (after changing it, so the app shows the result at once).
    pub async fn issue_with_history(&self, key: &str) -> Result<(Vec<Issue>, Vec<Change>)> {
        let issues = self.search(&format!("key = \"{}\"", key.replace('"', ""))).await?;
        let changes = self.changelogs(&issues).await?;
        Ok((issues, changes))
    }

    /// A Jira project's numeric id (for linking Harvest entries the way Harvest's Jira integration does).
    pub async fn project_id(&self, key: &str) -> Result<String> {
        let v = self.send(self.http.get(format!("{}/rest/api/3/project/{key}", self.base))).await?;
        v["id"].as_str().map(String::from).ok_or_else(|| anyhow!("Jira didn't return the project"))
    }

    pub async fn myself(&self) -> Result<Me> {
        let v = self.send(self.http.get(format!("{}/rest/api/3/myself", self.base))).await?;
        Ok(Me {
            account_id: v["accountId"].as_str().ok_or_else(|| anyhow!("Jira didn't return an account"))?.to_string(),
            name: v["displayName"].as_str().unwrap_or("you").to_string(),
        })
    }

    /// Tickets that involve the user, updated since `since`: assigned to them now or at any point,
    /// reported by them, or moved / edited / commented on by them (`account_id` is theirs).
    pub async fn involved_issues(&self, since: Ts, account_id: &str) -> Result<Vec<Issue>> {
        match self.search(&involved_jql(since, account_id, true)).await {
            Ok(issues) => Ok(issues),
            // updatedBy() is Jira Cloud only and fairly new; if the site rejects it, do without.
            Err(e) if e.to_string().contains("400") => self.search(&involved_jql(since, account_id, false)).await,
            Err(e) => Err(e),
        }
    }

    async fn search(&self, jql: &str) -> Result<Vec<Issue>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..20 {
            let body = json!({
                "jql": jql,
                "fields": ["summary", "description", "status", "issuetype", "priority", "project", "assignee", "reporter",
                           "created", "updated", "resolutiondate", "comment"],
                "maxResults": 100,
                "nextPageToken": token,
            });
            let v = self.send(self.http.post(format!("{}/rest/api/3/search/jql", self.base)).json(&body)).await?;
            for i in v["issues"].as_array().into_iter().flatten() {
                out.push(parse_issue(i));
            }
            if v["isLast"].as_bool() != Some(false) {
                break;
            }
            token = v["nextPageToken"].as_str().map(String::from);
            if token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// Status / assignee / resolution history for the given tickets.
    pub async fn changelogs(&self, issues: &[Issue]) -> Result<Vec<Change>> {
        let key_of: std::collections::HashMap<&str, &str> = issues.iter().map(|i| (i.id.as_str(), i.key.as_str())).collect();
        let mut out = Vec::new();
        for chunk in issues.chunks(1000) {
            let ids: Vec<&str> = chunk.iter().map(|i| i.id.as_str()).collect();
            let mut token: Option<String> = None;
            for _ in 0..50 {
                let body = json!({
                    "issueIdsOrKeys": ids,
                    "fieldIds": ["status", "assignee", "resolution"],
                    "maxResults": 1000,
                    "nextPageToken": token,
                });
                let v = self
                    .send(self.http.post(format!("{}/rest/api/3/changelog/bulkfetch", self.base)).json(&body))
                    .await?;
                for log in v["issueChangeLogs"].as_array().into_iter().flatten() {
                    let Some(key) = log["issueId"].as_str().and_then(|id| key_of.get(id)) else { continue };
                    for h in log["changeHistories"].as_array().into_iter().flatten() {
                        out.extend(parse_history(key, h));
                    }
                }
                token = v["nextPageToken"].as_str().map(String::from);
                if token.is_none() {
                    break;
                }
            }
        }
        Ok(out)
    }

    pub fn browse_url(&self, key: &str) -> String {
        format!("{}/browse/{key}", self.base)
    }
}

/// Jira's error body in words: its messages, then any per-field errors.
fn error_text(body: &Value) -> String {
    let mut parts: Vec<String> = body["errorMessages"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(String::from)).collect();
    if let Some(fields) = body["errors"].as_object() {
        parts.extend(fields.iter().map(|(field, msg)| format!("{field}: {}", msg.as_str().unwrap_or_default())));
    }
    parts.join("; ")
}

/// A move a ticket can make: "Start progress" → In Progress.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub id: String,
    /// The transition's own name ("Start progress").
    pub name: String,
    /// The status it moves to ("In Progress").
    pub to: String,
    /// new | indeterminate | done
    pub category: String,
}

/// Someone a ticket can be assigned to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraUser {
    pub account_id: String,
    pub name: String,
    pub avatar: Option<String>,
}

/// JQL for tickets involving the user. `updated_by` adds `updatedBy()`, which also catches comments and edits.
pub fn involved_jql(since: Ts, account_id: &str, updated_by: bool) -> String {
    let date = since.format("%Y/%m/%d");
    let mut who = vec![
        "assignee WAS currentUser()".to_string(),
        "reporter = currentUser()".to_string(),
        format!("status CHANGED BY currentUser() AFTER \"{date}\""),
    ];
    if updated_by {
        let id = account_id.replace(['"', '\\'], "");
        who.push(format!("issuekey IN updatedBy(\"{id}\", \"{date}\")"));
    }
    format!(
        "({}) AND updated >= \"{}\" ORDER BY updated DESC",
        who.join(" OR "),
        since.format("%Y-%m-%d %H:%M")
    )
}

fn parse_issue(i: &Value) -> Issue {
    let f = &i["fields"];
    let s = |v: &Value| v.as_str().map(String::from);
    let comments = f["comment"]["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(Comment {
                id: c["id"].as_str()?.to_string(),
                author_id: c["author"]["accountId"].as_str().unwrap_or_default().to_string(),
                author: c["author"]["displayName"].as_str().unwrap_or_default().to_string(),
                at: parse_jira_ts(c["created"].as_str()?)?,
                text: adf_text(&c["body"]),
                mentions: adf_mentions(&c["body"]),
            })
        })
        .collect();
    Issue {
        id: i["id"].as_str().unwrap_or_default().to_string(),
        key: i["key"].as_str().unwrap_or_default().to_string(),
        summary: s(&f["summary"]).unwrap_or_default(),
        description: Some(adf_plain(&f["description"])).filter(|d| !d.is_empty()),
        status: s(&f["status"]["name"]).unwrap_or_default(),
        category: s(&f["status"]["statusCategory"]["key"]).unwrap_or_else(|| "new".into()),
        issue_type: s(&f["issuetype"]["name"]).unwrap_or_default(),
        priority: s(&f["priority"]["name"]),
        project: s(&f["project"]["key"]).unwrap_or_default(),
        assignee: f["assignee"]["accountId"]
            .as_str()
            .map(|id| (id.to_string(), f["assignee"]["displayName"].as_str().unwrap_or_default().to_string())),
        reporter: s(&f["reporter"]["accountId"]),
        created: f["created"].as_str().and_then(parse_jira_ts),
        updated: f["updated"].as_str().and_then(parse_jira_ts),
        resolved: f["resolutiondate"].as_str().and_then(parse_jira_ts),
        comments,
    }
}

fn parse_history(key: &str, h: &Value) -> Vec<Change> {
    let Some(at) = h["created"].as_str().and_then(parse_jira_ts) else { return Vec::new() };
    let actor_id = h["author"]["accountId"].as_str().map(String::from);
    let actor = h["author"]["displayName"].as_str().map(String::from);
    let hid = h["id"].as_str().unwrap_or_default();
    h["items"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(n, item)| {
            let kind = match item["fieldId"].as_str().or(item["field"].as_str())? {
                "status" => "status",
                "assignee" => "assignee",
                "resolution" => "resolution",
                _ => return None,
            };
            let from = item["fromString"].as_str().map(String::from);
            let to = item["toString"].as_str().map(String::from);
            let detail = match kind {
                "status" => format!("{} → {}", from.as_deref().unwrap_or("?"), to.as_deref().unwrap_or("?")),
                "assignee" => match &to {
                    Some(t) => format!("Assigned to {t}"),
                    None => "Unassigned".into(),
                },
                _ => match &to {
                    Some(t) => format!("Resolved as {t}"),
                    None => "Reopened".into(),
                },
            };
            Some(Change {
                id: format!("{hid}:{n}"),
                issue_key: key.to_string(),
                at,
                kind: kind.into(),
                // Assignee changes keep account ids so "assigned to you" can be recognised.
                from: if kind == "assignee" { item["from"].as_str().map(String::from).or(from) } else { from },
                to: if kind == "assignee" { item["to"].as_str().map(String::from).or(to) } else { to },
                actor_id: actor_id.clone(),
                actor: actor.clone(),
                detail,
            })
        })
        .collect()
}

/// Ticket creation and comments as changes too: the user's own ("comment"), other people's that
/// @mention the user ("mention"), and other people's ("comment", by someone else).
pub fn issue_changes(i: &Issue, me: &str) -> Vec<Change> {
    let mut out = Vec::new();
    if let (Some(created), Some(reporter)) = (i.created, &i.reporter) {
        out.push(Change {
            id: format!("created:{}", i.key),
            issue_key: i.key.clone(),
            at: created,
            kind: "created".into(),
            from: None,
            to: None,
            actor_id: Some(reporter.clone()),
            actor: None,
            detail: "Created".into(),
        });
    }
    for c in &i.comments {
        let mention = c.author_id != me && c.mentions.iter().any(|m| m == me);
        let snippet: String = c.text.chars().take(140).collect();
        let verb = if mention { "Mentioned you" } else { "Commented" };
        out.push(Change {
            id: format!("comment:{}", c.id),
            issue_key: i.key.clone(),
            at: c.at,
            kind: if mention { "mention" } else { "comment" }.into(),
            from: None,
            to: None,
            actor_id: Some(c.author_id.clone()),
            actor: Some(c.author.clone()),
            detail: if snippet.is_empty() { verb.into() } else { format!("{verb}: {snippet}") },
        });
    }
    out
}

// ---------- storage ----------

pub fn store(conn: &Connection, client: &Client, me: &str, issues: &[Issue], changes: &[Change]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for i in issues {
        tx.execute(
            "INSERT INTO jira_issues (key, id, summary, status, category, issue_type, priority, project, assignee,
                                      assigned_to_me, url, created, updated, resolved, description)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(key) DO UPDATE SET summary = excluded.summary, status = excluded.status,
                description = excluded.description,
                category = excluded.category, issue_type = excluded.issue_type, priority = excluded.priority,
                assignee = excluded.assignee, assigned_to_me = excluded.assigned_to_me, updated = excluded.updated,
                resolved = excluded.resolved",
            params![
                i.key,
                i.id,
                i.summary,
                i.status,
                i.category,
                i.issue_type,
                i.priority,
                i.project,
                i.assignee.as_ref().map(|a| a.1.clone()),
                i.assignee.as_ref().is_some_and(|a| a.0 == me),
                client.browse_url(&i.key),
                i.created.map(fmt_ts),
                i.updated.map(fmt_ts),
                i.resolved.map(fmt_ts),
                i.description,
            ],
        )?;
    }
    for c in changes {
        tx.execute(
            "INSERT OR IGNORE INTO jira_events (id, issue_key, at, kind, from_value, to_value, actor, actor_is_me, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                c.id,
                c.issue_key,
                fmt_ts(c.at),
                c.kind,
                c.from,
                c.to,
                c.actor,
                c.actor_id.as_deref() == Some(me),
                c.detail,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub category: String,
    pub issue_type: String,
    pub priority: Option<String>,
    pub project: String,
    pub assignee: Option<String>,
    pub assigned_to_me: bool,
    /// How the ticket involves the user: assigned | was_assigned | reported | updated
    pub involvement: String,
    pub url: String,
    pub updated: Option<String>,
    pub resolved: Option<String>,
    /// The latest movement, for the list ("You moved it to In Review").
    pub last_move: Option<String>,
    pub last_move_at: Option<String>,
}

pub fn list_tickets(conn: &Connection, me: &str) -> Result<Vec<Ticket>> {
    let mut stmt = conn.prepare(
        "SELECT t.key, t.summary, t.status, t.category, t.issue_type, t.priority, t.project, t.assignee,
                t.assigned_to_me, t.url, t.updated, t.resolved,
                (SELECT detail FROM jira_events e WHERE e.issue_key = t.key AND e.kind NOT IN ('comment', 'mention') ORDER BY e.at DESC LIMIT 1),
                (SELECT at FROM jira_events e WHERE e.issue_key = t.key AND e.kind NOT IN ('comment', 'mention') ORDER BY e.at DESC LIMIT 1),
                CASE
                  WHEN t.assigned_to_me THEN 'assigned'
                  WHEN EXISTS (SELECT 1 FROM jira_events e WHERE e.issue_key = t.key AND e.kind = 'assignee'
                               AND (e.from_value = ?1 OR e.to_value = ?1)) THEN 'was_assigned'
                  WHEN EXISTS (SELECT 1 FROM jira_events e WHERE e.issue_key = t.key AND e.kind = 'created' AND e.actor_is_me) THEN 'reported'
                  ELSE 'updated'
                END
         FROM jira_issues t ORDER BY COALESCE(t.updated, '') DESC",
    )?;
    let rows = stmt
        .query_map([me], |r| {
            Ok(Ticket {
                key: r.get(0)?,
                summary: r.get(1)?,
                status: r.get(2)?,
                category: r.get(3)?,
                issue_type: r.get(4)?,
                priority: r.get(5)?,
                project: r.get(6)?,
                assignee: r.get(7)?,
                assigned_to_me: r.get(8)?,
                url: r.get(9)?,
                updated: r.get(10)?,
                resolved: r.get(11)?,
                last_move: r.get(12)?,
                last_move_at: r.get(13)?,
                involvement: r.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketEvent {
    pub at: String,
    pub kind: String,
    pub detail: String,
    pub actor: Option<String>,
    pub actor_is_me: bool,
    /// For assignee changes: this assigned the ticket to / away from the user.
    pub to_me: bool,
    pub from_me: bool,
}

pub fn ticket_events(conn: &Connection, key: &str, me: &str) -> Result<Vec<TicketEvent>> {
    let mut stmt = conn.prepare(
        "SELECT at, kind, detail, actor, actor_is_me, from_value, to_value FROM jira_events WHERE issue_key = ?1 ORDER BY at DESC",
    )?;
    let rows = stmt
        .query_map([key], |r| {
            let kind: String = r.get(1)?;
            let from: Option<String> = r.get(5)?;
            let to: Option<String> = r.get(6)?;
            let assignee = kind == "assignee";
            Ok(TicketEvent {
                at: r.get(0)?,
                detail: r.get(2)?,
                actor: r.get(3)?,
                actor_is_me: r.get(4)?,
                to_me: assignee && to.as_deref() == Some(me),
                from_me: assignee && from.as_deref() == Some(me),
                kind,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// (summary, url, status) by ticket key, for labelling the Day view.
pub fn titles(conn: &Connection) -> Result<std::collections::HashMap<String, (String, String, String)>> {
    let mut stmt = conn.prepare("SELECT key, summary, url, status FROM jira_issues")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Jira movements in [start, end) for the Day view: (key, at, detail, by_me).
pub fn events_between(conn: &Connection, start: &str, end: &str) -> Result<Vec<(String, String, String, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT issue_key, at, detail, actor_is_me FROM jira_events
         WHERE at >= ?1 AND at < ?2 AND kind NOT IN ('created', 'mention') AND NOT (kind = 'comment' AND NOT actor_is_me)
         ORDER BY at",
    )?;
    let rows = stmt
        .query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// One movement on a ticket, for the Day view's "Ticket moves".
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayMove {
    pub key: String,
    pub summary: String,
    /// The ticket's status now.
    pub status: String,
    pub category: String,
    pub url: String,
    pub at: String,
    /// status | assignee | resolution | comment | created
    pub kind: String,
    pub detail: String,
    pub actor: Option<String>,
    pub actor_is_me: bool,
    /// An assignee change that gave the ticket to the user.
    pub to_me: bool,
}

/// Every movement between `start` and `end` on the tickets that involve the user (`me`), oldest first.
pub fn moves_between(conn: &Connection, start: &str, end: &str, me: &str) -> Result<Vec<DayMove>> {
    let mut stmt = conn.prepare(
        "SELECT e.issue_key, t.summary, t.status, t.category, t.url, e.at, e.kind, e.detail, e.actor, e.actor_is_me,
                COALESCE(e.kind = 'assignee' AND e.to_value = ?3, 0)
         FROM jira_events e JOIN jira_issues t ON t.key = e.issue_key
         WHERE e.at >= ?1 AND e.at < ?2 AND NOT (e.kind = 'comment' AND NOT e.actor_is_me) ORDER BY e.at",
    )?;
    let rows = stmt
        .query_map(params![start, end, me], |r| {
            Ok(DayMove {
                key: r.get(0)?,
                summary: r.get(1)?,
                status: r.get(2)?,
                category: r.get(3)?,
                url: r.get(4)?,
                at: r.get(5)?,
                kind: r.get(6)?,
                detail: r.get(7)?,
                actor: r.get(8)?,
                actor_is_me: r.get(9)?,
                to_me: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------- what needs you ----------

/// A ticket that needs you: something happened on it, by someone else, since you last looked.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub category: String,
    pub url: String,
    /// The latest thing that happened.
    pub at: String,
    /// assignee | mention | comment | status | resolution
    pub kind: String,
    /// "Sam assigned it to you", "Sam moved it In QA → In Progress", "Sam mentioned you: …"
    pub reason: String,
    /// How many things happened since you last looked.
    pub count: usize,
}

/// What another person did, in words.
pub fn describe(kind: &str, detail: &str, actor: Option<&str>) -> String {
    let who = actor.unwrap_or("Someone");
    let lower = |s: &str| format!("{}{}", s.chars().next().map(|c| c.to_lowercase().to_string()).unwrap_or_default(), s.chars().skip(1).collect::<String>());
    match kind {
        "assignee" => format!("{who} assigned it to you"),
        "status" => format!("{who} moved it {detail}"),
        _ => format!("{who} {}", lower(detail)),
    }
}

/// Events that make a ticket need you, newest first. `since` stops
/// history from before you connected counting; anything you did on the ticket afterwards settles it.
const ATTENTION_SQL: &str = "
    SELECT t.key, t.summary, t.status, t.category, t.url, e.at, e.kind, e.detail, e.actor
    FROM jira_events e JOIN jira_issues t ON t.key = e.issue_key
    WHERE NOT e.actor_is_me AND e.at > ?2
      AND e.at > COALESCE(t.seen_at, '')
      AND e.at > COALESCE((SELECT MAX(m.at) FROM jira_events m WHERE m.issue_key = t.key AND m.actor_is_me), '')
      AND (   (e.kind = 'assignee' AND e.to_value = ?1 AND t.assigned_to_me)
           OR e.kind = 'mention'
           OR (e.kind = 'comment' AND (t.assigned_to_me OR EXISTS (
                 SELECT 1 FROM jira_events c WHERE c.issue_key = t.key AND c.kind = 'created' AND c.actor_is_me)))
           OR (e.kind IN ('status', 'resolution') AND t.assigned_to_me))
      {extra}
    ORDER BY e.at DESC";

fn attention_rows(conn: &Connection, me: &str, since: &str, extra: &str) -> Result<Vec<Attention>> {
    let mut stmt = conn.prepare(&ATTENTION_SQL.replace("{extra}", extra))?;
    let rows = stmt
        .query_map(params![me, since], |r| {
            let kind: String = r.get(6)?;
            let detail: String = r.get(7)?;
            let actor: Option<String> = r.get(8)?;
            Ok(Attention {
                key: r.get(0)?,
                summary: r.get(1)?,
                status: r.get(2)?,
                category: r.get(3)?,
                url: r.get(4)?,
                at: r.get(5)?,
                reason: describe(&kind, &detail, actor.as_deref()),
                kind,
                count: 1,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Tickets that need you, most recent first; each shows its latest event and how many there were.
pub fn attention(conn: &Connection, me: &str, since: &str) -> Result<Vec<Attention>> {
    let mut out: Vec<Attention> = Vec::new();
    for item in attention_rows(conn, me, since, "")? {
        match out.iter_mut().find(|a| a.key == item.key) {
            Some(a) => a.count += 1,
            None => out.push(item),
        }
    }
    Ok(out)
}

/// New events that need you and haven't been notified yet; marks every event as notified.
pub fn take_notifications(conn: &Connection, me: &str, since: &str) -> Result<Vec<Attention>> {
    let fresh = attention_rows(conn, me, since, "AND NOT e.notified")?;
    conn.execute("UPDATE jira_events SET notified = 1 WHERE NOT notified", [])?;
    Ok(fresh)
}

/// Mark everything so far as notified (the first sync shouldn't notify 30 days of history).
pub fn mark_all_notified(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE jira_events SET notified = 1 WHERE NOT notified", [])?;
    Ok(())
}

/// You've looked at the ticket: whatever happened so far no longer needs you. True if it did.
pub fn mark_seen(conn: &Connection, key: &str, me: &str, since: &str, now: &str) -> Result<bool> {
    let needed = attention(conn, me, since)?.iter().any(|a| a.key == key);
    conn.execute("UPDATE jira_issues SET seen_at = ?2 WHERE key = ?1", params![key, now])?;
    Ok(needed)
}

/// A ticket's description, if it has one.
pub fn description(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT description FROM jira_issues WHERE key = ?1", [key], |r| r.get(0))
        .optional()?
        .flatten())
}

pub fn forget(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM jira_issues; DELETE FROM jira_events;
         DELETE FROM settings WHERE key IN ('jira_site', 'jira_email', 'jira_account', 'jira_name', 'jira_last_sync', 'jira_error', 'jira_scope', 'jira_attention_since');",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn involved_jql_covers_assigned_reported_and_updated() {
        let since = parse_jira_ts("2026-09-16T08:00:00.000+0000").unwrap();
        let q = involved_jql(since, "abc\"123", true);
        assert_eq!(
            q,
            "(assignee WAS currentUser() OR reporter = currentUser() OR status CHANGED BY currentUser() AFTER \"2026/09/16\" \
             OR issuekey IN updatedBy(\"abc123\", \"2026/09/16\")) AND updated >= \"2026-09-16 08:00\" ORDER BY updated DESC"
        );
        assert!(!involved_jql(since, "abc", false).contains("updatedBy"));
    }

    #[test]
    fn description_as_plain_text() {
        let t = |s: &str| json!({"type": "text", "text": s});
        let p = |c: Value| json!({"type": "paragraph", "content": c});
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"level": 2}, "content": [t("Problem")]},
            p(json!([t("Receipts use the "), {"type": "text", "text": "UK", "marks": [{"type": "strong"}]}, t(" rate."), {"type": "hardBreak"}, t("Ask "), {"type": "mention", "attrs": {"id": "x", "text": "@Sam"}}])),
            {"type": "bulletList", "content": [
                {"type": "listItem", "content": [p(json!([t("Germany")]))]},
                {"type": "listItem", "content": [p(json!([t("France")])), {"type": "orderedList", "content": [
                    {"type": "listItem", "content": [p(json!([t("Paris")]))]}]}]}]},
            p(json!([{"type": "inlineCard", "attrs": {"url": "https://example.com/x"}}])),
        ]});
        assert_eq!(
            adf_plain(&doc),
            "Problem\n\nReceipts use the UK rate.\nAsk @Sam\n\n• Germany\n• France\n   1. Paris\n\nhttps://example.com/x"
        );
        assert_eq!(adf_plain(&Value::Null), "");
    }

    #[test]
    fn finds_mentions() {
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "mention", "attrs": {"id": "me", "text": "@Harry"}}, {"type": "text", "text": " can you check?"}]}]});
        assert_eq!(adf_mentions(&body), ["me"]);
    }

    #[test]
    fn jira_errors_tell_a_dead_login_from_a_missing_ticket() {
        let err = |status: reqwest::StatusCode, text: &str| -> anyhow::Error { HttpError { status, text: text.into() }.into() };
        let gone = err(reqwest::StatusCode::NOT_FOUND, "Issue does not exist or you do not have permission to see it.");
        assert!(not_found(&gone) && !login_rejected(&gone));
        assert_eq!(gone.to_string(), "Jira returned 404 Not Found Issue does not exist or you do not have permission to see it.");
        let dead = err(reqwest::StatusCode::UNAUTHORIZED, "");
        assert!(login_rejected(&dead) && !not_found(&dead));
        assert_eq!(dead.to_string(), "Jira rejected the email or API token (401 Unauthorized)", "the message connecting shows");
        assert!(login_rejected(&err(reqwest::StatusCode::FORBIDDEN, "")));
        let offline = anyhow!("Couldn't reach Jira");
        assert!(!not_found(&offline) && !login_rejected(&offline));
    }

    #[test]
    fn what_needs_you() {
        let path = std::env::temp_dir().join(format!("dev-inbox-attention-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        let ticket = |key: &str, mine: bool| {
            conn.execute(
                "INSERT INTO jira_issues (key, id, summary, status, category, issue_type, project, assigned_to_me, url)
                 VALUES (?1, ?1, 'S', 'In Progress', 'indeterminate', 'Story', 'ES', ?2, 'u')",
                params![key, mine],
            )
            .unwrap();
        };
        let event = |id: &str, key: &str, at: &str, kind: &str, by_me: bool, to: Option<&str>| {
            conn.execute(
                "INSERT INTO jira_events (id, issue_key, at, kind, to_value, actor, actor_is_me, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'In QA → In Progress')",
                params![id, key, at, kind, to, if by_me { "You" } else { "Sam" }, by_me],
            )
            .unwrap();
        };
        ticket("ES-1", true); // yours: moved back by Sam
        event("a", "ES-1", "2026-10-16T09:00:00Z", "status", false, None);
        ticket("ES-2", true); // Sam assigned it to you, then you moved it: settled
        event("b", "ES-2", "2026-10-16T09:00:00Z", "assignee", false, Some("me"));
        event("c", "ES-2", "2026-10-16T09:30:00Z", "status", true, None);
        ticket("ES-3", false); // not yours: a status move doesn't need you, a mention does
        event("d", "ES-3", "2026-10-16T10:00:00Z", "status", false, None);
        event("e", "ES-3", "2026-10-16T11:00:00Z", "mention", false, None);
        ticket("ES-4", true); // before you connected
        event("f", "ES-4", "2026-10-01T09:00:00Z", "status", false, None);

        let since = "2026-10-15T00:00:00Z";
        let keys = |a: Vec<Attention>| a.into_iter().map(|a| a.key).collect::<Vec<_>>();
        assert_eq!(keys(attention(&conn, "me", since).unwrap()), ["ES-3", "ES-1"]);
        let es3 = attention(&conn, "me", since).unwrap().remove(0);
        assert_eq!(es3.kind, "mention");
        assert_eq!(es3.count, 1, "the status move on someone else's ticket isn't counted");
        assert_eq!(attention(&conn, "me", since).unwrap()[1].reason, "Sam moved it In QA → In Progress");

        assert_eq!(keys(take_notifications(&conn, "me", since).unwrap()), ["ES-3", "ES-1"]);
        assert!(take_notifications(&conn, "me", since).unwrap().is_empty(), "each event notifies once");

        assert!(mark_seen(&conn, "ES-1", "me", since, "2026-10-16T12:00:00Z").unwrap());
        assert_eq!(keys(attention(&conn, "me", since).unwrap()), ["ES-3"]);
        event("g", "ES-1", "2026-10-16T13:00:00Z", "comment", false, None);
        assert_eq!(keys(attention(&conn, "me", since).unwrap()), ["ES-1", "ES-3"], "something new brings it back");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn jira_errors_in_words() {
        let body = json!({"errorMessages": ["You can't do that"], "errors": {"resolution": "Resolution is required."}});
        assert_eq!(error_text(&body), "You can't do that; resolution: Resolution is required.");
        assert_eq!(error_text(&Value::Null), "");
    }

    #[test]
    fn sites_and_dates() {
        assert_eq!(normalise_site("fullfatthings.atlassian.net").unwrap(), "https://fullfatthings.atlassian.net");
        assert_eq!(
            normalise_site("https://fullfatthings.atlassian.net/jira/software/c/projects/ES/boards/153").unwrap(),
            "https://fullfatthings.atlassian.net"
        );
        assert!(normalise_site("nope").is_err());
        let t = parse_jira_ts("2026-10-14T10:12:33.123+0100").unwrap();
        assert_eq!(fmt_ts(t), "2026-10-14T09:12:33Z");
    }

    #[test]
    fn parses_history_and_comments() {
        let h = json!({"id": "1001", "created": "2026-10-14T10:12:33.123+0100",
            "author": {"accountId": "me", "displayName": "Harry"},
            "items": [
                {"field": "status", "fieldId": "status", "fromString": "In Progress", "toString": "In Review"},
                {"field": "assignee", "fieldId": "assignee", "from": "other", "fromString": "Sam", "to": "me", "toString": "Harry"},
                {"field": "labels", "fieldId": "labels", "fromString": "", "toString": "x"}
            ]});
        let c = parse_history("ES-12", &h);
        assert_eq!(c.len(), 2, "unrelated fields are ignored");
        assert_eq!(c[0].detail, "In Progress → In Review");
        assert_eq!(c[1].to.as_deref(), Some("me"));
        assert_eq!(c[1].detail, "Assigned to Harry");

        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Looks"}, {"type": "text", "text": " good"}]}]});
        assert_eq!(adf_text(&body), "Looks good");
    }
}
