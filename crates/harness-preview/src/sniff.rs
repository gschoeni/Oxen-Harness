//! Detecting the URL a dev server announces on startup.
//!
//! Frameworks don't all honor the `PORT` we assign (Vite, notably, picks its
//! own), so the server's stdout/stderr is the source of truth: the first
//! local URL it prints wins over the port we asked for.

/// Hosts that count as "this machine" in a printed dev-server URL.
const LOCAL_HOSTS: &[&str] = &["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"];

/// Strip ANSI escape sequences (dev servers love colored output) so URL
/// detection sees plain text.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // CSI sequence: ESC [ ... terminated by a byte in @..~. Anything else
        // after ESC is a short escape; dropping the ESC alone is close enough.
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// Find the first local dev-server URL in one line of output, returning
/// `(port, url)`. Wildcard hosts (`0.0.0.0`, `[::]`) are rewritten to
/// `localhost` so the result is directly loadable in a browser.
pub fn detect_local_url(line: &str) -> Option<(u16, String)> {
    let line = strip_ansi(line);
    let mut rest = line.as_str();
    while let Some(idx) = rest.find("http") {
        rest = &rest[idx..];
        let scheme = ["http://", "https://"]
            .into_iter()
            .find(|s| rest.starts_with(s));
        let Some(scheme) = scheme else {
            rest = &rest[4..];
            continue;
        };
        if let Some((host, port)) = parse_host_port(&rest[scheme.len()..]) {
            let host = match host {
                "0.0.0.0" | "[::]" => "localhost",
                other => other,
            };
            return Some((port, format!("{scheme}{host}:{port}")));
        }
        rest = &rest[scheme.len()..];
    }
    None
}

/// Parse `<local-host>:<port>` at the start of `rest`.
fn parse_host_port(rest: &str) -> Option<(&'static str, u16)> {
    let host = LOCAL_HOSTS.iter().find(|h| rest.starts_with(**h))?;
    let after = rest[host.len()..].strip_prefix(':')?;
    let digits: &str = &after[..after
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(after.len())];
    digits
        .parse()
        .ok()
        // ":0" is a bind-to-any placeholder some servers echo, never a real
        // listening port — treating it as a candidate would poison readiness.
        .filter(|port| *port != 0)
        .map(|port| (*host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vite_style_colored_output() {
        let line = "  \x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m";
        assert_eq!(
            detect_local_url(line),
            Some((5173, "http://localhost:5173".to_string()))
        );
    }

    #[test]
    fn normalizes_wildcard_hosts_to_localhost() {
        let line = "Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ...";
        assert_eq!(
            detect_local_url(line),
            Some((8000, "http://localhost:8000".to_string()))
        );
    }

    #[test]
    fn ignores_lines_without_local_urls() {
        assert_eq!(detect_local_url("compiled successfully in 512ms"), None);
        assert_eq!(detect_local_url("see https://vitejs.dev/config"), None);
        assert_eq!(detect_local_url("http://localhost:notaport"), None);
        assert_eq!(detect_local_url("binding http://localhost:0 …"), None);
    }

    #[test]
    fn skips_remote_url_and_finds_later_local_one() {
        let line = "docs at https://nextjs.org — ready on http://127.0.0.1:3000";
        assert_eq!(
            detect_local_url(line),
            Some((3000, "http://127.0.0.1:3000".to_string()))
        );
    }

    #[test]
    fn strip_ansi_leaves_plain_text_alone() {
        assert_eq!(strip_ansi("hello world"), "hello world");
        assert_eq!(strip_ansi("\x1b[1;36mhello\x1b[0m"), "hello");
    }
}
