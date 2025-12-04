use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

/// Валидация Telegram Web App init data
///
/// Telegram подписывает данные с помощью HMAC-SHA256.
/// Ключ для HMAC создаётся из bot token: HMAC_SHA256("WebAppData", bot_token)
///
/// # Аргументы
/// * `init_data` - Строка с параметрами от Telegram (query string format)
/// * `bot_token` - Токен бота
///
/// # Возвращает
/// `Ok(user_id)` если валидация успешна, иначе `Err`
///
/// # Пример
/// ```rust
/// let init_data = "query_id=...&user={...}&auth_date=...&hash=...";
/// let user_id = validate_telegram_webapp_data(init_data, &bot_token)?;
/// ```
pub fn validate_telegram_webapp_data(init_data: &str, bot_token: &str) -> Result<i64> {
    // Парсим query string в HashMap
    let params: HashMap<String, String> = init_data
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some(key), Some(value)) => {
                    // URL decode значений
                    let decoded_value = urlencoding::decode(value).ok()?;
                    Some((key.to_string(), decoded_value.to_string()))
                }
                _ => None,
            }
        })
        .collect();

    // Извлекаем hash из параметров
    let received_hash = params
        .get("hash")
        .ok_or_else(|| anyhow!("Missing hash parameter"))?;

    // Создаём data_check_string (все параметры кроме hash, отсортированные по ключу)
    let mut check_pairs: Vec<String> = params
        .iter()
        .filter(|(key, _)| key.as_str() != "hash")
        .map(|(key, value)| format!("{}={}", key, value))
        .collect();

    check_pairs.sort();
    let data_check_string = check_pairs.join("\n");

    // Создаём secret key: HMAC_SHA256("WebAppData", bot_token)
    let mut secret_key_mac =
        HmacSha256::new_from_slice(b"WebAppData").expect("HMAC can take key of any size");
    secret_key_mac.update(bot_token.as_bytes());
    let secret_key = secret_key_mac.finalize().into_bytes();

    // Вычисляем hash: HMAC_SHA256(data_check_string, secret_key)
    let mut mac = HmacSha256::new_from_slice(&secret_key).expect("HMAC can take key of any size");
    mac.update(data_check_string.as_bytes());
    let calculated_hash = hex::encode(mac.finalize().into_bytes());

    // Сравниваем хеши
    if calculated_hash != *received_hash {
        return Err(anyhow!("Invalid hash - data may be tampered"));
    }

    // Проверяем auth_date (не старше 24 часов)
    if let Some(auth_date_str) = params.get("auth_date") {
        if let Ok(auth_date) = auth_date_str.parse::<i64>() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            let age_seconds = now - auth_date;
            if age_seconds > 86400 {
                // 24 часа
                return Err(anyhow!("Init data is too old ({} seconds)", age_seconds));
            }
        }
    }

    // Извлекаем user_id из параметра user
    let user_json = params
        .get("user")
        .ok_or_else(|| anyhow!("Missing user parameter"))?;

    let user: serde_json::Value =
        serde_json::from_str(user_json).map_err(|e| anyhow!("Failed to parse user JSON: {}", e))?;

    let user_id = user
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("Missing user id in user JSON"))?;

    Ok(user_id)
}

/// Извлечение user_id из Telegram init data БЕЗ валидации
///
/// Используется когда валидация отключена (для разработки)
///
/// # Аргументы
/// * `init_data` - Строка с параметрами от Telegram
///
/// # Возвращает
/// `Ok(user_id)` если параметр user найден, иначе `Err`
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

    let user_json = params
        .get("user")
        .ok_or_else(|| anyhow!("Missing user parameter"))?;

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
        let user_id = extract_user_id_unsafe(init_data).unwrap();
        assert_eq!(user_id, 123456789);
    }

    #[test]
    fn test_missing_hash() {
        let init_data = "user={\"id\":123}&auth_date=1234567890";
        let bot_token = "test_token";
        let result = validate_telegram_webapp_data(init_data, bot_token);
        assert!(result.is_err());
    }
}
