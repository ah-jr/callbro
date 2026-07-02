# Deploying the callbro server on EasyPanel

The server lives in `server/` and is built by the root `Dockerfile`.

## 1. Create the app
1. In EasyPanel: **Project → + Service → App**.
2. **Source: GitHub** → connect and pick `ah-jr/callbro`, branch `main`.
   (For a private repo, authorize EasyPanel's GitHub app so it can pull.)
3. **Build method: Dockerfile** (path `Dockerfile`, context `/`).

## 2. Configure
- **Environment variables:**
  - `CALLBRO_ADMIN_KEY` = a strong admin password (only you will type this to edit the layout).
  - `CALLBRO_JOIN_SECRET` = the team code (you share this with employees; they type it once).
- **Port:** the container listens on **8080** — set that as the app's exposed/proxy port.
- **Volume:** add a persistent volume mounted at **`/data`** so the roster + seat
  layout survive restarts and redeploys.

## 3. Domain + HTTPS
- Under **Domains**, add a domain — EasyPanel gives you one automatically and
  provisions HTTPS. WebSockets work through its proxy with no extra config.
- Your client URL is then **`wss://<that-domain>`** (no port needed — 443 via the proxy).

## 4. Deploy
Hit deploy. Check the logs — you should see:
```
callbro-server listening on 0.0.0.0:8080 (state: /data)
```
If you see a `WARNING: CALLBRO_ADMIN_KEY is empty` / `CALLBRO_JOIN_SECRET is empty`
line, the corresponding env var didn't get set.

## Updating the server later
Push to `main` (or click **Deploy** in EasyPanel) and it rebuilds. Roster/layout
persist in the `/data` volume.
