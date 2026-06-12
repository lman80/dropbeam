# Your own DropBeam relay — free, ~15 minutes

When two devices can't connect **directly** (different networks, strict routers), DropBeam falls back to a **relay** to bounce the encrypted data between them. By default it uses iroh's shared public relays — these work, but they're number0's *canary* (test) servers and can be flaky for far-apart devices (you'll see stalled internet transfers). Running your **own** relay fixes that, and it's **free**.

The relay never sees your files unencrypted — it just forwards bytes. A tiny VM handles it easily.

> Local-network transfers (two devices on the same Wi-Fi) never touch any relay — they go direct. This only matters for transfers **across the internet**.

---

## What you need (all free)
1. **A small always-on Linux VM with a public IP.** Best free option: **Oracle Cloud "Always Free"** — a permanent-free Ampere ARM VM (no time limit, no card charge). Alternatives: Google Cloud `e2-micro` free tier, AWS free tier (12 mo), or any Linux box you already keep online with a public IP.
2. **A free hostname** pointing at that VM. Easiest: **DuckDNS** (https://duckdns.org) — sign in, pick a name like `yourname.duckdns.org`, set it to your VM's public IP. (TLS needs a hostname, not a bare IP.)

---

## Step 1 — Open the ports
On the VM's firewall / cloud security list, allow inbound:
- **TCP 80** (Let's Encrypt cert check)
- **TCP 443** (relay HTTPS)
- **UDP 7842** (relay QUIC — iroh's fast path)

On Oracle Cloud: VCN → Security List → add Ingress rules for those. Also on the VM itself:
```bash
sudo iptables -I INPUT -p tcp --dport 80 -j ACCEPT
sudo iptables -I INPUT -p tcp --dport 443 -j ACCEPT
sudo iptables -I INPUT -p udp --dport 7842 -j ACCEPT
# (Ubuntu's ufw: sudo ufw allow 80,443/tcp && sudo ufw allow 7842/udp)
```

## Step 2 — Install the relay
```bash
# Rust toolchain (one line), then the relay binary:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
cargo install iroh-relay --version ^0.98
```
This gives you the `iroh-relay` binary (`~/.cargo/bin/iroh-relay`).

## Step 3 — Config file
Create `~/relay.toml` (replace the hostname with YOUR DuckDNS name):
```toml
# Public HTTP — used only for the Let's Encrypt cert challenge.
http_bind_addr = "[::]:80"
enable_quic_addr_discovery = true

[tls]
hostname = "yourname.duckdns.org"
cert_mode = "LetsEncrypt"
prod_tls = true            # real (trusted) certificate
https_bind_addr = "[::]:443"
quic_bind_addr = "[::]:7842"
cert_dir = "/home/ubuntu/relay-certs"
```

## Step 4 — Run it (and keep it running)
Quick test first:
```bash
iroh-relay --config-path ~/relay.toml
```
Watch for it to fetch a Let's Encrypt cert and start listening. Then `Ctrl-C` and make it permanent with systemd:
```bash
sudo tee /etc/systemd/system/iroh-relay.service >/dev/null <<'UNIT'
[Unit]
Description=iroh relay
After=network-online.target
[Service]
ExecStart=/home/ubuntu/.cargo/bin/iroh-relay --config-path /home/ubuntu/relay.toml
Restart=always
User=ubuntu
[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl enable --now iroh-relay
journalctl -u iroh-relay -f      # live logs
```
(Adjust the `ubuntu` username/paths to match your VM.)

## Step 5 — Point DropBeam at it
On **BOTH** devices:
1. DropBeam → **Settings → Custom relay (advanced)**.
2. Set **Relay server URL** to `https://yourname.duckdns.org`.
3. Click **Restart**.

Both devices must use the **same** URL. After restart, internet transfers that can't go direct will route through your own relay instead of the public ones.

---

## Verify it's working
1. Turn on **Detailed logging** (Settings → Diagnostics) on both, Restart.
2. Do an internet transfer.
3. Export logs → you should see `using CUSTOM relay https://yourname.duckdns.org` near startup, and the relay traffic going to your hostname instead of `*.iroh-canary.iroh.link`.

## Notes / gotchas
- **Leave it blank to go back** to the public relays — the field is opt-in.
- If the cert fails: confirm port 80 is reachable from the internet and the DuckDNS name resolves to the VM's IP (`dig yourname.duckdns.org`).
- The relay is **forwarding-only** and end-to-end encrypted — it can't read your files.
- Cost: $0 on Oracle Always Free. Bandwidth on the free tier is generous (10 TB/mo on Oracle) — plenty for personal use.
