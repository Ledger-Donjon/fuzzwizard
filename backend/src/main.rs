use axum::{
    routing::{get, post},
    http::StatusCode,
    Json, Router,
};
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::fmt::init();

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(get_crashes_ids))
        // `POST /users` goes to `create_user`
        .route("/users", post(upload_crash));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn get_crashes_ids() -> (StatusCode, Json<Vec<u32>>) {
    // TODO: get ids from DB
    todo!()
}

async fn upload_crash(
    Json(report): Json<CrashReport>,
) -> StatusCode {
    // TODO: add crash to DB
    // and stash id

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