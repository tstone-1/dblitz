use super::util::StrErr;
use std::path::Path;

/// Prefix every "Open in Excel" workbook is written with (see [`export_to_xlsx`]).
/// The cleanup sweep only ever touches files carrying exactly this prefix and
/// the `.xlsx` extension, so nothing else in the export directory is at risk.
const EXPORT_PREFIX: &str = "dblitz_export_";

/// Age past which a previously-exported workbook is eligible for the cleanup
/// sweep. Roughly a week - long enough that a file a user re-opens the next
/// working day is never yanked out from under them.
const EXPORT_RETENTION: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// Best-effort removal of stale `dblitz_export_*.xlsx` files in `dir`. Every
/// "Open in Excel" drops a fresh timestamped workbook and nothing ever
/// reclaimed them, so the export directory grew without bound. Only files whose
/// name carries the exact [`EXPORT_PREFIX`] and `.xlsx` extension this module
/// writes are ever considered - no other file in the directory is touched.
///
/// Every step is deliberately fallible-and-ignored: a workbook the user still
/// has open in Excel is locked on Windows, and silently skipping it is the
/// desired behavior, not a failure worth surfacing. A directory that can't even
/// be read just yields no sweep.
fn sweep_stale_exports(dir: &Path) {
    let now = std::time::SystemTime::now();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !(name.starts_with(EXPORT_PREFIX) && name.ends_with(".xlsx")) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > EXPORT_RETENTION);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

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
        // `i64::MIN.abs()` overflows - there is no positive i64 representation
        // of 2^63, so `.abs()` panics in debug builds and wraps back to
        // i64::MIN in release. `unsigned_abs()` returns a u64 and sidesteps
        // the overflow entirely.
        if n.unsigned_abs() <= F64_EXACT_INT as u64 {
            CellValue::Number(n as f64)
        } else {
            CellValue::Text
        }
    } else if let Ok(n) = val.parse::<f64>() {
        // `inf`/`-inf`/`nan` parse successfully but aren't valid XLSX numbers
        // (Excel has no representation for them) - fall back to text rather
        // than writing a value the file format can't hold.
        if n.is_finite() {
            CellValue::Number(n)
        } else {
            CellValue::Text
        }
    } else {
        CellValue::Text
    }
}

fn dedupe_headers(headers: &[String]) -> Vec<String> {
    use std::collections::HashSet;

    let mut used: HashSet<String> = HashSet::with_capacity(headers.len());
    headers
        .iter()
        .map(|header| {
            if used.insert(header.clone()) {
                return header.clone();
            }
            // Keep bumping the suffix until it lands on a name nothing else
            // has claimed - a straight `_{count}` collided when a later
            // duplicate's generated name matched a header that was already
            // present verbatim (e.g. `["a", "a", "a_2"]` naively produced
            // `["a", "a_2", "a_2"]`, a fresh collision).
            let mut n = 2;
            loop {
                let candidate = format!("{header}_{n}");
                if used.insert(candidate.clone()) {
                    return candidate;
                }
                n += 1;
            }
        })
        .collect()
}

