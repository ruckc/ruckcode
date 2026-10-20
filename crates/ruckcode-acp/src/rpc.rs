//! JSON-RPC 2.0 over newline-delimited JSON, usable in both directions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message} ({code})")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("method not found: {method}"))
    }
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(-32602, msg)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(-32603, msg)
    }
}

impl From<anyhow::Error> for RpcError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(format!("{e:#}"))
    }
}

#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError>;
    async fn notification(&self, method: &str, params: Value);
}

type Reply = Result<Value, RpcError>;

#[derive(Default)]
struct Pending {
    closed: bool,
    map: HashMap<i64, oneshot::Sender<Reply>>,
}

struct Inner {
    out: mpsc::UnboundedSender<String>,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
}

/// Cheaply clonable handle to one end of a JSON-RPC connection.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl Connection {
    pub fn new<R, W>(
        reader: R,
        writer: W,
        handler: Arc<dyn Handler>,
    ) -> (Connection, tokio::task::JoinHandle<()>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        let conn = Connection {
            inner: Arc::new(Inner {
                out,
                pending: Mutex::new(Pending::default()),
                next_id: AtomicI64::new(1),
            }),
        };

        // Writer task.
        tokio::spawn(async move {
            let mut writer = writer;
            while let Some(line) = out_rx.recv().await {
                let mut buf = line.into_bytes();
                buf.push(b'\n');
                if writer.write_all(&buf).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });

        // Ordered notification dispatcher.
        let (ntx, mut nrx) = mpsc::unbounded_channel::<(String, Value)>();
        let nh = handler.clone();
        tokio::spawn(async move {
            while let Some((method, params)) = nrx.recv().await {
                nh.notification(&method, params).await;
            }
        });

        // Reader task.
        let c = conn.clone();
        let join = tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            loop {
                let line = match lines.next_line().await {
                    Ok(Some(l)) => l,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::debug!("rpc read error: {e}");
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("malformed rpc line: {e}");
                        continue;
                    }
                };
                c.dispatch(msg, &handler, &ntx);
            }
            c.close_pending();
        });
        (conn, join)
    }

    fn dispatch(
        &self,
        msg: Value,
        handler: &Arc<dyn Handler>,
        ntx: &mpsc::UnboundedSender<(String, Value)>,
    ) {
        let id = msg.get("id").filter(|v| !v.is_null()).cloned();
        if let Some(method) = msg.get("method").and_then(Value::as_str) {
            let method = method.to_string();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            match id {
                Some(id) => {
                    let handler = handler.clone();
                    let conn = self.clone();
                    tokio::spawn(async move {
                        let resp = match handler.request(&method, params).await {
                            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                            Err(err) => json!({"jsonrpc":"2.0","id":id,"error":err}),
                        };
                        conn.send_raw(&resp);
                    });
                }
                None => {
                    let _ = ntx.send((method, params));
                }
            }
        } else if let Some(id) = id.as_ref().and_then(Value::as_i64) {
            let reply = if let Some(err) = msg.get("error") {
                Err(serde_json::from_value::<RpcError>(err.clone())
                    .unwrap_or_else(|_| RpcError::internal(format!("bad error object: {err}"))))
            } else {
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            };
            let tx = self.inner.pending.lock().unwrap().map.remove(&id);
            if let Some(tx) = tx {
                let _ = tx.send(reply);
            }
        } else if let Some(id) = id {
            // Request-shaped without a method we could parse.
            let err = RpcError::new(-32600, "invalid request");
            self.send_raw(&json!({"jsonrpc":"2.0","id":id,"error":err}));
        } else {
            tracing::warn!("ignoring unrecognised rpc message");
        }
    }

    fn close_pending(&self) {
        let mut p = self.inner.pending.lock().unwrap();
        p.closed = true;
        for (_, tx) in p.map.drain() {
            let _ = tx.send(Err(RpcError::internal("connection closed")));
        }
    }

    fn send_raw(&self, v: &Value) {
        let _ = self.inner.out.send(v.to_string());
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut p = self.inner.pending.lock().unwrap();
            if p.closed {
                return Err(RpcError::internal("connection closed"));
            }
            p.map.insert(id, tx);
        }
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        if self.inner.out.send(msg.to_string()).is_err() {
            self.inner.pending.lock().unwrap().map.remove(&id);
            return Err(RpcError::internal("connection closed"));
        }
        rx.await
            .unwrap_or_else(|_| Err(RpcError::internal("connection closed")))
    }

    pub async fn request_typed<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, RpcError> {
        let params =
            serde_json::to_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))?;
        let v = self.request(method, params).await?;
        serde_json::from_value(v).map_err(|e| RpcError::internal(format!("bad response: {e}")))
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        let msg = json!({"jsonrpc":"2.0","method":method,"params":params});
        self.inner
            .out
            .send(msg.to_string())
            .map_err(|_| RpcError::internal("connection closed"))
    }

    /// Two in-process connections wired together over `tokio::io::duplex`.
    pub fn pair(a: Arc<dyn Handler>, b: Arc<dyn Handler>) -> (Connection, Connection) {
        let (x, y) = tokio::io::duplex(64 * 1024);
        let (xr, xw) = tokio::io::split(x);
        let (yr, yw) = tokio::io::split(y);
        let (ca, _) = Connection::new(xr, xw, a);
        let (cb, _) = Connection::new(yr, yw, b);
        (ca, cb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    struct Echo {
        peer: OnceLock<Connection>,
        seen: Mutex<Vec<i64>>,
    }
    impl Echo {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                peer: OnceLock::new(),
                seen: Mutex::new(vec![]),
            })
        }
    }
    #[async_trait]
    impl Handler for Echo {
        async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
            match method {
                "echo" => Ok(params),
                "callback" => {
                    // Call back into the peer while handling.
                    let r = self
                        .peer
                        .get()
                        .unwrap()
                        .request("echo", json!("hi"))
                        .await?;
                    Ok(json!({ "peer": r }))
                }
                m => Err(RpcError::method_not_found(m)),
            }
        }
        async fn notification(&self, _m: &str, params: Value) {
            self.seen.lock().unwrap().push(params.as_i64().unwrap());
        }
    }

    #[tokio::test]
    async fn round_trip_and_callback_no_deadlock() {
        let (ha, hb) = (Echo::new(), Echo::new());
        let (a, b) = Connection::pair(ha.clone(), hb.clone());
        ha.peer.set(b.clone()).ok();
        hb.peer.set(a.clone()).ok();
        let (r1, r2) = tokio::join!(
            a.request("callback", Value::Null),
            b.request("callback", Value::Null)
        );
        assert_eq!(r1.unwrap(), json!({"peer":"hi"}));
        assert_eq!(r2.unwrap(), json!({"peer":"hi"}));
        let t: String = a.request_typed("echo", &"x").await.unwrap();
        assert_eq!(t, "x");
    }

    #[tokio::test]
    async fn unknown_method_and_notification_order() {
        let (ha, hb) = (Echo::new(), Echo::new());
        let (a, _b) = Connection::pair(ha, hb.clone());
        let e = a.request("nope", Value::Null).await.unwrap_err();
        assert_eq!(e.code, -32601);
        for i in 0..50 {
            a.notify("n", json!(i)).unwrap();
        }
        a.request("echo", Value::Null).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let seen = hb.seen.lock().unwrap().clone();
        assert_eq!(seen, (0..50).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn pending_fails_when_peer_closes() {
        let (x, y) = tokio::io::duplex(1024);
        let (xr, xw) = tokio::io::split(x);
        let (a, _) = Connection::new(xr, xw, Echo::new());
        let req = tokio::spawn({
            let a = a.clone();
            async move { a.request("echo", Value::Null).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(y);
        let err = req.await.unwrap().unwrap_err();
        assert_eq!(err.code, -32603);
        assert!(a.request("echo", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn malformed_lines_skipped() {
        use tokio::io::AsyncReadExt;
        let (x, mut y) = tokio::io::duplex(4096);
        let (xr, xw) = tokio::io::split(x);
        let (_a, _) = Connection::new(xr, xw, Echo::new());
        y.write_all(
            b"garbage\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"echo\",\"params\":[1]}\n",
        )
        .await
        .unwrap();
        let mut buf = vec![0u8; 256];
        let n = y.read(&mut buf).await.unwrap();
        let v: Value = serde_json::from_slice(&buf[..n]).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["result"], json!([1]));
    }
}
