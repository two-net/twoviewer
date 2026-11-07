//! Emails as they are sent: rendered from their templates, with the name and logo of TwoViewer.

use lettre::{
    Address, Message,
    message::{
        Attachment, Mailbox, MultiPart, SinglePart,
        header::{ContentType, Header, HeaderName, HeaderValue},
    },
};
use uuid::Uuid;

/// The name emails come from. It is never translated (UI-01, `brand/README.md`).
const SENDER_NAME: &str = "TwoViewer";

/// The logo in the header of every email, as PNG, since email clients don't show SVG.
/// `cargo xtask icons` makes it from `brand/logo.svg`.
const LOGO: &[u8] = include_bytes!("../../templates/email/logo.png");

/// The Content-ID of the logo, which the HTML shows as `cid:logo`.
const LOGO_ID: &str = "logo";

/// An email TwoViewer sends.
pub trait Email {
    /// A short name for logs, such as `verify_email`. Never anything personal.
    const NAME: &'static str;

    /// The subject and both bodies, from the email's templates.
    fn render(&self) -> askama::Result<Content>;
}

/// The text of an email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    /// One line, as made by [`Content::subject`].
    pub subject: String,
    pub html: String,
    pub text: String,
}

impl Content {
    /// The subject as rendered from a template, on one line, without surrounding whitespace.
    pub fn subject(rendered: &str) -> String {
        rendered.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// `content` as an email from `from` to `to`, and its Message-ID, without the angle brackets.
///
/// The body has both the plain text and the HTML, so that every client shows one of them. The HTML
/// comes with the logo, which it shows as `cid:logo`.
pub(super) fn build(
    from: &Address,
    to: Address,
    content: Content,
) -> Result<(Message, String), lettre::error::Error> {
    // On the sender's domain rather than this machine's name, which would otherwise be used.
    let message_id = format!("{}@{}", Uuid::new_v4(), from.domain());
    let logo = Attachment::new_inline(LOGO_ID.to_owned()).body(
        LOGO.to_vec(),
        ContentType::parse("image/png").expect("a valid content type"),
    );
    let message = Message::builder()
        .from(Mailbox::new(Some(SENDER_NAME.to_owned()), from.clone()))
        .to(Mailbox::new(None, to))
        .subject(content.subject)
        .message_id(Some(format!("<{message_id}>")))
        .header(AutoSubmitted)
        .multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::plain(content.text))
                .multipart(
                    MultiPart::related()
                        .singlepart(SinglePart::html(content.html))
                        .singlepart(logo),
                ),
        )?;
    Ok((message, message_id))
}

/// `Auto-Submitted: auto-generated` (RFC 3834): the email was sent automatically, so mail servers
/// don't answer it automatically, for example with an out-of-office notice.
#[derive(Clone, Copy, Debug)]
struct AutoSubmitted;

