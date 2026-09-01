# AITerm Relay

AITerm Relay is a blind TCP relay for the existing AITerm remote gateway. It
does not implement sessions, pairing, device trust, terminal rendering, or file
transfer. The Android app still establishes pinned TLS directly with the
desktop and still authenticates with its remembered device key. The relay only
sees encrypted byte counts, timing, a random route id, and network addresses.

Connection order is:

1. local LAN address;
2. VPN/overlay address;
3. the route-specific relay hostname.

The desktop maintains one outbound WebSocket to the connector listener. A
phone connects to the ingress listener with SNI
`<route-id>.<public-domain>`. The ingress peeks at (but does not consume) that
ClientHello, opens a multiplexed stream to the matching desktop connector, and
then copies the complete TLS stream unchanged.

## Build and test

```sh
cargo test --manifest-path relay/Cargo.toml
cargo build --release --manifest-path relay/Cargo.toml
```

The daemon takes one JSON configuration path:

```sh
./relay/target/release/aiterm-relay /etc/aiterm-relay/relay.json
```

`relay.example.json` documents the configuration. Connector tokens are random
secrets held only by a desktop; the server stores their lowercase SHA-256
hashes. A route id is public, random, lowercase DNS-label text of 8–63
characters.

Generate a token and its server-side hash with standard tools:

```sh
route="desk-$(openssl rand -hex 12)"
token="$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=\n')"
printf '%s' "$token" | sha256sum
```

The token itself goes into AITerm Settings. Only its hash goes into
`relay.json`.

## Production edge

The intended single-VM layout uses two public ports:

- TCP 443: raw phone ingress owned by `aiterm-relay`;
- TCP 8443: ordinary TLS/WebSocket edge, proxied to the connector listener on
  `127.0.0.1:8080`.

This keeps application TLS end-to-end on port 443 while allowing a conventional
ACME proxy to manage the connector certificate on 8443. The desktop setting is
then `wss://control.<domain>:8443/v1/connect`; a route's public endpoint is
`<route-id>.<domain>:443`. DNS needs records for `control.<domain>` and the
wildcard `*.<domain>`.

The Google Cloud VM, firewall rules, DNS records, ACME proxy, and secrets are
deliberately not provisioned by this branch. `deploy/aiterm-relay.service` is a
service template for that later step.
