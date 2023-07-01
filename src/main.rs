mod iserv;
mod socks5;
mod secure;
mod server;
mod client;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use rand::{Rng, RngCore, SeedableRng};
use reqwest::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tokio::sync::RwLock;
use tokio::sync::mpsc::error::TryRecvError;
use once_cell::sync::{Lazy, OnceCell};

type Senders = Arc<RwLock<HashMap<u128, Sender<Bytes>>>>;

const BASE_PORT : u16 = 3182;
const ISERV_BASE_DATA_DIR : &str = "Files/Downloads";
static AES_KEY : OnceCell<String> = OnceCell::new();
const ERROR_PREFIX : &str = "|!!-ISERV PROXY ERROR-!!|:";
const BUNDLE_SIZE : usize = 64;
const FOLDER_NAME_OFFSET : u64 = 42069;
const FOLDER_CHANGE_MINUTES : u16 = 60;

static ISERV_DATA_DIRS: Lazy<tokio::sync::Mutex<Vec<String>>> = Lazy::new(|| {
    tokio::sync::Mutex::new(vec![])
});


async fn get_data_from_iserv(client : &Client, from_server : bool, delete_files : &Vec<(String, String)>) -> (Vec<Bytes>, Vec<(String, String)>) {
    let mut acceptable_files = vec![];
    {
        iserv_data_dir(&client).await; // Just to update ISERV_DATA_DIRS
        let iserv_data_dirs = ISERV_DATA_DIRS.lock().await;
        for iserv_data_dir in iserv_data_dirs.iter() {
            acceptable_files.append(&mut match iserv::get_files(&client, iserv_data_dir.clone()).await {
                Ok(response) => response.files,
                Err(e) => {eprintln!("{}", e); vec![]}
            });
        }
    }
    if acceptable_files.is_empty() { return (vec![], vec![]); }
    acceptable_files.retain(|f| {
        let current_unix_time = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap();
        let file_creation_time = std::time::Duration::from_micros(f.name.text[1..].replace(".tmp", "").parse().unwrap());
        let is_new = (current_unix_time.as_secs() as i128 - file_creation_time.as_secs() as i128) < 90;
        let marked_as_delete = delete_files.iter().find(|df| &df.0 == f.path.get("link").unwrap() && df.1 == f.name.text).is_some();
        match from_server {
            false => f.name.text.starts_with("0") && is_new && f.name.text.ends_with(".tmp") && !marked_as_delete,
            true => f.name.text.starts_with("1") && is_new && f.name.text.ends_with(".tmp") && !marked_as_delete
        }
    });

    let mut delete_files = Vec::new();
    let mut output_handles = Vec::with_capacity(acceptable_files.len());
    for f in acceptable_files {
        let file_path = f.path.get("link").unwrap().clone();
        let file_name = f.name.text.clone();
        delete_files.push((file_path.clone(), file_name.clone()));
        let client_copy = client.clone();
        output_handles.push(tokio::spawn(async move {
            let file_url= f.name.link.replace("?show=true", "");
            let mut data = match iserv::download_data(&client_copy, &file_url).await {
                Ok(data) => data,
                Err(e) => {eprintln!("{}", e); return None}
            };
            // Read iv from msg
            let mut iv = [0u8; 16];
            data.copy_to_slice(&mut iv);
            // Decrypt msg
            let plain_data = secure::decrypt(data.as_ref(), AES_KEY.get().unwrap(), &iv).unwrap();
            Some(Bytes::from(plain_data))
        }));
    }
    // Wait until all downloads are complete
    let mut output_data = Vec::with_capacity(output_handles.len());
    for output_handle in output_handles {
        let value_option = output_handle.await.unwrap();
        if value_option.is_some() {
            let mut value = value_option.unwrap();
            while value.remaining() > 0 {
                let data_size = value.get_u32();
                let data = value.copy_to_bytes(data_size as usize);
                output_data.push(data);
            }
        }
    }
    (output_data, delete_files)
}

async fn write_data_to_iserv(client : &Client, data : &Vec<Bytes>, from_server : bool, iserv_data_dir : &str) {
    let ms_current = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros();
    let file_name = match from_server {
        false => format!("0{}.tmp", ms_current),
        true => format!("1{}.tmp", ms_current)
    };
    let mut bytes = BytesMut::new();
    for d in data {
        bytes.put_u32(d.len() as u32);
        bytes.extend(d);
    }
    // Encrypt msg
    let (cypher, iv) = secure::encrypt(bytes.as_ref(), AES_KEY.get().unwrap()).unwrap();
    bytes.clear();
    bytes.extend(&iv);
    bytes.extend(cypher);
    // Upload msg
    for i in 0..3 {
        match iserv::upload_file(&client, iserv_data_dir.to_string(), file_name.clone(), bytes.as_ref(), false).await {
            Ok(_) => break,
            Err(e) => eprintln!("Retry {} because of error: {}", i, e)
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100*i)).await;
    }

}

