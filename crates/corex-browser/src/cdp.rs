use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Cdp {
    outgoing: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
    events: broadcast::Sender<Value>,
}

impl Cdp {
    pub fn from_socket<S>(socket: tokio_tungstenite::WebSocketStream<S>) -> Arc<Self>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = socket.split();
        let (outgoing, mut queue) = mpsc::unbounded_channel::<Message>();
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (events, _) = broadcast::channel(512);

        tokio::spawn(async move {
            while let Some(message) = queue.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });

        let reader_pending = pending.clone();
        let reader_alive = alive.clone();
        let reader_events = events.clone();
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let Message::Text(text) = message else { continue };
                let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                match value.get("id").and_then(Value::as_u64) {
                    Some(id) => {
                        let sender = reader_pending.lock().ok().and_then(|mut map| map.remove(&id));
                        if let Some(sender) = sender {
                            let _ = sender.send(match value.get("error") {
                                Some(error) => Err(error["message"].as_str().unwrap_or("CDP error").to_string()),
                                None => Ok(value["result"].clone()),
                            });
                        }
                    }
                    None => {
                        let _ = reader_events.send(value);
                    }
                }
            }
            reader_alive.store(false, Ordering::SeqCst);
            if let Ok(mut map) = reader_pending.lock() {
                map.clear();
            }
        });

        Arc::new(Self {
            outgoing,
            pending,
            next_id: AtomicU64::new(1),
            alive,
            events,
        })
    }

    pub fn notify(&self, method: &str, params: Value) {
        let request = json!({ "id": self.next_id.fetch_add(1, Ordering::SeqCst), "method": method, "params": params });
        let _ = self.outgoing.send(Message::Text(request.to_string().into()));
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub async fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        if !self.is_alive() {
            return Err(anyhow!("the browser connection is closed"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (reply, response) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("CDP state poisoned"))?
            .insert(id, reply);

        let mut request = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.outgoing
            .send(Message::Text(request.to_string().into()))
            .map_err(|_| anyhow!("the browser connection is closed"))?;

        match tokio::time::timeout(Duration::from_secs(30), response).await {
            Ok(Ok(result)) => result.map_err(|message| anyhow!("{method}: {message}")),
            Ok(Err(_)) => Err(anyhow!("the browser closed while running {method}")),
            Err(_) => {
                if let Ok(mut map) = self.pending.lock() {
                    map.remove(&id);
                }
                Err(anyhow!("{method} timed out"))
            }
        }
    }
}

impl Drop for Cdp {
    fn drop(&mut self) {
        let _ = self.outgoing.send(Message::Close(None));
    }
}
