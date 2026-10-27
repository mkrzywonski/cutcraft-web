//! Framed, correlated worker calls. One writer owns stdin, and the reader always drains stdout
//! independently of HTTP clients. Cancellation has its own call/reply and cannot steal a reply.
use cutcraft_web_protocol::worker::{Call, Response, WorkerReply, WorkerRequest};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

const MAX_FRAME: usize = 128 * 1024 * 1024;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<WorkerReply>>>>;
enum Write {
    Call(Box<Call>),
    Close,
}

#[derive(Clone)]
pub(super) struct Pipe {
    writer: mpsc::Sender<Write>,
    pending: Pending,
    next: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}
impl Pipe {
    pub fn new(mut stdin: tokio::process::ChildStdin, stdout: tokio::process::ChildStdout) -> Self {
        let (writer, mut writes) = mpsc::channel(32);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let write_closed = closed.clone();
        let read_closed = closed.clone();
        let failed = pending.clone();
        tokio::spawn(async move {
            while let Some(message) = writes.recv().await {
                let Write::Call(call) = message else {
                    break;
                };
                let Ok(mut bytes) = serde_json::to_vec(&call) else {
                    break;
                };
                bytes.push(b'\n');
                if stdin.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            let mut failed = failed.lock().unwrap_or_else(|p| p.into_inner());
            write_closed.store(true, Ordering::Release);
            failed.clear();
        });
        let replies = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let result: Result<(), ()> = async {
                    loop {
                        let buffer = reader.fill_buf().await.map_err(|_| ())?;
                        if buffer.is_empty() {
                            return Err(());
                        }
                        let used = buffer
                            .iter()
                            .position(|b| *b == b'\n')
                            .map_or(buffer.len(), |at| at + 1);
                        if line.len() + used > MAX_FRAME {
                            return Err(());
                        }
                        let complete = buffer[used - 1] == b'\n';
                        line.extend_from_slice(&buffer[..used]);
                        reader.consume(used);
                        if complete {
                            break;
                        }
                    }
                    Ok(())
                }
                .await;
                if result.is_err() {
                    break;
                }
                let Ok(response) = serde_json::from_slice::<Response>(&line) else {
                    break;
                };
                if let Some(reply) = replies
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&response.id)
                {
                    let _ = reply.send(response.reply);
                }
            }
            let mut replies = replies.lock().unwrap_or_else(|p| p.into_inner());
            read_closed.store(true, Ordering::Release);
            replies.clear();
        });
        Self {
            writer,
            pending,
            next: Arc::new(AtomicU64::new(1)),
            closed,
        }
    }
    pub async fn begin(
        &self,
        request: WorkerRequest,
    ) -> Result<(u64, oneshot::Receiver<WorkerReply>), String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (send, reply) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            if self.closed.load(Ordering::Acquire) {
                return Err("the worker stopped".into());
            }
            pending.insert(id, send);
        }
        if self
            .writer
            .send(Write::Call(Box::new(Call { id, request })))
            .await
            .is_err()
        {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return Err("the worker stopped".into());
        }
        Ok((id, reply))
    }
    pub async fn ask(&self, request: WorkerRequest) -> Result<WorkerReply, String> {
        let (_, reply) = self.begin(request).await?;
        reply.await.map_err(|_| "the worker stopped".into())
    }
    pub async fn close(&self) {
        let _ = self.writer.send(Write::Close).await;
    }
}
