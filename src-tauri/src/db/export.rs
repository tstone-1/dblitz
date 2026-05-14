use super::StrErr;

/// Classify a SQLite declared column type as numeric-affinity or not, using
/// the SQLite type-affinity rules.
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

    for (ci, h) in headers.iter().enumerate() {
        ws.write_string(0, ci as u16, h).str_err()?;
    }

    const F64_EXACT_INT: i64 = 1i64 << 53;
    for (ri, row) in rows.iter().enumerate() {
        for (ci, val) in row.iter().enumerate() {
            if numeric.get(ci).copied().unwrap_or(true) {
                if let Ok(n) = val.parse::<i64>() {
                    if n.abs() <= F64_EXACT_INT {
                        ws.write_number((ri + 1) as u32, ci as u16, n as f64)
                            .str_err()?;
                    } else {
                        ws.write_string((ri + 1) as u32, ci as u16, val).str_err()?;
                    }
                } else if let Ok(n) = val.parse::<f64>() {
                    ws.write_number((ri + 1) as u32, ci as u16, n).str_err()?;
                } else {
                    ws.write_string((ri + 1) as u32, ci as u16, val).str_err()?;
                }
            } else {
                ws.write_string((ri + 1) as u32, ci as u16, val).str_err()?;
            }
        }
    }

    let last_row = rows.len() as u32;
    let last_col = if headers.is_empty() {
        0
    } else {
        (headers.len() - 1) as u16
    };
    let columns: Vec<TableColumn> = headers
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
