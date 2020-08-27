#![feature(generators, async_closure)]

use async_io_macros::async_read;
use futures::{executor::block_on, io::AsyncReadExt};

#[test]
fn smoke_read() {
    let read = async_read! {
        let mut bytes = &b"hello world"[..];
        while !bytes.is_empty() {
            futures::future::ready(()).await;
            yield |buffer| {
                let len = buffer.len().min(bytes.len());
                buffer[..len].copy_from_slice(&bytes[..len]);
                let (_, tail) = bytes.split_at(len);
                bytes = tail;
                Ok(len)
            };
        }
        Ok(())
    };

    futures::pin_mut!(read);
    let mut buffer = Vec::new();
    block_on(read.read_to_end(&mut buffer)).unwrap();
    assert_eq!(buffer, b"hello world");
}
