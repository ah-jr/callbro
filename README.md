# callbro

An office intercom for the LAN. Everyone wears headphones, so walking over and
talking to someone is hard — callbro lets you click a person on a map of the
office and their computer pops up a window, plays a chime, and says out loud
**"Fulano tá te chamando"**.

- Runs on **macOS and Windows** (one native app, ~5–10 MB).
- Works entirely on your **local network** — no internet, no cloud, no login.
- One machine (the admin's) runs a tiny server; everyone else finds it
  automatically on the LAN.

---

## How it works

There are **two separate apps** built from this one codebase:

| App | Who runs it | What it does |
|-----|-------------|--------------|
| **callbro** (client) | Everyone | Types their name once, appears in the pool, sees the map, clicks a person to call them. Cannot host or edit anything. |
| **callbro-admin** | Only you | Hosts the server on its machine, shows the **Editar layout** button, and is the *only* app that can move people, rename them, or resize the grid. |

On first launch the app asks for your name and auto-joins — no accounts, no
passwords. When someone calls you, you get a real OS notification, the window
jumps to the front, a chime plays, and a pt-BR voice announces who's calling.

### Why only you are the admin

Two independent locks:

1. **The client app has no admin code.** It can't host a server and has no
   layout editor — there's no button to press and nothing to toggle.
2. **The server rejects edits without a secret key.** The admin app generates a
   random key on first run and stores it *only on your machine*. Every
   layout/name change must carry that key, so even someone hand-crafting network
   messages (bypassing the UI entirely) cannot change anything — the server
   simply ignores it.

Clients discover the server via **mDNS/Bonjour**; if that's blocked they can type
the admin's IP manually. The admin app keeps the roster and layout in a small
JSON file on your machine.

### Names

Each person types their name **once**, on first launch. There is no rename in the
client app. If someone makes a typo, you (admin) can fix it: **Editar layout** →
click the ✎ next to their name.

---

## Development

Requires **Node 18+** and the **Rust toolchain** (`rustup`).

```bash
npm install
npm run tauri dev                       # run the client app locally
npm run tauri dev -- --features admin   # run the admin app locally
```

## Building installers

- **macOS** (from a Mac):
  ```bash
  npm run tauri build                                                          # client
  npm run tauri build -- --features admin --config src-tauri/tauri.admin.conf.json  # admin
  ```
  Output: `src-tauri/target/**/release/bundle/` (`.dmg` and `.app`).

- **Windows** (and Mac): use the included GitHub Actions workflow. It builds
  **both apps for both OSes** and attaches them to a **draft release**:
  ```bash
  git tag v0.1.0 && git push origin v0.1.0
  ```
  Cross-compiling Windows from a Mac is unreliable, so CI is the clean path.
  You can also trigger it manually from the repo's **Actions** tab.

> **Status:** the macOS build is tested and runs. The Windows build has not been
> run yet — it must be produced by CI (above). Everything is cross-platform, but
> treat Windows as unverified until the first CI run succeeds.

---

## Shipping to the office

1. Run the CI build (or build locally) to get four installers:
   `callbro` and `callbro-admin`, each for macOS and Windows.
2. **Your machine:** install **callbro-admin**. Type your name.
   - macOS will ask *"Do you want callbro-admin to accept incoming network
     connections?"* → **Allow** (this opens port **8787** on the LAN).
   - Open ⚙︎ to see your IP in case anyone needs it manually.
3. **Everyone else:** install **callbro** (the client). They type their name and
   should connect automatically ("conectado" top-right). Hand it out however you
   like — network share, USB, email, or your MDM/software-deploy tool.
4. **You:** click **Editar layout**, set the grid to roughly match the office,
   then click a person and click an empty desk to seat them. Repeat.
5. Done. Anyone can now click a seated, online person to call them.

> Your admin machine must be **on** for callbro to work, since it hosts the
> server. To make it always-available, install **callbro-admin** on an always-on
> office PC instead of your laptop.

---

## Antivirus & first-launch warnings

The installers aren't code-signed (fine for an internal LAN tool), so the OS
shows a one-time warning:

- **macOS (Gatekeeper):** right-click the app → **Open** → **Open**. Or run
  `xattr -dr com.apple.quarantine /Applications/callbro.app` (use
  `callbro-admin.app` for the admin app).
- **Windows (SmartScreen):** click **More info** → **Run anyway**.

Because callbro uses the OS's built-in webview (not a bundled browser) and isn't
a script bundle, antivirus generally leaves it alone. If your org wants zero
warnings, sign the builds with an Apple Developer ID / Windows code-signing cert
(the CI workflow has env slots for that).

---

## Troubleshooting

- **"servidor não encontrado":** mDNS may be blocked on your network. Open ⚙︎ on
  the client and enter the admin's address manually, e.g. `192.168.0.10:8787`.
- **No sound / no voice:** click anywhere in the app once (webviews require a
  first interaction before playing audio). The spoken name uses the system's
  pt-BR voice — install one in OS settings if missing; the chime plays
  regardless.
- **No notification popup:** allow notifications for callbro in macOS/Windows
  notification settings.
- **Can't connect after admin restarts:** clients auto-reconnect within a few
  seconds and re-discover the server.

## Where data lives

- Per-machine settings (`config.json`): OS app-config dir for
  `com.fazcapital.callbro`.
- Roster + layout (`state.json`, admin machine only): OS app-data dir for
  `com.fazcapital.callbro`.

## Notes / limits (v1)

- The admin key protects against edits over the network, but a determined person
  with admin rights on *your* machine could read it from your config file. That's
  an acceptable threat model for an internal office intercom — it stops casual
  and remote tampering, not someone sitting at your unlocked admin computer.
- Presence is based on the live connection; a machine that sleeps without
  closing cleanly may show online briefly until the connection drops.
