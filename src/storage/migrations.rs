use refinery::error::Error;
use rusqlite::Connection;

mod embedded {
    use refinery::embed_migrations;

    embed_migrations!("./migrations");
}

pub fn run_migrations(conn: &mut Connection) -> Result<(), Error> {
    embedded::migrations::runner().run(conn).map(|_| ())
}
