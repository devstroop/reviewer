//! Thin wrapper over the GitHub client for PR metadata, diff fetching,
//! and sticky comment posting.

use crate::config::Settings;
use crate::error::Result;
use crate::github::{GitHub, PullRequest, ReviewEvent};

/// Marker prefix for identifying sticky reviewer comments across runs.
const STICKY_MARKER: &str = "<!-- reviewer-sticky -->";

/// Service for fetching PR data from GitHub.
pub(crate) struct GithubService {
    github: GitHub,
}

impl GithubService {
    pub(crate) fn new(settings: &Settings) -> Result<Self> {
        Ok(Self {
            github: GitHub::new(settings)?,
        })
    }

    /// Fetch pull request metadata.
    pub(crate) async fn get_pr_metadata(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest> {
        self.github.get_pr_metadata(owner, repo, number).await
    }

    /// Fetch the raw unified diff for a pull request.
    pub(crate) async fn get_pr_diff(&self, owner: &str, repo: &str, number: u64) -> Result<String> {
        self.github.get_pr_diff(owner, repo, number).await
    }

    /// Post a review comment on a pull request (one-shot, no sticky update).
    pub(crate) async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<()> {
        self.github
            .publish_review(owner, repo, number, body, Some(ReviewEvent::Comment))
            .await?;
        Ok(())
    }

    /// Post or update a sticky review comment.
    ///
    /// Finds an existing comment by marker and edits it in place.
    /// If no existing comment is found, creates a new one with the marker
    /// embedded so future runs can find and update it.
    pub(crate) async fn post_or_update_review(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<()> {
        let marked_body = format!("{}\n{}", body.trim(), STICKY_MARKER);

        // Check for an existing comment from a previous run.
        if let Some(existing) = self
            .github
            .find_comment(owner, repo, number, STICKY_MARKER)
            .await?
        {
            let edit_body = format!("{}\n\n*Review updated*", marked_body);
            self.github
                .edit_comment(owner, repo, existing.id, &edit_body)
                .await?;
        } else {
            self.github
                .publish_review(
                    owner,
                    repo,
                    number,
                    &marked_body,
                    Some(ReviewEvent::Comment),
                )
                .await?;
        }
        Ok(())
    }
}
