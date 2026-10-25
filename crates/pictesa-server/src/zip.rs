//! Streaming ZIP32 with stored media and data descriptors (bounded to 1 GiB by the engine).
use std::io;

use axum::body::Body;
use pictesa_engine::library::ZipEntry;
use tokio::{io::AsyncReadExt, sync::mpsc};
use tokio_stream::wrappers::ReceiverStream;

type Sender = mpsc::Sender<io::Result<Vec<u8>>>;

fn u16le(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn u32le(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

async fn send(tx: &Sender, bytes: Vec<u8>) -> io::Result<()> {
    tx.send(Ok(bytes)).await.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Download closed"))
}

async fn write(tx: &Sender, entries: Vec<ZipEntry>) -> io::Result<()> {
    let mut offset = 0u32;
    let mut directory = Vec::new();
    let count = entries.len() as u16;
    for mut entry in entries {
        let name = entry.name.as_bytes();
        let start = offset;
        let mut header = Vec::new();
        u32le(&mut header, 0x04034b50);
        u16le(&mut header, 20); // version
        u16le(&mut header, 0x0808); // UTF-8, trailing data descriptor
        u16le(&mut header, 0); // stored
        u16le(&mut header, 0); // time
        u16le(&mut header, 33); // 01.01.1980
        for _ in 0..3 {
            u32le(&mut header, 0);
        }
        u16le(&mut header, name.len() as u16);
        u16le(&mut header, 0);
        header.extend_from_slice(name);
        offset += header.len() as u32;
        send(tx, header).await?;
        let mut crc = crc32fast::Hasher::new();
        let mut left = entry.size;
        while left > 0 {
            let mut chunk = vec![0; left.min(64 * 1024) as usize];
            entry.file.read_exact(&mut chunk).await?;
            crc.update(&chunk);
            left -= chunk.len() as u64;
            offset += chunk.len() as u32;
            send(tx, chunk).await?;
        }
        let crc = crc.finalize();
        let size = entry.size as u32;
        let mut descriptor = Vec::new();
        u32le(&mut descriptor, 0x08074b50);
        u32le(&mut descriptor, crc);
        u32le(&mut descriptor, size);
        u32le(&mut descriptor, size);
        offset += descriptor.len() as u32;
        send(tx, descriptor).await?;

        u32le(&mut directory, 0x02014b50);
        u16le(&mut directory, 20); // creator
        u16le(&mut directory, 20);
        u16le(&mut directory, 0x0808);
        u16le(&mut directory, 0);
        u16le(&mut directory, 0);
        u16le(&mut directory, 33);
        u32le(&mut directory, crc);
        u32le(&mut directory, size);
        u32le(&mut directory, size);
        u16le(&mut directory, name.len() as u16);
        for _ in 0..4 {
            u16le(&mut directory, 0);
        }
        u32le(&mut directory, 0);
        u32le(&mut directory, start);
        directory.extend_from_slice(name);
    }
    let dir_size = directory.len() as u32;
    send(tx, directory).await?;
    let mut end = Vec::new();
    u32le(&mut end, 0x06054b50);
    u16le(&mut end, 0);
    u16le(&mut end, 0);
    u16le(&mut end, count);
    u16le(&mut end, count);
    u32le(&mut end, dir_size);
    u32le(&mut end, offset);
    u16le(&mut end, 0);
    send(tx, end).await
}

pub fn body(entries: Vec<ZipEntry>) -> Body {
    let (tx, rx) = mpsc::channel(2);
    tokio::spawn(async move {
        if let Err(error) = write(&tx, entries).await {
            let _ = tx.send(Err(error)).await;
        }
    });
    Body::from_stream(ReceiverStream::new(rx))
}
