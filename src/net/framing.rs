use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::protocol::MAX_FRAME;

pub async fn write_msg<W, T>(writer: &mut W, message: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = postcard::to_allocvec(message).context("encoding message")?;
    if bytes.len() > MAX_FRAME {
        bail!("outgoing frame is too large ({} bytes)", bytes.len());
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_msg<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await.context("reading frame length")?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len == 0 {
        bail!("empty frame");
    }
    if len > MAX_FRAME {
        bail!("incoming frame is too large ({len} bytes)");
    }
    let mut buffer = vec![0u8; len];
    reader
        .read_exact(&mut buffer)
        .await
        .context("reading frame body")?;
    postcard::from_bytes(&buffer).context("decoding message")
}

/// Buffered frame reader whose `read` future is cancel-safe: dropping it never
/// loses bytes, so it can be raced against timers in `tokio::select!`.
pub struct MsgReader<R> {
    reader: R,
    buffer: Vec<u8>,
    start: usize,
}

impl<R: AsyncRead + Unpin> MsgReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::new(),
            start: 0,
        }
    }

    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<T> {
        loop {
            if let Some(message) = self.try_parse()? {
                return Ok(message);
            }
            let mut chunk = [0u8; 16 * 1024];
            let n = self.reader.read(&mut chunk).await?;
            if n == 0 {
                bail!("connection closed");
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    fn try_parse<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        let available = self.buffer.len() - self.start;
        if available < 4 {
            return Ok(None);
        }
        let header: [u8; 4] = self.buffer[self.start..self.start + 4]
            .try_into()
            .expect("four length bytes");
        let len = u32::from_be_bytes(header) as usize;
        if len == 0 {
            bail!("empty frame");
        }
        if len > MAX_FRAME {
            bail!("incoming frame is too large ({len} bytes)");
        }
        if available < 4 + len {
            return Ok(None);
        }
        let body = &self.buffer[self.start + 4..self.start + 4 + len];
        let message = postcard::from_bytes(body).context("decoding message")?;
        self.start += 4 + len;
        if self.start >= 64 * 1024 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::{ClientMsg, HostMsg};
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn roundtrips_messages_and_rejects_oversized_frames() {
        let (mut writer, mut reader) = tokio::io::duplex(64 * 1024);
        let messages = vec![
            ClientMsg::Hello {
                proto: 1,
                app_version: "test".to_string(),
            },
            ClientMsg::Info,
            ClientMsg::JobStart {
                tx: 7,
                printer_id: uuid::Uuid::new_v4(),
                client_job_ref: uuid::Uuid::new_v4(),
                doc_name: "doc".to_string(),
                len: 1234,
                sha256: [3u8; 32],
            },
            ClientMsg::JobData {
                tx: 7,
                data: vec![1, 2, 3, 4],
            },
            ClientMsg::Ping { nonce: 99 },
        ];
        for message in &messages {
            write_msg(&mut writer, message).await.unwrap();
            let decoded: ClientMsg = read_msg(&mut reader).await.unwrap();
            let reencoded = postcard::to_allocvec(&decoded).unwrap();
            let expected = postcard::to_allocvec(message).unwrap();
            assert_eq!(reencoded, expected);
        }

        let reply = HostMsg::JobDone { tx: 7, pages: 3 };
        write_msg(&mut writer, &reply).await.unwrap();
        let decoded: HostMsg = read_msg(&mut reader).await.unwrap();
        assert!(matches!(decoded, HostMsg::JobDone { pages: 3, .. }));

        writer
            .write_all(&((MAX_FRAME as u32 + 1).to_be_bytes()))
            .await
            .unwrap();
        assert!(read_msg::<_, HostMsg>(&mut reader).await.is_err());

        writer.write_all(&0u32.to_be_bytes()).await.unwrap();
        assert!(read_msg::<_, HostMsg>(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn buffered_reader_decodes_back_to_back_frames() {
        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let mut reader = MsgReader::new(reader);
        write_msg(&mut writer, &ClientMsg::Ping { nonce: 1 }).await.unwrap();
        write_msg(&mut writer, &ClientMsg::Ping { nonce: 2 }).await.unwrap();
        let first: ClientMsg = reader.read().await.unwrap();
        let second: ClientMsg = reader.read().await.unwrap();
        assert!(matches!(first, ClientMsg::Ping { nonce: 1 }));
        assert!(matches!(second, ClientMsg::Ping { nonce: 2 }));
    }

    #[tokio::test]
    async fn buffered_reader_survives_a_cancelled_read() {
        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let mut reader = MsgReader::new(reader);
        let message = ClientMsg::Ping { nonce: 7 };
        let body = postcard::to_allocvec(&message).unwrap();
        let mut framed = (body.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&body);
        let split = framed.len() - 1;
        writer.write_all(&framed[..split]).await.unwrap();

        let interrupted = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            reader.read::<ClientMsg>(),
        )
        .await;
        assert!(interrupted.is_err(), "a partial frame must not decode");

        writer.write_all(&framed[split..]).await.unwrap();
        let decoded: ClientMsg = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reader.read::<ClientMsg>(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(decoded, ClientMsg::Ping { nonce: 7 }));
    }
}
