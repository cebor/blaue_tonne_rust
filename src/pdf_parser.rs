use chrono::NaiveDate;
use pdf_extract::{Document, MediaBox, OutputDev, OutputError, output_doc_page};

use crate::errors::AppError;

const DATE_LENGTH: usize = 8;
// Tolerance (in PDF points) for grouping characters on the same row
const ROW_Y_TOLERANCE: f64 = 3.0;
// Minimum horizontal gap (in PDF points) to split a row into separate cells
const CELL_X_GAP: f64 = 4.0;

// ---------------------------------------------------------------------------
// Character collector – implements OutputDev to gather per-character positions
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CharEntry {
    x: f64,
    y: f64,
    ch: String,
}

struct TableExtractor {
    chars: Vec<CharEntry>,
}

impl TableExtractor {
    fn new() -> Self {
        Self { chars: Vec::new() }
    }
}

impl OutputDev for TableExtractor {
    fn begin_page(
        &mut self,
        _page_num: u32,
        _media_box: &MediaBox,
        _art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError> {
        self.chars.clear();
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    fn output_character(
        &mut self,
        trm: &pdf_extract::Transform,
        _width: f64,
        _spacing: f64,
        _font_size: f64,
        char: &str,
    ) -> Result<(), OutputError> {
        // trm is a euclid Transform2D; m31 = tx (x), m32 = ty (y)
        self.chars.push(CharEntry {
            x: trm.m31,
            y: trm.m32,
            ch: char.to_string(),
        });
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Table reconstruction
// ---------------------------------------------------------------------------

/// Reconstruct a table as `Vec<Vec<String>>` (rows × cells) from raw char entries.
///
/// Algorithm:
/// 1. Sort chars by Y descending (PDF Y=0 is bottom of page).
/// 2. Group chars whose Y values are within `ROW_Y_TOLERANCE` → rows.
/// 3. Within each row, sort by X ascending and split by gaps > `CELL_X_GAP` → cells.
fn reconstruct_rows(mut chars: Vec<CharEntry>) -> Vec<Vec<String>> {
    if chars.is_empty() {
        return vec![];
    }

    // Sort by Y descending (top of page first), then X ascending within same Y
    chars.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap()
            .then(a.x.partial_cmp(&b.x).unwrap())
    });

    let mut rows: Vec<Vec<CharEntry>> = Vec::new();
    let mut current_row: Vec<CharEntry> = Vec::new();
    let mut current_y = chars[0].y;

    for ch in chars {
        if (ch.y - current_y).abs() <= ROW_Y_TOLERANCE {
            current_row.push(ch);
        } else {
            if !current_row.is_empty() {
                rows.push(current_row);
            }
            current_y = ch.y;
            current_row = vec![ch];
        }
    }
    if !current_row.is_empty() {
        rows.push(current_row);
    }

    // Convert each row into cells
    rows.into_iter()
        .map(|row| {
            // row is already sorted by X from the initial sort
            split_into_cells(row)
        })
        .collect()
}

/// Split a sorted (by X) row of chars into cells by detecting significant X gaps.
fn split_into_cells(row: Vec<CharEntry>) -> Vec<String> {
    let mut cells: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut prev_x: Option<f64> = None;

    for ch in row {
        if let Some(px) = prev_x
            && ch.x - px > CELL_X_GAP
        {
            let trimmed = current_cell.trim().to_string();
            if !trimmed.is_empty() {
                cells.push(trimmed);
            }
            current_cell = String::new();
        }
        prev_x = Some(ch.x + 1.0); // advance past this char's approximate width
        current_cell.push_str(&ch.ch);
    }

    let trimmed = current_cell.trim().to_string();
    if !trimmed.is_empty() {
        cells.push(trimmed);
    }

    cells
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
    let doc = Document::load_mem(pdf_bytes).map_err(|e| AppError::PdfError(e.to_string()))?;

    let page_numbers: Vec<u32> = pages
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();

    for page_num in page_numbers {
        let mut extractor = TableExtractor::new();
        output_doc_page(&doc, &mut extractor, page_num)
            .map_err(|e| AppError::PdfError(format!("{e:?}")))?;

        let table = reconstruct_rows(extractor.chars);

        // District names in the PDF are stored as character fragments, so we
        // concatenate all cells in a row and compare without spaces.
        let district_key: String = district.chars().filter(|c| !c.is_whitespace()).collect();

        for (row_idx, row) in table.iter().enumerate() {
            let row_text: String = row
                .iter()
                .flat_map(|cell| cell.chars().filter(|c| !c.is_whitespace()))
                .collect();

            if row_text == district_key {
                let mut dates: Vec<NaiveDate> = Vec::new();
                // dates row BEFORE the name row (first half of the year)
                if row_idx > 0
                    && let Some(prev_row) = table.get(row_idx - 1)
                {
                    dates.extend(parse_dates_from_row(prev_row));
                }
                // dates row AFTER the name row (second half of the year)
                if let Some(next_row) = table.get(row_idx + 1) {
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
pub fn debug_extract(pdf_bytes: &[u8], pages: &str) -> Vec<Vec<String>> {
    let doc = Document::load_mem(pdf_bytes).expect("load pdf");
    let page_numbers: Vec<u32> = pages
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();
    let mut all_rows = Vec::new();
    for page_num in page_numbers {
        let mut extractor = TableExtractor::new();
        output_doc_page(&doc, &mut extractor, page_num).expect("output page");
        let rows = reconstruct_rows(extractor.chars);
        all_rows.extend(rows);
    }
    all_rows
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
