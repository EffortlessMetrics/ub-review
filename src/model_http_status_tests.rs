use super::*;

#[test]
fn both_status_marker_cases_survive_wrapped_errors() {
    for marker in ["http status Some(", "HTTP status Some("] {
        for status in [401, 429, 500] {
            let error = anyhow::anyhow!("request failed with {marker}{status}): rejected")
                .context("outer operation context");
            assert_eq!(http_status_from_error(&error), Some(status));
        }
    }
}

#[test]
fn absent_or_malformed_status_is_not_invented() {
    for message in [
        "pending review identity mismatch 500",
        "HTTP status None: request failed",
        "http status Some(): request failed",
        "HTTP status Some(500: missing delimiter",
        "HTTP status Some(error): request failed",
        "HTTP status Some(65536): request failed",
    ] {
        let error = anyhow::anyhow!("{message}").context("outer operation context");
        assert_eq!(http_status_from_error(&error), None, "{message}");
    }
}
