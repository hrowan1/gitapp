//! Harvest (time tracking), for submitting the timesheet the Day view estimates. Personal access
//! token + account id (token in the keychain). Entries are linked to their Jira ticket the same way
//! Harvest's own Jira integration links them, learnt from entries the user already logged that way,
//! so they show up on the ticket in Jira's Harvest panel.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const BASE: &str = "https://api.harvestapp.com/v2";

pub struct Client {
    http: reqwest::Client,
    token: String,
    account_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub id: i64,
    pub name: String,
}

/// A Harvest project ("bucket") the user can log to, with its tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub code: Option<String>,
    pub client: Option<String>,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub name: String,
}

/// A time entry already in Harvest.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub spent_date: String,
    pub hours: f64,
    pub notes: String,
    /// (id, group_id, account_id, permalink)
    pub reference: Option<(String, String, String, String)>,
}

impl Client {
    pub fn new(account_id: &str, token: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?,
            token: token.trim().to_string(),
            account_id: account_id.trim().to_string(),
        })
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value> {
        let res = req
            .bearer_auth(&self.token)
            .header("Harvest-Account-Id", &self.account_id)
            .header("User-Agent", "Dev Inbox (https://github.com/hrowan1/gitapp)")
            .header("Accept", "application/json")
            .send()
            .await
            .context("Couldn't reach Harvest")?;
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!("Harvest rejected the account ID or token ({status})");
        }
        if !status.is_success() {
            let msg = body["message"].as_str().or(body["error_description"].as_str()).unwrap_or("");
            bail!("Harvest returned {status} {msg}");
        }
        Ok(body)
    }

    /// Follow `links.next` through every page of a list.
    async fn all(&self, first: String, field: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut url = Some(first);
        for _ in 0..50 {
            let Some(u) = url.take() else { break };
            let v = self.send(self.http.get(&u)).await?;
            out.extend(v[field].as_array().cloned().unwrap_or_default());
            url = v["links"]["next"].as_str().map(String::from);
        }
        Ok(out)
    }

    pub async fn me(&self) -> Result<Me> {
        let v = self.send(self.http.get(format!("{BASE}/users/me"))).await?;
        let name = format!("{} {}", v["first_name"].as_str().unwrap_or(""), v["last_name"].as_str().unwrap_or("")).trim().to_string();
        Ok(Me { id: v["id"].as_i64().context("Harvest didn't return a user")?, name })
    }

    /// The active projects and tasks the user can log time to.
    pub async fn projects(&self) -> Result<Vec<Project>> {
        let rows = self.all(format!("{BASE}/users/me/project_assignments?per_page=2000"), "project_assignments").await?;
        let mut out: Vec<Project> = rows
            .iter()
            .filter(|a| a["is_active"].as_bool() != Some(false))
            .filter_map(|a| {
                Some(Project {
                    id: a["project"]["id"].as_i64()?,
                    name: a["project"]["name"].as_str()?.to_string(),
                    code: a["project"]["code"].as_str().filter(|c| !c.is_empty()).map(String::from),
                    client: a["client"]["name"].as_str().map(String::from),
                    tasks: a["task_assignments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|t| t["is_active"].as_bool() != Some(false))
                        .filter_map(|t| Some(Task { id: t["task"]["id"].as_i64()?, name: t["task"]["name"].as_str()?.to_string() }))
                        .collect(),
                })
            })
            .collect();
        out.sort_by_key(|p| p.name.to_lowercase());
        Ok(out)
    }

    /// The user's entries between two dates (inclusive, YYYY-MM-DD).
    pub async fn entries(&self, user_id: i64, from: &str, to: &str) -> Result<Vec<Entry>> {
        let rows = self
            .all(format!("{BASE}/time_entries?user_id={user_id}&from={from}&to={to}&per_page=2000"), "time_entries")
            .await?;
        Ok(rows.iter().filter_map(parse_entry).collect())
    }

    /// Recent entries that are linked to something (to learn how Jira links look).
    pub async fn recent_linked(&self, user_id: i64) -> Result<Vec<Entry>> {
        let to = chrono::Local::now().date_naive();
        let from = to - chrono::Duration::days(120);
        Ok(self.entries(user_id, &from.to_string(), &to.to_string()).await?.into_iter().filter(|e| e.reference.is_some()).collect())
    }

    /// Create an entry (duration-based); returns its id.
    pub async fn create(&self, entry: &NewEntry) -> Result<i64> {
        let v = self.send(self.http.post(format!("{BASE}/time_entries")).json(&entry.body())).await?;
        v["id"].as_i64().context("Harvest didn't return the new entry")
    }

    /// Update an entry this app created earlier.
    pub async fn update(&self, id: i64, entry: &NewEntry) -> Result<()> {
        self.send(self.http.patch(format!("{BASE}/time_entries/{id}")).json(&entry.body())).await?;
        Ok(())
    }

    /// Remove an entry this app created (when its hours are set back to nothing).
    pub async fn delete(&self, id: i64) -> Result<()> {
        self.send(self.http.delete(format!("{BASE}/time_entries/{id}"))).await?;
        Ok(())
    }
}

