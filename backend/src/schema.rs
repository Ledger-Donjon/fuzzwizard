// @generated automatically by Diesel CLI.

diesel::table! {
    crashes (id) {
        id -> Int4,
        input -> Bytea,
        report -> Bytea,
    }
}
