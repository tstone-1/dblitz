use super::util::StrErr;

/// Classify a SQLite declared column type as numeric-affinity or not, using
/// the SQLite type-affinity rules (https://www.sqlite.org/datatype3.html section 3.1).
fn is_numeric_affinity(declared: &str) -> bool {
    let t = declared.to_ascii_uppercase();
    if t.contains("INT") {
        return true;
    }
    if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        return false;
    }
    if t.contains("BLOB") || t.is_empty() {
        return false;
    }
    true
}

/// Largest absolute integer that an f64 can represent exactly. Integers beyond
/// this lose precision when stored as a number, so we emit them as text.
const F64_EXACT_INT: i64 = 1i64 << 53;

/// How a cell value should be written to the worksheet.
#[derive(Debug, PartialEq)]
enum CellValue {
    Number(f64),
    Text,
}

/// Decide whether a cell is written as a number or as text. Numeric-affinity
/// columns try i64 first (emitting values beyond ±2^53 as text to preserve
/// precision), then f64; everything else stays text.
fn classify_cell(numeric: bool, val: &str) -> CellValue {
    if !numeric {
        return CellValue::Text;
    }
    if let Ok(n) = val.parse::<i64>() {
        if n.abs() <= F64_EXACT_INT {
            CellValue::Number(n as f64)
        } else {
            CellValue::Text
        }
    } else if let Ok(n) = val.parse::<f64>() {
        CellValue::Number(n)
    } else {
        CellValue::Text
    }
}

fn dedupe_headers(headers: &[String]) -> Vec<String> {
    use std::collections::HashMap;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    headers
        .iter()
        .map(|header| {
            let count = counts.entry(header.as_str()).or_insert(0);
            *count += 1;
            if *count == 1 {
                header.clone()
            } else {
                format!("{header}_{count}")
            }
        })
        .collect()
}

pub fn export_to_xlsx(
    headers: &[String],
    rows: &[Vec<String>],
    column_types: &[String],
) -> Result<String, String> {
    use rust_xlsxwriter::*;

    if headers.is_empty() {
        return Err("No data to export".to_string());
    }

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();

    let numeric: Vec<bool> = (0..headers.len())
        .map(|i| column_types.get(i).is_none_or(|t| is_numeric_affinity(t)))
        .collect();
    if rows.iter().any(|row| row.len() > headers.len()) {
        return Err("Export row has more cells than headers".to_string());
    }

    let table_headers = dedupe_headers(headers);

    for (ci, h) in headers.iter().enumerate() {
        ws.write_string(0, ci as u16, h).str_err()?;
    }
    for (ri, row) in rows.iter().enumerate() {
        for (ci, val) in row.iter().enumerate() {
            match classify_cell(numeric[ci], val) {
                CellValue::Number(n) => {
                    ws.write_number((ri + 1) as u32, ci as u16, n).str_err()?;
                }
                CellValue::Text => {
                    ws.write_string((ri + 1) as u32, ci as u16, val).str_err()?;
                }
            }
        }
    }

    let last_row = rows.len() as u32;
    let last_col = if headers.is_empty() {
        0
    } else {
        (headers.len() - 1) as u16
    };
    let columns: Vec<TableColumn> = table_headers
        .iter()
        .map(|h| TableColumn::new().set_header(h))
        .collect();
    let table = Table::new()
        .set_style(TableStyle::Medium2)
        .set_columns(&columns);
    ws.add_table(0, 0, last_row, last_col, &table).str_err()?;
    ws.autofit();

    let temp_dir = std::env::temp_dir();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = temp_dir.join(format!("dblitz_export_{}.xlsx", ts));
    let path_str = path.to_string_lossy().to_string();
    wb.save(&path).str_err()?;

    Ok(path_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_affinity_classifies_declared_types() {
        for t in ["INTEGER", "BIGINT", "REAL", "NUMERIC", "DOUBLE", "int"] {
            assert!(is_numeric_affinity(t), "{t} should be numeric");
        }
        for t in ["VARCHAR(20)", "TEXT", "CLOB", "CHARACTER", "BLOB", ""] {
            assert!(!is_numeric_affinity(t), "{t} should not be numeric");
        }
    }

    #[test]
    fn non_numeric_columns_always_stay_text() {
        assert_eq!(classify_cell(false, "123"), CellValue::Text);
        assert_eq!(classify_cell(false, "1.5"), CellValue::Text);
    }

    #[test]
    fn numeric_columns_emit_integers_and_floats_as_numbers() {
        assert_eq!(classify_cell(true, "42"), CellValue::Number(42.0));
        assert_eq!(classify_cell(true, "-7"), CellValue::Number(-7.0));
        assert_eq!(classify_cell(true, "1.5"), CellValue::Number(1.5));
    }

    #[test]
    fn numeric_columns_keep_non_numeric_text_as_text() {
        assert_eq!(classify_cell(true, ""), CellValue::Text);
        assert_eq!(classify_cell(true, "N/A"), CellValue::Text);
    }

    #[test]
    fn bigints_beyond_f64_exact_range_stay_text() {
        // 2^53 is exactly representable; 2^53 + 1 is not.
        assert_eq!(
            classify_cell(true, "9007199254740992"),
            CellValue::Number(9007199254740992.0)
        );
        assert_eq!(classify_cell(true, "9007199254740993"), CellValue::Text);
        assert_eq!(classify_cell(true, "-9007199254740993"), CellValue::Text);
    }

    #[test]
    fn export_rejects_empty_data() {
        let err = export_to_xlsx(&[], &[], &[]).unwrap_err();
        assert_eq!(err, "No data to export");
    }

    #[test]
    fn export_rejects_row_wider_than_headers() {
        let headers = vec!["id".to_string()];
        let rows = vec![vec!["a".to_string(), "b".to_string()]];
        let err = export_to_xlsx(&headers, &rows, &[]).unwrap_err();
        assert!(err.contains("more cells than headers"), "got: {err}");
    }

    #[test]
    fn export_allows_duplicate_headers() {
        let headers = vec!["a".to_string(), "a".to_string()];
        let rows = vec![vec!["1".to_string(), "2".to_string()]];
        let path = export_to_xlsx(
            &headers,
            &rows,
            &["INTEGER".to_string(), "INTEGER".to_string()],
        )
        .unwrap();

        assert!(std::path::Path::new(&path).exists());
        let _ = std::fs::remove_file(path);
    }
}