fn parse_entry(v: &Value) -> Option<Entry> {
    let r = &v["external_reference"];
    let s = |x: &Value| match x {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    Some(Entry {
        id: v["id"].as_i64()?,
        spent_date: v["spent_date"].as_str()?.to_string(),
        hours: v["hours"].as_f64().unwrap_or(0.0),
        notes: v["notes"].as_str().unwrap_or_default().to_string(),
        reference: r.is_object().then(|| (s(&r["id"]), s(&r["group_id"]), s(&r["account_id"]), s(&r["permalink"]))),
    })
}

/// An entry to create or update.
pub struct NewEntry {
    pub project_id: i64,
    pub task_id: i64,
    pub spent_date: String,
    pub hours: f64,
    pub notes: String,
    pub reference: Option<Value>,
}

impl NewEntry {
    fn body(&self) -> Value {
        let mut b = json!({
            "project_id": self.project_id,
            "task_id": self.task_id,
            "spent_date": self.spent_date,
            "hours": self.hours,
            "notes": self.notes,
        });
        if let Some(r) = &self.reference {
            b["external_reference"] = r.clone();
        }
        b
    }
}

// ---------- linking entries to Jira the way Harvest's Jira integration does ----------

/// Where one field of the Jira link comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Source {
    IssueId,
    IssueKey,
    ProjectId,
    ProjectKey,
    /// The same value on every Jira-linked entry (e.g. the Jira site's id).
    Literal(String),
    None,
}

/// How Harvest's Jira integration fills in `external_reference`, learnt from existing entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkRule {
    pub id: Source,
    pub group_id: Source,
    pub account_id: Source,
    /// Whether it was learnt from real entries (rather than the default guess).
    pub learnt: bool,
}

impl Default for LinkRule {
    fn default() -> Self {
        Self { id: Source::IssueId, group_id: Source::ProjectId, account_id: Source::None, learnt: false }
    }
}

/// What's known about a Jira ticket when matching or building links.
pub struct Ticket<'a> {
    pub key: &'a str,
    pub issue_id: &'a str,
    pub project_key: &'a str,
    pub project_id: Option<&'a str>,
    pub url: &'a str,
}