async fn write_bundler(client : Client, from_server : bool, mut receiver: tokio::sync::mpsc::Receiver<Bytes>) {
    let mut messages_buffer : Vec<Bytes> = Vec::with_capacity(BUNDLE_SIZE);
    let mut start_time = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
        if (messages_buffer.len() >= BUNDLE_SIZE || start_time.elapsed().as_millis() >= 100) && !messages_buffer.is_empty() {
            let from_text = match from_server {
                true => "Server",
                false => "Client"
            };
            println!("[{}] Request Bundler sending : {} requests", from_text, messages_buffer.len());
            write_data_to_iserv(&client, &messages_buffer, from_server, &iserv_data_dir(&client).await).await;
            start_time = tokio::time::Instant::now();
            messages_buffer.clear();
        }
        // Read all messages until the receiver is empty
        loop {
            messages_buffer.push(match receiver.try_recv() {
                Ok(v) => v,
                Err(e) => match e
                {
                    TryRecvError::Empty => break,
                    TryRecvError::Disconnected => break
                }
            });
        }
    }
}

/// Tries to read from a list of files and bulk deletes them. After the deletion is done the deleted files are removed from the list.
async fn delete_handler(client : Client, to_delete_files : Arc<tokio::sync::RwLock<Vec<(String, String)>>>) {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let delete_files_read = to_delete_files.read().await;
        let to_delete_files_copy = delete_files_read.to_vec();
        std::mem::drop(delete_files_read);
        if to_delete_files_copy.len() > 1 {
            // Split files by path
            let mut to_delete_files_split : Vec<(String, Vec<&String>)> = vec![];
            for file_to_delete in &to_delete_files_copy {
                match to_delete_files_split.iter_mut().find(|p| &p.0 == &file_to_delete.0) {
                    Some(path_files) => {
                        path_files.1.push(&file_to_delete.1);
                    },
                    None => {
                        to_delete_files_split.push((file_to_delete.0.clone(), vec![&file_to_delete.1]))
                    }
                }
            }
            println!("Deleting: {:?}", to_delete_files_split);
            for path_files in to_delete_files_split {
                let path = path_files.0;
                for i in 0..3 {
                    match iserv::delete_files(&client,
                                              &path, path_files.1.clone()).await
                    {
                        Ok(_) => break,
                        Err(e) => println!("Retrying delete {} because of error : {}", i, e)
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100*i)).await;
                }
            }
            // Remove deleted files from to_delete_files
            let mut to_delete_files_write = to_delete_files.write().await;
            to_delete_files_write.retain(|v| !to_delete_files_copy.contains(v));
        } else {continue}
    }
}

async fn relay_to(domain: &str, dst_port: u16) {
    let mut stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), BASE_PORT)).await.unwrap();
    // Create listener
    let mut port: u16 = dst_port;
    let listener = loop {
        match tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port)).await {
            Ok(l) => break l,
            Err(_) => {}
        }
        port = rand::thread_rng().gen_range(49152..65535);
    };
    // Do handshake
    let mut buffer = [0u8; 8000];
    stream.write(&[5, 0, 0]).await.unwrap();
    stream.read(&mut buffer).await.unwrap();
    let mut request_packet = BytesMut::new();
    request_packet.extend(&[5, 1, 0, 3]);
    request_packet.put_u8(domain.len() as u8);
    request_packet.extend(domain.as_bytes());
    request_packet.put_u16(dst_port);
    // Send socks request packet
    stream.write(request_packet.as_ref()).await.unwrap();
    stream.read(&mut buffer).await.unwrap();
    println!("[Relay] Relay to {}:{} is available on 127.0.0.1:{}", domain, dst_port, port);
    // Relay data to socks client
    if let Ok((relay_stream, _)) = listener.accept().await {
        let (mut relay_stream_read_half, mut relay_stream_write_half) = relay_stream.into_split();
        let (mut stream_read_half, mut stream_write_half) = stream.into_split();
        // Relay from local application to socks client
        let a = tokio::spawn(async move {
            loop {
                let mut buffer = [0u8; 8000];
                let size = relay_stream_read_half.read(&mut buffer).await.unwrap();
                if size == 0 { break; }
                stream_write_half.write(&buffer).await.unwrap();
            }
        });
        // Relay from socks client to local application
        let b = tokio::spawn(async move {
            loop {
                let mut buffer = [0u8; 8000];
                let size = stream_read_half.read(&mut buffer).await.unwrap();
                if size == 0 { break; }
                relay_stream_write_half.write(&buffer).await.unwrap();
            }
        });
        a.await.unwrap();
        b.await.unwrap();
    }
    println!("[Relay] Stopped");
}

pub fn load_credentials() -> (String, String) {
    let creds_path = std::path::Path::new("credentials.txt");
    if !creds_path.is_file() {
        panic!("Cannot find {:?}", creds_path);
    }
    let creds_string = std::fs::read_to_string("credentials.txt").unwrap();
    let creds_list : Vec<&str> = creds_string.split("\n").collect();
    AES_KEY.get_or_init(|| {
        creds_list.get(2).expect("Credentials did not contain AES key").to_string().replace("\r", "")
    });
    (creds_list[0].to_string().replace("\r", ""), creds_list[1].to_string().replace("\r", ""))
}

