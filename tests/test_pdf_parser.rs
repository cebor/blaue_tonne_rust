use std::path::PathBuf;

use blaue_tonne_rust::pdf_parser::get_dates;
use blaue_tonne_rust::errors::AppError;

fn fixture_pdf() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lk_rosenheim_2026.pdf");
    std::fs::read(&path).expect("fixture PDF not found")
}

const PLANS_PAGES: &str = "1,2";

const DISTRICTS: &[&str] = &[
    "Albaching", "Amerang", "Aschau", "Babensham", "Bad Aibling", "Bad Endorf",
    "Bad Feilnbach", "Bernau", "Brannenburg", "Breitbrunn", "Bruckmühl 1", "Bruckmühl 2",
    "Edling", "Eggstätt", "Eiselfing", "Feldkirchen 1", "Feldkirchen 2", "Flintsbach",
    "Frasdorf", "Griesstätt", "Großkarolinenfeld 1", "Großkarolinenfeld 2", "Gstadt",
    "Halfing", "Höslwang", "Kiefersfelden", "Kolbermoor", "Neubeuern", "Nußdorf am Inn",
    "Oberaudorf", "Pfaffing", "Prien a. Chiemsee", "Prutting", "Ramerberg", "Raubling 1",
    "Raubling 2", "Raubling 3", "Riedering", "Rimsting", "Rohrdorf", "Rott am Inn",
    "Samerberg", "Schechen", "Schonstett", "Soyen", "Stephanskirchen 1", "Stephanskirchen 2",
    "Söchtenau", "Tuntenhausen", "Vogtareuth",
];

// ---------------------------------------------------------------------------
// Happy-path: every known district must yield at least one date
// ---------------------------------------------------------------------------

macro_rules! district_test {
    ($name:ident, $district:expr) => {
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
    };
}

district_test!(test_district_albaching, "Albaching");
district_test!(test_district_amerang, "Amerang");
district_test!(test_district_aschau, "Aschau");
district_test!(test_district_babensham, "Babensham");
district_test!(test_district_bad_aibling, "Bad Aibling");
district_test!(test_district_bad_endorf, "Bad Endorf");
district_test!(test_district_bad_feilnbach, "Bad Feilnbach");
district_test!(test_district_bernau, "Bernau");
district_test!(test_district_brannenburg, "Brannenburg");
district_test!(test_district_breitbrunn, "Breitbrunn");
district_test!(test_district_bruckmuhl_1, "Bruckmühl 1");
district_test!(test_district_bruckmuhl_2, "Bruckmühl 2");
district_test!(test_district_edling, "Edling");
district_test!(test_district_eggstatt, "Eggstätt");
district_test!(test_district_eiselfing, "Eiselfing");
district_test!(test_district_feldkirchen_1, "Feldkirchen 1");
district_test!(test_district_feldkirchen_2, "Feldkirchen 2");
district_test!(test_district_flintsbach, "Flintsbach");
district_test!(test_district_frasdorf, "Frasdorf");
district_test!(test_district_griesstatt, "Griesstätt");
district_test!(test_district_grosskarolinenfeld_1, "Großkarolinenfeld 1");
district_test!(test_district_grosskarolinenfeld_2, "Großkarolinenfeld 2");
district_test!(test_district_gstadt, "Gstadt");
district_test!(test_district_halfing, "Halfing");
district_test!(test_district_hoslwang, "Höslwang");
district_test!(test_district_kiefersfelden, "Kiefersfelden");
district_test!(test_district_kolbermoor, "Kolbermoor");
district_test!(test_district_neubeuern, "Neubeuern");
district_test!(test_district_nussdorf, "Nußdorf am Inn");
district_test!(test_district_oberaudorf, "Oberaudorf");
district_test!(test_district_pfaffing, "Pfaffing");
district_test!(test_district_prien, "Prien a. Chiemsee");
district_test!(test_district_prutting, "Prutting");
district_test!(test_district_ramerberg, "Ramerberg");
district_test!(test_district_raubling_1, "Raubling 1");
district_test!(test_district_raubling_2, "Raubling 2");
district_test!(test_district_raubling_3, "Raubling 3");
district_test!(test_district_riedering, "Riedering");
district_test!(test_district_rimsting, "Rimsting");
district_test!(test_district_rohrdorf, "Rohrdorf");
district_test!(test_district_rott, "Rott am Inn");
district_test!(test_district_samerberg, "Samerberg");
district_test!(test_district_schechen, "Schechen");
district_test!(test_district_schonstett, "Schonstett");
district_test!(test_district_soyen, "Soyen");
district_test!(test_district_stephanskirchen_1, "Stephanskirchen 1");
district_test!(test_district_stephanskirchen_2, "Stephanskirchen 2");
district_test!(test_district_soechtenau, "Söchtenau");
district_test!(test_district_tuntenhausen, "Tuntenhausen");
district_test!(test_district_vogtareuth, "Vogtareuth");

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
