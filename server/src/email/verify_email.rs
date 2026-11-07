//! The email with the link that verifies an address (AUTH-02), sent at sign-up.
//!
//! It never contains the display name, which whoever signs up chooses: otherwise sign-up could be
//! used to send text of one's choosing to any address.

use std::time::Duration;

use askama::Template;

use super::{Content, Email};

pub struct VerifyEmail<'a> {
    /// The link, with its single-use token. It is a secret: never log it.
    pub link: &'a str,
    /// How long the link works (`accounts.verification_link_lifetime`).
    pub valid_for: Duration,
    /// Whether the address already had an account that isn't verified, which a sign-up with the
    /// same address doesn't change.
    pub repeated: bool,
}

#[derive(Template)]
#[template(path = "email/verify_email.txt", blocks = ["subject", "body"])]
struct Text<'a> {
    link: &'a str,
    validity: u64,
    in_hours: bool,
    repeated: bool,
}

#[derive(Template)]
#[template(path = "email/verify_email.html")]
struct Html<'a> {
    subject: &'a str,
    link: &'a str,
    validity: u64,
    in_hours: bool,
    repeated: bool,
}

/// `duration` in whole hours if it is that, otherwise in minutes, rounded up.
pub(super) fn validity(duration: Duration) -> (u64, bool) {
    let minutes = duration.as_secs().div_ceil(60).max(1);
    if minutes.is_multiple_of(60) {
        (minutes / 60, true)
    } else {
        (minutes, false)
    }
}

impl Email for VerifyEmail<'_> {
    const NAME: &'static str = "verify_email";

    fn render(&self) -> askama::Result<Content> {
        let (validity, in_hours) = validity(self.valid_for);
        let text = Text {
            link: self.link,
            validity,
            in_hours,
            repeated: self.repeated,
        };
        let subject = Content::subject(&text.as_subject().render()?);
        Ok(Content {
            html: Html {
                subject: &subject,
                link: self.link,
                validity,
                in_hours,
                repeated: self.repeated,
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

    const LINK: &str = "https://twoviewer.example/verify-email#0123abcd";

    fn render(repeated: bool) -> Content {
        VerifyEmail {
            link: LINK,
            valid_for: Duration::from_secs(24 * 60 * 60),
            repeated,
        }
        .render()
        .unwrap()
    }

    #[test]
    fn renders_the_link_and_how_long_it_works() {
        let content = render(false);

        assert_eq!(content.subject, "Verify your email address for TwoViewer");
        assert!(
            content.text.starts_with(&format!(
                "Verify your email address\n\nTo finish signing up for TwoViewer, confirm that \
                 this is your email address by opening this link:\n\n{LINK}\n\nThe link works \
                 once, for 24 hours.\n\nIf you didn't sign up"
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
            content.html.contains(">Verify email address</a>"),
            "{}",
            content.html
        );
        assert!(content.html.contains("for 24 hours."), "{}", content.html);
        assert!(
            !content.text.contains("first signed up"),
            "{}",
            content.text
        );
    }

    #[test]
    fn a_repeated_sign_up_says_which_password_to_use() {
        let content = render(true);

        for body in [&content.text, &content.html] {
            assert!(body.contains("signed up for TwoViewer again"), "{body}");
            assert!(
                body.contains("the password you chose when you first signed up"),
                "{body}"
            );
        }
    }

    #[test]
    fn validity_is_in_hours_or_minutes() {
        assert_eq!(validity(Duration::from_secs(24 * 60 * 60)), (24, true));
        assert_eq!(validity(Duration::from_secs(60 * 60)), (1, true));
        assert_eq!(validity(Duration::from_secs(90 * 60)), (90, false));
        assert_eq!(validity(Duration::from_secs(61)), (2, false));
        assert_eq!(validity(Duration::from_millis(1)), (1, false));

        let content = VerifyEmail {
            link: LINK,
            valid_for: Duration::from_secs(60 * 60),
            repeated: false,
        }
        .render()
        .unwrap();
        assert!(content.text.contains("for 1 hour."), "{}", content.text);
    }
}
