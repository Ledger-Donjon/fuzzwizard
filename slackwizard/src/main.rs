use reqwest::{header::{self, HeaderMap, AUTHORIZATION, CONTENT_TYPE}, Client};
use serde::Serialize;

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
            channel: "fuzzing-alerts".to_owned(),
            text: "Ceci est un test".to_owned(),
        })
        .send()
        .unwrap();

    println!("{:?}", res.status());
    println!("{:?}", res.text());

    //let client = reqwest::blocking::Client::new();
    //let mut headers_backend = HeaderMap::new();
    //headers_backend.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    //let res = client
    //    .get("http://127.0.0.1:3000/get_crash_by_id")
    //    .headers(headers_backend)
    //    .body("1")
    //    .send()
    //    .unwrap();

    //println!("{:?}", res.status());
    //println!("{:?}", res.text());

    //let message = res.text().unwrap();

    //send_message(message, &headers);

}
