//! Client descriptions, such as "Firefox on macOS", from the `User-Agent` header: the browser and
//! operating system of audit events (AUD-02) and sign-ins (AUTH-09).
//!
//! The description is made only of the fixed names below, never of text from the header, so that
//! what a client sends cannot end up in audit events or logs. Only the families that matter to
//! people reading their audit log are told apart; versions are left out.

/// Browsers, in the order they are recognized: a browser built on another names that one in its
/// `User-Agent` too, so Edge comes before Chrome, and Chrome before Safari.
const BROWSERS: &[(&str, &str)] = &[
    ("Edg/", "Edge"),
    ("EdgA/", "Edge"),
    ("EdgiOS/", "Edge"),
    ("OPR/", "Opera"),
    ("SamsungBrowser/", "Samsung Internet"),
    ("Firefox/", "Firefox"),
    ("FxiOS/", "Firefox"),
    ("CriOS/", "Chrome"),
    ("Chrome/", "Chrome"),
    ("Chromium/", "Chromium"),
    ("Safari/", "Safari"),
];

/// Operating systems, in the order they are recognized: iPhones say "like Mac OS X", and Android
/// and ChromeOS say "Linux".
const SYSTEMS: &[(&str, &str)] = &[
    ("iPhone", "iOS"),
    ("iPad", "iPadOS"),
    ("Android", "Android"),
    ("CrOS", "ChromeOS"),
    ("Windows", "Windows"),
    ("Macintosh", "macOS"),
    ("Mac OS X", "macOS"),
    ("Linux", "Linux"),
];

/// Describes the client that sent `user_agent`, such as "Firefox on macOS".
pub fn describe(user_agent: &str) -> String {
    let find = |names: &[(&str, &'static str)]| {
        names
            .iter()
            .find(|(token, _)| user_agent.contains(token))
            .map(|(_, name)| *name)
    };
    match (find(BROWSERS), find(SYSTEMS)) {
        (Some(browser), Some(system)) => format!("{browser} on {system}"),
        (Some(browser), None) => browser.to_owned(),
        (None, Some(system)) => format!("Unknown browser on {system}"),
        (None, None) => "Unknown browser".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_browsers_are_recognized() {
        for (user_agent, description) in [
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.6; rv:133.0) Gecko/20100101 Firefox/133.0",
                "Firefox on macOS",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36",
                "Chrome on Windows",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
                "Edge on Windows",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like \
                 Gecko) Version/18.1 Safari/605.1.15",
                "Safari on macOS",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
                "Safari on iOS",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) CriOS/131.0.6778.73 Mobile/15E148 Safari/604.1",
                "Chrome on iOS",
            ),
            (
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Mobile Safari/537.36",
                "Chrome on Android",
            ),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36",
                "Chrome on ChromeOS",
            ),
            (
                "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
                "Firefox on Linux",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36 OPR/115.0.0.0",
                "Opera on Windows",
            ),
        ] {
            assert_eq!(describe(user_agent), description, "{user_agent}");
        }
    }

    #[test]
    fn unknown_clients_are_described_without_their_text() {
        assert_eq!(describe(""), "Unknown browser");
        assert_eq!(describe("curl/8.7.1"), "Unknown browser");
        assert_eq!(
            describe("Mozilla/5.0 (Windows NT 10.0) <script>"),
            "Unknown browser on Windows"
        );
        assert_eq!(describe("Firefox/133.0"), "Firefox");
    }
}
