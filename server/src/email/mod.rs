//! Transactional emails (IF-12): email verification, password reset, email change and security
//! notifications.
//!
//! Emails go over SMTP to an external email delivery service (DE-01), which delivers them from a
//! TwoViewer domain with sender authentication. `server/README.md` says how to set that up, and
//! how to check it with `twoviewer-server send-test-email`.
//!
//! # Sending
//!
//! A feature sends an email with [`Mailer::send`] once the change behind it is committed:
//!
//! ```ignore
//! txn.commit().await?;
//! mailer.send(&user.email, &VerifyEmail { link });
//! ```
//!
//! `send` returns at once, and the email is delivered in the background, with retries. So a
//! response takes as long whether or not it sends an email, and reveals nothing about whether an
//! address is registered (SEC-14). Whether an email arrived shows only in logs and metrics.
//!
//! # Writing an email
//!
//! Each email is a type that implements [`Email`], with two templates in `server/templates/email/`:
//! `<name>.txt`, the subject and the plain text, and `<name>.html`. They extend `layout.txt` and
//! `layout.html`, which add the name and logo (UI-01) and work in light and dark mode (THM-11), and
//! say which blocks to fill. All their text is in the templates rather than in code, so that it can
//! be translated later (A11Y-04). [`TestEmail`] is the simplest example.
//!
//! # Secrets
//!
//! Emails carry secrets, such as single-use links, and personal data. So recipients, subjects and
//! content are never logged (MNT-03), and emails waiting to be sent are kept in memory only, never
//! in the database, where tokens are stored only as hashes (SEC-07). The price is that emails
//! waiting when the server crashes are lost; people ask for the link again.

mod mailer;
mod message;
pub mod palette;
mod test_email;

pub use mailer::{Mailer, SendError, SetupError};
pub use message::{Content, Email};
pub use test_email::TestEmail;
