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
    coverage_files (id) {
        id -> Int4,
        snapshot_id -> Int4,
        path_id -> Int4,
        lines_covered -> Int4,
        lines_total -> Int4,
        functions_covered -> Int4,
        functions_total -> Int4,
        regions_covered -> Int4,
        regions_total -> Int4,
        branches_covered -> Int4,
        branches_total -> Int4,
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
    coverage_paths (id) {
        id -> Int4,
        path -> Text,
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

diesel::table! {
    fuzzer_coverage_points (id) {
        id -> Int4,
        snapshot_id -> Int4,
        scheduler_name -> Text,
        fuzzer_sha -> Text,
        captured_at -> Timestamp,
        runtime_seconds -> Nullable<Int8>,
        lines_covered -> Int4,
        lines_total -> Int4,
        functions_covered -> Int4,
        functions_total -> Int4,
        regions_covered -> Int4,
        regions_total -> Int4,
        branches_covered -> Int4,
        branches_total -> Int4,
        fuzzer_name -> Nullable<Text>,
    }
}

diesel::table! {
    fuzzer_runtime (id) {
        id -> Int4,
        scheduler_name -> Text,
        fuzzer_sha -> Text,
        total_runtime_seconds -> Int8,
        last_reported_seconds -> Int8,
        scheduler_instance -> Text,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    fuzzer_status (id) {
        id -> Int4,
        scheduler_name -> Text,
        fuzzer_sha -> Text,
        fuzzer_path -> Text,
        status -> Text,
        started_at -> Nullable<Timestamp>,
        last_seen_at -> Timestamp,
    }
}

diesel::joinable!(coverage_files -> coverage_paths (path_id));
diesel::joinable!(coverage_files -> coverage_snapshots (snapshot_id));
diesel::joinable!(coverage_metrics -> coverage_snapshots (snapshot_id));
diesel::joinable!(fuzzer_coverage_points -> coverage_snapshots (snapshot_id));

diesel::allow_tables_to_appear_in_same_query!(
    admin_events,
    coverage_files,
    coverage_metrics,
    coverage_paths,
    coverage_snapshots,
    crashes,
    fuzzer_coverage_points,
    fuzzer_runtime,
    fuzzer_status,
);
