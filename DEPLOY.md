# Deploying the callbro server (AWS Lightsail Container Service)

The server lives in `server/` and is built by the root `Dockerfile`. It runs on
an **AWS Lightsail container service** in the `casmedia` account
(`762233729991`), region `sa-east-1`.

- **Service:** `callbro` (power `nano`, scale 1)
- **Public URL:** `https://callbro.3z77v07vd48by.sa-east-1.cs.amazonlightsail.com/`
- **Client connects to:** `wss://callbro.3z77v07vd48by.sa-east-1.cs.amazonlightsail.com`
  (Lightsail terminates TLS; WebSockets pass through over HTTP/1.1)
- **Env vars:** `CALLBRO_ADMIN_KEY`, `CALLBRO_JOIN_SECRET`, `PORT=8080`, `CALLBRO_STATE_DIR=/data`
- **Health check:** `GET /health` → `200 ok` (the server answers any non-WebSocket
  request on the same port with a plain 200, so the load balancer probe passes).

> ⚠️ **No persistent volume.** Lightsail container services can't mount disks, so
> the roster + seat layout in `/data/state.json` are wiped on every redeploy.
> Re-seed after a deploy by re-issuing `set_grid` + `assign` with the admin key
> for online users. (Same trade-off as the old EasyPanel setup.)

## Prerequisites
- AWS CLI with the `casmedia` profile.
- Docker running locally.
- The `lightsailctl` plugin (needed for `push-container-image`):
  ```bash
  curl -fsSL "https://s3.us-west-2.amazonaws.com/lightsailctl/latest/darwin-arm64/lightsailctl" \
    -o ~/.local/bin/lightsailctl && chmod +x ~/.local/bin/lightsailctl
  export PATH="$HOME/.local/bin:$PATH"
  ```

## Updating the server (push a new image)
```bash
cd /path/to/callbro
export PATH="$HOME/.local/bin:$PATH"

# 1. Build for the Lightsail container arch (amd64), from the repo root.
docker buildx build --platform linux/amd64 -t callbro-server:latest --load .

# 2. Push it — note the ":callbro.server.N" reference it prints.
aws lightsail push-container-image \
  --service-name callbro --label server --image callbro-server:latest \
  --profile casmedia --region sa-east-1

# 3. Deploy that image (edit deploy.json's "image" to the ref from step 2).
aws lightsail create-container-service-deployment \
  --service-name callbro --cli-input-json file://deploy.json \
  --profile casmedia --region sa-east-1
```

`deploy.json`:
```json
{
  "containers": {
    "callbro": {
      "image": ":callbro.server.N",
      "environment": {
        "CALLBRO_ADMIN_KEY": "…",
        "CALLBRO_JOIN_SECRET": "…",
        "PORT": "8080",
        "CALLBRO_STATE_DIR": "/data"
      },
      "ports": { "8080": "HTTP" }
    }
  },
  "publicEndpoint": {
    "containerName": "callbro",
    "containerPort": 8080,
    "healthCheck": {
      "path": "/health", "successCodes": "200-299",
      "intervalSeconds": 10, "timeoutSeconds": 5,
      "healthyThreshold": 2, "unhealthyThreshold": 5
    }
  }
}
```

## Verify
```bash
URL=callbro.3z77v07vd48by.sa-east-1.cs.amazonlightsail.com
curl -s -i https://$URL/health              # -> HTTP 200 "ok"
# WebSocket handshake must be HTTP/1.1 (browsers do this automatically):
curl -s -i --http1.1 -H "Connection: Upgrade" -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
  https://$URL/                              # -> HTTP 101 Switching Protocols
```

## Client
`src/main.js` → `DEFAULT_SERVER` points at the `wss://` URL above. Changing it
requires shipping a new app release (tag `vX.Y.Z`); installed v0.3.0+ clients
auto-update and pick up the new server.
