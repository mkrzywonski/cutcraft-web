//! Framed, correlated worker calls. One writer owns stdin, and the reader always drains stdout
//! independently of HTTP clients. Cancellation has its own call/reply and cannot steal a reply.
use cutcraft_web_protocol::worker::{Call, Response, WorkerReply, WorkerRequest};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

const MAX_FRAME: usize = 128 * 1024 * 1024;
/// Export is read-only: an oversized reply can be refused without losing the worker or
/// pretending a mutation failed. Reserve enough framing space for any correlation id.
pub(super) fn bounded_export(reply: WorkerReply) -> WorkerReply {
    bound_export(reply, MAX_FRAME)
}

fn bound_export(reply: WorkerReply, limit: usize) -> WorkerReply {
    match serde_json::to_vec(&reply) {
        Ok(bytes) if bytes.len().saturating_add(64) <= limit => reply,
        _ => WorkerReply::Failed {
            message: "This export is too large to transfer. The design is still open; use a smaller design or a different export format.".into(),
        },
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn maximum_original_download_fits_the_worker_frame_limit() {
        // Byte arrays encoded as JSON integers can exceed this boundary for a valid upload.
        let data_base64 = base64::engine::general_purpose::STANDARD
            .encode(vec![255; crate::design::UPLOAD_LIMIT]);
        let frame = serde_json::to_vec(&Response {
            id: 1,
            reply: WorkerReply::Original {
                filename: "original.svg".into(),
                data_base64,
            },
        })
        .unwrap();
        assert!(
            frame.len() + 1 < MAX_FRAME,
            "maximum upload must be downloadable"
        );
    }
    #[test]
    fn maximum_source_project_and_binary_downloads_fit_without_integer_arrays() {
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(vec![255; crate::design::UPLOAD_LIMIT]);
        for (content, binary) in [
            (format!(r#"{{"source":{{"base64":"{encoded}"}}}}"#), false),
            (encoded, true),
        ] {
            let reply = bounded_export(WorkerReply::Download {
                mime: "application/octet-stream".into(),
                content,
                base64: binary,
            });
            assert!(matches!(reply, WorkerReply::Download { .. }));
            let frame = serde_json::to_vec(&Response {
                id: u64::MAX,
                reply,
            })
            .unwrap();
            assert!(frame.len() < MAX_FRAME);
        }
    }

    #[test]
    fn oversized_exports_are_normal_refusals_and_leave_room_for_the_next_reply() {
        // Escapes count toward the frame size, not just the content's byte length.
        let oversized = WorkerReply::Download {
            mime: "text/plain".into(),
            content: "\"".repeat(1024),
            base64: false,
        };
        assert!(matches!(
            bound_export(oversized, 1024),
            WorkerReply::Failed { .. }
        ));
        let next = WorkerReply::Download {
            mime: "text/plain".into(),
            content: "still available".into(),
            base64: false,
        };
        assert_eq!(bound_export(next.clone(), 1024), next);
    }
}