pub fn export_to_xlsx(
    headers: &[String],
    rows: &[Vec<String>],
    column_types: &[String],
    dest_dir: &Path,
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
    let last_col = (headers.len() - 1) as u16;
    let columns: Vec<TableColumn> = table_headers
        .iter()
        .map(|h| TableColumn::new().set_header(h))
        .collect();
    let table = Table::new()
        .set_style(TableStyle::Medium2)
        .set_columns(&columns);
    ws.add_table(0, 0, last_row, last_col, &table).str_err()?;
    ws.autofit();

    // Reclaim old exports before writing the new one. Best-effort: a failure
    // here must never block the export the user actually asked for.
    sweep_stale_exports(dest_dir);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dest_dir.join(format!("{EXPORT_PREFIX}{ts}.xlsx"));
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
    fn non_finite_floats_stay_text() {
        assert_eq!(classify_cell(true, "inf"), CellValue::Text);
        assert_eq!(classify_cell(true, "-inf"), CellValue::Text);
        assert_eq!(classify_cell(true, "nan"), CellValue::Text);
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
    fn i64_min_does_not_overflow_and_stays_text() {
        // i64::MIN.abs() has no positive i64 representation and would panic
        // (debug) or wrap (release) via the old `.abs()` check. It's also far
        // beyond F64_EXACT_INT, so the correct classification is Text.
        assert_eq!(classify_cell(true, "-9223372036854775808"), CellValue::Text);
    }

    #[test]
    fn export_rejects_empty_data() {
        let err = export_to_xlsx(&[], &[], &[], &std::env::temp_dir()).unwrap_err();
        assert_eq!(err, "No data to export");
    }

    #[test]
    fn export_rejects_row_wider_than_headers() {
        let headers = vec!["id".to_string()];
        let rows = vec![vec!["a".to_string(), "b".to_string()]];
        let err = export_to_xlsx(&headers, &rows, &[], &std::env::temp_dir()).unwrap_err();
        assert!(err.contains("more cells than headers"), "got: {err}");
    }

    #[test]
    fn export_allows_duplicate_headers() {
        let dir = tempfile::TempDir::new().unwrap();
        let headers = vec!["a".to_string(), "a".to_string()];
        let rows = vec![vec!["1".to_string(), "2".to_string()]];
        let path = export_to_xlsx(
            &headers,
            &rows,
            &["INTEGER".to_string(), "INTEGER".to_string()],
            dir.path(),
        )
        .unwrap();

        assert!(std::path::Path::new(&path).exists());
        // The file lands in the requested destination directory, not temp.
        assert_eq!(std::path::Path::new(&path).parent(), Some(dir.path()));
    }

    #[test]
    fn dedupe_headers_suffixes_repeats() {
        let headers = vec![
            "a".to_string(),
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
        ];

        assert_eq!(dedupe_headers(&headers), vec!["a", "a_2", "b", "a_3"]);
    }

    #[test]
    fn dedupe_headers_bumps_past_pre_existing_suffixed_name() {
        // "a_2" is a real header on its own here, not a generated one - the
        // naive `_{count}` scheme collided with it and produced
        // ["a", "a_2", "a_2"], a fresh duplicate. The fix must keep bumping
        // until it finds a name nothing else has claimed.
        let headers = vec!["a".to_string(), "a".to_string(), "a_2".to_string()];

        let deduped = dedupe_headers(&headers);

        assert_eq!(deduped, vec!["a", "a_2", "a_2_2"]);
        let unique: std::collections::HashSet<_> = deduped.iter().collect();
        assert_eq!(
            unique.len(),
            deduped.len(),
            "deduped headers must all be unique"
        );
    }

    #[test]
    fn sweep_removes_only_stale_export_files() {
        use std::time::{Duration, SystemTime};

        let dir = tempfile::TempDir::new().unwrap();
        let stale = dir.path().join("dblitz_export_111.xlsx");
        let fresh = dir.path().join("dblitz_export_222.xlsx");
        let wrong_ext = dir.path().join("dblitz_export_333.txt");
        let unrelated = dir.path().join("quarterly_report.xlsx");
        for p in [&stale, &fresh, &wrong_ext, &unrelated] {
            std::fs::write(p, b"x").unwrap();
        }

        // Backdate only the stale export past the retention window. The others
        // keep their just-now mtime.
        let old = SystemTime::now() - EXPORT_RETENTION - Duration::from_secs(60);
        let handle = std::fs::File::options().write(true).open(&stale).unwrap();
        handle.set_modified(old).unwrap();
        drop(handle);

        sweep_stale_exports(dir.path());

        assert!(!stale.exists(), "a stale dblitz export must be swept");
        assert!(fresh.exists(), "a recent dblitz export must be kept");
        assert!(
            wrong_ext.exists(),
            "a non-.xlsx file must never be swept, even with the export prefix"
        );
        assert!(
            unrelated.exists(),
            "a file without the dblitz_export_ prefix must never be swept"
        );
    }
}
