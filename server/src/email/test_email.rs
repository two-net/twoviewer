//! The email that `twoviewer-server send-test-email` sends, to check email delivery and sender
//! authentication (IF-12), and to look at the layout in email clients (THM-11).

use askama::Template;

use super::{Content, Email};

#[derive(Clone, Copy, Debug, Default)]
pub struct TestEmail;

#[derive(Template)]
#[template(path = "email/test.txt", blocks = ["subject", "body"])]
struct Text;

#[derive(Template)]
#[template(path = "email/test.html")]
struct Html<'a> {
    subject: &'a str,
}

impl Email for TestEmail {
    const NAME: &'static str = "test";

    fn render(&self) -> askama::Result<Content> {
        let text = Text;
        let subject = Content::subject(&text.as_subject().render()?);
        Ok(Content {
            html: Html { subject: &subject }.render()?,
            text: text.as_body().render()?,
            subject,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders() {
        let content = TestEmail.render().unwrap();

        assert_eq!(content.subject, "Test email from TwoViewer");
        assert!(
            content.html.contains("<h1>Email delivery works</h1>"),
            "{}",
            content.html
        );
        assert!(
            content.text.starts_with("Email delivery works\n\n"),
            "{}",
            content.text
        );
    }
}
