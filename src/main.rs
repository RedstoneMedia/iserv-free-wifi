mod iserv;
mod socks5;
mod secure;
mod server;
mod client;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use reqwest::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Sender};
use tokio::sync::{RwLock};

const BASE_PORT : u16 = 8181;
const ISERV_DATA_DIR : &str = "Files/Downloads/temp";
const AES_KEY : &str = "Balzing fast 🚀💨";
const ERROR_PREFIX : &str = "|!!-ISERV PROXY ERROR-!!|:";

async fn get_data_from_iserv(client : &Client, from_server : bool) -> Vec<Bytes> {
    let mut acceptable_files = match iserv::get_files(&client, ISERV_DATA_DIR.to_string()).await {
        Ok(response) => response.files,
        Err(e) => {eprintln!("{}", e); return vec![];}
    };
    acceptable_files.retain(|f| {
        match from_server {
            false => f.name.text.starts_with("0") && f.name.text.ends_with(".tmp"),
            true => f.name.text.starts_with("1") && f.name.text.ends_with(".tmp")
        }
    });

    let mut output_handles = Vec::with_capacity(acceptable_files.len());
    for f in acceptable_files {
        let client_copy = client.clone();
        output_handles.push(tokio::spawn(async move {
            let file_path= f.name.link.replace("?show=true", "");
            let mut data = match iserv::download_data(&client_copy, &file_path).await {
                Ok(data) => data,
                Err(e) => {eprintln!("{}", e); return None}
            };
            // Delete later
            tokio::spawn(async move {
                for i in 0..3 {
                    match iserv::delete_file(&client_copy, f.path.get("link").unwrap(), &f.name.text).await {
                        Ok(_) => break,
                        Err(e) => println!("Retry {} because of error : {}", i, e)
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100*i)).await;
                }
            }).await.unwrap();
            // Read iv from msg
            let mut iv = [0u8; 16];
            data.copy_to_bytes(16).copy_to_slice(&mut iv);
            // Decrypt msg
            let plain_data = secure::decrypt(data.as_ref(), AES_KEY, &iv).unwrap();
            Some(Bytes::from(plain_data))
        }));
    }
    // Wait until all downloads are complete
    let mut output_data = Vec::with_capacity(output_handles.len());
    for output_handle in output_handles {
        let value = output_handle.await.unwrap();
        if value.is_some() {
            output_data.push(value.unwrap());
        }
    }
    output_data
}

async fn write_data_to_iserv(client : &Client, data : &[u8], from_server : bool) {
    let ms_current = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros();
    let file_name = match from_server {
        false => format!("0{}.tmp", ms_current),
        true => format!("1{}.tmp", ms_current)
    };
    let mut bytes = BytesMut::with_capacity(data.len());
    // Encrypt msg
    let (cypher, iv) = secure::encrypt(data, AES_KEY).unwrap();
    bytes.extend(&iv);
    bytes.extend(cypher);
    // Upload msg
    for i in 0..3 {
        match iserv::upload_file(&client, ISERV_DATA_DIR.to_string(), file_name.clone(), bytes.as_ref(), false).await {
            Ok(_) => break,
            Err(e) => eprintln!("Retry {} because of error: {}", i, e)
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100*i)).await;
    }

}

type Senders = Arc<RwLock<HashMap<u128, Sender<Bytes>>>>;

#[tokio::main]
async fn main() {
    use std::io::Write;

    let matches = clap::App::new("IServ Free WiFi")
        .arg(clap::Arg::with_name("server")
            .short("s")
            .long("server")
            .takes_value(false))
        .arg(clap::Arg::with_name("client")
            .short("c")
            .long("client")
            .takes_value(false))
        .arg(clap::Arg::with_name("request")
            .short("r")
            .long("request")
            .required(false)
            .value_name("path")
            .takes_value(true))
        .arg(clap::Arg::with_name("domain")
            .short("d")
            .long("domain")
            .required(false)
            .value_name("domain")
            .takes_value(true))
        .get_matches();

    if let Some(path) = matches.value_of("request") {
        let domain = matches.value_of("domain").unwrap();
        let c = tokio::spawn(async {
            client::client().await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), BASE_PORT)).await.unwrap();
        let mut buffer = [0u8; 8000];
        stream.write(&[5, 0, 0]).await.unwrap();
        stream.read(&mut buffer).await.unwrap();

        // Do handshake
        let mut request_packet = BytesMut::new();
        request_packet.extend(&[5, 1, 0, 3]);
        request_packet.put_u8(domain.len() as u8);
        request_packet.extend(domain.as_bytes());
        request_packet.put_u16(80);
        // Send socks request packet
        stream.write(request_packet.as_ref()).await.unwrap();
        stream.read(&mut buffer).await.unwrap();

        // Connection is established now send data
        let request_data = std::fs::read(path).unwrap();
        stream.write(request_data.as_slice()).await.unwrap();
        // Write output to file
        let mut file = std::fs::File::create("response.txt").unwrap();
        loop {
            buffer = [0u8; 8000];
            let size = stream.read(&mut buffer).await.unwrap();
            if size == 0 {break;}
            std::fs::File::write(&mut file, &buffer[..size]).unwrap();
        }
        c.abort();
        return;
    }

    match (matches.is_present("server"), matches.is_present("client")) {
        (true, false) => {
            let s = tokio::spawn(async {
                server::server().await;
            });
            s.await.unwrap();
        },
        (false, true) => {
            let c = tokio::spawn(async {
                client::client().await;
            });
            c.await.unwrap();
        },
        (true, true) => {
            let s = tokio::spawn(async {
                server::server().await;
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            let c = tokio::spawn(async {
                client::client().await;
            });
            s.await.unwrap();
            c.await.unwrap();
        },
        (false, false) => println!("{}", matches.usage())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io::Write;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use bytes::BufMut;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn test_html_request() {
        let c = tokio::spawn(async {
            client::client().await;
        });

        let s = tokio::spawn(async {
            server::server().await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), BASE_PORT)).await.unwrap();
        let mut buffer = [0u8; 8000];
        stream.write(&[5, 0, 0]).await.unwrap();
        stream.read(&mut buffer).await.unwrap();

        let mut test_packet = BytesMut::new();
        test_packet.extend(&[5, 1, 0, 3]);
        let domain = "192.168.178.67";
        test_packet.put_u8(domain.len() as u8);
        test_packet.extend(domain.as_bytes());
        test_packet.put_u16(5000);

        stream.write(test_packet.as_ref()).await.unwrap(); // 19, 136
        stream.read(&mut buffer).await.unwrap();
        stream.write(include_bytes!("req")).await.unwrap();
        let mut file = std::fs::File::create("response.txt").unwrap();
        loop {
            buffer = [0u8; 8000];
            let size = stream.read(&mut buffer).await.unwrap();
            if size == 0 {break;}
            std::fs::File::write(&mut file, &buffer[..size]).unwrap();
        }

        s.abort();
        c.abort();
    }
}