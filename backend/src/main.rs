use std::{env, sync::{Arc, Mutex}};

use axum::{
    extract::State, http::StatusCode, routing::{get, post}, Json, Router
};
use serde::{Deserialize, Serialize};
use diesel::prelude::*;
use schema::crashes;
mod schema;

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::fmt::init();

    let db_url = env::var("DATABASE_URL").expect("DATABASE URL must be set");
    let db_connection = Arc::new(Mutex::new(PgConnection::establish(&db_url).unwrap())); 



    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(get_crashes_ids))
        // `POST /users` goes to `create_user`
        .route("/users", post(upload_crash))
        .with_state(db_connection);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn get_crashes_ids(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
) -> (StatusCode, Json<Vec<i32>>) {
    // TODO: get ids from DB
    
    let mut db_connection = db_connection.lock().unwrap(); 
    let crashes_id = crashes::table.select(crashes::id).get_results::<i32>(&mut* db_connection).unwrap();
    (StatusCode::OK, Json(crashes_id))
}

async fn upload_crash(
    State(db_connection): State<Arc<Mutex<PgConnection>>>,
    Json(report): Json<CrashReport>,
) -> StatusCode {
    // TODO: add crash to DB
    // and stash id
    let mut db_connection = db_connection.lock().unwrap(); 
    diesel::insert_into(crashes::table)
                .values(crashes::report.eq(report.report))
                .execute(&mut* db_connection).unwrap();
    StatusCode::CREATED
}

struct Crash {
    id: u32,
    report: CrashReport
}

#[derive(Deserialize)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>
}