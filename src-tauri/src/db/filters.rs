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

fn contains_pattern(value: &str) -> String {
    format!("%{}%", escape_like(value))
}

/// Whether a declared type names a BLOB.
///
/// SQLite's affinity rules say a column whose declared type contains "BLOB" -
/// *or* which declares no type at all - has BLOB affinity, i.e. no affinity.
/// This asks only about the first half, deliberately: the global filter uses it
/// to decide which columns to leave out of its `LIKE` sweep, and an
/// undeclared-type column (every expression column of a view, every column of
/// `CREATE TABLE t(a, b)`) usually holds exactly the text a user is searching
/// for. Wrongly including a BLOB column costs a slow comparison that could
/// never match; wrongly excluding a text column silently loses rows, which is
/// the worse failure, so the two are not treated alike.
fn declares_blob(declared_type: &str) -> bool {
    declared_type.to_ascii_uppercase().contains("BLOB")
}

/// Whether a column has no numeric affinity, so SQLite will not convert a bound
/// text operand to a number when comparing against it. See the numeric-equality
/// arm in [`build_where_clause`].
fn has_no_affinity(declared_type: &str) -> bool {
    declared_type.trim().is_empty() || declares_blob(declared_type)
}

/// Build the `WHERE` clause for one view.
///
/// `columns` is the table's visible column list with its declared types, in
/// `SELECT *` order - the types decide two things below that a bare name list
/// cannot: which columns the global filter may sweep, and whether `=` needs a
/// numeric arm to reach a numeric cell.
pub(super) fn build_where_clause(
    columns: &[ColumnInfo],
    filters: &[ColumnFilter],
    global_filter: &str,
) -> Result<WhereResult, String> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut regex_filters: Vec<(usize, Regex)> = Vec::new();

    if !global_filter.is_empty() {
        // BLOB columns are left out of the sweep. `LIKE` against a BLOB never
        // matches anything a user typed - SQLite compares it byte-wise against
        // the pattern, and the grid does not even show the bytes, it shows
        // `[BLOB n bytes]` - so every one of them was a full column scan
        // guaranteed to contribute nothing. Measured on the 5M-row / 870 MB
        // fixture, one BLOB column among twelve: 1.48 s -> 1.30 s per global
        // filter, about 12%. (The gap between a 12-column sweep and a
        // single-column one is much larger - 1.48 s against 0.23 s - but the
        // other eleven columns can match, so only the BLOB is free to drop.)
        // A column with no declared type stays in (see `declares_blob`).
        let searchable: Vec<&ColumnInfo> = columns
            .iter()
            .filter(|c| !declares_blob(&c.col_type))
            .collect();
        // Every column excluded would leave `()` - an empty group is a syntax
        // error, and silently dropping the filter would show unfiltered rows as
        // though they matched. A filter that can match nothing must match
        // nothing.
        if searchable.is_empty() {
            where_parts.push("0".to_string());
        } else {
            let or_conditions: Vec<String> = searchable
                .iter()
                .map(|c| format!("{} LIKE ? ESCAPE '\\'", quote_ident(&c.name)))
                .collect();
            where_parts.push(format!("({})", or_conditions.join(" OR ")));
            let pattern = contains_pattern(global_filter);
            for _ in &searchable {
                params.push(pattern.clone());
            }
        }
    }

    for f in filters {
        if f.value.is_empty() {
            continue;
        }
        let column = columns.iter().position(|c| c.name == f.column);
        if f.is_regex {
            // A regex naming a column this table does not have used to be
            // dropped in silence, which showed the user an unfiltered grid
            // under a filter chip that said otherwise. The LIKE path below
            // interpolates the name and lets SQLite refuse it; a regex never
            // reaches SQL, so the refusal has to happen here. Same wording
            // SQLite uses, because to the user it is the same mistake.
            let Some(idx) = column else {
                return Err(format!("no such column: {}", f.column));
            };
            match Regex::new(&f.value) {
                Ok(re) => regex_filters.push((idx, re)),
                Err(e) => return Err(format!("Invalid regex '{}': {}", f.column, e)),
            }
        } else {
            let no_affinity = column
                .map(|idx| has_no_affinity(&columns[idx].col_type))
                .unwrap_or(false);
            let col_escaped = quote_ident(&f.column);

            // Split on semicolon for multi-criteria: exclusions=AND, inclusions=OR
            let criteria: Vec<&str> = f
                .value
                .split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            if criteria.is_empty() {
                continue;
            }

            let mut and_parts: Vec<String> = Vec::new();
            let mut and_params: Vec<String> = Vec::new();
            let mut or_parts: Vec<String> = Vec::new();
            let mut or_params: Vec<String> = Vec::new();

            // Operator prefixes recognized below: "<>", ">=", "<=", ">", "<",
            // "=". The frontend mirrors the operand-requiring subset in
            // src/lib/components/filterOperators.ts (OPERAND_REQUIRED_OPS) to
            // gate half-typed filters — keep the two in sync when adding ops.
            // `operand_required_ops_match_frontend` below parses that TS file
            // and pins the two lists together, so an op added on one side only
            // fails a test rather than shipping.
            for val in &criteria {
                if let Some(rest) = val.strip_prefix("<>") {
                    if rest.is_empty() {
                        and_parts.push(format!(
                            "{} IS NOT NULL AND {} != ''",
                            col_escaped, col_escaped
                        ));
                    } else {
                        and_parts.push(format!("{} NOT LIKE ? ESCAPE '\\'", col_escaped));
                        and_params.push(contains_pattern(rest));
                    }
                } else if let Some(rest) = val.strip_prefix(">=") {
                    and_parts.push(format!("{} >= ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix("<=") {
                    and_parts.push(format!("{} <= ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('>') {
                    and_parts.push(format!("{} > ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('<') {
                    and_parts.push(format!("{} < ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('=') {
                    // On a column with no affinity, SQLite compares a bound
                    // text operand against a numeric cell across storage
                    // classes, where a number is always less than any text - so
                    // `=3` against a REAL 3.0 matched nothing at all, and the
                    // user had no spelling that would. Add a second arm holding
                    // the value as a number whenever it parses as one. It is
                    // interpolated rather than bound because the parameter list
                    // is text-only; `render_real` emits digits, a point, `e`
                    // and a sign, so nothing else can reach the SQL. Only added
                    // where plain `=` provably cannot match: on a column with
                    // numeric affinity SQLite converts the operand itself, and
                    // an extra OR arm there would cost the index.
                    match rest.parse::<f64>() {
                        Ok(number) if no_affinity && number.is_finite() => {
                            or_parts.push(format!(
                                "({col} = ? OR {col} = {literal})",
                                col = col_escaped,
                                literal = render_real(number)
                            ));
                        }
                        _ => or_parts.push(format!("{} = ?", col_escaped)),
                    }
                    or_params.push(rest.to_string());
                } else {
                    or_parts.push(format!("{} LIKE ? ESCAPE '\\'", col_escaped));
                    or_params.push(contains_pattern(val));
                }
            }

            let mut col_parts: Vec<String> = Vec::new();
            if or_parts.len() == 1 {
                col_parts.push(or_parts.remove(0));
            } else if or_parts.len() > 1 {
                col_parts.push(format!("({})", or_parts.join(" OR ")));
            }
            col_parts.extend(and_parts);
            params.extend(or_params);
            params.extend(and_params);

            if col_parts.len() == 1 {
                where_parts.push(col_parts.remove(0));
            } else if col_parts.len() > 1 {
                where_parts.push(format!("({})", col_parts.join(" AND ")));
            }
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
            " WHERE (\"name\" LIKE ? ESCAPE '\\' OR \"age\" LIKE ? ESCAPE '\\')"
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
            " WHERE (\"name\" LIKE ? ESCAPE '\\' OR \"city\" LIKE ? ESCAPE '\\') AND \"name\" LIKE ? ESCAPE '\\'"
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
    fn global_filter_skips_blob_columns() {
        // A BLOB column can never match what a user typed - the grid shows it
        // as `[BLOB n bytes]`, and SQLite compares the pattern against the raw
        // bytes - so sweeping it was a guaranteed-empty full column scan.
        // Measured 1.48 s -> 1.30 s on a 5M-row table with one BLOB column of
        // twelve.
        let columns = typed_cols(&[("name", "TEXT"), ("payload", "BLOB"), ("n", "INTEGER")]);
        let r = build_where_clause(&columns, &[], "x").unwrap();
        assert_eq!(
            r.clause, " WHERE (\"name\" LIKE ? ESCAPE '\\' OR \"n\" LIKE ? ESCAPE '\\')",
            "the BLOB column must not appear in the sweep, and the numeric one must"
        );
        assert_eq!(
            r.params,
            vec!["%x%", "%x%"],
            "one bound pattern per swept column, not per declared column"
        );
    }

    #[test]
    fn global_filter_keeps_untyped_columns() {
        // The control for the exclusion above: a column with no declared type
        // has BLOB affinity by SQLite's rules but usually holds text (every
        // expression column of a view is like this). Dropping those would lose
        // rows silently, which is the failure the exclusion must not cause.
        let columns = typed_cols(&[("a", ""), ("b", "blob")]);
        let r = build_where_clause(&columns, &[], "x").unwrap();
        assert_eq!(r.clause, " WHERE (\"a\" LIKE ? ESCAPE '\\')");
        assert_eq!(r.params, vec!["%x%"]);
    }

    #[test]
    fn global_filter_over_only_blob_columns_matches_nothing() {
        // Excluding every column would otherwise emit `WHERE ()` (a syntax
        // error) or no clause at all (every row shown as a match under a filter
        // chip that says otherwise).
        let columns = typed_cols(&[("a", "BLOB"), ("b", "BLOB")]);
        let r = build_where_clause(&columns, &[], "x").unwrap();
        assert_eq!(r.clause, " WHERE 0");
        assert!(r.params.is_empty());
    }

    #[test]
    fn regex_on_an_unknown_column_is_an_error() {
        // It used to be dropped in silence, leaving an unfiltered grid under a
        // filter chip claiming a filter. The LIKE path errors on the same
        // mistake because SQLite refuses the name.
        let columns = cols(&["name"]);
        let err = build_where_clause(&columns, &[regex_filter("ghost", "^x")], "").unwrap_err();
        assert_eq!(err, "no such column: ghost");
    }

    #[test]
    fn exact_match_on_a_column_with_no_affinity_also_compares_numerically() {
        // `=3` against a REAL 3.0 in a column with no declared type compares
        // text to number across storage classes and can never match, so the
        // clause carries a numeric arm as well. `render_real` writes the
        // literal, which is why it reads `3.0` rather than `3`.
        let columns = typed_cols(&[("v", "")]);
        let r = build_where_clause(&columns, &[filter("v", "=3")], "").unwrap();
        assert_eq!(r.clause, " WHERE (\"v\" = ? OR \"v\" = 3.0)");
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
