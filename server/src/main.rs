// callbro central hub — standalone WebSocket server (deploy on EasyPanel).
//
// Clients connect over wss:// (TLS terminated by EasyPanel's proxy; internally
// plain ws on $PORT). The server keeps the authoritative roster (users + seats
// + grid), persists it to disk, and relays "call" messages.
//
// Access control:
//   * CALLBRO_JOIN_SECRET  — the "team code"; a connection must present it in its
//                            join message or it gets denied. Empty = open (dev).
//   * CALLBRO_ADMIN_KEY    — the admin password; required for any layout/name edit.
//
// Config via env:
//   PORT (default 8080), CALLBRO_STATE_DIR (default ./data),
//   CALLBRO_JOIN_SECRET, CALLBRO_ADMIN_KEY

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

// ---------- persisted data ----------

#[derive(Clone, Serialize, Deserialize)]
struct Seat {
    row: u32,
    col: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct User {
    id: String,
    name: String,
    #[serde(default)]
    seat: Option<Seat>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Grid {
    rows: u32,
    cols: u32,
}

impl Default for Grid {
    fn default() -> Self {
        Grid { rows: 5, cols: 8 }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct Persisted {
    #[serde(default)]
    grid: Grid,
    #[serde(default)]
    users: HashMap<String, User>,
}

// ---------- runtime state ----------

struct Conn {
    user_id: Option<String>,
    authorized: bool,
    tx: mpsc::UnboundedSender<Message>,
}

struct ServerState {
    data: Persisted,
    conns: HashMap<u64, Conn>,
    next_id: u64,
    state_path: PathBuf,
    admin_key: String,
    join_secret: String,
}

impl ServerState {
    fn new(state_path: PathBuf, admin_key: String, join_secret: String) -> Self {
        let data = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        ServerState {
            data,
            conns: HashMap::new(),
            next_id: 1,
            state_path,
            admin_key,
            join_secret,
        }
    }

    fn admin_ok(&self, key: &str) -> bool {
        !self.admin_key.is_empty() && key == self.admin_key
    }

    fn join_ok(&self, secret: &str) -> bool {
        self.join_secret.is_empty() || secret == self.join_secret
    }

    fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.data) {
            let _ = std::fs::write(&self.state_path, s);
        }
    }

    fn online_ids(&self) -> HashSet<String> {
        self.conns
            .values()
            .filter_map(|c| c.user_id.clone())
            .collect()
    }

    fn snapshot(&self) -> serde_json::Value {
        let online = self.online_ids();
        let mut users: Vec<serde_json::Value> = self
            .data
            .users
            .values()
            .map(|u| {
                json!({
                    "id": u.id,
                    "name": u.name,
                    "seat": u.seat,
                    "online": online.contains(&u.id),
                })
            })
            .collect();
        users.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        });
        json!({ "type": "state", "grid": self.data.grid, "users": users })
    }

    fn broadcast(&self) {
        let msg = Message::text(self.snapshot().to_string());
        for c in self.conns.values() {
            if c.authorized {
                let _ = c.tx.send(msg.clone());
            }
        }
    }

    fn send_to_user(&self, user_id: &str, value: &serde_json::Value) {
        let msg = Message::text(value.to_string());
        for c in self.conns.values() {
            if c.user_id.as_deref() == Some(user_id) {
                let _ = c.tx.send(msg.clone());
            }
        }
    }
}

