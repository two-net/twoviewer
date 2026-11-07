//! The email sent when sign-in to an account is blocked after too many failed attempts (AUTH-08,
//! AUTH-12).
//!
//! It is sent only to addresses that have an account, in the background, so the sign-in that
//! started the block gets the same answer either way (SEC-14). It names neither where the attempts
//! came from nor the account's display name.

use std::time::Duration;

use askama::Template;

use super::{Content, Email, verify_email::validity};

pub struct SignInBlocked<'a> {
    /// How many failed attempts started the block (`sign_in.max_failures`).
    pub failures: usize,
    /// How long the block lasts (`sign_in.block_duration`).
    pub blocked_for: Duration,
    pub reset_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/sign_in_blocked.txt", blocks = ["subject", "body"])]
struct Text<'a> {
    failures: usize,
    duration: u64,
    in_hours: bool,
    reset_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/sign_in_blocked.html")]
struct Html<'a> {
    subject: &'a str,
    failures: usize,
    duration: u64,
    in_hours: bool,
    reset_url: &'a str,
}

impl Email for SignInBlocked<'_> {
    const NAME: &'static str = "sign_in_blocked";

    fn render(&self) -> askama::Result<Content> {
        let (duration, in_hours) = validity(self.blocked_for);
        let text = Text {
            failures: self.failures,
            duration,
            in_hours,
            reset_url: self.reset_url,
        };
        let subject = Content::subject(&text.as_subject().render()?);
        Ok(Content {
            html: Html {
                subject: &subject,
                failures: self.failures,
                duration,
                in_hours,
                reset_url: self.reset_url,
            }
            .render()?,
            text: text.as_body().render()?,
            subject,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_long_sign_in_is_blocked_and_links_to_reset_the_password() {
        let content = SignInBlocked {
            failures: 10,
            blocked_for: Duration::from_secs(15 * 60),
            reset_url: "https://twoviewer.example/reset-password",
        }
        .render()
        .unwrap();

        assert_eq!(
            content.subject,
            "Sign-in to your TwoViewer account is paused"
        );
        for expected in [
            "the wrong password 10 times",
            "paused for 15 minutes",
            "https://twoviewer.example/reset-password",
        ] {
            assert!(
                content.text.contains(expected),
                "{expected}: {}",
                content.text
            );
        }
        assert!(
            content
                .html
                .contains(r#"href="https://twoviewer.example/reset-password""#),
            "{}",
            content.html
        );
        assert!(content.html.contains("paused for 15 minutes"));
    }

    #[test]
    fn long_blocks_are_in_hours() {
        let content = SignInBlocked {
            failures: 1,
            blocked_for: Duration::from_secs(60 * 60),
            reset_url: "https://twoviewer.example/reset-password",
        }
        .render()
        .unwrap();

        assert!(
            content.text.contains("the wrong password once"),
            "{}",
            content.text
        );
        assert!(
            content.text.contains("paused for 1 hour."),
            "{}",
            content.text
        );
    }
}
