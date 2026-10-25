//! Host and Origin checks for the phone-facing server. Shared by OpenRemote and Kai Remote: edit it in
//! the cuppa plugin repo (shared/) and run `harness.mjs sync --write`, never in a project copy.
use std::net::IpAddr;

/// The address the client used must be one a person would type: an IP address, `localhost`, this PC's name
/// or a `.local` name. That stops DNS rebinding (a web page re-pointing its own name at this PC).
pub fn host_allowed(host: &str, machine: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => match host.rsplit_once(':') {
            Some((n, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => n,
            _ => host,
        },
    };
    if name.is_empty() {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    name.parse::<IpAddr>().is_ok()
        || lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || (!machine.is_empty() && lower == machine.to_ascii_lowercase())
}

/// A request that carries an `Origin` (every browser WebSocket and every cross-site fetch does) must come from
/// the page this server itself serves: the origin's host and port must equal the `Host` header.
pub fn origin_matches(origin: &str, host: &str) -> bool {
    let rest = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://"));
    rest.is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_people_type_are_allowed() {
        for h in [
            "192.168.1.20:1990",
            "127.0.0.1:1990",
            "[::1]:1990",
            "localhost:1990",
            "localhost",
            "my-pc:1990",
            "MY-PC:1990",
            "laptop.local:1990",
            "10.0.0.5",
        ] {
            assert!(host_allowed(h, "My-PC"), "{h}");
        }
    }

    #[test]
    fn rebinding_style_hosts_are_refused() {
        for h in ["", "evil.example:1990", "evil.example", "my-pc.evil.example:1990", ":1990", "attacker.com:80"] {
            assert!(!host_allowed(h, "My-PC"), "{h}");
        }
    }

    #[test]
    fn origin_must_equal_the_host() {
        assert!(origin_matches("http://192.168.1.20:1990", "192.168.1.20:1990"));
        assert!(origin_matches("http://MY-PC:1990", "my-pc:1990"));
        assert!(!origin_matches("http://evil.example", "192.168.1.20:1990"));
        assert!(!origin_matches("http://192.168.1.20:8080", "192.168.1.20:1990"));
        assert!(!origin_matches("null", "192.168.1.20:1990"));
        assert!(!origin_matches("", "192.168.1.20:1990"));
        assert!(!origin_matches("chrome-extension://abc", "192.168.1.20:1990"));
    }
}
