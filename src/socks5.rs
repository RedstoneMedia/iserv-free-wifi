use std::net::{Ipv4Addr, Ipv6Addr};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn socks_handshake(stream : &mut TcpStream) -> bool {
    let mut buffer = [0u8; 3];
    stream.read_exact(&mut buffer).await.unwrap();
    if buffer[0] != 5 {
        stream.write(&[0, 255]).await.unwrap();
        stream.shutdown().await.unwrap();
        return false;
    }
    stream.write(&[5, 0]).await.unwrap();
    true
}

#[derive(Debug)]
pub enum SocksCommandType {
    ConnectTCP,
    BindTCP,
    AssociateUDP
}

impl SocksCommandType {
    pub fn to_bytes(&self) -> BytesMut {
        let mut bytes = BytesMut::new();
        match self {
            SocksCommandType::ConnectTCP => {
                bytes.put_u8(1);
            },
            SocksCommandType::BindTCP => {
                bytes.put_u8(2);
            },
            SocksCommandType::AssociateUDP => {
                bytes.put_u8(3);
            },
        }
        bytes
    }

    pub fn from_bytes(bytes: &mut Bytes) -> Self {
        let cmd_type = bytes.get_u8();
        match cmd_type {
            1 => SocksCommandType::ConnectTCP,
            2 => SocksCommandType::BindTCP,
            3 => SocksCommandType::AssociateUDP,
            _ => panic!()
        }
    }
}

#[derive(Debug)]
pub enum SocksAddress {
    IPv4(Ipv4Addr),
    DomainName(String),
    IPv6(Ipv6Addr)
}

impl SocksAddress {
    pub fn to_bytes(&self) -> BytesMut {
        let mut bytes = BytesMut::new();
        match self {
            SocksAddress::IPv4(a) => {
                bytes.put_u8(1);
                bytes.extend_from_slice(&a.octets());
            },
            SocksAddress::DomainName(n) => {
                bytes.put_u8(2);
                bytes.put_u16(n.len() as u16);
                bytes.extend_from_slice(n.as_bytes());
            },
            SocksAddress::IPv6(a) => {
                bytes.put_u8(3);
                bytes.extend_from_slice(&a.octets());
            },
        }
        bytes
    }

    pub fn from_bytes(bytes: &mut Bytes) -> Self {
        let addr_type = bytes.get_u8();
        match addr_type {
            1 => {
                let mut octets = [0u8; 4];
                bytes.take(4).copy_to_slice(&mut octets);
                SocksAddress::IPv4(Ipv4Addr::from(octets))
            },
            2 => {
                let len = bytes.get_u16();
                let bytes_vec = bytes.take(len as usize).copy_to_bytes(len as usize).to_vec();
                let s = &String::from_utf8(bytes_vec).unwrap().to_string();
                bytes.take(len as usize);
                Self::DomainName(s.clone())
            },
            3 => {
                let mut octets = [0u8; 16];
                bytes.take(16).copy_to_slice(&mut octets);
                SocksAddress::IPv6(Ipv6Addr::from(octets))
            }
            _ => panic!()
        }
    }
}

#[derive(Debug)]
pub struct SocksRequest {
    pub command_type : SocksCommandType,
    pub address : SocksAddress,
    pub port : u16
}

impl SocksRequest {
    pub fn to_bytes(&self) -> BytesMut {
        let mut bytes = BytesMut::new();
        bytes.extend(self.command_type.to_bytes());
        bytes.extend(self.address.to_bytes());
        bytes.put_u16(self.port);
        bytes
    }

    pub fn from_bytes(bytes : &mut Bytes) -> Self {
        Self {
            command_type: SocksCommandType::from_bytes(bytes),
            address: SocksAddress::from_bytes(bytes),
            port: bytes.get_u16()
        }
    }
}

pub async fn read_socks_request(stream : &mut TcpStream) -> SocksRequest {
    let mut buffer = [0u8; 4];
    stream.read_exact(&mut buffer).await.unwrap();
    let command_type = match buffer[1] {
        1 => SocksCommandType::ConnectTCP,
        2 => SocksCommandType::BindTCP,
        3 => SocksCommandType::AssociateUDP,
        _ => panic!()
    };
    assert_eq!(buffer[2], 0u8);
    let address = match buffer[3] {
        1 => {
            stream.read_exact(&mut buffer).await.unwrap();
            SocksAddress::IPv4(Ipv4Addr::from(buffer.clone()))
        },
        3 => {
            let mut length_buff = [0u8; 1];
            stream.read_exact(&mut length_buff).await.unwrap();
            let mut domain_string_buff = vec![0u8; length_buff[0] as usize];
            stream.read_exact(&mut domain_string_buff).await.unwrap();
            SocksAddress::DomainName(String::from_utf8_lossy(domain_string_buff.as_slice()).to_string())
        },
        4 => {
            let mut ipv6_buffer = [0u8; 16];
            stream.read_exact(&mut ipv6_buffer).await.unwrap();
            SocksAddress::IPv6(Ipv6Addr::from(ipv6_buffer))
        }
        _ => panic!()
    };
    stream.read(&mut buffer).await.unwrap();
    let mut bytes = Bytes::from(buffer.to_vec());
    let port = bytes.get_u16();
    SocksRequest {
        command_type,
        address,
        port
    }
}