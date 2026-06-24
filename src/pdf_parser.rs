use chrono::NaiveDate;
use pdf_oxide::layout::TextSpan;
use pdf_oxide::PdfDocument;

use crate::errors::AppError;

const DATE_LENGTH: usize = 8;
const Y_TOLERANCE: f32 = 5.0;

// ---------------------------------------------------------------------------
// Span helpers
// ---------------------------------------------------------------------------

/// Group spans into rows by proximity in Y, sorted top-to-bottom then left-to-right.
/// PDF Y coordinates increase upward, so we sort by Y descending.
fn spans_to_rows(spans: &[TextSpan]) -> Vec<Vec<String>> {
    let mut sorted: Vec<&TextSpan> = spans.iter().collect();
    sorted.sort_by(|a, b| {
        b.bbox.y
            .total_cmp(&a.bbox.y)
            .then(a.bbox.x.total_cmp(&b.bbox.x))
    });

    let mut rows: Vec<(f32, Vec<String>)> = Vec::new();
    for span in sorted {
        if let Some(last) = rows.last_mut()
            && (span.bbox.y - last.0).abs() <= Y_TOLERANCE {
                last.1.push(span.text.clone());
                continue;
            }
        rows.push((span.bbox.y, vec![span.text.clone()]));
    }
    rows.into_iter().map(|(_, texts)| texts).collect()
}

// ---------------------------------------------------------------------------
// Date parsing
// ---------------------------------------------------------------------------

/// Parse a date from a cell string. The date is always the last 8 characters
/// in "dd.mm.yy" format (e.g. "06.01.26" or "Mo. 06.01.26" → "06.01.26").
fn parse_date(cell: &str) -> Option<NaiveDate> {
    let cell = cell.trim();
    if cell.len() < DATE_LENGTH {
        return None;
    }
    let date_str = &cell[cell.len() - DATE_LENGTH..];
    NaiveDate::parse_from_str(date_str, "%d.%m.%y").ok()
}

/// Parse all dates from a row of cells.
fn parse_dates_from_row(row: &[String]) -> Vec<NaiveDate> {
    row.iter().filter_map(|cell| parse_date(cell)).collect()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Extract waste collection dates for `district` from a PDF.
///
/// `pdf_bytes` – raw bytes of the downloaded PDF.
/// `pages`     – comma-separated 1-based page numbers, e.g. `"1,2"`.
/// `district`  – district name to search for (must appear verbatim in a table cell).
///
/// Returns the dates found on the district row and the following row.
/// Returns `AppError::DistrictNotFound` when the district is not in any table.
pub fn get_dates(
    pdf_bytes: &[u8],
    pages: &str,
    district: &str,
) -> Result<Vec<NaiveDate>, AppError> {
    let doc =
        PdfDocument::from_bytes(pdf_bytes.to_vec()).map_err(|e| AppError::PdfError(e.to_string()))?;

    // pages are 1-based in plans.yaml; extract_tables uses 0-based indices
    let page_numbers: Vec<usize> = pages
        .split(',')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .filter_map(|n| n.checked_sub(1))
        .collect();

    // District names in the PDF may be stored as character fragments, so we
    // concatenate all cells in a row and compare without spaces.
    let district_key: String = district.chars().filter(|c| !c.is_whitespace()).collect();

    for page_idx in page_numbers {
        let spans = doc
            .extract_spans(page_idx)
            .map_err(|e| AppError::PdfError(e.to_string()))?;
        let rows = spans_to_rows(&spans);

        for (row_idx, row) in rows.iter().enumerate() {
            let row_text: String = row
                .iter()
                .flat_map(|s| s.chars().filter(|c| !c.is_whitespace()))
                .collect();

            if row_text == district_key {
                let mut dates: Vec<NaiveDate> = Vec::new();
                // dates row BEFORE the name row (first half of the year)
                if row_idx > 0
                    && let Some(prev_row) = rows.get(row_idx - 1) {
                        dates.extend(parse_dates_from_row(prev_row));
                    }
                // dates row AFTER the name row (second half of the year)
                if let Some(next_row) = rows.get(row_idx + 1) {
                    dates.extend(parse_dates_from_row(next_row));
                }
                if !dates.is_empty() {
                    return Ok(dates);
                }
            }
        }
    }

    Err(AppError::DistrictNotFound)
}

/// Debug helper: returns reconstructed table rows for a page.
#[doc(hidden)]
pub fn debug_extract(pdf_bytes: &[u8], pages: &str) -> Result<Vec<Vec<String>>, AppError> {
    let doc = PdfDocument::from_bytes(pdf_bytes.to_vec())
        .map_err(|e| AppError::PdfError(e.to_string()))?;
    let page_indices: Vec<usize> = pages
        .split(',')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .filter_map(|n| n.checked_sub(1))
        .collect();
    let mut all_rows = Vec::new();
    for page_idx in page_indices {
        let spans = doc
            .extract_spans(page_idx)
            .map_err(|e| AppError::PdfError(e.to_string()))?;
        all_rows.extend(spans_to_rows(&spans));
    }
    Ok(all_rows)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_date_exact_length() {
        let result = parse_date("06.01.26");
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2026, 1, 6).unwrap()));
    }

    #[test]
    fn test_parse_date_with_prefix() {
        // "Mo. 06.01.26" – last 8 chars are "06.01.26"
        let result = parse_date("Mo. 06.01.26");
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2026, 1, 6).unwrap()));
    }

    #[test]
    fn test_parse_date_too_short() {
        assert_eq!(parse_date("1.1.26"), None);
    }

    #[test]
    fn test_parse_date_invalid() {
        assert_eq!(parse_date("Ort Name"), None);
    }
}