#[allow(unused_must_use)]
async fn cleanup(client : &Client) {
    let iserv_data_dirs = ISERV_DATA_DIRS.lock().await;
    for iserv_data_dir in iserv_data_dirs.iter() {
        let files = iserv::get_files(&client,iserv_data_dir.clone()).await.unwrap();
        let file_names : Vec<&String> = files.files.iter().map(|f| &f.name.text).collect();
        if file_names.len() > 1 {
            let file_path = files.files[0].path.get("link").unwrap().clone();
            iserv::delete_files(client, &file_path, file_names).await;
        }
    }
}

// IServ Data Directory stuff

pub fn get_time_dependant_folder_name(offset: u64) -> String {
    let seed = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_secs() / (60 * FOLDER_CHANGE_MINUTES as u64);
    let mut rng = rand::rngs::StdRng::seed_from_u64(offset + seed);
    let mut n = rng.next_u32().to_string();
    let mut replacements = ["con", "down", "fig", "ja", "va", "ta", "bin", "up", "load", "date"].map(|s| s.to_string());
    for _ in 0..3 { replacements[rng.gen_range(0..10)] += "-"; }
    for (i, r) in replacements.iter().enumerate() {
        n = n.replace(&i.to_string(), &r);
    }
    return n.trim_end_matches("-").replace("--", "-").to_string();
}

#[allow(unused_must_use)]
async fn remove_iserv_data_dir(client : &Client, iserv_data_dir : &str) {
    iserv::get_files(&client, iserv_data_dir.to_string()).await.unwrap(); // Just to make sure that the directory actually exists
    let iserv_data_dir_split = iserv_data_dir.split("/").collect::<Vec<&str>>();
    // Delete folder
    iserv::delete_files(
        client,
        &iserv_data_dir_split[0..iserv_data_dir_split.len()-1].join("/"),
        vec![&iserv_data_dir_split.last().unwrap().to_string()]
    ).await.or_else(|_| {eprintln!("Could not remove folder: {}", iserv_data_dir); Err(())});
}

/// Gets the current iserv data dir and creates new ones if it is necessary
async fn iserv_data_dir(client: &Client) -> String {
    let folder_name = get_time_dependant_folder_name(FOLDER_NAME_OFFSET);
    let iserv_data_dir = format!("{}/{}", ISERV_BASE_DATA_DIR, folder_name);
    let mut iserv_data_dirs = ISERV_DATA_DIRS.lock().await;
    if iserv_data_dirs.contains(&iserv_data_dir) {
        return iserv_data_dir;
    } else if iserv_data_dirs.len() == 2 {
        let old_dir = iserv_data_dirs.remove(0);
        println!("[*] Removing old folder: {}", old_dir);
        remove_iserv_data_dir(client, &old_dir).await;
    }
    println!("[*] Creating new folder: {}", iserv_data_dir);
    for i in 0..3 {
        match iserv::create_folder_structure(&client, &iserv_data_dir).await {
            Ok(_) => break,
            Err(e) => {eprintln!("Cannot create folder: {} Try: {}/3", e, i+1)}
        }
    }
    iserv_data_dirs.push(iserv_data_dir.clone());
    iserv_data_dir
}

fn main() {

    let matches = clap::App::new("IServ Free WiFi")
        .arg(clap::Arg::with_name("server")
            .short('s')
            .long("server")
            .takes_value(false))
        .arg(clap::Arg::with_name("client")
            .short('c')
            .long("client")
            .takes_value(false))
        .arg(clap::Arg::with_name("iserv_base")
            .short('i')
            .long("iserv-base")
            .help("The base url for your IServ instance, usually https://<DOMAIN>/iserv")
            .required(true)
            .takes_value(true)
        )
        .arg(clap::Arg::with_name("relay")
            .short('r')
            .long("relay")
            .required(false)
            .help("Sets up a local relay to forward data to the specified domain")
            .value_name("relay domain")
            .takes_value(true))
        .group(clap::ArgGroup::new("mode")
            .required(true)
            .args(&["server", "client", "relay"])
        )
        .arg(clap::Arg::with_name("port")
            .short('p')
            .long("port")
            .required(false)
            .value_name("port")
            .takes_value(true))
        .get_matches();

    let Some(iserv_base_url) = matches.value_of("iserv_base") else {panic!("IServ base url was not provided")};
    iserv::ISERV_BASE_URL.set(iserv_base_url.to_string()).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16777216)
        .build()
        .expect("Cannot build tokio runtime");
    runtime.block_on(async move {
        if let Some(domain) = matches.value_of("relay") {
            let dst_port: u16 = matches.value_of("port").unwrap().parse().unwrap();
            let c = tokio::spawn(async {
                client::client().await;
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            relay_to(domain, dst_port).await;
            c.abort();
            return;
        }

        match (matches.is_present("server"), matches.is_present("client")) {
            (true, false) => {
                loop {
                    let s = tokio::spawn(async {
                        server::server().await;
                    });
                    let result = s.await;
                    match result {
                        Ok(_) => {break},
                        Err(e) => eprintln!("[Server] SERVER exited on error: {}, restarting in one Minute", e)
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                }
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
            _ => unreachable!()
        }
    });
}