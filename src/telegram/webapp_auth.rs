use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

/// Validates Telegram Web App init data.
///
/// Telegram signs the data using HMAC-SHA256.
/// The HMAC key is derived from the bot token: HMAC_SHA256("WebAppData", bot_token)
///
/// # Arguments
/// * `init_data` - Parameter string from Telegram (query string format)
/// * `bot_token` - Bot token
///
/// # Returns
/// `Ok(user_id)` when validation succeeds, otherwise `Err`
///
/// # Example
/// ```no_run
/// use doradura::telegram::webapp_auth::validate_telegram_webapp_data;
///
/// # fn main() -> anyhow::Result<()> {
/// let init_data = "query_id=...&user={...}&auth_date=...&hash=...";
/// let bot_token = "your_bot_token";
/// let user_id = validate_telegram_webapp_data(init_data, bot_token)?;
/// # Ok(())
/// # }
/// ```
pub fn validate_telegram_webapp_data(init_data: &str, bot_token: &str) -> Result<i64> {
    // Parse the query string into a HashMap
    let params: HashMap<String, String> = init_data
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some(key), Some(value)) => {
                    // URL decode the values
                    let decoded_value = urlencoding::decode(value).ok()?;
                    Some((key.to_string(), decoded_value.to_string()))
                }
                _ => None,
            }
        })
        .collect();

    // Extract the hash from parameters
    let received_hash = params.get("hash").ok_or_else(|| anyhow!("Missing hash parameter"))?;

    // Build data_check_string (all parameters except hash, sorted by key)
    let mut check_pairs: Vec<String> = params
        .iter()
        .filter(|(key, _)| key.as_str() != "hash")
        .map(|(key, value)| format!("{}={}", key, value))
        .collect();

    check_pairs.sort();
    let data_check_string = check_pairs.join("\n");

    // Build secret key: HMAC_SHA256("WebAppData", bot_token)
    let mut secret_key_mac = HmacSha256::new_from_slice(b"WebAppData").expect("HMAC can take key of any size");
    secret_key_mac.update(bot_token.as_bytes());
    let secret_key = secret_key_mac.finalize().into_bytes();

    // Compute the hash: HMAC_SHA256(data_check_string, secret_key)
    let mut mac = HmacSha256::new_from_slice(&secret_key).expect("HMAC can take key of any size");
    mac.update(data_check_string.as_bytes());
    let calculated_hash = hex::encode(mac.finalize().into_bytes());

    // Compare hashes
    if calculated_hash != *received_hash {
        return Err(anyhow!("Invalid hash - data may be tampered"));
    }

    // Validate auth_date (must not be older than 24 hours)
    if let Some(auth_date_str) = params.get("auth_date") {
        if let Ok(auth_date) = auth_date_str.parse::<i64>() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| anyhow!("System time error: {}", e))?
                .as_secs() as i64;

            let age_seconds = now - auth_date;
            if age_seconds > 86400 {
                // 24 hours
                return Err(anyhow!("Init data is too old ({} seconds)", age_seconds));
            }
        }
    }

    // Extract user_id from the user parameter
    let user_json = params.get("user").ok_or_else(|| anyhow!("Missing user parameter"))?;

    let user: serde_json::Value =
        serde_json::from_str(user_json).map_err(|e| anyhow!("Failed to parse user JSON: {}", e))?;

    let user_id = user
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("Missing user id in user JSON"))?;

    Ok(user_id)
}

/// Extracts user_id from Telegram init data WITHOUT validation.
///
/// Used when validation is disabled (for development).
///
/// # Arguments
/// * `init_data` - Parameter string from Telegram
///
/// # Returns
/// `Ok(user_id)` if the user parameter exists, otherwise `Err`
pub fn extract_user_id_unsafe(init_data: &str) -> Result<i64> {
    let params: HashMap<String, String> = init_data
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some(key), Some(value)) => {
                    let decoded_value = urlencoding::decode(value).ok()?;
                    Some((key.to_string(), decoded_value.to_string()))
                }
                _ => None,
            }
        })
        .collect();

    let user_json = params.get("user").ok_or_else(|| anyhow!("Missing user parameter"))?;

    let user: serde_json::Value =
        serde_json::from_str(user_json).map_err(|e| anyhow!("Failed to parse user JSON: {}", e))?;

    let user_id = user
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("Missing user id in user JSON"))?;

    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_user_id() {
        let init_data = "user=%7B%22id%22%3A123456789%2C%22first_name%22%3A%22Test%22%7D&auth_date=1234567890&hash=abc";
        let user_id = extract_user_id_unsafe(init_data).expect("Failed to extract user_id in test");
        assert_eq!(user_id, 123456789);
    }

    #[test]
    fn test_missing_hash() {
        let init_data = "user={\"id\":123}&auth_date=1234567890";
        let bot_token = "test_token";
        let result = validate_telegram_webapp_data(init_data, bot_token);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing hash"));
    }

    #[test]
    fn test_extract_user_id_missing_user() {
        let init_data = "auth_date=1234567890&hash=abc";
        let result = extract_user_id_unsafe(init_data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing user"));
    }

    #[test]
    fn test_extract_user_id_invalid_json() {
        let init_data = "user=not-valid-json&auth_date=1234567890&hash=abc";
        let result = extract_user_id_unsafe(init_data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to parse"));
    }

    #[test]
    fn test_extract_user_id_missing_id_in_json() {
        let init_data = "user=%7B%22first_name%22%3A%22Test%22%7D&auth_date=1234567890&hash=abc";
        let result = extract_user_id_unsafe(init_data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing user id"));
    }

    #[test]
    fn test_validate_missing_user() {
        let init_data = "auth_date=1234567890&hash=abc123def456";
        let bot_token = "test_token";
        let result = validate_telegram_webapp_data(init_data, bot_token);
        // First it will fail on hash validation or missing user
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_invalid_hash() {
        let init_data = "user=%7B%22id%22%3A123%7D&auth_date=1234567890&hash=invalidhash";
        let bot_token = "test_token";
        let result = validate_telegram_webapp_data(init_data, bot_token);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid hash"));
    }

    #[test]
    fn test_url_decoding() {
        // Test URL encoded JSON
        let init_data = "user=%7B%22id%22%3A999%7D&auth_date=1234567890&hash=abc";
        let result = extract_user_id_unsafe(init_data);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 999);
    }

    #[test]
    fn test_extract_with_complex_user() {
        // Test with more user fields
        let user = r#"{"id":12345,"first_name":"John","last_name":"Doe","username":"johndoe"}"#;
        let encoded_user = urlencoding::encode(user);
        let init_data = format!("user={}&auth_date=1234567890&hash=abc", encoded_user);
        let result = extract_user_id_unsafe(&init_data);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 12345);
    }

    #[test]
    fn test_empty_init_data() {
        let result = extract_user_id_unsafe("");
        assert!(result.is_err());
    }

    #[test]
    fn test_malformed_pair() {
        // A pair without = should be skipped
        let init_data = "invalid&user=%7B%22id%22%3A123%7D&auth_date=1234567890";
        let result = extract_user_id_unsafe(init_data);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 123);
    }
}
