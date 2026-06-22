// WebSocket server for receiving external messages
// Accepts JSON messages and forwards to state machine

use crate::message::{PetMessage, PetNotice, PetUsageMetric};
use crate::state_machine::PetStateMachine;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tauri::async_runtime::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

pub struct WebSocketServer {
    port: u16,
    state_machine: Arc<Mutex<PetStateMachine>>,
    cancellation_token: CancellationToken,
}

impl WebSocketServer {
    pub fn new(
        port: u16,
        state_machine: Arc<Mutex<PetStateMachine>>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            port,
            state_machine,
            cancellation_token,
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&addr).await?;
        log::info!("WebSocket server listening on ws://{}", addr);

        while !self.cancellation_token.is_cancelled() {
            if !self.state_machine.lock().await.websocket_enabled() {
                tokio::select! {
                    _ = self.cancellation_token.cancelled() => break,
                    _ = sleep(Duration::from_millis(700)) => {}
                }
                continue;
            }

            let accepted = tokio::select! {
                _ = self.cancellation_token.cancelled() => break,
                result = listener.accept() => result,
            };

            match accepted {
                Ok((stream, _)) => {
                    let state_machine = self.state_machine.clone();
                    let cancellation_token = self.cancellation_token.clone();
                    tokio::spawn(handle_connection(stream, state_machine, cancellation_token));
                }
                Err(e) => return Err(e.into()),
            }
        }

        Ok(())
    }
}

async fn handle_connection(
    stream: TcpStream,
    state_machine: Arc<Mutex<PetStateMachine>>,
    cancellation_token: CancellationToken,
) {
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log::error!("WebSocket handshake failed: {}", e);
            return;
        }
    };

    log::info!("New WebSocket connection");

    let (mut write, mut read) = ws_stream.split();

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "connected",
        "message": "Agent Pet WebSocket server",
        "protocol": "codex-pet-v1",
    });
    let _ = write.send(Message::Text(welcome.to_string())).await;

    loop {
        let msg = tokio::select! {
            _ = cancellation_token.cancelled() => break,
            msg = read.next() => msg,
        };

        let Some(msg) = msg else {
            break;
        };

        match msg {
            Ok(Message::Text(text)) => {
                log::debug!("Received: {}", text);

                match handle_text_message(&text, &state_machine).await {
                    Ok(response) => {
                        let _ = write.send(Message::Text(response.to_string())).await;
                    }
                    Err(e) => {
                        log::warn!("Invalid message format: {}", e);
                        let error = serde_json::json!({
                            "type": "error",
                            "message": e,
                        });
                        let _ = write.send(Message::Text(error.to_string())).await;
                    }
                }
            }
            Ok(Message::Close(_)) => {
                log::info!("WebSocket connection closed");
                break;
            }
            Ok(Message::Ping(data)) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Err(e) => {
                log::error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }
}

async fn handle_text_message(
    text: &str,
    state_machine: &Arc<Mutex<PetStateMachine>>,
) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;

    if message_kind_is(&value, "usage") {
        let metric: PetUsageMetric = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut sm = state_machine.lock().await;
        if !sm.websocket_enabled() {
            return Ok(serde_json::json!({
                "type": "disabled",
                "message": "WebSocket message handling is disabled",
                "current_state": sm.current_state().to_string(),
            }));
        }

        sm.upsert_usage_metric(metric.clone());
        return Ok(serde_json::json!({
            "type": "ack",
            "kind": "usage",
            "id": metric.id,
            "current_state": sm.current_state().to_string(),
        }));
    }

    if message_kind_is(&value, "notice") {
        let notice: PetNotice = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let sm = state_machine.lock().await;
        if !sm.websocket_enabled() {
            return Ok(serde_json::json!({
                "type": "disabled",
                "message": "WebSocket message handling is disabled",
                "current_state": sm.current_state().to_string(),
            }));
        }

        sm.show_notice(&notice);
        return Ok(serde_json::json!({
            "type": "ack",
            "kind": "notice",
            "title": notice.title,
            "current_state": sm.current_state().to_string(),
        }));
    }

    let pet_msg: PetMessage = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let mut sm = state_machine.lock().await;
    match sm.handle_websocket_message(&pet_msg.message_type) {
        Some(new_state) => Ok(serde_json::json!({
            "type": "ack",
            "message_type": pet_msg.message_type,
            "current_state": new_state.to_string(),
        })),
        None => Ok(serde_json::json!({
            "type": "disabled",
            "message": "WebSocket message handling is disabled",
            "message_type": pet_msg.message_type,
            "current_state": sm.current_state().to_string(),
        })),
    }
}

fn message_kind_is(value: &serde_json::Value, expected: &str) -> bool {
    value
        .get("kind")
        .or_else(|| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case(expected))
}
