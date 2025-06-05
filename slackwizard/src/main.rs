use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap};
use serde::Serialize;

#[derive(Serialize)]
struct Payload {
    channel: String,
    text: String,
}

fn main() {
    let token = "REPLACEME";

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());

    let client = reqwest::blocking::Client::new();
    let res = client
        .post("https://slack.com/api/chat.postMessage")
        .headers(headers)
        .json(&Payload {
            channel: "REPLACEME".to_owned(),
            text: "salut".to_owned(),
        })
        .send()
        .unwrap();
    println!("{:?}", res.status());
    println!("{:?}", res.text());
}
