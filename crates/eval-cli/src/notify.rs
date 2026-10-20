//! Narrow-write GitHub notifications, deliberately separate from the
//! read-only watcher. The default identity is `EVAL_GITHUB_WRITE_TOKEN`;
//! the watch token is rejected so one credential cannot do both jobs.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::PathBuf;

const WATCH_TOKEN_ENV: &str = "EVAL_WATCH_TOKEN";

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IssueComment {
    #[serde(default)]
    body: Option<String>,
}

pub struct NotifyArgs {
    pub repo: String,
    pub base_url: String,
    pub issue: Option<u64>,
    pub body: Option<String>,
    pub marker: String,
    pub token_env: String,
    pub token_file: Option<PathBuf>,
    pub status_sha: Option<String>,
    pub status_state: Option<String>,
    pub status_context: Option<String>,
}

fn validate_args(args: &NotifyArgs) -> Result<()> {
    let parts: Vec<&str> = args.repo.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("notify.repo must be owner/name");
    }
    if args.token_env == WATCH_TOKEN_ENV {
        bail!("notify must use a separate narrow-write token, not {WATCH_TOKEN_ENV}");
    }
    if args.issue.is_some() != args.body.is_some() {
        bail!("--issue and --body must be supplied together");
    }
    if args.issue.is_none() && args.status_sha.is_none() {
        bail!("nothing to do: pass --issue/--body or --status-sha");
    }
    if args.status_sha.is_some() && (args.status_state.is_none() || args.status_context.is_none()) {
        bail!("--status-sha requires --status-state and --status-context");
    }
    if let Some(state) = &args.status_state {
        if !matches!(state.as_str(), "error" | "failure" | "pending" | "success") {
            bail!("--status-state must be error, failure, pending, or success");
        }
    }
    if let Some(issue) = args.issue {
        if issue == 0 {
            bail!("--issue must be positive");
        }
    }
    if let Some(body) = &args.body {
        if body.trim().is_empty() {
            bail!("--body must not be empty");
        }
        if body.len() > 60_000 {
            bail!("--body is too large");
        }
    }
    Ok(())
}

fn body_with_marker(body: &str, marker: &str) -> String {
    let marker = marker.trim();
    if marker.is_empty() {
        body.to_string()
    } else {
        format!("{body}\n\n<!-- {marker} -->")
    }
}

fn comments_contain_marker(comments: &[IssueComment], marker: &str) -> bool {
    comments
        .iter()
        .filter_map(|c| c.body.as_deref())
        .any(|body| body.contains(marker))
}

pub async fn cmd_notify(args: NotifyArgs) -> Result<i32> {
    validate_args(&args)?;
    let token = read_token(&args)?;
    let client = reqwest::Client::builder()
        .user_agent("eval-service/notify")
        .build()
        .context("building GitHub notification client")?;
    let base = args.base_url.trim_end_matches('/');

    if let (Some(issue), Some(body)) = (args.issue, args.body.as_deref()) {
        let comments_url = format!(
            "{base}/repos/{}/issues/{issue}/comments?per_page=100",
            args.repo
        );
        let response = client
            .get(&comments_url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .with_context(|| format!("reading comments from {comments_url}"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("GitHub returned {status} for {comments_url}");
        }
        let comments: Vec<IssueComment> = response
            .json()
            .await
            .with_context(|| format!("decoding comments from {comments_url}"))?;
        if comments_contain_marker(&comments, &args.marker) {
            println!("notify: marker already present; comment is idempotent");
        } else {
            let payload = serde_json::json!({ "body": body_with_marker(body, &args.marker) });
            let response = client
                .post(&comments_url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .json(&payload)
                .send()
                .await
                .with_context(|| format!("posting comment to {comments_url}"))?;
            let status = response.status();
            if !status.is_success() {
                bail!("GitHub returned {status} posting a comment");
            }
            println!("notify: comment posted");
        }
    }

    if let (Some(sha), Some(state), Some(context)) = (
        args.status_sha.as_deref(),
        args.status_state.as_deref(),
        args.status_context.as_deref(),
    ) {
        let url = format!("{base}/repos/{}/statuses/{sha}", args.repo);
        let payload = serde_json::json!({
            "state": state,
            "context": context,
            "description": "eval gate",
        });
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("posting status to {url}"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("GitHub returned {status} posting a status");
        }
        println!("notify: status updated ({context}={state})");
    }

    Ok(0)
}

fn read_token(args: &NotifyArgs) -> Result<String> {
    if let Some(path) = &args.token_file {
        let token = std::fs::read_to_string(path)
            .with_context(|| format!("reading write token {}", path.display()))?;
        let token = token.trim();
        if token.is_empty() {
            bail!("write token file {} is empty", path.display());
        }
        return Ok(token.to_string());
    }
    let token =
        std::env::var(&args.token_env).with_context(|| format!("{} is not set", args.token_env))?;
    if token.trim().is_empty() {
        bail!("{} is empty", args.token_env);
    }
    Ok(token.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(token_env: &str) -> NotifyArgs {
        NotifyArgs {
            repo: "owner/repo".into(),
            base_url: "https://api.github.com".into(),
            issue: Some(7),
            body: Some("gate passed".into()),
            marker: "eval-gate:run-123".into(),
            token_env: token_env.into(),
            token_file: None,
            status_sha: None,
            status_state: None,
            status_context: None,
        }
    }

    #[test]
    fn watch_token_cannot_be_reused_for_writes() {
        let error = validate_args(&args(WATCH_TOKEN_ENV)).unwrap_err();
        assert!(error.to_string().contains("separate narrow-write token"));
    }

    #[test]
    fn marker_deduplicates_comments() {
        let comments = vec![
            IssueComment {
                body: Some("old result".into()),
            },
            IssueComment {
                body: Some("new result\n\n<!-- eval-gate:run-123 -->".into()),
            },
        ];
        assert!(comments_contain_marker(&comments, "eval-gate:run-123"));
        assert!(!comments_contain_marker(&comments, "eval-gate:run-124"));
    }

    #[test]
    fn marker_is_appended_once() {
        let body = body_with_marker("ok", "eval-gate:run-123");
        assert!(body.ends_with("<!-- eval-gate:run-123 -->"));
        assert_eq!(body.matches("eval-gate:run-123").count(), 1);
    }
}
