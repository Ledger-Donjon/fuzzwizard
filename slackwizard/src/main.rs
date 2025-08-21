use reqwest::{header::{HeaderMap, AUTHORIZATION, CONTENT_TYPE}};
use serde::{Serialize, Deserialize};
use std::time::Duration;
use chrono::{NaiveDate, NaiveDateTime};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use log::info;
//use std::collections::HashMap;

#[derive(Serialize)]
struct Payload {
    text: String,
    channel: String,
}

/*SEND FILE TEST
#[derive(Serialize,Deserialize)]
struct FileInfo {
    id: String,
    title: String,
}

#[derive(Serialize)]
struct Payload2 {
    files: Vec<FileInfo>,
    channel_id: String,
}

#[derive(Deserialize)]
struct JsonFileResponse {
    ok: bool,
    upload_url: String,
    file_id: String,
}
*/

#[derive(Deserialize, Serialize, Clone)]
struct CrashReport {
    id: i32,
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    created_at:NaiveDateTime,
}

fn send_message_to_channel(report:CrashReport){
    let token = std::env::var("SLACK_TOKEN").expect("SLACK_TOKEN must be set");
    let channel = std::env::var("SLACK_CHANNEL").expect("SLACK_CHANNEL must be set");
 
    info!("filename: {}", &report.filename);

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    
    //let to_send = format!("Nouveau crash for fuzzer {} !\n{}\n```{}```\n",&report.filename, String::from_utf8_lossy(&report.input), String::from_utf8_lossy(&report.report));
    let created_fmt = report.created_at.format("%Y-%m-%dT%H-%M-%S");
    let stem = Path::new(&report.filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let dir = format!("/tmp/tmp_{}_{}", created_fmt, stem);

    let to_send = format!(
        "Nouveau crash pour {} (id: {})\n\nPour récupérer les artefacts localement:\n\n$ curl -X POST http://127.0.0.1:3000/collect_crashes -H 'Content-Type: application/json' -d '{}'\n\nLes fichiers seront créés sous {}\n- {} (binaire du programme)\n- crash_info.txt (rapport)\n- crash_input.txt (input)\n",
        &report.filename,
        report.id,
        report.id,
        dir,
        &report.filename
    );

    let client = reqwest::blocking::Client::new();
    let res = client
        .post("https://slack.com/api/chat.postMessage")
        .headers(headers.clone())
        .json(&Payload {
            channel,
            text: to_send.to_owned(),
        })
        .send()
        .unwrap();
    info!("slack status: {:?}", res.status());
    info!("slack body: {:?}", res.text());

    // Wait for file:write has been enabled on slack channel
    //let mut file = File::create("/tmp/fuzz_error.bin").unwrap();
    //file.write_all(&report.program);
    /* SEND FILES TEST
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    //headers.insert(ACCEPT_CHARSET, "utf-8".parse().unwrap());
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());

    let test = "toto";
    //let content = String::from_utf8_lossy(&report.program).to_string();

    let mut params = HashMap::new();
    let l = test.len().to_string();
    params.insert("filename", "fuzz_program.bin");
    params.insert("length", &l);

    let slack_url_info = client
        .get("https://slack.com/api/files.getUploadURLExternal")
        .headers(headers.clone())
        .query(&params)
        .send()
        .unwrap();

    println!("{:?}", slack_url_info.status());
    //println!("{:?}", slack_url_info.text());

    let parsed_elt: JsonFileResponse = serde_json::from_str(&slack_url_info.text().unwrap()).expect("NOT OK");
    println!("ok: {}", parsed_elt.ok);
    println!("upload_url: {}", parsed_elt.upload_url);
    println!("file_id: {}", parsed_elt.file_id);


    if parsed_elt.ok{
        let send_file = client
            .post(parsed_elt.upload_url)
            .body(test)
            .send()
            .unwrap();

        println!("{:?}", send_file.status());
        println!("{:?}", send_file.text());

        let rep_files = FileInfo {
            id: parsed_elt.file_id,
            title: "fuzz_program.bin".to_owned(),
        };

        let complete = client
            .post("https://slack.com/api/files.completeUploadExternal")
            .headers(headers.clone())
            .json(&Payload2 {
                files: vec![rep_files],
                channel_id: "CHANGE_ME".to_owned(),
            })
            .send()
            .unwrap();

        println!("{:?}", complete.status());
        println!("{:?}", complete.text());
    }
    */

}

fn main() {
    let file_path = "/tmp/fuzzwizzard_date_file.txt";
    let mut file;
    if Path::new(file_path).exists(){
        file = File::open(file_path).unwrap();
    }
    else {
        file = File::create(file_path).unwrap();
    }

    let mut contents = String::new();
    let _ = file.read_to_string(&mut contents);

    let mut last_date:NaiveDateTime;
    if contents.is_empty(){
        last_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
    }
    else{
        last_date = NaiveDateTime::parse_from_str(&contents, "%Y-%m-%d %H:%M:%S%.f").unwrap();
    }


    env_logger::init();

    loop{

        let client_crash = reqwest::blocking::Client::new();
        let mut headers_backend = HeaderMap::new();
        headers_backend.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        let list_crashes=client_crash
            .post("http://127.0.0.1:3000/get_last_crashes")
            .headers(headers_backend)
            .json(&last_date)
            .send()
            .unwrap();
        
        if list_crashes.status().is_success(){
            let body = list_crashes.text().unwrap();
            let current_vec:Vec<CrashReport> = serde_json::from_str(&body).expect("test");
            info!("received {} crashes", current_vec.len());
            if !current_vec.is_empty(){
                for elt in current_vec.iter(){

                    send_message_to_channel(elt.clone());
                    if last_date < elt.created_at{
                        last_date = elt.created_at;
                    }}
            }
            
        }

        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(file_path).unwrap();

        let _ = file.write_all(last_date.format("%Y-%m-%d %H:%M:%S%.f").to_string().as_bytes());

        std::thread::sleep(Duration::from_secs(10));

    }
}
