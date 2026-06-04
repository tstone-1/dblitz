use regex::Regex;

use super::types::ColumnFilter;
use super::util::safe_ident;

#[derive(Debug)]
pub(super) struct WhereResult {
    pub(super) clause: String,
    pub(super) params: Vec<String>,
    pub(super) regex_filters: Vec<(usize, Regex)>,
}

pub(super) fn build_where_clause(
    columns: &[String],
    filters: &[ColumnFilter],
    global_filter: &str,
) -> Result<WhereResult, String> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut regex_filters: Vec<(usize, Regex)> = Vec::new();

    if !global_filter.is_empty() {
        let or_conditions: Vec<String> = columns
            .iter()
            .map(|c| format!("\"{}\" LIKE ?", safe_ident(c)))
            .collect();
        where_parts.push(format!("({})", or_conditions.join(" OR ")));
        let pattern = format!("%{}%", global_filter);
        for _ in columns {
            params.push(pattern.clone());
        }
    }

    for f in filters {
        if f.value.is_empty() {
            continue;
        }
        if f.is_regex {
            if let Some(idx) = columns.iter().position(|c| c == &f.column) {
                match Regex::new(&f.value) {
                    Ok(re) => regex_filters.push((idx, re)),
                    Err(e) => return Err(format!("Invalid regex '{}': {}", f.column, e)),
                }
            }
        } else {
            let col_escaped = safe_ident(&f.column);

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
            // src/lib/components/BrowseData.svelte (OPERAND_REQUIRED_OPS) to
            // gate half-typed filters — keep the two in sync when adding ops.
            for val in &criteria {
                if let Some(rest) = val.strip_prefix("<>") {
                    if rest.is_empty() {
                        and_parts.push(format!(
                            "\"{}\" IS NOT NULL AND \"{}\" != ''",
                            col_escaped, col_escaped
                        ));
                    } else {
                        and_parts.push(format!("\"{}\" NOT LIKE ?", col_escaped));
                        and_params.push(format!("%{}%", rest));
                    }
                } else if let Some(rest) = val.strip_prefix(">=") {
                    and_parts.push(format!("\"{}\" >= ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix("<=") {
                    and_parts.push(format!("\"{}\" <= ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('>') {
                    and_parts.push(format!("\"{}\" > ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('<') {
                    and_parts.push(format!("\"{}\" < ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('=') {
                    or_parts.push(format!("\"{}\" = ?", col_escaped));
                    or_params.push(rest.to_string());
                } else {
                    or_parts.push(format!("\"{}\" LIKE ?", col_escaped));
                    or_params.push(format!("%{}%", val));
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

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
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
        assert_eq!(r.clause, " WHERE \"name\" LIKE ?");
        assert_eq!(r.params, vec!["%foo%"]);
        assert!(r.regex_filters.is_empty());
    }

    #[test]
    fn global_filter_or_across_columns() {
        let columns = cols(&["name", "age"]);
        let r = build_where_clause(&columns, &[], "test").unwrap();
        assert_eq!(r.clause, " WHERE (\"name\" LIKE ? OR \"age\" LIKE ?)");
        assert_eq!(r.params, vec!["%test%", "%test%"]);
    }

    #[test]
    fn semicolon_multi_criteria() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "foo;bar")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE (\"name\" LIKE ? OR \"name\" LIKE ?)");
        assert_eq!(r.params, vec!["%foo%", "%bar%"]);
    }

    #[test]
    fn exclusion_not_like() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "<>bad")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" NOT LIKE ?");
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
        assert_eq!(r.clause, " WHERE \"col\"\"name\" LIKE ?");
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
        assert_eq!(r.clause, " WHERE (\"name\" LIKE ? AND \"name\" NOT LIKE ?)");
        assert_eq!(r.params, vec!["%good%", "%bad%"]);
    }
}
