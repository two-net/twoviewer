//! The email that tells the old address of an account that the account's address has changed
//! (AUTH-11). It is sent once the new address is confirmed, and only if the old one was verified:
//! an address that never was may belong to someone else.
//!
//! It names the new address, so that the owner can tell whose it is if they didn't make the
//! change. It cannot link to a password reset, which would go to the new address.

use askama::Template;

use super::{Content, Email};

pub struct EmailChanged<'a> {
    /// The account's new address.
    pub new_email: &'a str,
    /// Settings in the web client, where a browser that is still signed in can change it back.
    pub settings_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/email_changed.txt", blocks = ["subject", "body"])]
struct Text<'a> {
    new_email: &'a str,
    settings_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/email_changed.html")]
struct Html<'a> {
    subject: &'a str,
    new_email: &'a str,
    settings_url: &'a str,
}

impl Email for EmailChanged<'_> {
    const NAME: &'static str = "email_changed";

    fn render(&self) -> askama::Result<Content> {
        let text = Text {
            new_email: self.new_email,
            settings_url: self.settings_url,
        };
        let subject = Content::subject(&text.as_subject().render()?);
        Ok(Content {
            html: Html {
                subject: &subject,
                new_email: self.new_email,
                settings_url: self.settings_url,
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
    fn names_the_new_address() {
        let content = EmailChanged {
            new_email: "ann.new@example.com",
            settings_url: "https://twoviewer.example/settings",
        }
        .render()
        .unwrap();

        assert_eq!(
            content.subject,
            "The email address of your TwoViewer account has changed"
        );
        assert!(
            content.text.starts_with(
                "Your email address has changed\n\nThe email address of your TwoViewer account \
                 was changed from this one to ann.new@example.com. From now on, sign in with the \
                 new address."
            ),
            "{}",
            content.text
        );
        for body in [&content.text, &content.html] {
            assert!(body.contains("ann.new@example.com"), "{body}");
            assert!(
                body.contains("https://twoviewer.example/settings"),
                "{body}"
            );
            assert!(body.contains("change your password"), "{body}");
        }
    }

    #[test]
    fn the_address_is_escaped_in_html() {
        let content = EmailChanged {
            new_email: "\"<b>\"@example.com",
            settings_url: "https://twoviewer.example/settings",
        }
        .render()
        .unwrap();

        assert!(!content.html.contains("<b>"), "{}", content.html);
    }
}
