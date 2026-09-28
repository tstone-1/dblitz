//! The `WHERE` clause for a Parquet view. Same grammar as the SQLite builder -
//! both parse with `db::filters::parse_criteria` - with three deliberate
//! differences, each forced by DuckDB rather than chosen:
//!
//! - **Text matching is `ILIKE` against the rendered text** ([`text_expr`]).
//!   SQLite's `LIKE` is case-insensitive and DuckDB's is not, and matching the
//!   rendered text is what makes a filter agree with what the grid shows.
//! - **A comparison operand is cast to the column's type**, and one that cannot
//!   be cast is an error rather than an empty grid - the same rule as a
//!   filter naming an unknown column. SQLite would instead compare across
//!   storage classes, which DuckDB's typed comparison does not do.
//! - **A regex runs inside DuckDB (RE2)**, not in Rust. NULL still matches as
//!   the empty string and a BLOB still never matches.

use crate::db::{contains_pattern, parse_criteria, ColumnFilter, ColumnInfo, Criterion};

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
        let cast = |op: &str| match cast_target(&col.col_type) {
            Some(t) => format!("{q} {op} CAST(? AS {t})"),
            None => format!("{text} {op} ?"),
        };
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
                Criterion::Ge(v) => (cast(">="), Some(v.to_string())),
                Criterion::Le(v) => (cast("<="), Some(v.to_string())),
                Criterion::Gt(v) => (cast(">"), Some(v.to_string())),
                Criterion::Lt(v) => (cast("<"), Some(v.to_string())),
                Criterion::Eq(v) => (cast("="), Some(v.to_string())),
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
    fn comparisons_cast_the_operand_to_the_column_type() {
        let w = build_where(&[col("n", "DECIMAL(18,4)")], &[f("n", ">=2;<5")], "").unwrap();
        assert_eq!(
            w.clause,
            " WHERE (\"n\" >= CAST(? AS DECIMAL(18,4)) AND \"n\" < CAST(? AS DECIMAL(18,4)))"
        );
        assert_eq!(w.params, ["2", "5"]);
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
