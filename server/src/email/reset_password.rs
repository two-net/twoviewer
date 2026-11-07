//! The email with the link that resets a forgotten password (AUTH-07).
//!
//! It is sent only to addresses that have an account, in the background, so the request gets the
//! same answer either way (SEC-14). Like every email, it never contains the display name.

use std::time::Duration;

use askama::Template;

use super::{Content, Email, verify_email::validity};

pub struct ResetPassword<'a> {
    /// The link, with its single-use token. It is a secret: never log it.
    pub link: &'a str,
    /// How long the link works (`password_reset.link_lifetime`).
    pub valid_for: Duration,
}

#[derive(Template)]
#[template(path = "email/reset_password.txt", blocks = ["subject", "body"])]
struct Text<'a> {
    link: &'a str,
    validity: u64,
    in_hours: bool,
}

#[derive(Template)]
#[template(path = "email/reset_password.html")]
struct Html<'a> {
    subject: &'a str,
    link: &'a str,
    validity: u64,
    in_hours: bool,
}

impl Email for ResetPassword<'_> {
    const NAME: &'static str = "reset_password";

    fn render(&self) -> askama::Result<Content> {
        let (validity, in_hours) = validity(self.valid_for);
        let text = Text {
            link: self.link,
            validity,
            in_hours,
        };
        let subject = Content::subject(&text.as_subject().render()?);
        Ok(Content {
            html: Html {
                subject: &subject,
                link: self.link,
                validity,
                in_hours,
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

    const LINK: &str = "https://twoviewer.example/reset-password#0123abcd";

    #[test]
    fn renders_the_link_and_how_long_it_works() {
        let content = ResetPassword {
            link: LINK,
            valid_for: Duration::from_secs(60 * 60),
        }
        .render()
        .unwrap();

        assert_eq!(content.subject, "Reset your TwoViewer password");
        assert!(
            content.text.starts_with(&format!(
                "Reset your password\n\nSomeone, probably you, asked to reset the password of the \
                 TwoViewer account with this email address. To choose a new password, open this \
                 link:\n\n{LINK}\n\nThe link works once, for 1 hour. Resetting your password \
                 signs you out everywhere.\n\nIf you didn't ask for this"
            )),
            "{}",
            content.text
        );
        assert!(
            content.html.contains(&format!(r#"href="{LINK}""#)),
            "{}",
            content.html
        );
        assert!(
            content.html.contains(">Choose a new password</a>"),
            "{}",
            content.html
        );
        for body in [&content.text, &content.html] {
            assert!(body.contains("for 1 hour."), "{body}");
            assert!(body.contains("Your password stays the same."), "{body}");
        }
    }

    #[test]
    fn short_lifetimes_are_in_minutes() {
        let content = ResetPassword {
            link: LINK,
            valid_for: Duration::from_secs(30 * 60),
        }
        .render()
        .unwrap();

        assert!(content.text.contains("for 30 minutes."), "{}", content.text);
    }
}
