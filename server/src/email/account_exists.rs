//! The email sent when someone signs up with an address that already has a verified account.
//!
//! The sign-up gets the same answer as any other, so that it doesn't reveal that the address is
//! registered (SEC-14). Only the owner of the address learns it, from this email, which tells them
//! how to sign in or reset their password instead.

use askama::Template;

use super::{Content, Email};

pub struct AccountExists<'a> {
    pub sign_in_url: &'a str,
    pub reset_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/account_exists.txt", blocks = ["subject", "body"])]
struct Text<'a> {
    sign_in_url: &'a str,
    reset_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/account_exists.html")]
struct Html<'a> {
    subject: &'a str,
    sign_in_url: &'a str,
    reset_url: &'a str,
}

impl Email for AccountExists<'_> {
    const NAME: &'static str = "account_exists";

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
    fn renders_links_to_sign_in_and_reset_the_password() {
        let content = AccountExists {
            sign_in_url: "https://twoviewer.example/sign-in",
            reset_url: "https://twoviewer.example/reset-password",
        }
        .render()
        .unwrap();

        assert_eq!(content.subject, "You already have a TwoViewer account");
        assert!(
            content.text.contains(
                "sign in:\nhttps://twoviewer.example/sign-in\n\nIf you don't remember your \
                 password, reset it:\nhttps://twoviewer.example/reset-password\n"
            ),
            "{}",
            content.text
        );
        assert!(
            content
                .html
                .contains(r#"href="https://twoviewer.example/sign-in""#),
            "{}",
            content.html
        );
        assert!(
            content
                .html
                .contains(r#"href="https://twoviewer.example/reset-password""#),
            "{}",
            content.html
        );
    }
}
