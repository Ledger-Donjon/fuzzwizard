// @generated automatically by Diesel CLI.

diesel::table! {
    crashes (id) {
        id -> Int4,
        report -> Nullable<Bytea>,
    }
}
