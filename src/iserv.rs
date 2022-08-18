use std::collections::HashMap;
use std::time::Duration;
use serde::{Serialize, Deserialize};

const ISERV_BASE_URL : &str = "https://***REMOVED***/iserv";

pub(crate) async fn get_iserv_client(creds : (String, String)) -> Option<reqwest::Client> {
    for i in 0..10 {
        match _get_iserv_client(&creds.0, &creds.1).await {
            Some(c) => return Some(c),
            None => {}
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100*i)).await;
    }
    None
}

async fn _get_iserv_client(username : &str, password : &str) -> Option<reqwest::Client> {
    let client = reqwest::ClientBuilder::new()
        .cookie_store(true)
        .timeout(Duration::from_secs(5))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 11_4; rv:97.0esr) Gecko/20010101 Firefox/97.0esr")
        .build()
        .unwrap();

    let response = client.post(format!("{}/auth/login", ISERV_BASE_URL))
        .header("Upgrade-Insecure-Requests", "1")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "same-origin")
        .header("Sec-Fetch-User", "?1")
        .header("Sec-GPC", "1")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("_username={}&_password={}", username, password))
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }
    let text = response.text().await.ok()?;
    if text.contains("Anmeldung fehlgeschlagen!") || text.contains("IServ-Anmeldung") {
        return None;
    }
    Some(client)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetFilesResponse {
    #[serde(rename = "data")]
    pub files : Vec<FileResponse>,
    pub writable : bool,
    pub breadcrumbs : Vec<HashMap<String, String>>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileResponse {
    pub id: String,
    pub name: FileNamResponse,
    pub size: FileSizeResponse,
    #[serde(rename = "type")]
    pub type_field: HashMap<String, String>,
    pub thumbnail: bool,
    pub owner: String,
    pub date: HashMap<String, String>,
    pub path: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSizeResponse {
    pub display: String,
    pub order: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileNamResponse {
    pub icon: String,
    pub link: String,
    pub text: String,
    pub order: String,
    pub windows: bool,
}

pub(crate) async fn get_files(client : &reqwest::Client, path: String) -> Result<GetFilesResponse, String> {
    let start_unix_time_ms = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_millis();
    let url = format!("{}/file/-/{}?_={}", ISERV_BASE_URL, path, start_unix_time_ms);
    let response = client.get(url)
        .header("Accept","application/json, text/javascript, */*; q=0.01")
        .header("Accept-Encoding", "gzip, deflate, br")
        .header("TE", "trailers")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("DNT", "1")
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;
    let status_code = response.status();
    if !status_code.is_success() {
        return Err(format!("Status code was {}", status_code));
    }
    let text = response.text().await
        .or_else(|e| Err(e.to_string()))?;
    let value : serde_json::Value = serde_json::from_str(&text).unwrap();
    let json_filtered = serde_json::to_string(&value).unwrap();
    Ok(serde_json::from_str(&json_filtered).unwrap()) // response.json().await.or_else(|e| Err(e.to_string()))?
}

pub(crate) async fn upload_file(client : &reqwest::Client, path: String, file_name : String, data : &[u8], no_overwrite : bool) -> Result<serde_json::Value, String> {
    let files_url = format!("{}/file/-/Files/{}", ISERV_BASE_URL, path);
    // Get upload_token
    let response = client.get(files_url)
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;

    let upload_token : String;
    {
        let document = scraper::Html::parse_document(&response.text().await.or_else(|e| Err(e.to_string()))?);
        let selector = scraper::selector::Selector::parse("#upload__token").unwrap();
        upload_token = document.select(&selector).next().unwrap().value().attr("value").unwrap().to_string();
    }
    let uuid = uuid::Uuid::new_v4().hyphenated().to_string();

    // Check
    if no_overwrite {
        let form_encoded = form_urlencoded::Serializer::new(String::new())
            .append_pair("files[]", &file_name)
            .append_pair("path", &path)
            .finish();

        let response = client.post(format!("{}/file/upload/check", ISERV_BASE_URL))
            .header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
            .header("X-Requested-With", "XMLHttpRequest")
            .body(form_encoded)
            .send()
            .await
            .or_else(|e| Err(e.to_string()))?;
        // If the file already exists throw an error
        response.json::<Vec<String>>().await
            .or_else(|e| Err(e.to_string()))
            .and_then(|r| if r.len() > 0 {Err(format!("File alread exsits"))} else {Ok(r)})?;
    }

    // Build request body form, containing the data with one chunk
    let multipart = reqwest::multipart::Form::new()
        .text("dzuuid", uuid)
        .text("dzchunkindex", "0")
        .text("dztotalfilesize", data.len().to_string())
        .text("dzchunksize", "2000000")
        .text("dztotalchunkcount", "1")
        .text("dzchunkbyteoffset", "0")
        .text("upload[path]", path)
        .text("upload[_token]", upload_token)
        .part("file", reqwest::multipart::Part::bytes(data.to_vec()).file_name(file_name).mime_str("application/octet-stream").unwrap());

    // Send Upload request
    let response = client.post(format!("{}/file/upload", ISERV_BASE_URL))
        .header("Content-Type", format!("multipart/form-data boundary={}", multipart.boundary()))
        .header("Accept","application/json")
        .header("Accept-Encoding", "gzip, deflate, br")
        .header("TE", "trailers")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("DNT", "1")
        .multipart(multipart)
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;
    let status_code = response.status();
    if !status_code.is_success() {
        return Err(format!("Status code was {}", status_code));
    }
    Ok(
        response.json().await
            .or_else(|e| Err(e.to_string()))?
    )
}

pub(crate) async fn download_data(client : &reqwest::Client, iserv_path: &String) -> Result<bytes::Bytes, String> {
    let download_url = format!("{}{}", ISERV_BASE_URL.replace("/iserv", ""), iserv_path);
    // Send Download request
    let response = client.get(download_url)
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;
    let status_code = response.status();
    if !status_code.is_success() {
        return Err(format!("Status code was {}", status_code));
    }
    let bytes = response.bytes().await
        .or_else(|e| Err(e.to_string()))?;
    Ok(bytes)
}

pub(crate) async fn delete_files(client : &reqwest::Client, path: &String, file_names : Vec<&String>) -> Result<(), String> {
    let path = path.replace("/iserv/file/-/", "");
    let files_url = format!("{}/file/-/{}", ISERV_BASE_URL, path);
    // Get form_token
    let response = client.get(files_url)
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;
    let form_token : String;
    {
        let document = scraper::Html::parse_document(&response.text().await.or_else(|e| Err(e.to_string()))?);
        let selector = scraper::selector::Selector::parse("#form__token").unwrap();
        form_token = document.select(&selector).next().unwrap().value().attr("value").unwrap().to_string();
    }

    let form_body : String;
    {
        // Create remove requests args
        let mut form_encoded_serializer = form_urlencoded::Serializer::new(String::new());
        for file_name in file_names {
            form_encoded_serializer.append_pair("form[files][]", &base64::encode(format!("{}/{}", path, file_name)));
        }
        form_body = form_encoded_serializer
            .append_pair("form[path]", &path)
            .append_pair("form[current_search]", "")
            .append_pair("form[confirm]", "1")
            .append_pair("form[action]",  "delete")
            .append_pair("form[actions][confirm]",  "")
            .append_pair("form[_token]",  &form_token)
            .finish();
    }

    // Send remove request
    let response = client.post(format!("{}/file/multiaction", ISERV_BASE_URL))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;
    let status_code = response.status();
    if !status_code.is_success() {
        return Err(format!("Status code was {}", status_code));
    }
    Ok(())
}

pub(crate) async fn create_folder_structure(client : &reqwest::Client, path: &String) -> Result<(), String> {
    let components = path.split("/").collect::<Vec<&str>>();
    for (i, component) in components.iter().enumerate() {
        if i == 0 {continue} // First component is always a folder that cannot be deleted so it makes no sense to recreate it
        match create_folder(client, &components[0..i].join("/"), &component.to_string()).await {
            Ok(_) => {},
            Err(s) => if !s.contains("error decoding response body") {return Err(s)}
        }
    }
    Ok(())
}

pub(crate) async fn create_folder(client : &reqwest::Client, path: &String, folder_name : &String) -> Result<(), String> {
    // Get file_factory_token
    let response = client.get(format!("{}/file/add/folder", ISERV_BASE_URL))
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;
    let file_factory_token : String;
    {
        let document = scraper::Html::parse_document(&response.text().await.or_else(|e| Err(e.to_string()))?);
        let selector = scraper::selector::Selector::parse("#file_factory__token").unwrap();
        file_factory_token = document.select(&selector).next().unwrap().value().attr("value").unwrap().to_string();
    }
    // Construct request data
    let form_encoded = form_urlencoded::Serializer::new(String::new())
        .append_pair("file_factory[item][name]", &folder_name)
        .append_pair("file_factory[path]", &path)
        .append_pair("file_factory[_token]", &file_factory_token)
        .append_pair("file_factory[submit]", "")
        .finish();
    // Send request to create folder
    let response = client.post(format!("{}/file/add/folder", ISERV_BASE_URL))
        .header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
        .header("X-Requested-With", "XMLHttpRequest")
        .body(form_encoded)
        .send()
        .await
        .or_else(|e| Err(e.to_string()))?;
    // Check if operation was successful
    let status_code = response.status();
    if !status_code.is_success() {
        return Err(format!("Status code was {}", status_code));
    }
    let json_response : serde_json::Map<String, serde_json::Value> = response.json().await.or_else(|e| Err(e.to_string()))?;
    if json_response.get("status").ok_or("No such key: status".to_string())?.as_str().ok_or("status is not a string".to_string())? == "success" {
        Ok(())
    } else {
        Err("Status was not success".to_string())
    }

}

#[cfg(test)]
mod test {
    use crate::load_credentials;
    use super::*;

    #[tokio::test]
    async fn test_upload_download() {
        let client = get_iserv_client(load_credentials()).await.unwrap();
        let data = include_bytes!("test_req");
        let mut total_time = tokio::time::Duration::new(0, 0);
        let runs = 10;
        create_folder_structure(&client, &"Files/Downloads/test".to_string()).await.expect("Could not create test directory");
        for _ in 0..runs {
            let start = tokio::time::Instant::now();
            upload_file(&client, "Files/Downloads/test".to_string(), "test.txt".to_string(), data, false).await.unwrap();
            let files = get_files(&client, "Files/Downloads/test".to_string()).await.unwrap();
            let file_path = files.files.first().expect("File was not uploaded").name.link.replace("?show=true", "");
            let downloaded_data = download_data(&client, &file_path).await.unwrap();
            let dur = start.elapsed();
            println!("Download and upload of {}kb took: {}ms", data.len() as f64 / 1000.0, dur.as_millis());
            total_time = total_time + dur;
            assert_eq!(String::from_utf8_lossy(downloaded_data.as_ref()), String::from_utf8_lossy(data));
        }
        let avg_download_and_upload_time = total_time.as_secs_f64() / runs as f64;
        let size_kb = data.len() as f64 / 1000.0;
        println!("Download and upload of {}kb took: {}s; That makes : {:.3}kb/s", size_kb, avg_download_and_upload_time, size_kb / avg_download_and_upload_time);
        delete_files(&client, &"Files/Downloads/".to_string(), vec![&"test".to_string()]).await.expect("Could not remove test directory");
    }

    #[tokio::test]
    async fn test_create_folder() {
        let client = get_iserv_client(load_credentials()).await.unwrap();
        create_folder_structure(&client, &"Files/Downloads/test/test2/test3/test4".to_string()).await.unwrap();
        delete_files(&client, &"Files/Downloads/test/test2/test3".to_string(), vec![&"test4".to_string()]).await.unwrap();
        delete_files(&client, &"Files/Downloads/".to_string(), vec![&"test".to_string()]).await.expect("Could not remove test directory");
    }

}