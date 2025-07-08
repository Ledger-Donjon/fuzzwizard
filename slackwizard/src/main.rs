use reqwest::{header::{self, HeaderMap, AUTHORIZATION, CONTENT_TYPE}, Client};
use serde::{Serialize, Deserialize};
use serde_json::{Result, Value};
use axum::{
    Json
};
use std::time::Duration;



#[derive(Serialize)]
struct Payload {
    text: String,
    channel: String,
}
//struct Payload {
//    user_id: String,
//    message: String,
//}
/*fn send_message(message:String, headers:&HeaderMap){
    
    let client = reqwest::blocking::Client::new();
   let res = client
        .post("https://hooks.slack.com/triggers/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")
        .headers(headers.clone())
        .json(&Payload {
            user_id: "U03KR8MGVDF".to_owned(),
            message: message.to_owned(),
        })
        .send()
        .unwrap();
    
    println!("{:?}", res.status());
    println!("{:?}", res.text()); 
    
    let res = client
        .post("https://hooks.slack.com/triggers/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")
        .headers(headers.clone())
        .json(&Payload {
            user_id: "U057ARSM31C".to_owned(),
            message: message.to_owned(),
        })
        .send()
        .unwrap();
    
    println!("{:?}", res.status());
    println!("{:?}", res.text());  
    
    let res = client
        .post("https://hooks.slack.com/triggers/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")
        .headers(headers.clone())
        .json(&Payload {
            user_id: "U06HAC5D728".to_owned(),
            message: message.to_owned(),
        })
        .send()
        .unwrap();
    
    println!("{:?}", res.status());
    println!("{:?}", res.text());   
}*/

#[derive(Deserialize, Serialize, Clone)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
}

fn send_message_to_channel(id:i32){

    let token = "REPLACEME"
    let mut headers_backend = HeaderMap::new();
    headers_backend.insert(CONTENT_TYPE, "application/json".parse().unwrap());

    let client_crash = reqwest::blocking::Client::new();
 

    let res_crash = client_crash
        .get("http://127.0.0.1:3000/get_crash_by_id")
        .headers(headers_backend)
        .body(id.to_string())
        .send()
        .unwrap();


    let message = res_crash.text().unwrap();
    let report:CrashReport = serde_json::from_str(&message).expect("test2");
    println!("{:?}", String::from_utf8_lossy(&report.report));
    println!("{:?}", String::from_utf8_lossy(&report.input));

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());

    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    
    let to_send = format!("Nouveau crash !\n{}\n{}\n",String::from_utf8_lossy(&report.input), String::from_utf8_lossy(&report.report));

    println!("{:?}", to_send);
    let client = reqwest::blocking::Client::new();
    let res = client
        .post("https://slack.com/api/chat.postMessage")
        .headers(headers)
        .json(&Payload {
            channel: "fuzzing-alerts".to_owned(),
            text: to_send.to_owned(),
        })
        .send()
        .unwrap();

    println!("{:?}", res.status());
    println!("{:?}", res.text());
    


}

fn main() {
    let mut id_done: Vec<i32> = vec![];


    loop{
        let client_crash = reqwest::blocking::Client::new();
        let mut headers_backend = HeaderMap::new();
        headers_backend.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        let list_crashes=client_crash
            .get("http://127.0.0.1:3000/get_crashes")
            .headers(headers_backend)
            .body("1")
            .send()
            .unwrap();
        
        if list_crashes.status().is_success(){
            let body = list_crashes.text().unwrap();
            
            let mut current_vec:Vec<i32> = serde_json::from_str(&body).expect("test");
            if current_vec.is_empty(){
                println!("Empty"); 
            }
            else{
                current_vec.retain(|item| !id_done.contains(item)); 
                for elt in current_vec.iter(){
                    id_done.push(*elt);
                    println!("{:?}", elt.to_string());
                    send_message_to_channel(*elt);
                }
            }
            
        }
        std::thread::sleep(Duration::from_secs(10));
    
    }
    /*
    let res_crash = client_crash
        .get("http://127.0.0.1:3000/get_crash_by_id")
        .headers(headers_backend)
        .body("1")
        .send()
        .unwrap();

    //println!("{:?}", res_crash.status());
    //println!("{:?}", res_crash.text());

    let message = res_crash.text().unwrap();

    let client = reqwest::blocking::Client::new();
    let res = client
        .post("https://slack.com/api/chat.postMessage")
        .headers(headers)
        .json(&Payload {
            channel: "fuzzing-alerts".to_owned(),
            text: message.to_owned(),
        })
        .send()
        .unwrap();

    println!("{:?}", res.status());
    println!("{:?}", res.text());



    //send_message(message, &headers);
    */

}
