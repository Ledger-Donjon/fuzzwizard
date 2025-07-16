use reqwest::{header::{HeaderMap, AUTHORIZATION, CONTENT_TYPE}};
use serde::{Serialize, Deserialize};
use std::time::Duration;
use chrono::{NaiveDate, NaiveDateTime};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Serialize)]
struct Payload {
    text: String,
    channel: String,
}

#[derive(Deserialize, Serialize, Clone)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    created_at:NaiveDateTime,
}

fn send_message_to_channel(report:CrashReport){
    let token = "REPLACEME";
 
    println!("{:?}", &report.filename);

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    
    let to_send = format!("Nouveau crash for fuzzer {} !\n{}\n{}\n",&report.filename, String::from_utf8_lossy(&report.input), String::from_utf8_lossy(&report.report));

    let client = reqwest::blocking::Client::new();
    let _ = client
        .post("https://slack.com/api/chat.postMessage")
        .headers(headers.clone())
        .json(&Payload {
            channel: "fuzzing-alerts".to_owned(),
            text: to_send.to_owned(),
        })
        .send()
        .unwrap();

     /* Wait for file:write has been enabled on slack channel
    //let mut file = File::create("/tmp/fuzz_error.bin").unwrap();
    file.write_all(&report.program);
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(ACCEPT_CHARSET, "utf-8".parse().unwrap());
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    

    let res2 = client
        .post("https://slack.com/api/files.upload")
        .headers(headers.clone())
        .json(&Payload2 {
            channel: "fuzzing-alerts".to_owned(),
            filename: "/tmp/fuzz_error.bin".to_owned(),
            content: report.program,
        })
        .send()
        .unwrap();

    println!("{:?}", res2.status());
    println!("{:?}", res2.text())

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
            if current_vec.is_empty(){
                println!("Empty"); 
            }
            else{
                for elt in current_vec.iter(){

                    send_message_to_channel(elt.clone());
                    if last_date < elt.created_at{
                        last_date = elt.created_at;
                    }}
            }
            
        }
        println!("{}", last_date);

        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(file_path).unwrap();

        let _ = file.write_all(last_date.format("%Y-%m-%d %H:%M:%S%.f").to_string().as_bytes());

        std::thread::sleep(Duration::from_secs(10));

    }
}
