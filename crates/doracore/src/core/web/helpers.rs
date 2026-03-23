//! Small utility functions shared across the web module.

/// Insert an audit log entry into the admin_audit_log table.
pub(super) fn log_audit(
    conn: &rusqlite::Connection,
    admin_id: i64,
    action: &str,
    target_type: &str,
    target_id: &str,
    details: Option<&str>,
) {
    let _ = conn.execute(
        "INSERT INTO admin_audit_log (admin_id, action, target_type, target_id, details) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![admin_id, action, target_type, target_id, details],
    );
}

/// Wrap a search term for SQL `LIKE ? ESCAPE '\'`, escaping `%`, `_` and `\`.
pub(super) fn like_param(term: &str) -> String {
    let escaped = term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Return true only for http(s) URLs; rejects javascript:, data:, etc.
pub(super) fn is_safe_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Constant-time byte-level string comparison to prevent timing side-channels.
pub(super) fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Format seconds as MM:SS or H:MM:SS.
#[allow(dead_code)]
pub(super) fn format_duration(secs: i64) -> String {
    if secs < 0 {
        return String::new();
    }
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// Parse streaming links JSON into individual URLs.
#[allow(dead_code)]
pub(super) fn parse_streaming_links(json_str: &str) -> serde_json::Value {
    serde_json::from_str(json_str).unwrap_or_default()
}

/// Format an integer with thousands separators, e.g. 1234567 -> "1,234,567".
pub(super) fn fmt_num(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let offset = bytes.len() % 3;
    for (i, &b) in bytes.iter().enumerate() {
        if i != 0 && (i % 3 == offset) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// Minimal HTML entity escaping.
pub(super) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
