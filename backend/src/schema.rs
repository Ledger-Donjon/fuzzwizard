// @generated automatically by Diesel CLI.

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
