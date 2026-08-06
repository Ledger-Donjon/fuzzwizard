// @generated automatically by Diesel CLI.

diesel::table! {
    admin_events (id) {
        id -> Int4,
        event_type -> Text,
        message -> Text,
        metadata -> Nullable<Text>,
        created_at -> Timestamp,
        processed -> Bool,
    }
}

diesel::table! {
    coverage_metrics (id) {
        id -> Int4,
        snapshot_id -> Int4,
        metric_name -> Text,
        covered -> Int4,
        total -> Int4,
    }
}

diesel::table! {
    coverage_snapshots (id) {
        id -> Int4,
        scheduler_name -> Text,
        fuzzer_engine -> Text,
        build_fingerprint -> Text,
        num_fuzzers -> Int4,
        captured_at -> Timestamp,
        schema_version -> Int4,
    }
}

diesel::table! {
    crashes (id) {
        id -> Int4,
        input -> Bytea,
        report -> Bytea,
        filename -> Text,
        program -> Bytea,
        scheduler_name -> Text,
        created_at -> Timestamp,
    }
}

diesel::joinable!(coverage_metrics -> coverage_snapshots (snapshot_id));

diesel::allow_tables_to_appear_in_same_query!(
    admin_events,
    coverage_metrics,
    coverage_snapshots,
    crashes,
);
