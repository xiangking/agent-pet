// WebSocket server for receiving external messages
// Accepts JSON messages and forwards to state machine

use crate::message::PetMessage;
use crate::state_machine::PetStateMachine;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tauri::async_runtime::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};

pub struct WebSocketServer {
    port: u16,
    state_machine: Arc<Mutex<PetStateMachine>>,
}

impl WebSocketServer {
    pub fn new(port: u16, state_machine: Arc<Mutex<PetStateMachine>>) -> Self {
        Self {
            port,
            state_machine,
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&addr).await?;
        log::info!("WebSocket server listening on ws://{}", addr);

        while let Ok((stream, _)) = listener.accept().await {
            let state_machine = self.state_machine.clone();
            tokio::spawn(handle_connection(stream, state_machine));
        }

        Ok(())
    }
}

async fn handle_connection(stream: TcpStream, state_machine: Arc<Mutex<PetStateMachine>>) {
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

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                log::debug!("Received: {}", text);

                match serde_json::from_str::<PetMessage>(&text) {
                    Ok(pet_msg) => {
                        let mut sm = state_machine.lock().await;
                        match sm.handle_websocket_message(&pet_msg.message_type) {
                            Some(new_state) => {
                                let ack = serde_json::json!({
                                    "type": "ack",
                                    "message_type": pet_msg.message_type,
                                    "current_state": new_state.to_string(),
                                });
                                let _ = write.send(Message::Text(ack.to_string())).await;
                            }
                            None => {
                                let disabled = serde_json::json!({
                                    "type": "disabled",
                                    "message": "WebSocket message handling is disabled",
                                    "message_type": pet_msg.message_type,
                                    "current_state": sm.current_state().to_string(),
                                });
                                let _ = write.send(Message::Text(disabled.to_string())).await;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Invalid message format: {}", e);
                        let error = serde_json::json!({
                            "type": "error",
                            "message": format!("Invalid message: {}", e),
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
