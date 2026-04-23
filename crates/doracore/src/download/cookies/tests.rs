use super::diagnose_cookies_content;

#[test]
fn test_get_cookies_path() {
    // This test will depend on env vars, just ensure it doesn't crash
    let _path = super::file_ops::get_cookies_path();
}

/// Regression test: modern YouTube exports contain `__Secure-3PSID` /
/// `__Secure-3PAPISID` / `LOGIN_INFO` but NONE of the legacy
/// `SID/HSID/SSID/APISID/SAPISID`. The diagnostic used to mark all 5
/// legacy cookies as ❌ missing, print them red in the report, AND
/// separately conclude "cookies look valid". This test pins the fixed
/// behaviour.
#[test]
fn diagnose_modern_youtube_cookies_are_valid_and_not_missing_legacy() {
    // 3 modern auth cookies + 2 secondary, all with far-future expiry.
    let content = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\t__Secure-3PSID\tmodern_psid\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\t__Secure-3PAPISID\tmodern_papisid\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tLOGIN_INFO\tmodern_login\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tPREF\tsome_pref\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tVISITOR_INFO1_LIVE\tsome_visitor\n";

    let diag = diagnose_cookies_content(content);

    // All 3 modern auth cookies should be recognised
    assert!(diag.auth_cookies_found.iter().any(|n| n == "__Secure-3PSID"));
    assert!(diag.auth_cookies_found.iter().any(|n| n == "__Secure-3PAPISID"));
    assert!(diag.auth_cookies_found.iter().any(|n| n == "LOGIN_INFO"));

    // No legacy cookies should be reported as missing — user is on the
    // modern scheme, legacy names are irrelevant.
    assert!(
        diag.auth_cookies_missing.is_empty(),
        "expected empty missing list, got: {:?}",
        diag.auth_cookies_missing
    );

    // The overall report should be valid
    assert!(diag.is_valid, "modern-only cookies should be valid");

    // Formatted report must NOT contain the legacy ❌ lines
    let report = diag.format_report();
    assert!(
        !report.contains("❌ SID"),
        "report should not show legacy SID as missing"
    );
    assert!(
        !report.contains("❌ HSID"),
        "report should not show legacy HSID as missing"
    );
    assert!(
        !report.contains("❌ SAPISID"),
        "report should not show legacy SAPISID as missing"
    );
}

/// Complement: a pure legacy export (SID + HSID + ...) must still work.
#[test]
fn diagnose_legacy_youtube_cookies_are_valid() {
    let content = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tSID\tlegacy_sid\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tHSID\tlegacy_hsid\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tSSID\tlegacy_ssid\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tAPISID\tlegacy_apisid\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tSAPISID\tlegacy_sapisid\n";

    let diag = diagnose_cookies_content(content);
    assert!(diag.is_valid, "legacy-only cookies should still be valid");
    assert!(diag.auth_cookies_missing.is_empty());
}

/// A truly broken export (no auth cookies at all) must be reported invalid.
#[test]
fn diagnose_no_auth_cookies_is_invalid() {
    let content = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t9999999999\tPREF\tjust_pref\n";

    let diag = diagnose_cookies_content(content);
    assert!(!diag.is_valid, "cookies with no auth should be invalid");
}

#[tokio::test]
async fn test_update_cookies_invalid_base64() {
    let result = super::update_cookies_from_base64("not-valid-base64!@#").await;
    assert!(result.is_err());
}

#[test]
fn test_cookies_validation_format() {
    let valid_content = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\ttest\tvalue";
    assert!(valid_content.contains("# Netscape HTTP Cookie File"));
    assert!(valid_content.contains(".youtube.com"));
}

#[test]
fn test_parse_cookies_for_domain() {
    let content = "# Netscape HTTP Cookie File\n\
        .instagram.com\tTRUE\t/\tTRUE\t0\tsessionid\tabc123\n\
        .instagram.com\tTRUE\t/\tTRUE\t0\tcsrftoken\txyz789\n\
        .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tyt_sid\n";

    let result = super::parse_cookies_for_domain(content, "instagram.com");
    assert!(result.is_some());
    let header = result.unwrap();
    assert!(header.contains("sessionid=abc123"));
    assert!(header.contains("csrftoken=xyz789"));
    assert!(!header.contains("SID"));
}

#[test]
fn test_parse_cookies_for_domain_no_match() {
    let content = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tyt_sid\n";
    let result = super::parse_cookies_for_domain(content, "instagram.com");
    assert!(result.is_none());
}

#[test]
fn test_diagnose_ig_cookies_content_valid() {
    let content = "# Netscape HTTP Cookie File\n\
        .instagram.com\tTRUE\t/\tTRUE\t9999999999\tsessionid\tabc123\n\
        .instagram.com\tTRUE\t/\tTRUE\t9999999999\tcsrftoken\txyz789\n\
        .instagram.com\tTRUE\t/\tTRUE\t9999999999\tds_user_id\t12345\n\
        .instagram.com\tTRUE\t/\tTRUE\t0\tmid\tmid_val\n";

    let diag = super::diagnose_ig_cookies_content(content);
    assert!(diag.is_valid);
    assert!(diag.auth_cookies_missing.is_empty());
    assert_eq!(diag.auth_cookies_found.len(), 3);
}

#[test]
fn test_diagnose_ig_cookies_content_missing_sessionid() {
    let content = "# Netscape HTTP Cookie File\n\
        .instagram.com\tTRUE\t/\tTRUE\t0\tcsrftoken\txyz789\n";

    let diag = super::diagnose_ig_cookies_content(content);
    assert!(!diag.is_valid);
    assert!(diag.auth_cookies_missing.contains(&"sessionid".to_string()));
}

#[test]
fn test_extract_cookie_value_for_domain() {
    let content = "# Netscape HTTP Cookie File\n\
        .instagram.com\tTRUE\t/\tTRUE\t0\tsessionid\tabc123\n\
        .instagram.com\tTRUE\t/\tTRUE\t0\tcsrftoken\tmy_csrf_token\n\
        .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tyt_sid\n";

    assert_eq!(
        super::extract_cookie_value_for_domain(content, "instagram.com", "csrftoken"),
        Some("my_csrf_token".to_string())
    );
    assert_eq!(
        super::extract_cookie_value_for_domain(content, "instagram.com", "sessionid"),
        Some("abc123".to_string())
    );
    assert_eq!(
        super::extract_cookie_value_for_domain(content, "instagram.com", "nonexistent"),
        None
    );
    assert_eq!(
        super::extract_cookie_value_for_domain(content, "youtube.com", "csrftoken"),
        None
    );
}
