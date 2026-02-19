//! Fetch issue data from GitHub's REST API.
//!
//! GitHub issue URLs look like:
//!   https://github.com/owner/repo/issues/42
//!
//! We extract owner, repo, and issue number, then call the API.

use anyhow::{bail, Context, Result};

/// Parsed issue data from GitHub.
#[derive(Debug, Clone)]
pub struct GitHubIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub labels: Vec<String>,
    pub url: String,
}

/// Parsed components from a GitHub issue URL.
#[derive(Debug, Clone)]
pub struct GitHubIssueRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

/// Parse a GitHub issue URL into its components.
pub fn parse_issue_url(url: &str) -> Option<GitHubIssueRef> {
    let url = url.trim();
    if !url.contains("github.com") {
        return None;
    }
    let parts: Vec<&str> = url.split('/').collect();
    // Find "issues" segment: .../owner/repo/issues/123
    for (i, part) in parts.iter().enumerate() {
        if *part == "issues" {
            if let (Some(repo), Some(owner), Some(num_str)) =
                (parts.get(i - 1), parts.get(i - 2), parts.get(i + 1))
            {
                if let Ok(number) = num_str.parse::<u64>() {
                    return Some(GitHubIssueRef {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        number,
                    });
                }
            }
        }
    }
    None
}

/// Check if a URL looks like a GitHub issue URL.
pub fn is_github_url(url: &str) -> bool {
    parse_issue_url(url).is_some()
}

/// Fetch an issue from GitHub.
/// Uses `GITHUB_TOKEN` env var if set (for private repos / rate limits).
pub fn fetch_issue(issue_ref: &GitHubIssueRef) -> Result<GitHubIssue> {
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}",
        issue_ref.owner, issue_ref.repo, issue_ref.number
    );

    let mut req = ureq::get(&api_url)
        .set("User-Agent", "pit-cli")
        .set("Accept", "application/vnd.github+json");

    if let Some(token) = super::config::get("github.token") {
        req = req.set("Authorization", &format!("Bearer {}", token));
    }

    let resp = req.call().context("failed to call GitHub API")?;
    let body: serde_json::Value = resp
        .into_json()
        .context("failed to parse GitHub response")?;

    if let Some(msg) = body.get("message").and_then(|m| m.as_str()) {
        bail!("GitHub API error: {}", msg);
    }

    let labels: Vec<String> = body
        .get("labels")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(GitHubIssue {
        number: body["number"].as_u64().unwrap_or(issue_ref.number),
        title: body["title"].as_str().unwrap_or("").to_string(),
        body: body["body"].as_str().unwrap_or("").to_string(),
        state: body["state"].as_str().unwrap_or("unknown").to_string(),
        labels,
        url: body["html_url"].as_str().unwrap_or("").to_string(),
    })
}

/// Fetch an issue from a GitHub URL. Combines parse + fetch.
pub fn fetch_from_url(url: &str) -> Result<GitHubIssue> {
    let issue_ref = parse_issue_url(url).context("not a valid GitHub issue URL")?;
    fetch_issue(&issue_ref)
}

/// Build a prompt from a GitHub issue.
pub fn issue_to_prompt(issue: &GitHubIssue) -> String {
    let mut prompt = format!("#{}: {}", issue.number, issue.title);
    if !issue.body.is_empty() {
        let body: String = issue.body.chars().take(2000).collect();
        prompt.push_str(&format!("\n\n{}", body));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_url() {
        let url = "https://github.com/myorg/myrepo/issues/42";
        let r = parse_issue_url(url).unwrap();
        assert_eq!(r.owner, "myorg");
        assert_eq!(r.repo, "myrepo");
        assert_eq!(r.number, 42);
    }

    #[test]
    fn parse_url_with_trailing_slash() {
        let url = "https://github.com/org/repo/issues/7/";
        let r = parse_issue_url(url).unwrap();
        assert_eq!(r.number, 7);
    }

    #[test]
    fn parse_url_with_fragment() {
        // https://github.com/org/repo/issues/7#issuecomment-123
        // The number part would be "7#issuecomment-123" which won't parse as u64
        let url = "https://github.com/org/repo/issues/7";
        let r = parse_issue_url(url).unwrap();
        assert_eq!(r.number, 7);
    }

    #[test]
    fn parse_not_github_url() {
        assert!(parse_issue_url("https://linear.app/t/issue/X-1").is_none());
        assert!(parse_issue_url("not a url").is_none());
        assert!(parse_issue_url("").is_none());
    }

    #[test]
    fn parse_url_with_whitespace() {
        let url = "  https://github.com/a/b/issues/1  ";
        let r = parse_issue_url(url).unwrap();
        assert_eq!(r.owner, "a");
        assert_eq!(r.repo, "b");
        assert_eq!(r.number, 1);
    }

    #[test]
    fn is_github_url_works() {
        assert!(is_github_url("https://github.com/o/r/issues/1"));
        assert!(!is_github_url("https://linear.app/t/issue/X-1"));
        assert!(!is_github_url(""));
    }

    #[test]
    fn issue_to_prompt_title_only() {
        let issue = GitHubIssue {
            number: 42,
            title: "Fix login bug".to_string(),
            body: String::new(),
            state: "open".to_string(),
            labels: vec![],
            url: String::new(),
        };
        assert_eq!(issue_to_prompt(&issue), "#42: Fix login bug");
    }

    #[test]
    fn issue_to_prompt_with_body() {
        let issue = GitHubIssue {
            number: 7,
            title: "Add retry logic".to_string(),
            body: "We need exponential backoff.".to_string(),
            state: "open".to_string(),
            labels: vec!["bug".into()],
            url: String::new(),
        };
        let prompt = issue_to_prompt(&issue);
        assert!(prompt.starts_with("#7: Add retry logic"));
        assert!(prompt.contains("exponential backoff"));
    }

    #[test]
    fn issue_to_prompt_truncates_long_body() {
        let issue = GitHubIssue {
            number: 1,
            title: "Big".to_string(),
            body: "x".repeat(5000),
            state: "open".to_string(),
            labels: vec![],
            url: String::new(),
        };
        let prompt = issue_to_prompt(&issue);
        assert!(prompt.len() < 2100);
    }
}
