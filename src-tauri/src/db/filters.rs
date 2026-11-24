use regex::Regex;

use super::types::{ColumnFilter, ColumnInfo};
use super::util::{quote_ident, render_real};

#[derive(Debug)]
pub(super) struct WhereResult {
    pub(super) clause: String,
    pub(super) params: Vec<String>,
    pub(super) regex_filters: Vec<(usize, Regex)>,
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub(crate) fn contains_pattern(value: &str) -> String {
    format!("%{}%", escape_like(value))
}

/// SQLite checks for INT and text affinity before looking for BLOB.
fn has_no_affinity(declared: &str) -> bool {
    let t = declared.trim().to_ascii_uppercase();
    t.is_empty()
        || (t.contains("BLOB")
            && !["INT", "CHAR", "CLOB", "TEXT"]
                .iter()
                .any(|k| t.contains(k)))
}

/// SQLite compares numeric cells in a column with no affinity against a
/// numeric literal. Preserve the full signed INTEGER range before trying REAL.
fn numeric_literal(no_affinity: bool, operand: &str) -> Option<String> {
    if !no_affinity {
        return None;
    }
    if let Ok(integer) = operand.parse::<i64>() {
        return Some(integer.to_string());
    }
    operand
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .map(render_real)
}

/// One `;`-separated piece of a column filter, after its operator prefix is
/// recognised. Shared by the SQLite builder below and the Parquet one in
/// `pq/filters.rs`, so the grammar exists once and cannot drift between them.
///
/// Operator prefixes recognized: "<>", ">=", "<=", ">", "<", "=". The frontend
/// mirrors the operand-requiring subset in
/// src/lib/components/filterOperators.ts (OPERAND_REQUIRED_OPS) to gate
/// half-typed filters - keep the two in sync when adding ops.
/// `operand_required_ops_match_frontend` below parses that TS file and pins the
/// two lists together, so an op added on one side only fails a test rather
/// than shipping.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Criterion<'a> {
    /// `<>` alone: not NULL and not empty.
    NotEmpty,
    /// `<>x`: does not contain x. ANDed.
    NotContains(&'a str),
    Ge(&'a str),
    Le(&'a str),
    Gt(&'a str),
    Lt(&'a str),
    /// `=x`. ORed.
    Eq(&'a str),
    /// No operator: contains the text. ORed.
    Contains(&'a str),
}

impl Criterion<'_> {
    /// ANDed with the column's other criteria, rather than ORed.
    pub(crate) fn is_and(&self) -> bool {
        !matches!(self, Criterion::Eq(_) | Criterion::Contains(_))
    }
}

/// Splits a column filter on `;` and parses each non-empty piece.
pub(crate) fn parse_criteria(value: &str) -> Vec<Criterion<'_>> {
    value
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|val| {
            if let Some(rest) = val.strip_prefix("<>") {
                if rest.is_empty() {
                    Criterion::NotEmpty
                } else {
                    Criterion::NotContains(rest)
                }
            } else if let Some(rest) = val.strip_prefix(">=") {
                Criterion::Ge(rest)
            } else if let Some(rest) = val.strip_prefix("<=") {
                Criterion::Le(rest)
            } else if let Some(rest) = val.strip_prefix('>') {
                Criterion::Gt(rest)
            } else if let Some(rest) = val.strip_prefix('<') {
                Criterion::Lt(rest)
            } else if let Some(rest) = val.strip_prefix('=') {
                Criterion::Eq(rest)
            } else {
                Criterion::Contains(val)
            }
        })
        .collect()
}

/// Join one column's parsed criteria into a single `WHERE` part: the ORed
/// inclusions as one group, ANDed with every exclusion and comparison. Pushes
/// nothing when the column contributed nothing.
///
/// Shared by both builders because the parameter order is part of it: the
/// inclusions' parameters go first because their SQL does, and a copy of this
/// that reordered one list but not the other would bind every value to the
/// wrong `?` without any error.
pub(crate) fn push_column_criteria(
    mut ors: Vec<String>,
    or_params: Vec<String>,
    ands: Vec<String>,
    and_params: Vec<String>,
    parts: &mut Vec<String>,
    params: &mut Vec<String>,
) {
    let mut col_parts: Vec<String> = Vec::new();
    match ors.len() {
        0 => {}
        1 => col_parts.push(ors.remove(0)),
        _ => col_parts.push(format!("({})", ors.join(" OR "))),
    }
    col_parts.extend(ands);
    params.extend(or_params);
    params.extend(and_params);
    match col_parts.len() {
        0 => {}
        1 => parts.push(col_parts.remove(0)),
        _ => parts.push(format!("({})", col_parts.join(" AND "))),
    }
}

/// One range comparison (`>=`, `<=`, `>`, `<`) against a bound text operand.
///
/// On a column with no affinity SQLite does not convert the bound text to a
/// number, and across storage classes a number always sorts below any text.
/// So `<5` against the INTEGERs 1 to 10 in `CREATE TABLE t(v)`, or in a view
/// column `a + 1 AS v`, returned all ten rows and `>5` returned none (measured
/// on SQLite 3.53.1). When the operand reads as a finite number, numeric cells
/// are compared against it as a number, using the same integer-preserving
/// conversion as the `=` arm, and text cells
/// keep the text comparison they always had. Each half is guarded by
/// `typeof`, because without the guard every text cell satisfies `> 5.0` and
/// every numeric cell satisfies `< '5'`. A typed column keeps the single
/// comparison: SQLite converts the operand there itself, and the extra arms
/// would cost the index.
fn range_comparison(col: &str, op: &str, operand: &str, no_affinity: bool) -> String {
    match numeric_literal(no_affinity, operand) {
        Some(literal) => format!(
            "((typeof({col}) IN ('integer', 'real') AND {col} {op} {literal}) \
             OR (typeof({col}) = 'text' AND {col} {op} ?))",
        ),
        _ => format!("{col} {op} ?"),
    }
}

/// Build the `WHERE` clause for one view.
///
/// `columns` is the table's visible column list with its declared types, in
/// `SELECT *` order. The declared type determines whether comparisons need a
/// numeric arm to reach numeric cells.
pub(super) fn build_where_clause(
    columns: &[ColumnInfo],
    filters: &[ColumnFilter],
    global_filter: &str,
) -> Result<WhereResult, String> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut regex_filters: Vec<(usize, Regex)> = Vec::new();

    if !global_filter.is_empty() {
        // A BLOB declaration permits text and numeric cells in ordinary tables.
        let terms: Vec<_> = columns
            .iter()
            .map(|column| {
                let col = quote_ident(&column.name);
                format!("(typeof({col}) != 'blob' AND {col} LIKE ? ESCAPE '\\')")
            })
            .collect();
        if terms.is_empty() {
            where_parts.push("0".to_string());
        } else {
            where_parts.push(format!("({})", terms.join(" OR ")));
            params.extend(std::iter::repeat_n(
                contains_pattern(global_filter),
                terms.len(),
            ));
        }
    }

    for f in filters {
        if f.value.is_empty() {
            continue;
        }
        // A filter naming a column this table does not have is refused here,
        // for both kinds. A regex never reaches SQL, so it used to be dropped
        // in silence, showing an unfiltered grid under a filter chip that said
        // otherwise. The plain path was assumed to be refused by SQLite, and
        // is not: with SQLite's default double-quoted-string setting an
        // unknown `"ghost"` becomes the string 'ghost', so `"ghost" LIKE
        // '%x%'` quietly compares a constant. Same wording SQLite uses for a
        // real unknown name, because to the user it is the same mistake.
        let Some(idx) = columns.iter().position(|c| c.name == f.column) else {
            return Err(format!("no such column: {}", f.column));
        };
        if f.is_regex {
            match Regex::new(&f.value) {
                Ok(re) => regex_filters.push((idx, re)),
                Err(e) => return Err(format!("Invalid regex '{}': {}", f.column, e)),
            }
        } else {
            let no_affinity = has_no_affinity(&columns[idx].col_type);
            let col_escaped = quote_ident(&f.column);

            // Split on semicolon for multi-criteria: exclusions=AND, inclusions=OR
            let criteria = parse_criteria(&f.value);

            if criteria.is_empty() {
                continue;
            }

            let mut and_parts: Vec<String> = Vec::new();
            let mut and_params: Vec<String> = Vec::new();
            let mut or_parts: Vec<String> = Vec::new();
            let mut or_params: Vec<String> = Vec::new();

            for criterion in criteria {
                match criterion {
                    Criterion::NotEmpty => {
                        and_parts.push(format!(
                            "{} IS NOT NULL AND {} != ''",
                            col_escaped, col_escaped
                        ));
                    }
                    Criterion::NotContains(rest) => {
                        and_parts.push(format!("{} NOT LIKE ? ESCAPE '\\'", col_escaped));
                        and_params.push(contains_pattern(rest));
                    }
                    Criterion::Ge(rest) => {
                        and_parts.push(range_comparison(&col_escaped, ">=", rest, no_affinity));
                        and_params.push(rest.to_string());
                    }
                    Criterion::Le(rest) => {
                        and_parts.push(range_comparison(&col_escaped, "<=", rest, no_affinity));
                        and_params.push(rest.to_string());
                    }
                    Criterion::Gt(rest) => {
                        and_parts.push(range_comparison(&col_escaped, ">", rest, no_affinity));
                        and_params.push(rest.to_string());
                    }
                    Criterion::Lt(rest) => {
                        and_parts.push(range_comparison(&col_escaped, "<", rest, no_affinity));
                        and_params.push(rest.to_string());
                    }
                    Criterion::Eq(rest) => {
                        if let Some(literal) = numeric_literal(no_affinity, rest) {
                            or_parts
                                .push(format!("({col_escaped} = ? OR {col_escaped} = {literal})"));
                        } else {
                            or_parts.push(format!("{col_escaped} = ?"));
                        }
                        or_params.push(rest.to_string());
                    }
                    Criterion::Contains(val) => {
                        or_parts.push(format!("{} LIKE ? ESCAPE '\\'", col_escaped));
                        or_params.push(contains_pattern(val));
                    }
                }
            }

            push_column_criteria(
                or_parts,
                or_params,
                and_parts,
                and_params,
                &mut where_parts,
                &mut params,
            );
        }
    }

    let clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_parts.join(" AND "))
    };

    Ok(WhereResult {
        clause,
        params,
        regex_filters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Columns with no declared type - the shape most of these tests want,
    /// since the clause they assert does not depend on affinity.
    fn cols(names: &[&str]) -> Vec<ColumnInfo> {
        typed_cols(&names.iter().map(|n| (*n, "TEXT")).collect::<Vec<_>>())
    }

    fn typed_cols(columns: &[(&str, &str)]) -> Vec<ColumnInfo> {
        columns
            .iter()
            .enumerate()
            .map(|(cid, (name, col_type))| ColumnInfo {
                cid: cid as i64,
                name: name.to_string(),
                col_type: col_type.to_string(),
                notnull: false,
                default_value: None,
                pk: false,
            })
            .collect()
    }

    fn filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.to_string(),
            value: value.to_string(),
            is_regex: false,
        }
    }

    fn regex_filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.to_string(),
            value: value.to_string(),
            is_regex: true,
        }
    }

    #[test]
    fn basic_contains_filter() {
        let columns = cols(&["name", "age"]);
        let filters = vec![filter("name", "foo")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" LIKE ? ESCAPE '\\'");
        assert_eq!(r.params, vec!["%foo%"]);
        assert!(r.regex_filters.is_empty());
    }

    #[test]
    fn global_filter_or_across_columns() {
        let columns = cols(&["name", "age"]);
        let r = build_where_clause(&columns, &[], "test").unwrap();
        assert_eq!(
            r.clause,
            " WHERE ((typeof(\"name\") != 'blob' AND \"name\" LIKE ? ESCAPE '\\') OR (typeof(\"age\") != 'blob' AND \"age\" LIKE ? ESCAPE '\\'))"
        );
        assert_eq!(r.params, vec!["%test%", "%test%"]);
    }

    #[test]
    fn semicolon_multi_criteria() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "foo;bar")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(
            r.clause,
            " WHERE (\"name\" LIKE ? ESCAPE '\\' OR \"name\" LIKE ? ESCAPE '\\')"
        );
        assert_eq!(r.params, vec!["%foo%", "%bar%"]);
    }

    #[test]
    fn exclusion_not_like() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "<>bad")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" NOT LIKE ? ESCAPE '\\'");
        assert_eq!(r.params, vec!["%bad%"]);
    }

    #[test]
    fn bare_exclusion_non_empty() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "<>")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" IS NOT NULL AND \"name\" != ''");
        assert!(r.params.is_empty());
    }

    #[test]
    fn comparison_operators() {
        let columns = cols(&["price"]);

        let r = build_where_clause(&columns, &[filter("price", ">100")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" > ?");
        assert_eq!(r.params, vec!["100"]);

        let r = build_where_clause(&columns, &[filter("price", "<=50")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" <= ?");
        assert_eq!(r.params, vec!["50"]);

        let r = build_where_clause(&columns, &[filter("price", ">=10")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" >= ?");
        assert_eq!(r.params, vec!["10"]);

        let r = build_where_clause(&columns, &[filter("price", "<5")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" < ?");
        assert_eq!(r.params, vec!["5"]);
    }

    #[test]
    fn exact_match_operator() {
        let columns = cols(&["status"]);
        let filters = vec![filter("status", "=active")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"status\" = ?");
        assert_eq!(r.params, vec!["active"]);
    }

    #[test]
    fn invalid_regex_returns_error() {
        let columns = cols(&["name"]);
        let filters = vec![regex_filter("name", "[invalid")];
        let r = build_where_clause(&columns, &filters, "");
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Invalid regex"));
    }

    #[test]
    fn valid_regex_produces_regex_filter() {
        let columns = cols(&["name", "age"]);
        let filters = vec![regex_filter("name", "^foo.*bar$")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert!(r.clause.is_empty());
        assert!(r.params.is_empty());
        assert_eq!(r.regex_filters.len(), 1);
        assert_eq!(r.regex_filters[0].0, 0);
    }

    #[test]
    fn column_name_with_quotes_is_escaped() {
        let columns = cols(&["col\"name"]);
        let filters = vec![filter("col\"name", "test")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"col\"\"name\" LIKE ? ESCAPE '\\'");
        assert_eq!(r.params, vec!["%test%"]);
    }

    #[test]
    fn empty_filter_value_is_skipped() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert!(r.clause.is_empty());
        assert!(r.params.is_empty());
    }

    #[test]
    fn mixed_include_and_exclude_with_semicolons() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "good;<>bad")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(
            r.clause,
            " WHERE (\"name\" LIKE ? ESCAPE '\\' AND \"name\" NOT LIKE ? ESCAPE '\\')"
        );
        assert_eq!(r.params, vec!["%good%", "%bad%"]);
    }

    #[test]
    fn like_wildcards_are_escaped_for_literal_matching() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", r"50%_done\ok")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" LIKE ? ESCAPE '\\'");
        assert_eq!(r.params, vec![r"%50\%\_done\\ok%"]);
    }

    #[test]
    fn global_filter_composes_with_column_filter_in_param_order() {
        let columns = cols(&["name", "city"]);
        let filters = vec![filter("name", "foo")];
        let r = build_where_clause(&columns, &filters, "x").unwrap();
        assert_eq!(
            r.clause,
            " WHERE ((typeof(\"name\") != 'blob' AND \"name\" LIKE ? ESCAPE '\\') OR (typeof(\"city\") != 'blob' AND \"city\" LIKE ? ESCAPE '\\')) AND \"name\" LIKE ? ESCAPE '\\'"
        );
        assert_eq!(r.params, vec!["%x%", "%x%", "%foo%"]);
    }

    /// The operator prefixes that are meaningless on their own and need an
    /// operand after them. `"<>"` is deliberately absent: a bare `<>` is a
    /// *complete* filter in `build_where_clause` (empty rest means IS NOT NULL
    /// AND != ''), which is why the frontend must not treat it as half-typed.
    const OPERAND_REQUIRED_OPS: &[&str] = &[">=", "<=", ">", "<", "="];

    #[test]
    fn operand_required_ops_are_what_the_builder_actually_parses() {
        // Pins the list above to this file's behaviour rather than to a comment,
        // so it cannot drift from the code it claims to describe - which is what
        // makes it a usable reference for the frontend comparison below.
        let columns = cols(&["c"]);
        for op in OPERAND_REQUIRED_OPS {
            let r = build_where_clause(&columns, &[filter("c", &format!("{op}5"))], "").unwrap();
            assert_eq!(
                r.clause,
                format!(" WHERE \"c\" {op} ?"),
                "'{op}' must be recognized as an operator prefix, not folded into a LIKE"
            );
            assert_eq!(r.params, vec!["5"]);
        }

        // The counterpart the frontend list must NOT contain: bare "<>" needs no
        // operand and already produces a complete clause.
        let r = build_where_clause(&columns, &[filter("c", "<>")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"c\" IS NOT NULL AND \"c\" != ''");
    }

    #[test]
    fn operand_required_ops_match_frontend() {
        // Same lockstep mechanism as `tint_presets_match_frontend_toolbar_utils`
        // in config.rs: read the TS as data, never edit it from here. The
        // frontend uses this list to suppress a query while the user is still
        // typing an operator expression; an op present in only one language
        // means either a filter that never fires or a spurious "incomplete"
        // block, neither of which errors anywhere.
        let frontend = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/components/filterOperators.ts"),
        )
        .unwrap();
        let line = frontend
            .lines()
            .find(|l| l.contains("OPERAND_REQUIRED_OPS ="))
            .expect("filterOperators.ts must declare OPERAND_REQUIRED_OPS");
        let array = line
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(inner, _)| inner)
            .expect("OPERAND_REQUIRED_OPS must be a single-line array literal");
        let mut frontend_ops: Vec<&str> = array
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .collect();
        // A parse that finds nothing reads exactly like a list that matches, so
        // fail on an empty population rather than reporting agreement.
        assert!(
            !frontend_ops.is_empty(),
            "parsed no operators out of OPERAND_REQUIRED_OPS - the declaration's shape changed"
        );

        let mut expected: Vec<&str> = OPERAND_REQUIRED_OPS.to_vec();
        frontend_ops.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            expected, frontend_ops,
            "OPERAND_REQUIRED_OPS in filterOperators.ts must match the operand-requiring \
             prefixes build_where_clause parses (note: bare \"<>\" is complete on its own \
             and is correctly absent from the frontend list)"
        );
    }

    #[test]
    fn global_filter_matches_text_stored_in_a_blob_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t(v BLOB); INSERT INTO t VALUES ('invoice'),(CAST('invoice' AS BLOB))",
        )
        .unwrap();
        let r = build_where_clause(&typed_cols(&[("v", "BLOB")]), &[], "invoice").unwrap();
        let mut stmt = conn
            .prepare(&format!("SELECT typeof(v) FROM t{}", r.clause))
            .unwrap();
        let hits: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(&r.params), |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(hits, ["text"]);
    }

    #[test]
    fn untyped_integer_filters_preserve_adjacent_large_values() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t(v)").unwrap();
        let cols = typed_cols(&[("v", "")]);
        for (lower, upper) in [
            (1_i64 << 53, (1_i64 << 53) + 1),
            (i64::MAX - 1, i64::MAX),
            (i64::MIN, i64::MIN + 1),
        ] {
            conn.execute("DELETE FROM t", []).unwrap();
            conn.execute(
                "INSERT INTO t(rowid,v) VALUES (1,?1),(2,?2),(3,?3)",
                rusqlite::params![lower, upper, upper.to_string()],
            )
            .unwrap();
            for (op, expected) in [
                ("=", vec![2, 3]),
                (">=", vec![2, 3]),
                (">", vec![]),
                ("<", vec![1]),
                ("<=", vec![1, 2, 3]),
            ] {
                let value = format!("{op}{upper}");
                let filter = build_where_clause(&cols, &[filter("v", &value)], "").unwrap();
                let sql = format!("SELECT rowid FROM t{} ORDER BY rowid", filter.clause);
                let actual: Vec<i64> = conn
                    .prepare(&sql)
                    .unwrap()
                    .query_map(rusqlite::params_from_iter(&filter.params), |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert_eq!(actual, expected, "{value}");
            }
        }
    }

    #[test]
    fn regex_on_an_unknown_column_is_an_error() {
        // It used to be dropped in silence, leaving an unfiltered grid under a
        // filter chip claiming a filter.
        let columns = cols(&["name"]);
        let err = build_where_clause(&columns, &[regex_filter("ghost", "^x")], "").unwrap_err();
        assert_eq!(err, "no such column: ghost");
    }

    #[test]
    fn a_plain_filter_on_an_unknown_column_is_an_error() {
        // SQLite does NOT refuse this one, although two comments here said it
        // did: with the default double-quoted-string setting the unknown
        // `"ghost"` is read as the string 'ghost'. The control below proves
        // that on a real connection, so this test pins the builder rather than
        // an assumption about the engine.
        let columns = cols(&["name"]);
        for value in ["x", "=x", ">5", "<>"] {
            let err = build_where_clause(&columns, &[filter("ghost", value)], "").unwrap_err();
            assert_eq!(err, "no such column: ghost", "filter {value:?}");
        }

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (name TEXT); INSERT INTO t VALUES ('a');")
            .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM t WHERE \"ghost\" LIKE '%gh%'",
                [],
                |r| r.get(0),
            )
            .expect("control: SQLite accepts an unknown double-quoted name as a string");
        assert_eq!(n, 1);
    }

    /// Rows of `source` that `filter` keeps, run through the real builder on a
    /// real SQLite connection.
    fn matching_rows(
        conn: &rusqlite::Connection,
        source: &str,
        columns: &[ColumnInfo],
        value: &str,
    ) -> i64 {
        let r = build_where_clause(columns, &[filter("v", value)], "").unwrap();
        let sql = format!("SELECT count(*) FROM {source}{}", r.clause);
        let params: Vec<&dyn rusqlite::types::ToSql> = r
            .params
            .iter()
            .map(|p| p as &dyn rusqlite::types::ToSql)
            .collect();
        conn.query_row(&sql, params.as_slice(), |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn range_filter_on_an_untyped_column_compares_numbers() {
        // Measured on SQLite 3.53.1 before this fix: on both sources below `<5`
        // returned all 10 rows and `>5` none, because the bound text operand
        // sorts above every number. The typed table is the control: SQLite
        // converts the operand there, so the answers must already agree.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (v);
             CREATE TABLE typed (v INTEGER);
             CREATE TABLE base (a INTEGER);
             CREATE VIEW computed AS SELECT a + 1 AS v FROM base;
             WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 10)
               INSERT INTO t SELECT x FROM n;
             INSERT INTO typed SELECT v FROM t;
             INSERT INTO base SELECT v - 1 FROM t;",
        )
        .unwrap();
        let untyped = typed_cols(&[("v", "")]);
        let integer = typed_cols(&[("v", "INTEGER")]);

        for (source, columns) in [("t", &untyped), ("computed", &untyped), ("typed", &integer)] {
            for (value, expected) in [("<5", 4), ("<=5", 5), (">5", 5), (">=5", 6)] {
                assert_eq!(
                    matching_rows(&conn, source, columns, value),
                    expected,
                    "{value} on {source}"
                );
            }
        }
    }

    #[test]
    fn range_filter_on_an_untyped_column_still_compares_text_cells_as_text() {
        // The text half of the clause: a text cell in the same untyped column
        // is compared as text, as before, and no number satisfies a text-only
        // operand.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (v); INSERT INTO t VALUES (3), (7), ('b'), ('d'), (NULL);",
        )
        .unwrap();
        let untyped = typed_cols(&[("v", "")]);
        // Numbers: 3 and 7 against 5 numerically; 'b' and 'd' against '5' as
        // text, where both letters sort above the digit.
        assert_eq!(matching_rows(&conn, "t", &untyped, ">5"), 3);
        assert_eq!(matching_rows(&conn, "t", &untyped, "<5"), 1);
        // A text operand: only the text cells can match, as before.
        assert_eq!(matching_rows(&conn, "t", &untyped, ">c"), 1);
    }

    #[test]
    fn range_filter_on_a_typed_column_stays_a_single_comparison() {
        // The index on a typed column must stay usable, so it gets no extra
        // arms; and a non-numeric operand on an untyped column has nothing
        // numeric to compare against either.
        let typed = typed_cols(&[("v", "INTEGER")]);
        let r = build_where_clause(&typed, &[filter("v", ">=5")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"v\" >= ?");

        let untyped = typed_cols(&[("v", "")]);
        let r = build_where_clause(&untyped, &[filter("v", ">=abc")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"v\" >= ?");
        assert_eq!(r.params, vec!["abc"]);
    }

    #[test]
    fn exact_match_on_a_column_with_no_affinity_also_compares_numerically() {
        // A bound text "3" can't match the REAL 3.0 in an untyped column.
        // The INTEGER literal 3 matches that REAL through numeric comparison.
        let columns = typed_cols(&[("v", "")]);
        let r = build_where_clause(&columns, &[filter("v", "=3")], "").unwrap();
        assert_eq!(r.clause, " WHERE (\"v\" = ? OR \"v\" = 3)");
        assert_eq!(r.params, vec!["3"]);
    }

    #[test]
    fn exact_match_on_a_typed_column_stays_a_single_comparison() {
        // The control: SQLite applies the column's affinity to the bound
        // operand here, so plain `=` already matches - and a second OR arm
        // would cost the index for nothing. Non-numeric operands get no arm
        // either, even with no affinity, or a search for text would start
        // matching numeric zeros.
        let typed = typed_cols(&[("v", "REAL")]);
        let r = build_where_clause(&typed, &[filter("v", "=3")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"v\" = ?");

        let untyped = typed_cols(&[("v", "")]);
        let r = build_where_clause(&untyped, &[filter("v", "=abc")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"v\" = ?");
        assert_eq!(r.params, vec!["abc"]);
    }

    #[test]
    fn range_filter_combines_comparisons_with_and() {
        let columns = cols(&["price"]);
        let filters = vec![filter("price", ">10;<100")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE (\"price\" > ? AND \"price\" < ?)");
        assert_eq!(r.params, vec!["10", "100"]);
    }
}
