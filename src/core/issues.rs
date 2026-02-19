//! Unified issue fetching — dispatches to Linear or GitHub based on URL.

use anyhow::{Context, Result};

use super::github;
use super::linear;

/// Fetched issue data (provider-agnostic).
#[derive(Debug, Clone)]
pub struct Issue {
    pub provider: Provider,
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Provider {
    Linear,
    GitHub,
    Unknown,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Linear => write!(f, "Linear"),
            Provider::GitHub => write!(f, "GitHub"),
            Provider::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detect the provider from a URL.
pub fn detect_provider(url: &str) -> Provider {
    if linear::is_linear_url(url) {
        Provider::Linear
    } else if github::is_github_url(url) {
        Provider::GitHub
    } else {
        Provider::Unknown
    }
}

/// Fetch an issue from any supported provider.
pub fn fetch(url: &str) -> Result<Issue> {
    let url = url.trim();
    match detect_provider(url) {
        Provider::Linear => {
            let issue = linear::fetch_from_url(url)?;
            Ok(Issue {
                provider: Provider::Linear,
                identifier: issue.identifier,
                title: issue.title,
                description: issue.description,
                state: issue.state,
            })
        }
        Provider::GitHub => {
            let issue = github::fetch_from_url(url)?;
            Ok(Issue {
                provider: Provider::GitHub,
                identifier: format!("#{}", issue.number),
                title: issue.title,
                description: issue.body,
                state: issue.state,
            })
        }
        Provider::Unknown => {
            anyhow::bail!("unrecognized issue URL: {}", url)
        }
    }
}

/// Build a prompt from a fetched issue.
pub fn issue_to_prompt(issue: &Issue) -> String {
    let mut prompt = format!("{}: {}", issue.identifier, issue.title);
    if !issue.description.is_empty() {
        let desc: String = issue.description.chars().take(2000).collect();
        prompt.push_str(&format!("\n\n{}", desc));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_linear() {
        assert_eq!(
            detect_provider("https://linear.app/team/issue/ENG-42/title"),
            Provider::Linear
        );
    }

    #[test]
    fn detect_github() {
        assert_eq!(
            detect_provider("https://github.com/org/repo/issues/42"),
            Provider::GitHub
        );
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(detect_provider("https://jira.example.com/browse/X-1"), Provider::Unknown);
        assert_eq!(detect_provider("not a url"), Provider::Unknown);
        assert_eq!(detect_provider(""), Provider::Unknown);
    }

    #[test]
    fn issue_to_prompt_works() {
        let issue = Issue {
            provider: Provider::Linear,
            identifier: "ENG-42".to_string(),
            title: "Fix timeout".to_string(),
            description: "Details here.".to_string(),
            state: "In Progress".to_string(),
        };
        let prompt = issue_to_prompt(&issue);
        assert!(prompt.starts_with("ENG-42: Fix timeout"));
        assert!(prompt.contains("Details here."));
    }

    #[test]
    fn issue_to_prompt_no_description() {
        let issue = Issue {
            provider: Provider::GitHub,
            identifier: "#7".to_string(),
            title: "Add tests".to_string(),
            description: String::new(),
            state: "open".to_string(),
        };
        assert_eq!(issue_to_prompt(&issue), "#7: Add tests");
    }
}
