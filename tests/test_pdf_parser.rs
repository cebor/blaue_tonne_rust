use std::path::PathBuf;

use blaue_tonne_rust::errors::PlanError;
use blaue_tonne_rust::pdf_parser::{index_districts, normalize_district};

fn fixture_pdf() -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lk_rosenheim_2026.pdf");
    std::fs::read(&path).expect("fixture PDF not found")
}

const PLANS_PAGES: &str = "1,2";

// ---------------------------------------------------------------------------
// Happy-path: every known district must appear in the index with at least one
// date. The macro generates one #[test] per district AND the DISTRICTS
// constant, so the district list exists exactly once.
// ---------------------------------------------------------------------------

macro_rules! district_tests {
    ($(($name:ident, $district:expr)),* $(,)?) => {
        const DISTRICTS: &[&str] = &[$($district),*];

        $(
            #[test]
            fn $name() {
                let pdf = fixture_pdf();
                let index = index_districts(&pdf, PLANS_PAGES)
                    .unwrap_or_else(|e| panic!("fixture failed to index: {:?}", e));
                let dates = index
                    .get(&normalize_district($district))
                    .unwrap_or_else(|| panic!("district '{}' is not in the index", $district));
                assert!(
                    !dates.is_empty(),
                    "no dates indexed for district '{}'",
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
fn test_unknown_district_is_absent_from_the_index() {
    let pdf = fixture_pdf();
    let index = index_districts(&pdf, PLANS_PAGES).expect("fixture must index");
    assert!(!index.contains_key("NonexistentDistrict"));
}

#[test]
fn test_invalid_bytes_rejected() {
    // index_districts doesn't validate the URL – that's done in download_pdf.
    // But we can verify invalid bytes are handled gracefully.
    // `index_districts` can only ever produce `PlanError::Failed`, so matching
    // the variant would assert nothing — the message is what carries the reason.
    let result = index_districts(b"not a pdf", "1");
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("cross-reference")),
        "expected a parse failure for invalid bytes, got: {result:?}"
    );
}

#[test]
fn test_page_past_the_end_of_the_document_is_an_error() {
    // A `pages` entry the document cannot satisfy is a config fault we would
    // rather see at startup than silently index nothing for.
    let pdf = fixture_pdf();
    let result = index_districts(&pdf, "999");
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("Page index 998")),
        "expected the out-of-range page to be named, got: {result:?}"
    );
}

#[test]
fn test_all_districts_count() {
    // Quick sanity check that our constant list has the right size
    assert_eq!(DISTRICTS.len(), 50);
}

// ---------------------------------------------------------------------------
// normalize_district — the rule the index is keyed on
// ---------------------------------------------------------------------------

#[test]
fn test_normalize_district_is_whitespace_insensitive() {
    for spelling in [
        "Bad Aibling",
        "BadAibling",
        "B a d  Aibling",
        "  Bad Aibling ",
    ] {
        assert_eq!(normalize_district(spelling), "BadAibling");
    }
}

#[test]
fn test_normalize_district_is_idempotent() {
    // `DistrictIndex::from_pairs` normalizes keys that `index_districts` already
    // produced in normalized form — that has to be a no-op.
    for district in DISTRICTS {
        let once = normalize_district(district);
        assert_eq!(normalize_district(&once), once, "district {district:?}");
    }
}

#[test]
fn test_index_keys_are_already_normalized() {
    // The handler looks up the normalized name, so every key the index produces
    // has to survive normalization unchanged — otherwise a district present in
    // the PDF would be unreachable over HTTP.
    let pdf = fixture_pdf();
    let index = index_districts(&pdf, PLANS_PAGES).expect("fixture must index");
    for key in index.keys() {
        assert_eq!(&normalize_district(key), key, "index key {key:?}");
    }
    assert!(index.contains_key(&normalize_district("Bad Aibling")));
}
