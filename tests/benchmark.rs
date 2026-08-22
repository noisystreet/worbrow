//! Fixture shape only. Live dumps are `cargo run --example search_benchmark` (needs network).

#[test]
fn benchmark_queries_fixture_has_id_and_query() {
    let parsed: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/benchmark_queries.json")).expect("JSON");
    let cases = parsed["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty());
    for c in cases {
        let id = c["id"].as_str().expect("id");
        let query = c["query"].as_str().expect("query");
        assert!(!id.is_empty());
        assert!(!query.is_empty());
    }
}
