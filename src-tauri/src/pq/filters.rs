//! The `WHERE` clause for a Parquet view. Same grammar as the SQLite builder -
//! both parse with `db::filters::parse_criteria` - with three deliberate
//! differences, each forced by DuckDB rather than chosen:
//!
//! - **Text matching is `ILIKE` against the rendered text** ([`text_expr`]).
//!   SQLite's `LIKE` is case-insensitive and DuckDB's is not, and matching the
//!   rendered text is what makes a filter agree with what the grid shows.
//! - **Comparisons use the column type**, preserving fractional bounds for
//!   integer columns. An operand that cannot be cast is an error rather than an empty grid - the same rule as a
//!   filter naming an unknown column. SQLite would instead compare across
//!   storage classes, which DuckDB's typed comparison does not do.
//! - **A regex runs inside DuckDB (RE2)**, not in Rust. NULL still matches as
//!   the empty string and a BLOB still never matches.

use crate::db::{
    contains_pattern, discrete_operator, parse_criteria, push_column_criteria, scaled_parts,
    ColumnFilter, ColumnInfo, Criterion,
};

use super::{quote_ident, sql_literal, text_expr};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Where {
    /// Empty, or ` WHERE ...`.
    pub(crate) clause: String,
    pub(crate) params: Vec<String>,
}

/// The column's type, when it is safe to put into SQL. The browse instance is
/// unlocked, and a file controls parts of a type string: an ENUM spells out
/// quoted values and a STRUCT its field names. So only a type made of letters,
/// digits, spaces, parentheses, commas and underscores is interpolated - every
/// scalar type, `DECIMAL(18,4)` and `TIMESTAMP WITH TIME ZONE` included.
/// Anything else is compared as rendered text.
fn cast_target(col_type: &str) -> Option<&str> {
    let safe = !col_type.is_empty()
        && col_type
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " (),_".contains(c));
    safe.then_some(col_type)
}

/// Integer columns are discrete. A non-integral threshold lies strictly
/// between two adjacent integers, even beyond the exact range of a double.
fn comparison(
    q: &str,
    text: &str,
    col_type: &str,
    op: &str,
    input: &str,
) -> (String, Option<String>) {
    let Some(t) = cast_target(col_type) else {
        return (format!("{text} {op} ?"), Some(input.into()));
    };
    if let Some((precision, scale)) = t
        .strip_prefix("DECIMAL(")
        .and_then(|s| s.strip_suffix(')'))
        .and_then(|s| s.split_once(','))
        .and_then(|(p, s)| {
            Some((
                p.trim().parse::<usize>().ok()?,
                s.trim().parse::<u8>().ok()?,
            ))
        })
        .filter(|(p, s)| *p <= 38 && usize::from(*s) <= *p)
    {
        if let Some((whole, fractional, negative)) = scaled_parts(input, scale) {
            let Some(op) = discrete_operator(op, fractional, negative) else {
                return ("false".into(), None);
            };
            let digits = whole.trim_start_matches('-');
            if digits.len() > precision {
                let matches = if negative {
                    op.starts_with('>')
                } else {
                    op.starts_with('<')
                };
                return (
                    if matches {
                        format!("{q} IS NOT NULL")
                    } else {
                        "false".into()
                    },
                    None,
                );
            }
            // DuckDB can reject an in-range exponent spelling before scaling.
            let scale = usize::from(scale);
            let padded = format!("{digits:0>width$}", width = scale + 1);
            let (left, right) = padded.split_at(padded.len() - scale);
            let sign = if negative { "-" } else { "" };
            let bound = if right.is_empty() {
                format!("{sign}{left}")
            } else {
                format!("{sign}{left}.{right}")
            };
            return (format!("{q} {op} CAST(? AS {t})"), Some(bound));
        }
    }
    let integral = matches!(
        t,
        "TINYINT"
            | "SMALLINT"
            | "INTEGER"
            | "BIGINT"
            | "HUGEINT"
            | "UTINYINT"
            | "USMALLINT"
            | "UINTEGER"
            | "UBIGINT"
            | "UHUGEINT"
    );
    if integral && input.contains(['.', 'e', 'E']) {
        if let Some((whole, fractional, negative)) = scaled_parts(input, 0) {
            let Some(op) = discrete_operator(op, fractional, negative) else {
                return ("false".into(), None);
            };
            // Keep common bounds on a native integer comparison. Mixing UHUGEINT
            // with a signed bound can otherwise promote the pair to DOUBLE.
            let sql = if t != "UHUGEINT" && whole.parse::<i64>().is_ok() {
                format!("{q} {op} CAST(? AS BIGINT)")
            } else {
                format!("CAST({q} AS BIGNUM) {op} CAST(? AS BIGNUM)")
            };
            return (sql, Some(whole));
        }
    }
    (format!("{q} {op} CAST(? AS {t})"), Some(input.into()))
}

