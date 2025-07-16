use std::{
    env,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{State,DefaultBodyLimit},
    http::StatusCode,
    routing::{get, post},
};

use diesel::{prelude::*, query_dsl::methods::FilterDsl};
use schema::crashes;
use serde::{Deserialize, Serialize};
use chrono::NaiveDateTime;

mod schema;

#[derive(Deserialize, Serialize, Insertable, Queryable, Clone)]
#[diesel(table_name=crashes)]
struct AddCrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
}

#[derive(Deserialize, Serialize, Insertable, Queryable, Clone)]
#[diesel(table_name=crashes)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    created_at:NaiveDateTime,
}

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::fmt::init();
    let db_url = env::var("DATABASE_URL").expect("DATABASE URL must be set");
    let db_connection = Arc::new(Mutex::new(PgConnection::establish(&db_url).unwrap()));
    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/get_crashes", get(get_crashes_ids))
        // `POST /users` goes to `create_user`
        .route("/upload_crash", post(upload_crash)).layer(DefaultBodyLimit::max(500 *1000 * 1000))
        .route("/get_crash_by_id", get(collect_crash_info))
        .route("/get_last_crashes", post(get_last_crashes))
        .with_state(db_connection);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn get_crashes_ids(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
) -> (StatusCode, Json<Vec<i32>>) {
    // TODO: get ids from DB

    let mut db_connection = db_connection.lock().unwrap();
    let crashes_id = crashes::table
        .select(crashes::id)
        .get_results::<i32>(&mut *db_connection)
        .unwrap();
    (StatusCode::OK, Json(crashes_id))
}

async fn upload_crash(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    Json(report): Json<AddCrashReport>,
) -> StatusCode {
    // TODO: add crash to DB
    // and stash id
    let mut db_connection = db_connection.lock().unwrap();
    diesel::insert_into(crashes::table)
        .values(report)
        .execute(&mut *db_connection)
        .unwrap();
    StatusCode::CREATED
}


#[axum::debug_handler]
async fn collect_crash_info(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    Json(id): Json<i32>,
) -> (StatusCode, Json<CrashReport>) {
    let mut db_connection = db_connection.lock().unwrap();
    let crash = FilterDsl::filter(crashes::table.select((crashes::report, crashes::input, crashes::filename, crashes::program, crashes::created_at)), crashes::id.eq(id))
        .get_results::<CrashReport>(&mut *db_connection)
        .unwrap();
    let result = crash[0].clone();

    (StatusCode::OK, Json(result))
}

async fn get_last_crashes(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    Json(date): Json<NaiveDateTime>,
) -> (StatusCode, Json<Vec<CrashReport>>) {
    let mut db_connection = db_connection.lock().unwrap();
    let crash = FilterDsl::filter(crashes::table.select((crashes::report, crashes::input, crashes::filename, crashes::program, crashes::created_at)), crashes::created_at.gt(date))
        .get_results::<CrashReport>(&mut *db_connection)
        .unwrap();

    (StatusCode::OK, Json(crash))
}



