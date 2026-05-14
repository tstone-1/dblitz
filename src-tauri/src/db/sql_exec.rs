use super::{read_row, DbState, SqlResult};

const SQL_RESULT_LIMIT: usize = 10_000;

pub fn execute_sql(state: &DbState, sql: &str) -> SqlResult {
    let guard = state.conn.lock();
    let conn = match guard.as_ref() {
        Some(c) => c,
        None => {
            return SqlResult {
                columns: vec![],
                rows: vec![],
                rows_affected: 0,
                error: Some("No database open".to_string()),
            }
        }
    };

    let trimmed = sql.trim();
    let mut stmt = match conn.prepare(trimmed) {
        Ok(s) => s,
        Err(e) => {
            return SqlResult {
                columns: vec![],
                rows: vec![],
                rows_affected: 0,
                error: Some(e.to_string()),
            };
        }
    };

    if !stmt.readonly() {
        return SqlResult {
            columns: vec![],
            rows: vec![],
            rows_affected: 0,
            error: Some(
                "dblitz is a read-only viewer — write statements (INSERT, UPDATE, DELETE, DROP, CREATE, ALTER, etc.) are not supported.".to_string(),
            ),
        };
    }

    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let col_count = columns.len();
    let result = match stmt.query([]) {
        Ok(mut rows_iter) => {
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            let mut truncated = false;
            loop {
                if rows.len() >= SQL_RESULT_LIMIT {
                    truncated = true;
                    break;
                }
                match rows_iter.next() {
                    Ok(Some(row)) => rows.push(read_row(row, col_count)),
                    Ok(None) => break,
                    Err(e) => {
                        return SqlResult {
                            columns,
                            rows,
                            rows_affected: 0,
                            error: Some(e.to_string()),
                        };
                    }
                }
            }
            let error = if truncated {
                Some(format!("Result truncated to {} rows", SQL_RESULT_LIMIT))
            } else {
                None
            };
            SqlResult {
                columns,
                rows,
                rows_affected: 0,
                error,
            }
        }
        Err(e) => SqlResult {
            columns: vec![],
            rows: vec![],
            rows_affected: 0,
            error: Some(e.to_string()),
        },
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::{close_database, open_database};
    use rusqlite::Connection;

    fn setup_temp_db_with_table() -> (DbState, std::path::PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dblitz_test_{}.sqlite", nanos));

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
                .unwrap();
            conn.execute("INSERT INTO users (name) VALUES ('alice')", [])
                .unwrap();
        }

        let state = DbState::new();
        open_database(&state, path.to_str().unwrap()).unwrap();
        (state, path)
    }

    #[test]
    fn execute_sql_rejects_writes_with_friendly_message() {
        let (state, path) = setup_temp_db_with_table();

        let result = execute_sql(&state, "DELETE FROM users");

        assert!(result.error.is_some(), "expected DELETE to be rejected");
        let err = result.error.unwrap();
        assert!(
            err.contains("read-only viewer"),
            "expected friendly read-only message, got: {err}"
        );
        assert!(result.rows.is_empty());
        assert_eq!(result.rows_affected, 0);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_allows_select_on_readonly_connection() {
        let (state, path) = setup_temp_db_with_table();

        let result = execute_sql(&state, "SELECT name FROM users");

        assert!(
            result.error.is_none(),
            "SELECT should succeed, got: {:?}",
            result.error
        );
        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0].as_deref(), Some("alice"));

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }
}
