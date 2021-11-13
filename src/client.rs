use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use rand::Rng;
use reqwest::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc::{Receiver};
use tokio::sync::{RwLock};
use crate::{BASE_PORT, ERROR_PREFIX, get_data_from_iserv, iserv, Senders, write_data_to_iserv};

use crate::socks5::{read_socks_request, socks_handshake, SocksAddress, SocksCommandType, SocksRequest};
const USE_DIRECT : bool = false;


async fn tcp_relay_iserv(mut client_read : OwnedReadHalf, mut client_write : OwnedWriteHalf, request : SocksRequest, mut receiver: Receiver<Bytes>, id : u128, client : Client) {
    // Relay data from client to server
    let a = tokio::spawn(async move {
        loop {
            let mut buffer = [0u8; 8000];
            let read_size = client_read.read(&mut buffer).await.unwrap();
            if read_size == 0 {return;}
            let mut packet = BytesMut::new();
            packet.put_u128(id);
            packet.extend(request.to_bytes());
            packet.extend(&buffer[..read_size]);
            println!("[Client] got request: {}kb", packet.len() as f64 / 1000.0);
            write_data_to_iserv(&client, packet.as_ref(), false).await;
        }
    });

    // Relay data from server to client
    tokio::spawn(async move {
        loop {
            let mut data = receiver.recv().await.unwrap();
            if data.len() > 2 {
                let possible_error_message_size = data.get_u16();
                if data.len() == possible_error_message_size as usize {
                    let possible_error_message_string = String::from_utf8_lossy(data.as_ref()).to_string();
                    if possible_error_message_string.starts_with(ERROR_PREFIX) {
                        println!("[Client] server error : {:?}", possible_error_message_string.replace(ERROR_PREFIX, ""));
                        client_write.shutdown().await.unwrap();
                        return;
                    }
                }
                let mut new_bytes = BytesMut::new();
                new_bytes.put_u16(possible_error_message_size);
                new_bytes.extend(data);
                data = new_bytes.freeze();
            }
            client_write.flush().await.unwrap();
            println!("[Client] got response: {}kb", data.len() as f64 / 1000.0);
            client_write.write(data.as_ref()).await.unwrap();
        }
    });
    #[allow(unused_must_use)]
    a.await.unwrap();
}

async fn tcp_relay_direct(mut client_read : OwnedReadHalf, mut client_write : OwnedWriteHalf, request : SocksRequest) {
    let (mut target_read, mut target_write) = crate::server::server_get_tcp_target_halfs(&request).await.unwrap();

    // Relay data from client to target
    let a = tokio::spawn(async move {
        loop {
            let mut buffer = [0u8; 8000];
            let read_size = client_read.read(&mut buffer).await.unwrap();
            if read_size == 0 {return;}
            target_write.write(&buffer[..read_size]).await.unwrap();
        }
    });

    // Relay data from target to client
    let b = tokio::spawn(async move {
        loop {
            let mut buffer = [0u8; 8000];
            let read_size = target_read.read(&mut buffer).await.unwrap();
            if read_size == 0 {return;}
            let data = &buffer[..read_size];
            client_write.flush().await.unwrap();
            println!("[Client] got response: {:?}", Bytes::from(data.to_vec()));
            client_write.write(data).await.unwrap();
        }
    });
    a.await.unwrap();
    b.await.unwrap();
}

async fn execute_socks_request(request : SocksRequest, mut stream : TcpStream, current_port : u16, receiver: Receiver<Bytes>, id : u128, client : Client) {
    match request.command_type {
        SocksCommandType::ConnectTCP => {
            // Write success response
            let mut bytes = BytesMut::new();
            bytes.put_u8(5);
            bytes.put_u8(0);
            bytes.put_u8(0);
            bytes.put_u8(1);
            bytes.put_u8(1);
            bytes.put_u8(172);
            bytes.put_u8(0);
            bytes.put_u8(0);
            bytes.put_u8(1);
            bytes.put_u8(current_port as u8);
            stream.write(bytes.as_ref()).await.unwrap();

            let (client_read, client_write) = stream.into_split();

            if !USE_DIRECT {
                tcp_relay_iserv(client_read, client_write, request, receiver, id, client).await;
            } else {
                tcp_relay_direct(client_read, client_write, request).await;
            }
        },
        SocksCommandType::BindTCP => {
            stream.write(&[5, 7, 0, 0]).await.unwrap();
            stream.shutdown().await.unwrap();
        },
        SocksCommandType::AssociateUDP => {
            stream.write(&[5, 7, 0, 0]).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    }
}

pub async fn client() {
    let recv_senders : Senders = Arc::new(RwLock::new(HashMap::new())); // Maybe use generational arena instead of hash map
    let senders_clone = recv_senders.clone();
    let client = iserv::get_iserv_client().await.unwrap();
    let client_copy = client.clone();
    println!("[Client] listening");
    // Try to get data from server and send it to the correct sender
    tokio::spawn(async move {
        loop {
            let random_wait_time = rand::thread_rng().gen_range(100..500);
            tokio::time::sleep(tokio::time::Duration::from_millis(random_wait_time)).await;
            for mut data in get_data_from_iserv(&client_copy, true).await {
                let id = data.get_u128();
                let id_to_remove = match senders_clone.read().await.get(&id) {
                    Some(sender) => {
                        match sender.send(data).await {
                            Ok(_) => {continue;}
                            Err(_) => {id}
                        }
                    },
                    None => { continue; }
                };
                senders_clone.write().await.remove(&id_to_remove).unwrap();
            }
        }
    });

    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), BASE_PORT)).await.unwrap();
    let mut counter = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_secs() as u128;
    while let Ok((mut stream, address)) = listener.accept().await {
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        recv_senders.write().await.insert(counter, sender);
        let client_copy = client.clone();
        tokio::spawn(async move {
            println!("[Client] Connection from : {}", address);
            if !socks_handshake(&mut stream).await {return}
            let request = read_socks_request(&mut stream).await;
            match &request.address {
                SocksAddress::DomainName(d) => {
                    if d.contains("firefox") || d.contains("mozilla") {
                        stream.write(&[5, 2, 0, 0]).await.unwrap();
                        stream.shutdown().await.unwrap();
                        return;
                    }
                },
                _ => {}
            }
            println!("[Client] socks request {:?}", request);
            execute_socks_request(request, stream, address.port(), receiver, counter, client_copy).await;
        });
        counter += 1;
    }
}