impl Header for AutoSubmitted {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("Auto-Submitted")
    }

    fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match value.trim() {
            "auto-generated" => Ok(Self),
            _ => Err("not auto-generated".into()),
        }
    }

    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), "auto-generated".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use askama::Template;

    use super::*;
    use crate::email::palette;

    /// An email with a value in it, to check what the templates do with it.
    struct Greeting<'a> {
        name: &'a str,
    }

    #[derive(Template)]
    #[template(
        ext = "txt",
        source = r#"{% extends "email/layout.txt" %}
{%- block subject %}
  Hello {{ name }}
{% endblock -%}
{%- block content %}Hello {{ name }}.{% endblock %}"#,
        blocks = ["subject", "body"]
    )]
    struct GreetingText<'a> {
        name: &'a str,
    }

    #[derive(Template)]
    #[template(
        ext = "html",
        source = r#"{% extends "email/layout.html" %}
{%- block content %}<p>Hello {{ name }}.</p>{% endblock %}"#
    )]
    struct GreetingHtml<'a> {
        subject: &'a str,
        name: &'a str,
    }

    impl Email for Greeting<'_> {
        const NAME: &'static str = "greeting";

        fn render(&self) -> askama::Result<Content> {
            let text = GreetingText { name: self.name };
            let subject = Content::subject(&text.as_subject().render()?);
            Ok(Content {
                html: GreetingHtml {
                    subject: &subject,
                    name: self.name,
                }
                .render()?,
                text: text.as_body().render()?,
                subject,
            })
        }
    }

    fn from() -> Address {
        "no-reply@twoviewer.test".parse().unwrap()
    }

    fn greeting(name: &str) -> Content {
        Greeting { name }.render().unwrap()
    }

    #[test]
    fn the_subject_is_one_line() {
        assert_eq!(greeting("Ann").subject, "Hello Ann");
        assert_eq!(Content::subject("\n  Your\n  code \n"), "Your code");
    }

    #[test]
    fn the_text_has_no_markup() {
        let text = greeting("<b>Ann</b>").text;

        assert!(text.starts_with("Hello <b>Ann</b>."), "{text}");
        assert!(text.contains("TwoViewer will never contact you"), "{text}");
        assert!(!text.contains("<p>") && !text.contains("{%"), "{text}");
    }

    #[test]
    fn values_are_escaped_in_the_html() {
        let html = greeting("<b>Ann</b>").html;

        assert!(
            html.contains("<p>Hello &#60;b&#62;Ann&#60;/b&#62;.</p>"),
            "{html}"
        );
        assert!(html.contains("<title>Hello &#60;b&#62;Ann"), "{html}");
    }

    #[test]
    fn the_html_shows_the_name_and_logo() {
        let html = greeting("Ann").html;

        assert!(html.contains(r#"src="cid:logo""#), "{html}");
        assert!(html.contains(">TwoViewer</td>"), "{html}");
        assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
        assert!(html.contains(r#"<html lang="en" dir="ltr">"#), "{html}");
    }

    /// Every color in the HTML is one of the palette's, set inline in light mode, on an element
    /// with the class that switches it to dark mode (THM-11).
    #[test]
    fn every_color_has_a_dark_mode() {
        let html = greeting("Ann").html;
        let (head, body) = html.split_once("</head>").unwrap();

        assert!(head.contains(r#"<meta name="color-scheme" content="light dark">"#));
        assert!(head.contains(r#"<meta name="supported-color-schemes" content="light dark">"#));
        let dark = head
            .split_once("@media (prefers-color-scheme: dark) {")
            .expect("a dark mode")
            .1;
        let mut used = Vec::new();
        for tag in body.split('<').skip(1) {
            let tag = tag.split_once('>').map_or(tag, |(tag, _)| tag);
            let attribute = |name: &str| {
                tag.split_once(&format!(r#" {name}=""#))
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .map_or("", |(value, _)| value)
            };
            let (style, classes) = (attribute("style"), attribute("class"));
            for (index, _) in style.match_indices('#') {
                let value = &style[index..index + 7];
                let (class, property, color) = palette::ROLES
                    .iter()
                    .find(|(_, _, color)| color.light == value)
                    .unwrap_or_else(|| panic!("{value} is not in the palette: <{tag}>"));
                assert!(
                    classes.split(' ').any(|c| c == *class),
                    "{value} needs the class {class}: <{tag}>"
                );
                let rule = format!(".{class} {{ {property}: {} !important; }}", color.dark);
                assert!(dark.contains(&rule), "no {rule}");
                used.push(*class);
            }
        }
        for (class, _, _) in palette::ROLES {
            assert!(used.contains(class), "{class} is not used");
        }
    }

    #[test]
    fn the_email_has_both_bodies_and_the_logo() {
        let content = greeting("Ann");
        let to: Address = "ann@example.com".parse().unwrap();

        let (message, message_id) = build(&from(), to, content).unwrap();
        let formatted = String::from_utf8(message.formatted()).unwrap();

        assert!(formatted.contains("From: TwoViewer <no-reply@twoviewer.test>\r\n"));
        assert!(formatted.contains("To: ann@example.com\r\n"));
        assert!(formatted.contains("Subject: Hello Ann\r\n"));
        assert!(formatted.contains("Auto-Submitted: auto-generated\r\n"));
        assert!(message_id.ends_with("@twoviewer.test"), "{message_id}");
        assert!(formatted.contains(&format!("Message-ID: <{message_id}>\r\n")));
        let alternative = formatted.find("multipart/alternative").unwrap();
        let plain = formatted.find("Content-Type: text/plain").unwrap();
        let related = formatted.find("multipart/related").unwrap();
        let html = formatted.find("Content-Type: text/html").unwrap();
        let logo = formatted.find("Content-Type: image/png").unwrap();
        assert!(alternative < plain && plain < related && related < html && html < logo);
        assert!(formatted.contains("Content-ID: <logo>\r\n"), "{formatted}");
        assert!(
            formatted.contains("Content-Disposition: inline"),
            "{formatted}"
        );
    }

    #[test]
    fn the_logo_is_a_png() {
        assert!(LOGO.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
