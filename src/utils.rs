/// Экранирует специальные символы в имени файла для безопасного использования.
/// 
/// Заменяет символ `/` на `_` для предотвращения проблем с путями файлов.
/// 
/// # Arguments
/// 
/// * `filename` - Исходное имя файла
/// 
/// # Returns
/// 
/// Безопасное имя файла с экранированными символами.
/// 
/// # Example
/// 
/// ```
/// use doradura::utils::escape_filename;
/// 
/// let safe = escape_filename("song/name.mp3");
/// assert_eq!(safe, "song_name.mp3");
/// ```
pub fn escape_filename(filename: &str) -> String {
    filename.replace("/", "_")
}

/// Возвращает правильную форму слова "секунд" для русского языка.
/// 
/// Правила склонения:
/// - 1, 21, 31, ... -> "секунду" (винительный падеж единственного числа)
/// - 2-4, 22-24, 32-34, ... -> "секунды" (именительный падеж множественного числа)
/// - 5-20, 25-30, 35-40, ... -> "секунд" (родительный падеж множественного числа)
/// 
/// # Arguments
/// 
/// * `n` - Число секунд
/// 
/// # Returns
/// 
/// Правильную форму слова "секунд" в зависимости от числа.
/// 
/// # Example
/// 
/// ```
/// use doradura::utils::pluralize_seconds;
/// 
/// assert_eq!(pluralize_seconds(1), "секунду");
/// assert_eq!(pluralize_seconds(5), "секунд");
/// assert_eq!(pluralize_seconds(2), "секунды");
/// ```
pub fn pluralize_seconds(n: u64) -> &'static str {
    let n_mod_100 = n % 100;
    let n_mod_10 = n % 10;
    
    // Исключения: 11, 12, 13, 14 - всегда "секунд"
    if n_mod_100 >= 11 && n_mod_100 <= 14 {
        return "секунд";
    }
    
    // Остальные случаи зависят от последней цифры
    match n_mod_10 {
        1 => "секунду",
        2..=4 => "секунды",
        _ => "секунд",
    }
}

#[cfg(test)]
mod tests {
    use super::pluralize_seconds;

    #[test]
    fn test_pluralize_seconds() {
        // Единственное число
        assert_eq!(pluralize_seconds(1), "секунду");
        assert_eq!(pluralize_seconds(21), "секунду");
        assert_eq!(pluralize_seconds(31), "секунду");
        assert_eq!(pluralize_seconds(101), "секунду");
        
        // Множественное число (2-4)
        assert_eq!(pluralize_seconds(2), "секунды");
        assert_eq!(pluralize_seconds(3), "секунды");
        assert_eq!(pluralize_seconds(4), "секунды");
        assert_eq!(pluralize_seconds(22), "секунды");
        assert_eq!(pluralize_seconds(23), "секунды");
        assert_eq!(pluralize_seconds(24), "секунды");
        assert_eq!(pluralize_seconds(32), "секунды");
        
        // Множественное число (5-20, 25-30, ...)
        assert_eq!(pluralize_seconds(5), "секунд");
        assert_eq!(pluralize_seconds(10), "секунд");
        assert_eq!(pluralize_seconds(15), "секунд");
        assert_eq!(pluralize_seconds(20), "секунд");
        assert_eq!(pluralize_seconds(25), "секунд");
        assert_eq!(pluralize_seconds(30), "секунд");
        
        // Исключения (11-14)
        assert_eq!(pluralize_seconds(11), "секунд");
        assert_eq!(pluralize_seconds(12), "секунд");
        assert_eq!(pluralize_seconds(13), "секунд");
        assert_eq!(pluralize_seconds(14), "секунд");
        assert_eq!(pluralize_seconds(111), "секунд");
        assert_eq!(pluralize_seconds(112), "секунд");
        
        // Пример из запроса пользователя
        assert_eq!(pluralize_seconds(71), "секунду");
    }
}