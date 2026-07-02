use std::path::PathBuf;

use blaue_tonne_rust::pdf_parser::get_dates;
use blaue_tonne_rust::errors::AppError;

fn fixture_pdf() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lk_rosenheim_2026.pdf");
    std::fs::read(&path).expect("fixture PDF not found")
}

const PLANS_PAGES: &str = "1,2";

// ---------------------------------------------------------------------------
// Happy-path: every known district must yield at least one date.
// The macro generates one #[test] per district AND the DISTRICTS constant,
// so the district list exists exactly once.
// ---------------------------------------------------------------------------

macro_rules! district_tests {
    ($(($name:ident, $district:expr)),* $(,)?) => {
        const DISTRICTS: &[&str] = &[$($district),*];

        $(
            #[test]
            fn $name() {
                let pdf = fixture_pdf();
                let dates = get_dates(&pdf, PLANS_PAGES, $district)
                    .unwrap_or_else(|e| panic!("district '{}' failed: {:?}", $district, e));
                assert!(
                    !dates.is_empty(),
                    "no dates returned for district '{}'",
                    $district
                );
            }
        )*
    };
}

district_tests! {
    (test_district_albaching, "Albaching"),
    (test_district_amerang, "Amerang"),
    (test_district_aschau, "Aschau"),
    (test_district_babensham, "Babensham"),
    (test_district_bad_aibling, "Bad Aibling"),
    (test_district_bad_endorf, "Bad Endorf"),
    (test_district_bad_feilnbach, "Bad Feilnbach"),
    (test_district_bernau, "Bernau"),
    (test_district_brannenburg, "Brannenburg"),
    (test_district_breitbrunn, "Breitbrunn"),
    (test_district_bruckmuhl_1, "Bruckmühl 1"),
    (test_district_bruckmuhl_2, "Bruckmühl 2"),
    (test_district_edling, "Edling"),
    (test_district_eggstatt, "Eggstätt"),
    (test_district_eiselfing, "Eiselfing"),
    (test_district_feldkirchen_1, "Feldkirchen 1"),
    (test_district_feldkirchen_2, "Feldkirchen 2"),
    (test_district_flintsbach, "Flintsbach"),
    (test_district_frasdorf, "Frasdorf"),
    (test_district_griesstatt, "Griesstätt"),
    (test_district_grosskarolinenfeld_1, "Großkarolinenfeld 1"),
    (test_district_grosskarolinenfeld_2, "Großkarolinenfeld 2"),
    (test_district_gstadt, "Gstadt"),
    (test_district_halfing, "Halfing"),
    (test_district_hoslwang, "Höslwang"),
    (test_district_kiefersfelden, "Kiefersfelden"),
    (test_district_kolbermoor, "Kolbermoor"),
    (test_district_neubeuern, "Neubeuern"),
    (test_district_nussdorf, "Nußdorf am Inn"),
    (test_district_oberaudorf, "Oberaudorf"),
    (test_district_pfaffing, "Pfaffing"),
    (test_district_prien, "Prien a. Chiemsee"),
    (test_district_prutting, "Prutting"),
    (test_district_ramerberg, "Ramerberg"),
    (test_district_raubling_1, "Raubling 1"),
    (test_district_raubling_2, "Raubling 2"),
    (test_district_raubling_3, "Raubling 3"),
    (test_district_riedering, "Riedering"),
    (test_district_rimsting, "Rimsting"),
    (test_district_rohrdorf, "Rohrdorf"),
    (test_district_rott, "Rott am Inn"),
    (test_district_samerberg, "Samerberg"),
    (test_district_schechen, "Schechen"),
    (test_district_schonstett, "Schonstett"),
    (test_district_soyen, "Soyen"),
    (test_district_stephanskirchen_1, "Stephanskirchen 1"),
    (test_district_stephanskirchen_2, "Stephanskirchen 2"),
    (test_district_soechtenau, "Söchtenau"),
    (test_district_tuntenhausen, "Tuntenhausen"),
    (test_district_vogtareuth, "Vogtareuth"),
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn test_district_not_found() {
    let pdf = fixture_pdf();
    let result = get_dates(&pdf, PLANS_PAGES, "NonexistentDistrict");
    assert!(
        matches!(result, Err(AppError::DistrictNotFound)),
        "expected DistrictNotFound, got: {:?}",
        result
    );
}

#[test]
fn test_invalid_url_rejected() {
    // get_dates itself doesn't validate the URL – that's done in download_pdf.
    // But we can verify invalid bytes are handled gracefully.
    let result = get_dates(b"not a pdf", "1", "Kolbermoor");
    assert!(
        matches!(result, Err(AppError::PdfError(_))),
        "expected PdfError for invalid bytes, got: {:?}",
        result
    );
}

#[test]
fn test_all_districts_count() {
    // Quick sanity check that our constant list has the right size
    assert_eq!(DISTRICTS.len(), 50);
}

// ---------------------------------------------------------------------------
// Debug: print raw extraction output
// ---------------------------------------------------------------------------

#[test]
fn test_debug_extraction() {
    let pdf = fixture_pdf();
    use blaue_tonne_rust::pdf_parser::debug_extract;
    let rows = debug_extract(&pdf, "1").expect("debug_extract failed");
    println!("=== Page 1: {} rows ===", rows.len());
    for (i, row) in rows.iter().take(30).enumerate() {
        println!("  row {:02}: {:?}", i, row);
    }
}
