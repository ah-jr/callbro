const { invoke } = window.__TAURI__.core;

// ---------- app state ----------
let config = null; // { user_id, name, admin_key, manual_server }
let admin = false; // is this the admin build?
let state = { grid: { rows: 5, cols: 8 }, users: [] };
let ws = null;
let reconnectTimer = null;
let editMode = false;
let selectedUserId = null; // in edit mode: user waiting to be placed
let heartbeat = null;

const $ = (id) => document.getElementById(id);

// ---------- boot ----------
window.addEventListener("DOMContentLoaded", async () => {
  wireEvents();
  admin = await invoke("is_admin").catch(() => false);
  config = await invoke("load_config");
  if (!config.name || !config.name.trim()) {
    show("setup");
    $("setup-name").focus();
  } else {
    startApp();
  }
});

function show(screen) {
  $("setup").classList.toggle("hidden", screen !== "setup");
  $("app").classList.toggle("hidden", screen !== "app");
}

async function startApp() {
  show("app");
  $("me").textContent = admin ? `Admin: ${config.name}` : `Você: ${config.name}`;
  $("edit-btn").classList.toggle("hidden", !admin);
  await connect();
}

// ---------- connection ----------
async function resolveServer() {
  if (admin) {
    const port = await invoke("start_host");
    return `127.0.0.1:${port}`;
  }
  if (config.manual_server && config.manual_server.trim()) {
    return config.manual_server.trim();
  }
  const found = await invoke("discover_server");
  return found || null;
}

async function connect() {
  clearTimeout(reconnectTimer);
  setStatus("conectando…", "");

  let addr;
  try {
    addr = await resolveServer();
  } catch (e) {
    addr = null;
  }

  if (!addr) {
    setStatus("servidor não encontrado", "bad");
    $("hint").textContent =
      "Não achei o servidor na rede. Peça ao admin o IP e informe em ⚙︎ → Servidor manual.";
    reconnectTimer = setTimeout(connect, 5000);
    return;
  }

  try {
    ws = new WebSocket(`ws://${addr}`);
  } catch (e) {
    scheduleReconnect();
    return;
  }

  ws.onopen = () => {
    setStatus("conectado", "ok");
    send({ type: "join", id: config.user_id, name: config.name });
    clearInterval(heartbeat);
    heartbeat = setInterval(() => send({ type: "heartbeat" }), 15000);
  };
  ws.onmessage = (ev) => {
    let msg;
    try {
      msg = JSON.parse(ev.data);
    } catch {
      return;
    }
    if (msg.type === "state") {
      state = msg;
      render();
    } else if (msg.type === "incoming_call") {
      onIncomingCall(msg.from_name || "Alguém");
    }
  };
  ws.onclose = () => scheduleReconnect();
  ws.onerror = () => {
    try { ws.close(); } catch {}
  };
}

function scheduleReconnect() {
  clearInterval(heartbeat);
  setStatus("reconectando…", "bad");
  clearTimeout(reconnectTimer);
  reconnectTimer = setTimeout(connect, 3000);
}

function send(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
}

// Admin edit messages carry the secret key (only the admin build has one).
function sendAdmin(obj) {
  send({ ...obj, key: config.admin_key || "" });
}

function setStatus(text, cls) {
  const el = $("status");
  el.textContent = text;
  el.className = "pill" + (cls ? " " + cls : "");
}

// ---------- rendering ----------
function seatMap() {
  const map = {};
  for (const u of state.users) {
    if (u.seat) map[`${u.seat.row},${u.seat.col}`] = u;
  }
  return map;
}

