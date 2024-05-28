use rusqlite::{Connection, Result};

struct User {
    telegram_id: i64,
    username: Option<String>,
    plan: String,
}

pub fn get_connection() -> Result<Connection> {
    Connection::open("database.sqlite")
}

fn create_user(conn: &Connection, telegram_id: i64, username: Option<String>) -> Result<()> {
    conn.execute(
        "INSERT INTO users (telegram_id, username) VALUES (?1, ?2)",
        &[&telegram_id as &dyn rusqlite::ToSql, &username as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

fn get_user(conn: &Connection, telegram_id: i64) -> Result<Option<User>> {
    let mut stmt = conn.prepare("SELECT telegram_id, username, plan FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let telegram_id: i64 = row.get(0)?;
        let username: Option<String> = row.get(1)?;
        let plan: String = row.get(2)?;

        Ok(Some(User {
            telegram_id,
            username,
            plan,
        }))
    } else {
        Ok(None)
    }
}

fn update_user_plan(conn: &Connection, telegram_id: i64, plan: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        &[&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

fn log_request(conn: &Connection, user_id: i64, request_text: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO request_history (user_id, request_text) VALUES (?1, ?2)",
        &[&user_id as &dyn rusqlite::ToSql, &request_text as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}
