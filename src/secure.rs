
use crypto::{ symmetriccipher, buffer, aes, blockmodes };
use crypto::buffer::{ ReadBuffer, WriteBuffer, BufferResult };
use crypto::digest::Digest;
use rand::RngCore;

use rand::rngs::OsRng;

fn key_string_to_bytes(key_string: &str) -> [u8; 32] {
    let mut key: [u8; 32] = [0; 32];
    let mut hasher = crypto::sha3::Sha3::sha3_256();
    hasher.input(key_string.as_bytes());
    hasher.result(&mut key);
    key
}


pub fn encrypt(data: &[u8], key_string: &str) -> Result<(Vec<u8>, [u8; 16]), symmetriccipher::SymmetricCipherError> {
    let key = key_string_to_bytes(key_string);
    let mut iv: [u8; 16] = [0; 16];
    OsRng.fill_bytes(&mut iv);

    let mut encryptor = aes::cbc_encryptor(
            aes::KeySize::KeySize256,
            &key,
            &iv,
            blockmodes::PkcsPadding);

    let mut final_result = Vec::<u8>::new();
    let mut read_buffer = buffer::RefReadBuffer::new(data);
    let mut buffer = [0; 4096];
    let mut write_buffer = buffer::RefWriteBuffer::new(&mut buffer);

    loop {
        let result = encryptor.encrypt(&mut read_buffer, &mut write_buffer, true)?;

        final_result.extend(write_buffer.take_read_buffer().take_remaining().iter().map(|&i| i));

        match result {
            BufferResult::BufferUnderflow => break,
            BufferResult::BufferOverflow => { }
        }
    }

    Ok((final_result, iv))
}

pub fn decrypt(encrypted_data: &[u8], key_string: &str, iv: &[u8]) -> Result<Vec<u8>, symmetriccipher::SymmetricCipherError> {
    let key = key_string_to_bytes(key_string);
    let mut decryptor = aes::cbc_decryptor(
            aes::KeySize::KeySize256,
            &key,
            iv,
            blockmodes::PkcsPadding);

    let mut final_result = Vec::<u8>::new();
    let mut read_buffer = buffer::RefReadBuffer::new(encrypted_data);
    let mut buffer = [0; 4096];
    let mut write_buffer = buffer::RefWriteBuffer::new(&mut buffer);

    loop {
        let result = decryptor.decrypt(&mut read_buffer, &mut write_buffer, true)?;
        final_result.extend(write_buffer.take_read_buffer().take_remaining().iter().map(|&i| i));
        match result {
            BufferResult::BufferUnderflow => break,
            BufferResult::BufferOverflow => { }
        }
    }

    Ok(final_result)
}