function render() {
  const grid = $("grid");
  const { rows, cols } = state.grid;
  grid.style.gridTemplateColumns = `repeat(${cols}, minmax(90px, 1fr))`;
  grid.innerHTML = "";
  const map = seatMap();

  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      const cell = document.createElement("div");
      cell.className = "cell";
      const user = map[`${r},${c}`];

      if (user) {
        cell.appendChild(seatEl(user));
      } else {
        cell.classList.add("empty");
        if (editMode) {
          cell.classList.add("editing");
          cell.onclick = () => {
            if (selectedUserId) {
              sendAdmin({ type: "assign", user: selectedUserId, row: r, col: c });
              selectedUserId = null;
            }
          };
        }
      }
      grid.appendChild(cell);
    }
  }

  if (editMode) {
    $("hint").textContent = "Modo edição: clique numa pessoa (na lista ou no mapa) e depois num lugar vazio.";
  } else {
    const seated = state.users.filter((u) => u.seat).length;
    $("hint").textContent =
      seated === 0
        ? "Ninguém foi posicionado ainda."
        : "Clique numa pessoa online para chamá-la.";
  }

  renderEditor();
}

function seatEl(user) {
  const el = document.createElement("div");
  const me = user.id === config.user_id;
  el.className = "seat" + (user.online ? "" : " offline") + (me ? " me" : "");
  if (editMode && selectedUserId === user.id) el.classList.add("selected");

  const dot = document.createElement("span");
  dot.className = "dot" + (user.online ? " on" : "");
  const name = document.createElement("span");
  name.className = "name";
  name.textContent = me ? `${user.name} (você)` : user.name;
  el.appendChild(dot);
  el.appendChild(name);

  el.onclick = () => {
    if (editMode) {
      selectedUserId = selectedUserId === user.id ? null : user.id;
      render();
    } else if (!me && user.online) {
      callUser(user);
    } else if (!me && !user.online) {
      toast(`${user.name} está offline.`);
    }
  };
  return el;
}

function renderEditor() {
  if (!editMode) return;
  $("grid-rows").value = state.grid.rows;
  $("grid-cols").value = state.grid.cols;

  const selUser = state.users.find((u) => u.id === selectedUserId);
  $("selected-info").textContent = selUser
    ? `Selecionado: ${selUser.name} — clique num lugar vazio.`
    : "";

  const unassigned = $("unassigned");
  const all = $("all-users");
  unassigned.innerHTML = "";
  all.innerHTML = "";

  for (const u of state.users) {
    if (!u.seat) unassigned.appendChild(chipEl(u, false));
    all.appendChild(chipEl(u, true));
  }
  if (!unassigned.children.length) {
    unassigned.innerHTML = '<div class="muted">Todos posicionados 🎉</div>';
  }
}

function chipEl(user, showActions) {
  const chip = document.createElement("div");
  chip.className = "chip" + (selectedUserId === user.id ? " selected" : "");

  const nameWrap = document.createElement("div");
  nameWrap.className = "chip-name";
  const dot = document.createElement("span");
  dot.className = "dot" + (user.online ? " on" : "");
  nameWrap.appendChild(dot);
  nameWrap.appendChild(document.createTextNode(user.name));
  chip.appendChild(nameWrap);

  chip.onclick = () => {
    selectedUserId = selectedUserId === user.id ? null : user.id;
    render();
  };

  if (showActions) {
    const actions = document.createElement("div");

    const edit = document.createElement("span");
    edit.className = "x";
    edit.textContent = "✎";
    edit.title = "Renomear";
    edit.onclick = (e) => {
      e.stopPropagation();
      const name = prompt(`Novo nome para "${user.name}":`, user.name);
      if (name && name.trim()) sendAdmin({ type: "rename_user", user: user.id, name: name.trim() });
    };

    const del = document.createElement("span");
    del.className = "x";
    del.textContent = user.seat ? "⌫" : "✕";
    del.title = user.seat ? "Tirar do mapa" : "Remover pessoa";
    del.onclick = (e) => {
      e.stopPropagation();
      if (user.seat) sendAdmin({ type: "unassign", user: user.id });
      else sendAdmin({ type: "remove", user: user.id });
    };

    actions.appendChild(edit);
    actions.appendChild(del);
    chip.appendChild(actions);
  }
  return chip;
}

