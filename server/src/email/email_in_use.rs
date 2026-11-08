//! The email sent when someone asks to change the address of their account to one that already
//! has an account (AUTH-11).
//!
//! The request gets the same answer as any other, so that it doesn't reveal to whoever is signed
//! in that the address is registered. Only the owner of the address learns it, from this email.

use askama::Template;

use super::{Content, Email};

pub struct EmailInUse<'a> {
    pub sign_in_url: &'a str,
    pub reset_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/email_in_use.txt", blocks = ["subject", "body"])]
struct Text<'a> {
    sign_in_url: &'a str,
    reset_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/email_in_use.html")]
struct Html<'a> {
    subject: &'a str,
    sign_in_url: &'a str,
    reset_url: &'a str,
}

impl Email for EmailInUse<'_> {
    const NAME: &'static str = "email_in_use";

    fn render(&self) -> askama::Result<Content> {
        let text = Text {
            sign_in_url: self.sign_in_url,
            reset_url: self.reset_url,
        };
        let subject = Content::subject(&text.as_subject().render()?);
        Ok(Content {
            html: Html {
                subject: &subject,
                sign_in_url: self.sign_in_url,
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
    fn says_the_address_was_not_changed() {
        let content = EmailInUse {
            sign_in_url: "https://twoviewer.example/sign-in",
            reset_url: "https://twoviewer.example/reset-password",
        }
        .render()
        .unwrap();

        assert_eq!(
            content.subject,
            "This email address already has a TwoViewer account"
        );
        assert!(
            content.text.starts_with(
                "This address already has an account\n\nSomeone, probably you, asked to change \
                 the email address of a TwoViewer account to this one. This address already has \
                 an account of its own, so nothing was changed."
            ),
            "{}",
            content.text
        );
        for body in [&content.text, &content.html] {
            assert!(body.contains("https://twoviewer.example/sign-in"), "{body}");
            assert!(
                body.contains("https://twoviewer.example/reset-password"),
                "{body}"
            );
        }
    }
}
