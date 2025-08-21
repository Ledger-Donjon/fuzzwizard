use crate::schema::crashes;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::NaiveDateTime;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fs,
    path::Path,
    sync::{Arc, Mutex},
};

mod schema;

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::fmt::init();

    let db_url = env::var("DATABASE_URL").expect("DATABASE URL must be set");
    let db_connection = Arc::new(Mutex::new(PgConnection::establish(&db_url).unwrap()));

    let app = Router::new()
        .route("/crashes", get(get_crashes))
        .route("/crashes", post(upload_crash))
        .route("/crashes/{id}", get(get_crash_by_id))
        .route("/collect_crashes", post(collect_crashes))
        .layer(DefaultBodyLimit::max(500 * 1000 * 1000))
        .with_state(db_connection);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[derive(Deserialize, Serialize, Insertable, Queryable, Clone)]
#[diesel(table_name = crashes)]
struct AddCrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
}

#[derive(Deserialize, Serialize, Queryable, Selectable)]
#[diesel(table_name = crashes)]
struct CrashReportWithId {
    id: i32,
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    created_at: NaiveDateTime,
}

#[derive(Deserialize, Serialize, Queryable, Selectable)]
#[diesel(table_name = crashes)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    created_at: NaiveDateTime,
}

async fn get_crashes(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<CrashReportWithId>>, StatusCode> {
    let mut db_connection = db_connection.lock().unwrap();

    let mut query = crashes::table
        .select(CrashReportWithId::as_select())
        .into_boxed();

    if let Some(after) = params.get("after") {
        match chrono::NaiveDateTime::parse_from_str(after, "%Y-%m-%dT%H:%M:%S") {
            Ok(after_date) => {
                query = query.filter(crashes::created_at.gt(after_date));
            }
            Err(_) => {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    let crashes = match query.get_results::<CrashReportWithId>(&mut *db_connection) {
        Ok(results) => results,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    Ok(Json(crashes))
}

async fn upload_crash(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    Json(report): Json<AddCrashReport>,
) -> StatusCode {
    let mut db_connection = db_connection.lock().unwrap();

    match diesel::insert_into(crashes::table)
        .values(report)
        .execute(&mut *db_connection)
    {
        Ok(_) => StatusCode::CREATED,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[axum::debug_handler]
async fn get_crash_by_id(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    axum::extract::Path(id): axum::extract::Path<i32>,
) -> Result<Json<CrashReport>, StatusCode> {
    let mut db_connection = db_connection.lock().unwrap();

    match crashes::table
        .select(CrashReport::as_select())
        .filter(crashes::id.eq(id))
        .get_result::<CrashReport>(&mut *db_connection)
    {
        Ok(crash) => Ok(Json(crash)),
        Err(diesel::result::Error::NotFound) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn collect_crashes(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    Json(id): Json<i32>,
) -> (StatusCode, Json<String>) {
    let mut db_connection = db_connection.lock().unwrap();
    let crash = crashes::table
        .select(CrashReport::as_select())
        .filter(crashes::id.eq(id))
        .get_result::<CrashReport>(&mut *db_connection)
        .optional()
        .unwrap();
    let Some(crash) = crash else {
        return (StatusCode::NOT_FOUND, Json("No crash found".to_string()));
    };

    let safe_filename = Path::new(&crash.filename)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let safe_stem = Path::new(&safe_filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let dir_name = format!(
        "/tmp/tmp_{}_{}",
        crash.created_at.format("%Y-%m-%dT%H-%M-%S"),
        safe_stem
    );

    fs::create_dir_all(&dir_name).unwrap();

    let program_path = Path::new(&dir_name).join(&safe_filename);
    let report_path = Path::new(&dir_name).join("crash_info.txt");
    let input_path = Path::new(&dir_name).join("crash_input.txt");

    fs::write(&program_path, &crash.program).unwrap();
    fs::write(&report_path, &crash.report).unwrap();
    fs::write(&input_path, &crash.input).unwrap();

    (StatusCode::CREATED, Json(dir_name))
}
