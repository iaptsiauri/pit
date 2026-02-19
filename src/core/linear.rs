//! Fetch issue data from Linear's GraphQL API.
//!
//! Linear issue URLs look like:
//!   https://linear.app/team-slug/issue/PROJ-123/optional-title-slug
//!
//! We extract the issue identifier (e.g. "PROJ-123") and query Linear's API.

use anyhow::{bail, Context, Result};

/// Parsed issue data from Linear.
#[derive(Debug, Clone)]
pub struct LinearIssue {
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub state: String,
    pub priority_label: String,
    pub url: String,
}

/// Extract the issue identifier from a Linear URL.
/// Returns e.g. "PROJ-123" from "https://linear.app/team/issue/PROJ-123/some-title"
pub fn parse_issue_id(url: &str) -> Option<String> {
    let url = url.trim();
    // Match: linear.app/<team>/issue/<ID>/...
    if !url.contains("linear.app") {
        return None;
    }
    let parts: Vec<&str> = url.split('/').collect();
    // Find "issue" segment, the next one is the ID
    for (i, part) in parts.iter().enumerate() {
        if *part == "issue" {
            if let Some(id) = parts.get(i + 1) {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

/// Check if a URL looks like a Linear issue URL.
pub fn is_linear_url(url: &str) -> bool {
    parse_issue_id(url).is_some()
}

/// Fetch an issue from Linear by identifier (e.g. "PROJ-123").
/// Requires `LINEAR_API_KEY` environment variable.
pub fn fetch_issue(identifier: &str) -> Result<LinearIssue> {
    let api_key = super::config::get("linear.api_key")
        .context("Linear API key not set. Run: pit config set linear.api_key <your-key>")?;

    let _query = format!(
        r#"{{
            "query": "query {{ issue(id: \"{id}\") {{ identifier title description state {{ name }} priority priorityLabel url }} }}"
        }}"#,
        id = identifier,
    );

    // Linear's GraphQL also supports filtering by identifier
    // But issue() takes the UUID, not the identifier.
    // We need to use issueSearch or issues filter instead.
    let query = serde_json::json!({
        "query": r#"
            query($filter: IssueFilter!) {
                issues(filter: $filter, first: 1) {
                    nodes {
                        identifier
                        title
                        description
                        state { name }
                        priorityLabel
                        url
                    }
                }
            }
        "#,
        "variables": {
            "filter": {
                "identifier": { "eq": identifier }
            }
        }
    });

    let resp = ureq::post("https://api.linear.app/graphql")
        .set("Authorization", &api_key)
        .set("Content-Type", "application/json")
        .send_string(&query.to_string())
        .context("failed to call Linear API")?;

    let body: serde_json::Value = resp
        .into_json()
        .context("failed to parse Linear response")?;

    // Check for errors
    if let Some(errors) = body.get("errors") {
        bail!("Linear API error: {}", errors);
    }

    let nodes = body
        .pointer("/data/issues/nodes")
        .and_then(|n| n.as_array())
        .context("unexpected Linear response shape")?;

    let node = nodes
        .first()
        .context(format!("issue '{}' not found in Linear", identifier))?;

    Ok(LinearIssue {
        identifier: node["identifier"].as_str().unwrap_or("").to_string(),
        title: node["title"].as_str().unwrap_or("").to_string(),
        description: node["description"].as_str().unwrap_or("").to_string(),
        state: node
            .pointer("/state/name")
            .and_then(|s| s.as_str())
            .unwrap_or("Unknown")
            .to_string(),
        priority_label: node["priorityLabel"].as_str().unwrap_or("").to_string(),
        url: node["url"].as_str().unwrap_or("").to_string(),
    })
}

/// Search for issues in Linear using a text query.
/// Returns up to `limit` matching issues, sorted by relevance.
pub fn search_issues(query: &str, limit: usize) -> Result<Vec<LinearIssue>> {
    let api_key = super::config::get("linear.api_key")
        .context("Linear API key not set. Run: pit config set linear.api_key <your-key>")?;

    let gql = serde_json::json!({
        "query": r#"
            query($query: String!, $first: Int!) {
                issueSearch(query: $query, first: $first, orderBy: updatedAt) {
                    nodes {
                        identifier
                        title
                        description
                        state { name }
                        priorityLabel
                        url
                    }
                }
            }
        "#,
        "variables": {
            "query": query,
            "first": limit,
        }
    });

    let resp = ureq::post("https://api.linear.app/graphql")
        .set("Authorization", &api_key)
        .set("Content-Type", "application/json")
        .send_string(&gql.to_string())
        .context("failed to call Linear API")?;

    let body: serde_json::Value = resp
        .into_json()
        .context("failed to parse Linear response")?;

    if let Some(errors) = body.get("errors") {
        bail!("Linear API error: {}", errors);
    }

    let nodes = body
        .pointer("/data/issueSearch/nodes")
        .and_then(|n| n.as_array())
        .context("unexpected Linear search response")?;

    let issues = nodes
        .iter()
        .map(|node| LinearIssue {
            identifier: node["identifier"].as_str().unwrap_or("").to_string(),
            title: node["title"].as_str().unwrap_or("").to_string(),
            description: node["description"].as_str().unwrap_or("").to_string(),
            state: node
                .pointer("/state/name")
                .and_then(|s| s.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            priority_label: node["priorityLabel"].as_str().unwrap_or("").to_string(),
            url: node["url"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    Ok(issues)
}

/// Fetch my assigned issues from Linear (current user's active issues).
pub fn my_issues(limit: usize) -> Result<Vec<LinearIssue>> {
    let api_key = super::config::get("linear.api_key")
        .context("Linear API key not set. Run: pit config set linear.api_key <your-key>")?;

    let gql = serde_json::json!({
        "query": r#"
            query($first: Int!) {
                viewer {
                    assignedIssues(
                        first: $first,
                        filter: {
                            state: { type: { nin: ["completed", "cancelled"] } }
                        },
                        orderBy: updatedAt
                    ) {
                        nodes {
                            identifier
                            title
                            description
                            state { name }
                            priorityLabel
                            url
                        }
                    }
                }
            }
        "#,
        "variables": {
            "first": limit,
        }
    });

    let resp = ureq::post("https://api.linear.app/graphql")
        .set("Authorization", &api_key)
        .set("Content-Type", "application/json")
        .send_string(&gql.to_string())
        .context("failed to call Linear API")?;

    let body: serde_json::Value = resp
        .into_json()
        .context("failed to parse Linear response")?;

    if let Some(errors) = body.get("errors") {
        bail!("Linear API error: {}", errors);
    }

    let nodes = body
        .pointer("/data/viewer/assignedIssues/nodes")
        .and_then(|n| n.as_array())
        .context("unexpected Linear response")?;

    let issues = nodes
        .iter()
        .map(|node| LinearIssue {
            identifier: node["identifier"].as_str().unwrap_or("").to_string(),
            title: node["title"].as_str().unwrap_or("").to_string(),
            description: node["description"].as_str().unwrap_or("").to_string(),
            state: node
                .pointer("/state/name")
                .and_then(|s| s.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            priority_label: node["priorityLabel"].as_str().unwrap_or("").to_string(),
            url: node["url"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    Ok(issues)
}

/// Fetch an issue from a Linear URL. Combines parse + fetch.
pub fn fetch_from_url(url: &str) -> Result<LinearIssue> {
    let identifier = parse_issue_id(url).context("not a valid Linear issue URL")?;
    fetch_issue(&identifier)
}

/// Build a prompt from a Linear issue.
pub fn issue_to_prompt(issue: &LinearIssue) -> String {
    let mut prompt = format!("{}: {}", issue.identifier, issue.title);
    if !issue.description.is_empty() {
        // Truncate very long descriptions
        let desc: String = issue.description.chars().take(2000).collect();
        prompt.push_str(&format!("\n\n{}", desc));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_url() {
        let url = "https://linear.app/myteam/issue/PROJ-123/fix-login-bug";
        assert_eq!(parse_issue_id(url), Some("PROJ-123".to_string()));
    }

    #[test]
    fn parse_url_without_title_slug() {
        let url = "https://linear.app/myteam/issue/ENG-42";
        assert_eq!(parse_issue_id(url), Some("ENG-42".to_string()));
    }

    #[test]
    fn parse_url_with_trailing_slash() {
        let url = "https://linear.app/myteam/issue/BUG-7/";
        assert_eq!(parse_issue_id(url), Some("BUG-7".to_string()));
    }

    #[test]
    fn parse_not_linear_url() {
        assert_eq!(
            parse_issue_id("https://github.com/org/repo/issues/42"),
            None
        );
        assert_eq!(parse_issue_id("not a url"), None);
        assert_eq!(parse_issue_id(""), None);
    }

    #[test]
    fn parse_url_with_whitespace() {
        let url = "  https://linear.app/team/issue/X-1/title  ";
        assert_eq!(parse_issue_id(url), Some("X-1".to_string()));
    }

    #[test]
    fn is_linear_url_works() {
        assert!(is_linear_url("https://linear.app/t/issue/A-1/title"));
        assert!(!is_linear_url("https://github.com/org/repo/issues/1"));
        assert!(!is_linear_url(""));
    }

    #[test]
    fn issue_to_prompt_title_only() {
        let issue = LinearIssue {
            identifier: "ENG-42".to_string(),
            title: "Fix login timeout".to_string(),
            description: String::new(),
            state: "In Progress".to_string(),
            priority_label: "High".to_string(),
            url: "https://linear.app/t/issue/ENG-42".to_string(),
        };
        assert_eq!(issue_to_prompt(&issue), "ENG-42: Fix login timeout");
    }

    #[test]
    fn issue_to_prompt_with_description() {
        let issue = LinearIssue {
            identifier: "ENG-42".to_string(),
            title: "Fix login timeout".to_string(),
            description: "Users on SSO see a 30s timeout.".to_string(),
            state: "In Progress".to_string(),
            priority_label: "High".to_string(),
            url: "https://linear.app/t/issue/ENG-42".to_string(),
        };
        let prompt = issue_to_prompt(&issue);
        assert!(prompt.starts_with("ENG-42: Fix login timeout"));
        assert!(prompt.contains("Users on SSO see a 30s timeout."));
    }

    #[test]
    fn issue_to_prompt_truncates_long_description() {
        let issue = LinearIssue {
            identifier: "X-1".to_string(),
            title: "Big issue".to_string(),
            description: "a".repeat(5000),
            state: "Todo".to_string(),
            priority_label: "".to_string(),
            url: String::new(),
        };
        let prompt = issue_to_prompt(&issue);
        // Title + \n\n + 2000 chars
        assert!(prompt.len() < 2100);
    }
}
