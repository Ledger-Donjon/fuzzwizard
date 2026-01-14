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

diesel::allow_tables_to_appear_in_same_query!(admin_events, crashes,);
