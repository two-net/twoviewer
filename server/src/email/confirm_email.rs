//! The email with the link that confirms a new address for an account (AUTH-11).
//!
//! It goes to the new address, and the change takes effect only once its link is opened, so an
//! account never gets an address that its owner doesn't receive mail at. Like every email, it
//! never contains the display name.

use std::time::Duration;

use askama::Template;

use super::{Content, Email, verify_email::validity};

pub struct ConfirmEmail<'a> {
    /// The link, with its single-use token. It is a secret: never log it.
    pub link: &'a str,
    /// How long the link works (`accounts.verification_link_lifetime`).
    pub valid_for: Duration,
}

#[derive(Template)]
#[template(path = "email/confirm_email.txt", blocks = ["subject", "body"])]
struct Text<'a> {
    link: &'a str,
    validity: u64,
    in_hours: bool,
}

#[derive(Template)]
#[template(path = "email/confirm_email.html")]
struct Html<'a> {
    subject: &'a str,
    link: &'a str,
    validity: u64,
    in_hours: bool,
}

impl Email for ConfirmEmail<'_> {
    const NAME: &'static str = "confirm_email";

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

    const LINK: &str = "https://twoviewer.example/confirm-email#0123abcd";

    #[test]
    fn renders_the_link_and_how_long_it_works() {
        let content = ConfirmEmail {
            link: LINK,
            valid_for: Duration::from_secs(24 * 60 * 60),
        }
        .render()
        .unwrap();

        assert_eq!(
            content.subject,
            "Confirm your new email address for TwoViewer"
        );
        assert!(
            content.text.starts_with(&format!(
                "Confirm your new email address\n\nSomeone, probably you, asked to change the \
                 email address of a TwoViewer account to this one. To confirm that it is yours, \
                 open this link:\n\n{LINK}\n\nThe link works once, for 24 hours. Until then, the \
                 account keeps its current address.\n\nIf you didn't ask for this"
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
            content.html.contains(">Confirm email address</a>"),
            "{}",
            content.html
        );
        for body in [&content.text, &content.html] {
            assert!(body.contains("for 24 hours."), "{body}");
            assert!(body.contains("Nothing changes."), "{body}");
        }
    }
}