pub(crate) fn build_where(
    columns: &[ColumnInfo],
    filters: &[ColumnFilter],
    global_filter: &str,
) -> Result<Where, String> {
    let mut parts: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let ilike = |expr: &str| format!("{expr} ILIKE ? ESCAPE '\\'");

    if !global_filter.is_empty() {
        let searchable: Vec<&ColumnInfo> = columns
            .iter()
            .filter(|c| !super::is_blob(&c.col_type))
            .collect();
        if searchable.is_empty() {
            // A filter that can match nothing must match nothing.
            parts.push("false".to_string());
        } else {
            let ors: Vec<String> = searchable.iter().map(|c| ilike(&text_expr(c))).collect();
            parts.push(format!("({})", ors.join(" OR ")));
            params.extend(searchable.iter().map(|_| contains_pattern(global_filter)));
        }
    }

    for f in filters {
        if f.value.is_empty() {
            continue;
        }
        let col = columns
            .iter()
            .find(|c| c.name == f.column)
            .ok_or_else(|| format!("no such column: {}", f.column))?;
        let text = text_expr(col);
        if f.is_regex {
            if super::is_blob(&col.col_type) {
                parts.push("false".to_string());
            } else {
                // Inlined as an escaped literal rather than bound, so DuckDB
                // compiles the pattern once instead of per row.
                parts.push(format!(
                    "regexp_matches(coalesce({text}, ''), {})",
                    sql_literal(&f.value)
                ));
            }
            continue;
        }

        let q = quote_ident(&col.name);
        let mut ors: Vec<String> = Vec::new();
        let mut or_params: Vec<String> = Vec::new();
        let mut ands: Vec<String> = Vec::new();
        let mut and_params: Vec<String> = Vec::new();
        for c in parse_criteria(&f.value) {
            let (sql, param) = match &c {
                Criterion::NotEmpty => (format!("{q} IS NOT NULL AND {text} != ''"), None),
                Criterion::NotContains(v) => (
                    format!("{text} NOT ILIKE ? ESCAPE '\\'"),
                    Some(contains_pattern(v)),
                ),
                Criterion::Ge(v) => comparison(&q, &text, &col.col_type, ">=", v),
                Criterion::Le(v) => comparison(&q, &text, &col.col_type, "<=", v),
                Criterion::Gt(v) => comparison(&q, &text, &col.col_type, ">", v),
                Criterion::Lt(v) => comparison(&q, &text, &col.col_type, "<", v),
                Criterion::Eq(v) => comparison(&q, &text, &col.col_type, "=", v),
                Criterion::Contains(v) => (ilike(&text), Some(contains_pattern(v))),
            };
            if c.is_and() {
                ands.push(sql);
                and_params.extend(param);
            } else {
                ors.push(sql);
                or_params.extend(param);
            }
        }
        push_column_criteria(ors, or_params, ands, and_params, &mut parts, &mut params);
    }

    Ok(Where {
        clause: if parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", parts.join(" AND "))
        },
        params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, t: &str) -> ColumnInfo {
        ColumnInfo {
            cid: 0,
            name: name.into(),
            col_type: t.into(),
            notnull: false,
            default_value: None,
            pk: false,
        }
    }

    fn f(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.into(),
            value: value.into(),
            is_regex: false,
        }
    }

    #[test]
    fn plain_text_is_a_case_insensitive_contains_on_the_rendered_text() {
        let w = build_where(&[col("name", "VARCHAR")], &[f("name", "Ab_c")], "").unwrap();
        assert_eq!(
            w.clause,
            " WHERE CAST(\"name\" AS VARCHAR) ILIKE ? ESCAPE '\\'"
        );
        assert_eq!(w.params, ["%Ab\\_c%"]);
    }

    #[test]
    fn fractional_integer_thresholds_preserve_the_side_of_zero_and_large_digits() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        for (operand, expected) in [
            (">-1.6", vec!["-1", "0", "9007199254740993"]),
            ("<-0.01", vec!["-2", "-1"]),
            (">9007199254740992.9", vec!["9007199254740993"]),
            ("=9007199254740993.0", vec!["9007199254740993"]),
        ] {
            let filter = build_where(&[col("id", "BIGINT")], &[f("id", operand)], "").unwrap();
            let sql = format!("SELECT id::VARCHAR FROM (VALUES (-2::BIGINT), (-1), (0), (9007199254740993)) t(id) {} ORDER BY id", filter.clause);
            let mut stmt = conn.prepare(&sql).unwrap();
            let rows = stmt
                .query_map(duckdb::params_from_iter(filter.params.iter()), |r| {
                    r.get::<_, String>(0)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(rows, expected, "{operand}");
        }
    }

    #[test]
    fn decimal_comparisons_preserve_thresholds_between_stored_values() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        for (ty, a, b, operand, expected) in [
            ("DECIMAL(8,2)", "1.23", "1.24", "=1.234", vec![]),
            ("DECIMAL(8,2)", "1.23", "1.24", "<1.234", vec![1]),
            ("DECIMAL(8,2)", "1.23", "1.24", "=1.23", vec![1]),
            ("DECIMAL(4,2)", "1.23", "1.24", "=123e-2", vec![1]),
            ("DECIMAL(4,2)", "1.23", "1.24", "<124e-2", vec![1]),
            (
                "DECIMAL(38,18)",
                "12345678901234567890.123456789012345678",
                "0",
                "=12345678901234567890123456789012345678e-18",
                vec![1],
            ),
            ("DECIMAL(8,2)", "-1.24", "-1.23", ">-1.234", vec![2]),
            (
                "DECIMAL(38,2)",
                "900719925474099312345678901234567890.12",
                "900719925474099312345678901234567890.13",
                "<900719925474099312345678901234567890.125",
                vec![1],
            ),
        ] {
            let pred = build_where(&[col("n", ty)], &[f("n", operand)], "").unwrap();
            let source = format!(
                " FROM (VALUES (1,'{a}'::{ty}),(2,'{b}'::{ty})) t(id,n) {}",
                pred.clause
            );
            let ids = conn
                .prepare(&format!("SELECT id{source} ORDER BY id"))
                .unwrap()
                .query_map(duckdb::params_from_iter(&pred.params), |r| {
                    r.get::<_, i32>(0)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let count: i64 = conn
                .query_row(
                    &format!("SELECT count(*){source}"),
                    duckdb::params_from_iter(&pred.params),
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(ids, expected, "{operand}");
            assert_eq!(count, expected.len() as i64);
        }
    }

    #[test]
    fn comparisons_cast_the_operand_to_the_column_type() {
        let w = build_where(&[col("n", "DECIMAL(18,4)")], &[f("n", ">=2;<5")], "").unwrap();
        assert_eq!(
            w.clause,
            " WHERE (\"n\" >= CAST(? AS DECIMAL(18,4)) AND \"n\" < CAST(? AS DECIMAL(18,4)))"
        );
        assert_eq!(w.params, ["2.0000", "5.0000"]);
    }

    #[test]
    fn a_type_string_a_file_can_shape_is_never_interpolated() {
        for t in ["ENUM('a', 'b')", "STRUCT(\"x); DROP\" BIGINT)", "BIGINT[]"] {
            let w = build_where(&[col("c", t)], &[f("c", "=1")], "").unwrap();
            assert_eq!(w.clause, " WHERE CAST(\"c\" AS VARCHAR) = ?", "type {t}");
        }
    }

    #[test]
    fn scalar_types_with_parameters_are_still_cast() {
        for t in [
            "TIMESTAMP WITH TIME ZONE",
            "MAP(VARCHAR, BIGINT)",
            "DECIMAL(9,2)",
        ] {
            let w = build_where(&[col("c", t)], &[f("c", "=1")], "").unwrap();
            assert_eq!(w.clause, format!(" WHERE \"c\" = CAST(? AS {t})"));
        }
    }

    #[test]
    fn inclusions_are_ored_and_exclusions_anded_as_on_sqlite() {
        let w = build_where(&[col("c", "VARCHAR")], &[f("c", "=a;=b;<>x")], "").unwrap();
        assert_eq!(
            w.clause,
            " WHERE ((\"c\" = CAST(? AS VARCHAR) OR \"c\" = CAST(? AS VARCHAR)) AND CAST(\"c\" AS VARCHAR) NOT ILIKE ? ESCAPE '\\')"
        );
        assert_eq!(w.params, ["a", "b", "%x%"]);
    }

    #[test]
    fn not_empty_alone_needs_no_operand() {
        let w = build_where(&[col("c", "BIGINT")], &[f("c", "<>")], "").unwrap();
        assert_eq!(
            w.clause,
            " WHERE \"c\" IS NOT NULL AND CAST(\"c\" AS VARCHAR) != ''"
        );
        assert!(w.params.is_empty());
    }

    #[test]
    fn an_unknown_column_is_an_error_not_a_dropped_filter() {
        let err = build_where(&[col("a", "VARCHAR")], &[f("zz", "x")], "").unwrap_err();
        assert_eq!(err, "no such column: zz");
    }

    #[test]
    fn global_filter_skips_blobs_and_matches_nothing_without_candidates() {
        let cols = [col("a", "VARCHAR"), col("b", "BLOB"), col("c", "BIGINT")];
        let w = build_where(&cols, &[], "x").unwrap();
        assert_eq!(
            w.clause,
            " WHERE (CAST(\"a\" AS VARCHAR) ILIKE ? ESCAPE '\\' OR CAST(\"c\" AS VARCHAR) ILIKE ? ESCAPE '\\')"
        );
        assert_eq!(w.params.len(), 2);
        let w = build_where(&[col("b", "BLOB")], &[], "x").unwrap();
        assert_eq!(w.clause, " WHERE false");
    }

    #[test]
    fn a_regex_is_an_escaped_literal_and_never_matches_a_blob() {
        let mut r = f("a", "it's");
        r.is_regex = true;
        let w = build_where(&[col("a", "VARCHAR")], &[r.clone()], "").unwrap();
        assert_eq!(
            w.clause,
            " WHERE regexp_matches(coalesce(CAST(\"a\" AS VARCHAR), ''), 'it''s')"
        );
        r.column = "b".into();
        let w = build_where(&[col("b", "BLOB")], &[r], "").unwrap();
        assert_eq!(w.clause, " WHERE false");
    }
}
