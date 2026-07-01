// callbro LAN server: WebSocket hub + mDNS advertising + JSON state persistence.
//
// Clients (the webview's own WebSocket) connect to ws://<host>:<port>. The server
// keeps the authoritative roster (users + seats + grid), persists it to disk, and
// relays "call" messages to the target user's connection(s).
//
// Layout/name edits are gated by an admin key that only the admin machine holds,
// so ordinary clients (or hand-crafted WebSocket messages) cannot change anything.

use futures_util::{SinkExt, StreamExt};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

pub const SERVICE_TYPE: &str = "_callbro._tcp.local.";
pub const DEFAULT_PORT: u16 = 8787;

// ---------- persisted data ----------

#[derive(Clone, Serialize, Deserialize)]
pub struct Seat {
    pub row: u32,
    pub col: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub seat: Option<Seat>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Grid {
    pub rows: u32,
    pub cols: u32,
}

impl Default for Grid {
    fn default() -> Self {
        Grid { rows: 5, cols: 8 }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Persisted {
    #[serde(default)]
    pub grid: Grid,
    #[serde(default)]
    pub users: HashMap<String, User>,
}

// ---------- runtime state ----------

struct Conn {
    user_id: Option<String>,
    tx: mpsc::UnboundedSender<Message>,
}

pub struct ServerState {
    data: Persisted,
    conns: HashMap<u64, Conn>,
    next_id: u64,
    state_path: PathBuf,
    admin_key: String,
}

impl ServerState {
    fn new(state_path: PathBuf, admin_key: String) -> Self {
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
        }
    }

    /// True only if the presented key matches the server's admin key.
    /// An empty admin key rejects everything (fail closed).
    fn admin_ok(&self, key: &str) -> bool {
        !self.admin_key.is_empty() && key == self.admin_key
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
        // stable order so the UI doesn't reshuffle on every render
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
            let _ = c.tx.send(msg.clone());
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
    // open to everyone
    Join { id: String, name: String },
    Heartbeat,
    Call { to: String },

    // admin-only: each carries the admin key
    Assign { user: String, row: u32, col: u32, #[serde(default)] key: String },
    Unassign { user: String, #[serde(default)] key: String },
    SetGrid { rows: u32, cols: u32, #[serde(default)] key: String },
    Remove { user: String, #[serde(default)] key: String },
    RenameUser { user: String, name: String, #[serde(default)] key: String },
}

async fn handle_text(text: &str, conn_id: u64, state: &Arc<Mutex<ServerState>>) {
    let msg: ClientMsg = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(_) => return,
    };

    let mut s = state.lock().await;
    match msg {
        ClientMsg::Join { id, name } => {
            if let Some(conn) = s.conns.get_mut(&conn_id) {
                conn.user_id = Some(id.clone());
            }
            let entry = s.data.users.entry(id.clone()).or_insert(User {
                id: id.clone(),
                name: name.clone(),
                seat: None,
            });
            entry.name = name; // keep latest name from this machine, preserve seat
            s.save();
            s.broadcast();
        }
        ClientMsg::Heartbeat => {}
        ClientMsg::Call { to } => {
            let from_id = s.conns.get(&conn_id).and_then(|c| c.user_id.clone());
            if let Some(from_id) = from_id {
                let from_name = s
                    .data
                    .users
                    .get(&from_id)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "Alguém".into());
                let payload = json!({
                    "type": "incoming_call",
                    "from": from_id,
                    "from_name": from_name,
                });
                s.send_to_user(&to, &payload);
            }
        }
        ClientMsg::Assign { user, row, col, key } => {
            if !s.admin_ok(&key) {
                return;
            }
            // one person per seat: clear anyone already sitting here
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
                return;
            }
            if let Some(u) = s.data.users.get_mut(&user) {
                u.seat = None;
                s.save();
                s.broadcast();
            }
        }
        ClientMsg::SetGrid { rows, cols, key } => {
            if !s.admin_ok(&key) {
                return;
            }
            let rows = rows.clamp(1, 40);
            let cols = cols.clamp(1, 40);
            // drop seats that fall outside the new grid
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
                return;
            }
            s.data.users.remove(&user);
            s.save();
            s.broadcast();
        }
        ClientMsg::RenameUser { user, name, key } => {
            if !s.admin_ok(&key) {
                return;
            }
            if let Some(u) = s.data.users.get_mut(&user) {
                u.name = name;
                s.save();
                s.broadcast();
            }
        }
    }
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
        s.conns.insert(id, Conn { user_id: None, tx });
        id
    };

    // writer task: drains the per-connection channel to the socket
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write.send(msg).await.is_err() {
                break;
            }
        }
    });

    // reader loop
    while let Some(Ok(msg)) = read.next().await {
        match msg {
            Message::Text(t) => handle_text(t.as_str(), conn_id, &state).await,
            Message::Close(_) => break,
            _ => {}
        }
    }

    // cleanup + let everyone know this user may now be offline
    {
        let mut s = state.lock().await;
        s.conns.remove(&conn_id);
        s.broadcast();
    }
    writer.abort();
}

fn advertise(port: u16, ip: Option<String>) {
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(_) => return,
    };
    let host_ipv4 = ip.unwrap_or_else(|| "0.0.0.0".to_string());
    let host_name = "callbro-server.local.";
    let properties: HashMap<String, String> = HashMap::new();
    if let Ok(info) = ServiceInfo::new(
        SERVICE_TYPE,
        "callbro-server",
        host_name,
        host_ipv4.as_str(),
        port,
        properties,
    ) {
        let info = info.enable_addr_auto();
        let _ = daemon.register(info);
    }
    // keep the daemon alive for the process lifetime
    std::mem::forget(daemon);
}

/// Run the hub forever. Returns only on a fatal bind error.
pub async fn run_server(
    port: u16,
    state_path: PathBuf,
    ip: Option<String>,
    admin_key: String,
) -> std::io::Result<()> {
    let state = Arc::new(Mutex::new(ServerState::new(state_path, admin_key)));
    advertise(port, ip);
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    loop {
        let (stream, _addr) = listener.accept().await?;
        let st = state.clone();
        tokio::spawn(handle_conn(stream, st));
    }
}

/// Blocking mDNS browse. Returns the first resolved "ip:port" (IPv4 preferred).
pub fn discover(timeout_ms: u64) -> Option<String> {
    let daemon = ServiceDaemon::new().ok()?;
    let receiver = daemon.browse(SERVICE_TYPE).ok()?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let addrs = info.get_addresses();
                let ipv4 = addrs.iter().find(|a| a.is_ipv4()).or_else(|| addrs.iter().next());
                if let Some(addr) = ipv4 {
                    let _ = daemon.shutdown();
                    return Some(format!("{}:{}", addr, info.get_port()));
                }
            }
            Ok(_) => continue,
            Err(_) => {
                let _ = daemon.shutdown();
                return None;
            }
        }
    }
}