// ---------- inbound messages ----------

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Join {
        id: String,
        name: String,
        #[serde(default)]
        secret: String,
    },
    Heartbeat,
    Call {
        to: String,
        #[serde(default)]
        action: String,
    },
    Assign { user: String, row: u32, col: u32, #[serde(default)] key: String },
    Unassign { user: String, #[serde(default)] key: String },
    SetGrid { rows: u32, cols: u32, #[serde(default)] key: String },
    Remove { user: String, #[serde(default)] key: String },
    RenameUser { user: String, name: String, #[serde(default)] key: String },
}

/// Returns false when the connection should be closed (e.g. wrong team code).
async fn handle_text(text: &str, conn_id: u64, state: &Arc<Mutex<ServerState>>) -> bool {
    let msg: ClientMsg = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(_) => return true,
    };

    let mut s = state.lock().await;

    // Everything except Join requires an authorized connection.
    let authorized = s.conns.get(&conn_id).map(|c| c.authorized).unwrap_or(false);
    if !matches!(msg, ClientMsg::Join { .. }) && !authorized {
        return true;
    }

    match msg {
        ClientMsg::Join { id, name, secret } => {
            if !s.join_ok(&secret) {
                if let Some(conn) = s.conns.get(&conn_id) {
                    let _ = conn.tx.send(Message::text(json!({"type":"denied"}).to_string()));
                }
                return false; // close the connection
            }
            if let Some(conn) = s.conns.get_mut(&conn_id) {
                conn.user_id = Some(id.clone());
                conn.authorized = true;
            }
            let entry = s.data.users.entry(id.clone()).or_insert(User {
                id: id.clone(),
                name: name.clone(),
                seat: None,
            });
            entry.name = name;
            s.save();
            s.broadcast();
        }
        ClientMsg::Heartbeat => {}
        ClientMsg::Call { to, action } => {
            let from_id = s.conns.get(&conn_id).and_then(|c| c.user_id.clone());
            if let Some(from_id) = from_id {
                let from_name = s
                    .data
                    .users
                    .get(&from_id)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "Alguém".into());
                let action = if action.is_empty() { "chamar".to_string() } else { action };
                let payload = json!({
                    "type": "incoming_call",
                    "from": from_id,
                    "from_name": from_name,
                    "action": action,
                });
                s.send_to_user(&to, &payload);
            }
        }
        ClientMsg::Assign { user, row, col, key } => {
            if !s.admin_ok(&key) {
                return true;
            }
            for u in s.data.users.values_mut() {
                if let Some(seat) = &u.seat {
                    if seat.row == row && seat.col == col {
                        u.seat = None;
                    }
                }
            }
            if let Some(u) = s.data.users.get_mut(&user) {
                u.seat = Some(Seat { row, col });
            }
            s.save();
            s.broadcast();
        }
        ClientMsg::Unassign { user, key } => {
            if !s.admin_ok(&key) {
                return true;
            }
            if let Some(u) = s.data.users.get_mut(&user) {
                u.seat = None;
                s.save();
                s.broadcast();
            }
        }
        ClientMsg::SetGrid { rows, cols, key } => {
            if !s.admin_ok(&key) {
                return true;
            }
            let rows = rows.clamp(1, 40);
            let cols = cols.clamp(1, 40);
            for u in s.data.users.values_mut() {
                if let Some(seat) = &u.seat {
                    if seat.row >= rows || seat.col >= cols {
                        u.seat = None;
                    }
                }
            }
            s.data.grid = Grid { rows, cols };
            s.save();
            s.broadcast();
        }
        ClientMsg::Remove { user, key } => {
            if !s.admin_ok(&key) {
                return true;
            }
            s.data.users.remove(&user);
            s.save();
            s.broadcast();
        }
        ClientMsg::RenameUser { user, name, key } => {
            if !s.admin_ok(&key) {
                return true;
            }
            if let Some(u) = s.data.users.get_mut(&user) {
                u.name = name;
                s.save();
                s.broadcast();
            }
        }
    }
    true
}

async fn handle_conn(stream: tokio::net::TcpStream, state: Arc<Mutex<ServerState>>) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(w) => w,
        Err(_) => return,
    };
    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    let conn_id = {
        let mut s = state.lock().await;
        let id = s.next_id;
        s.next_id += 1;
        s.conns.insert(
            id,
            Conn {
                user_id: None,
                authorized: false,
                tx,
            },
        );
        id
    };

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = read.next().await {
        match msg {
            Message::Text(t) => {
                if !handle_text(t.as_str(), conn_id, &state).await {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    {
        let mut s = state.lock().await;
        s.conns.remove(&conn_id);
        s.broadcast();
    }
    // Dropping the conn above drops this connection's sender, so the writer
    // finishes after draining anything still queued (e.g. a "denied" reply).
    let _ = writer.await;
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let state_dir = std::env::var("CALLBRO_STATE_DIR").unwrap_or_else(|_| "./data".to_string());
    let admin_key = std::env::var("CALLBRO_ADMIN_KEY").unwrap_or_default();
    let join_secret = std::env::var("CALLBRO_JOIN_SECRET").unwrap_or_default();

    std::fs::create_dir_all(&state_dir).ok();
    let state_path = PathBuf::from(&state_dir).join("state.json");

    if admin_key.is_empty() {
        eprintln!("WARNING: CALLBRO_ADMIN_KEY is empty — layout editing is disabled for everyone.");
    }
    if join_secret.is_empty() {
        eprintln!("WARNING: CALLBRO_JOIN_SECRET is empty — anyone can connect (no team code).");
    }

    let state = Arc::new(Mutex::new(ServerState::new(state_path, admin_key, join_secret)));

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| panic!("failed to bind 0.0.0.0:{port}: {e}"));
    println!("callbro-server listening on 0.0.0.0:{port} (state: {state_dir})");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let st = state.clone();
                tokio::spawn(handle_conn(stream, st));
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}