// ---------- calling ----------
function callUser(user) {
  send({ type: "call", to: user.id });
  toast(`Chamando ${user.name}…`);
}

function onIncomingCall(fromName) {
  const text = `${fromName} tá te chamando`;
  invoke("alert", { fromName }).catch(() => {});
  $("incoming-text").textContent = text;
  $("incoming").classList.remove("hidden");
  playChime();
  speak(text);
}

// ---------- sound ----------
let audioCtx = null;
function playChime() {
  try {
    audioCtx = audioCtx || new (window.AudioContext || window.webkitAudioContext)();
    if (audioCtx.state === "suspended") audioCtx.resume();
    const now = audioCtx.currentTime;
    [880, 1174].forEach((freq, i) => {
      const osc = audioCtx.createOscillator();
      const gain = audioCtx.createGain();
      osc.type = "sine";
      osc.frequency.value = freq;
      const t = now + i * 0.18;
      gain.gain.setValueAtTime(0, t);
      gain.gain.linearRampToValueAtTime(0.35, t + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.001, t + 0.35);
      osc.connect(gain).connect(audioCtx.destination);
      osc.start(t);
      osc.stop(t + 0.4);
    });
  } catch {}
}

function speak(text) {
  try {
    if (!window.speechSynthesis) return;
    const u = new SpeechSynthesisUtterance(text);
    u.lang = "pt-BR";
    const pt = speechSynthesis.getVoices().find((v) => /pt/i.test(v.lang));
    if (pt) u.voice = pt;
    u.rate = 1;
    setTimeout(() => speechSynthesis.speak(u), 350);
  } catch {}
}

// ---------- ui events ----------
function wireEvents() {
  $("setup-save").onclick = saveSetup;
  $("setup-name").addEventListener("keydown", (e) => {
    if (e.key === "Enter") saveSetup();
  });

  $("edit-btn").onclick = () => {
    editMode = !editMode;
    selectedUserId = null;
    $("editor").classList.toggle("hidden", !editMode);
    $("edit-btn").textContent = editMode ? "Concluir" : "Editar layout";
    render();
  };

  $("grid-apply").onclick = () => {
    const rows = parseInt($("grid-rows").value, 10);
    const cols = parseInt($("grid-cols").value, 10);
    if (rows > 0 && cols > 0) sendAdmin({ type: "set_grid", rows, cols });
  };

  $("incoming-dismiss").onclick = () => $("incoming").classList.add("hidden");

  $("settings-btn").onclick = openSettings;
  $("settings-close").onclick = () => $("settings").classList.add("hidden");
  $("settings-save").onclick = saveSettings;
}

async function saveSetup() {
  const name = $("setup-name").value.trim();
  if (!name) return;
  config.name = name;
  await invoke("save_config", { config });
  startApp();
}

async function openSettings() {
  $("cfg-client").classList.toggle("hidden", admin);
  $("cfg-admin").classList.toggle("hidden", !admin);
  if (admin) {
    const ip = await invoke("lan_ip").catch(() => null);
    $("admin-info").textContent = ip
      ? `Este é o computador admin. Os outros te encontram automaticamente. IP manual, se precisarem: ${ip}:8787`
      : "Este é o computador admin (hospeda o servidor).";
  } else {
    $("cfg-server").value = config.manual_server || "";
  }
  $("settings").classList.remove("hidden");
}

async function saveSettings() {
  if (!admin) {
    config.manual_server = $("cfg-server").value.trim();
    await invoke("save_config", { config });
    try { if (ws) ws.close(); } catch {}
    await connect();
  }
  $("settings").classList.add("hidden");
}

// ---------- toast ----------
let toastTimer = null;
function toast(text) {
  const el = $("toast");
  el.textContent = text;
  el.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add("hidden"), 2600);
}

// prime voices list (some platforms load async)
if (window.speechSynthesis) {
  speechSynthesis.onvoiceschanged = () => speechSynthesis.getVoices();
}
