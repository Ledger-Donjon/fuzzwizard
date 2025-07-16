// @generated automatically by Diesel CLI.

diesel::table! {
    crashes (id) {
        id -> Int4,
        input -> Bytea,
        report -> Bytea,
        filename -> Text,
        program -> Bytea,
        created_at -> Timestamp,
    }
}
