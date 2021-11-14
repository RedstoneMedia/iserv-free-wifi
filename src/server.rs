use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use rand::Rng;
use reqwest::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{RwLock};
use crate::socks5::{SocksAddress, SocksCommandType, SocksRequest};
use crate::{delete_handler, ERROR_PREFIX, get_data_from_iserv, iserv, load_credentials, Senders, write_bundler, write_data_to_iserv};

const BUFFER_SIZE : usize = 256000;

async fn server_send_error_message(client : Client, id : u128, msg : &str) {
    let mut packet = BytesMut::new();
    packet.put_u128(id);
    let error_msg = format!("{}{}", ERROR_PREFIX, msg);
    packet.put_u16(error_msg.len() as u16);
    packet.extend(error_msg.as_bytes());
    write_data_to_iserv(&client, &vec![packet.freeze()], true).await;
}

pub async fn server_get_tcp_target_halfs(request : &SocksRequest) -> Result<(OwnedReadHalf, OwnedWriteHalf), String> {
    let socket_addr = match request.address {
        SocksAddress::IPv4(a) => SocketAddr::new(a.into(), request.port),
        SocksAddress::DomainName(ref name) => {
            format!("{}:{}", name, request.port).to_socket_addrs().unwrap().next().unwrap()
        },
        SocksAddress::IPv6(a) => SocketAddr::new(a.into(), request.port)
    };

    let target_stream = match TcpStream::connect(socket_addr).await {
        Ok(s) => s,
        Err(e) => {
            return Err(e.to_string());
        }
    };

    let (target_read, target_write) = target_stream.into_split();
    Ok((target_read, target_write))
}


async fn server_answer_requests(request : SocksRequest, id : u128, mut receiver: Receiver<Bytes>, client : Client, response_bundler_sender : Sender<Bytes>) {
    match request.command_type {
        SocksCommandType::ConnectTCP => {
            println!("[Server {}] connecting to {:?}", id, request.address);
            let (mut target_read, mut target_write) = match server_get_tcp_target_halfs(&request).await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[Server {}] Connection error : {}", id, e);
                    server_send_error_message(client, id, &e.to_string()).await;
                    return;
                }
            };
            println!("[Server {}] connected to {:?}", id, request.address);

            // Relay data from target to client
            let a = tokio::spawn(async move {
                loop {
                    let mut buffer = [0u8; BUFFER_SIZE];
                    let read_size = match tokio::time::timeout(tokio::time::Duration::from_millis(90000), target_read.read(&mut buffer)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {server_send_error_message(client, id, &e.to_string()).await; return;},
                        Err(_) => {server_send_error_message(client, id, "Timeout").await; return;}
                    };
                    if read_size == 0 {
                        println!("[Server {}] close", id);
                        server_send_error_message(client, id, "Close").await; return;
                    }
                    let mut packet = BytesMut::new();
                    packet.put_u128(id);
                    packet.extend(&buffer[..read_size]);
                    println!("[Server {}] sending to client: {}kb", id, packet.len() as f64 / 1000.0);
                    response_bundler_sender.send(packet.freeze()).await.unwrap();
                }
            });

            // Relay data from client to target
            tokio::spawn(async move {
                loop {
                    let data = match receiver.recv().await {
                        Some(d) => d,
                        None => return
                    };
                    println!("[Server {}] got client data: {}kb", id, data.len() as f64 / 1000.0);
                    target_write.write(data.as_ref()).await.unwrap();
                }
            });
            #[allow(unused_must_use)]
            a.await.unwrap();
        },
        SocksCommandType::BindTCP => {},
        SocksCommandType::AssociateUDP => {}
    }
}

pub async fn server() {
    let read_senders: Senders = Arc::new(RwLock::new(HashMap::new()));
    let client = iserv::get_iserv_client(load_credentials()).await.unwrap();
    let mut last_request_time = tokio::time::Instant::now();
    println!("[Server] listening");

    // Bundle response from the target and send it to the client as one file
    let client_copy = client.clone();
    let (response_bundler_sender, response_bundler_receiver) = tokio::sync::mpsc::channel::<Bytes>(64);
    tokio::spawn(async move {
        write_bundler(client_copy, true, response_bundler_receiver).await;
    });

    let delete_files = Arc::new(RwLock::new(Vec::new()));
    let delete_files_clone = delete_files.clone();
    let client_copy = client.clone();
    tokio::spawn(async move {
        delete_handler(client_copy, delete_files_clone).await;
    });

    // Try to get data from client and send it to the correct sender
    loop {
        let random_wait_time = if last_request_time.elapsed().as_secs() > 30 {
            println!("[Server] sleeping");
            rand::thread_rng().gen_range(15000..17000)
        } else {
            rand::thread_rng().gen_range(300..800)
        };
        tokio::time::sleep(tokio::time::Duration::from_millis(random_wait_time)).await;
        let (data_vec, new_delete_files) = get_data_from_iserv(&client, false, &delete_files.read().await.to_vec()).await;
        delete_files.write().await.extend(new_delete_files);
        for mut data in data_vec {
            last_request_time = tokio::time::Instant::now();
            let id = data.get_u128();
            let request = SocksRequest::from_bytes(&mut data);
            let senders_reader = read_senders.read().await;
            let sender = match senders_reader.get(&id) {
                Some(s) => s,
                None => {
                    let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(256);
                    sender.send(data).await.unwrap();
                    std::mem::drop(senders_reader);
                    read_senders.write().await.insert(id, sender);
                    let client_copy = client.clone();
                    let read_senders_copy = read_senders.clone();
                    let response_bundler_sender_copy = response_bundler_sender.clone();
                    tokio::spawn(async move {
                        tokio::spawn(async move {
                            server_answer_requests(request, id, receiver, client_copy, response_bundler_sender_copy).await;
                        }).await;
                        read_senders_copy.write().await.remove(&id);
                    });
                    continue;
                }
            };
            sender.send(data).await.unwrap();
        }
    }
}