/// Learn the rule from Jira-linked entries (each paired with its ticket, when known).
pub fn learn(examples: &[((String, String, String, String), Ticket)]) -> LinkRule {
    if examples.is_empty() {
        return LinkRule::default();
    }
    let field = |pick: fn(&(String, String, String, String)) -> &String| -> Source {
        let values: Vec<(&String, &Ticket)> = examples.iter().map(|(r, t)| (pick(r), t)).collect();
        let all = |f: &dyn Fn(&String, &Ticket) -> bool| values.iter().all(|(v, t)| f(v, t));
        if all(&|v, _| v.is_empty()) {
            Source::None
        } else if all(&|v, t| v == t.issue_id) {
            Source::IssueId
        } else if all(&|v, t| v == t.key) {
            Source::IssueKey
        } else if all(&|v, t| v == t.project_key) {
            Source::ProjectKey
        } else if all(&|v, t| t.project_id.is_some_and(|p| v == p)) {
            Source::ProjectId
        } else if values.iter().all(|(v, _)| *v == values[0].0) {
            Source::Literal(values[0].0.clone())
        } else if all(&|v, _| v.chars().all(|c| c.is_ascii_digit())) {
            // Numeric, differing per project: most likely the Jira project id.
            Source::ProjectId
        } else {
            Source::None
        }
    };
    LinkRule { id: field(|r| &r.0), group_id: field(|r| &r.1), account_id: field(|r| &r.2), learnt: true }
}

/// The `external_reference` for a ticket under a rule.
pub fn link(rule: &LinkRule, t: &Ticket) -> Value {
    let value = |s: &Source| -> Option<String> {
        match s {
            Source::IssueId => Some(t.issue_id.to_string()),
            Source::IssueKey => Some(t.key.to_string()),
            Source::ProjectKey => Some(t.project_key.to_string()),
            Source::ProjectId => t.project_id.map(String::from),
            Source::Literal(v) => Some(v.clone()),
            Source::None => None,
        }
    };
    let mut r = json!({ "permalink": t.url });
    for (name, source) in [("id", &rule.id), ("group_id", &rule.group_id), ("account_id", &rule.account_id)] {
        if let Some(v) = value(source) {
            r[name] = json!(v);
        }
    }
    r
}

/// The Jira ticket key an entry is about: from its link, else from the start of its notes.
pub fn ticket_of(e: &Entry) -> Option<String> {
    if let Some((_, _, _, permalink)) = &e.reference {
        if let Some(key) = permalink.rsplit("/browse/").next().filter(|_| permalink.contains("/browse/")) {
            return Some(key.trim_end_matches('/').to_uppercase());
        }
    }
    crate::day::ticket_in_text(&e.notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, group: &str, account: &str, key: &str) -> (String, String, String, String) {
        (id.into(), group.into(), account.into(), format!("https://acme.atlassian.net/browse/{key}"))
    }

    #[test]
    fn learns_the_jira_link_shape() {
        let t1 = Ticket { key: "HAIR-12", issue_id: "10012", project_key: "HAIR", project_id: None, url: "u" };
        let t2 = Ticket { key: "BILL-142", issue_id: "20142", project_key: "BILL", project_id: None, url: "u" };
        let rule = learn(&[(r("10012", "10001", "site-1", "HAIR-12"), t1), (r("20142", "10002", "site-1", "BILL-142"), t2)]);
        assert_eq!(rule.id, Source::IssueId);
        assert_eq!(rule.group_id, Source::ProjectId, "numeric and per project");
        assert_eq!(rule.account_id, Source::Literal("site-1".into()), "the same everywhere");
        assert!(rule.learnt);

        let t = Ticket { key: "HAIR-99", issue_id: "10099", project_key: "HAIR", project_id: Some("10001"), url: "https://acme.atlassian.net/browse/HAIR-99" };
        assert_eq!(
            link(&rule, &t),
            json!({"id": "10099", "group_id": "10001", "account_id": "site-1", "permalink": "https://acme.atlassian.net/browse/HAIR-99"})
        );
        assert!(!learn(&[]).learnt);
    }

    #[test]
    fn finds_the_ticket_an_entry_is_about() {
        let e = |reference: Option<(String, String, String, String)>, notes: &str| Entry {
            id: 1,
            spent_date: "2026-10-16".into(),
            hours: 1.0,
            notes: notes.into(),
            reference,
        };
        assert_eq!(ticket_of(&e(Some(r("1", "2", "", "hair-12")), "")).as_deref(), Some("HAIR-12"));
        assert_eq!(ticket_of(&e(None, "BILL-142 Prorate plan changes")).as_deref(), Some("BILL-142"));
        assert_eq!(ticket_of(&e(None, "Standup")), None);
    }
}
