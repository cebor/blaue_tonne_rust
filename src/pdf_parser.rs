use std::collections::HashMap;

use chrono::NaiveDate;
use pdf_oxide::PdfDocument;
use pdf_oxide::layout::TextSpan;

use crate::errors::PlanError;

const DATE_LENGTH: usize = 8;
const Y_TOLERANCE: f32 = 5.0;

// ---------------------------------------------------------------------------
// Span helpers
// ---------------------------------------------------------------------------

/// Reconstruct the table rows of one page: spans grouped by proximity in Y,
/// sorted top-to-bottom then left-to-right. PDF Y coordinates increase upward,
/// so we sort by Y descending.
///
/// The spans are consumed rather than borrowed, so each cell's text is moved
/// into its row instead of copied.
fn page_rows(doc: &PdfDocument, page_idx: usize) -> Result<Vec<Vec<String>>, PlanError> {
    let mut spans: Vec<TextSpan> = doc
        .extract_spans(page_idx)
        .map_err(|e| PlanError::failed(e.to_string()))?;

    spans.sort_by(|a, b| {
        b.bbox
            .y
            .total_cmp(&a.bbox.y)
            .then(a.bbox.x.total_cmp(&b.bbox.x))
    });

    let mut rows: Vec<(f32, Vec<String>)> = Vec::new();
    for span in spans {
        if let Some(last) = rows.last_mut()
            && (span.bbox.y - last.0).abs() <= Y_TOLERANCE
        {
            last.1.push(span.text);
            continue;
        }
        rows.push((span.bbox.y, vec![span.text]));
    }
    Ok(rows.into_iter().map(|(_, texts)| texts).collect())
}

/// Parse comma-separated 1-based page numbers (e.g. `"1,2"`) into 0-based
/// indices for `pdf_oxide`. Invalid entries are ignored.
fn parse_page_numbers(pages: &str) -> Vec<usize> {
    pages
        .split(',')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .filter_map(|n| n.checked_sub(1))
        .collect()
}

// ---------------------------------------------------------------------------
// Date parsing
// ---------------------------------------------------------------------------

/// Parse a date from a cell string. The date is always the last 8 characters
/// in "dd.mm.yy" format (e.g. "06.01.26" or "Mo. 06.01.26" → "06.01.26").
fn parse_date(cell: &str) -> Option<NaiveDate> {
    let cell = cell.trim();
    // Byte offset of the 8th-from-last character; None if fewer than 8 chars.
    // Slicing by chars (not bytes) keeps multi-byte text like "Größe" safe.
    let start = cell.char_indices().rev().nth(DATE_LENGTH - 1)?.0;
    NaiveDate::parse_from_str(&cell[start..], "%d.%m.%y").ok()
}

/// Parse all dates from a row of cells.
fn parse_dates_from_row(row: &[String]) -> Vec<NaiveDate> {
    row.iter().filter_map(|cell| parse_date(cell)).collect()
}

// ---------------------------------------------------------------------------
// District names
// ---------------------------------------------------------------------------

/// Canonical form of a district name.
///
/// District names in the PDF are stored as character fragments (e.g.
/// "Bad Aibling" arrives as `["B", "ad", "A", "ib", "ling"]`), so all matching
/// happens on the whitespace-stripped form. [`index_districts`] keys on this,
/// and a caller looking a district up has to apply it first — otherwise
/// "Bad Aibling", "BadAibling" and "B a d Aibling" would not resolve to the one
/// row they all name.
///
/// Idempotent: applying it to an already-normalized name is a no-op.
pub fn normalize_district(district: &str) -> String {
    district.chars().filter(|c| !c.is_whitespace()).collect()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read every district of a plan PDF in one pass.
///
/// `pdf_bytes` – raw bytes of the downloaded PDF.
/// `pages`     – comma-separated 1-based page numbers, e.g. `"1,2"`.
///
/// Returns a map from the normalized district name (see [`normalize_district`])
/// to its collection dates. The whole document is opened once, so looking a
/// district up afterwards costs nothing — which is the point: the caller builds
/// this map at startup instead of re-parsing the PDF per request.
pub fn index_districts(
    pdf_bytes: &[u8],
    pages: &str,
) -> Result<HashMap<String, Vec<NaiveDate>>, PlanError> {
    let doc = PdfDocument::from_bytes(pdf_bytes.to_vec())
        .map_err(|e| PlanError::failed(e.to_string()))?;

    let mut index: HashMap<String, Vec<NaiveDate>> = HashMap::new();

    for page_idx in parse_page_numbers(pages) {
        let rows = page_rows(&doc, page_idx)?;

        for (row_idx, row) in rows.iter().enumerate() {
            // A row that carries dates itself is a date row, not a name row.
            // Skipping it keeps date strings out of the key space; no district
            // name can look like one, so nothing reachable is lost.
            if !parse_dates_from_row(row).is_empty() {
                continue;
            }

            // District names in the PDF may be stored as character fragments
            // ("Bad Aibling" arrives as ["B", "ad", "A", "ib", "ling"]), so the
            // key is the whitespace-stripped concatenation of the whole row —
            // exactly the form `normalize_district` produces.
            let name: String = row
                .iter()
                .flat_map(|s| s.chars().filter(|c| !c.is_whitespace()))
                .collect();

            if name.is_empty() {
                continue;
            }

            let mut dates: Vec<NaiveDate> = Vec::new();
            // dates row BEFORE the name row (first half of the year)
            if row_idx > 0
                && let Some(prev_row) = rows.get(row_idx - 1)
            {
                dates.extend(parse_dates_from_row(prev_row));
            }
            // dates row AFTER the name row (second half of the year)
            if let Some(next_row) = rows.get(row_idx + 1) {
                dates.extend(parse_dates_from_row(next_row));
            }

            // A name row without dates around it is not an entry — the same
            // rule a per-district search follows when it keeps scanning past a
            // match. First occurrence wins, so pages are read in order.
            if !dates.is_empty() {
                index.entry(name).or_insert(dates);
            }
        }
    }

    Ok(index)
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

    #[test]
    fn test_parse_date_multibyte_no_panic() {
        // Multi-byte chars within the last 8 bytes must not panic
        // (byte-based slicing would split "ö"/"ß" mid-character).
        assert_eq!(parse_date("Größenwahn"), None);
        assert_eq!(parse_date("Söchtenau"), None);
    }
}
