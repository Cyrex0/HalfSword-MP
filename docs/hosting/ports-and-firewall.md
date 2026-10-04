# Ports and firewall

## Port table

| What | Protocol and port | Direction on the host | Open to the internet? |
|---|---|---|---|
| Dedicated game server (`hsmp-server --bind`) | **UDP 7777** (default) | inbound | **Yes.** Players and the master's reachability check use it. Server-browser queries arrive on the same port. |
| Listen host (hosting from the in-game menu) | UDP, the **HOST PORT** in the multiplayer SETTINGS screen (default 7777, 1024-65535) | inbound | Yes, if players outside your LAN should join |
| LAN discovery (in-game browser) | UDP 7777-7786 (`lan_ports` in `hsmp.cfg`) | the browser sends broadcast queries out; servers answer from their game port | No. LAN only. A LAN server is only found if its port is in this range. |
| Master server (`hsmp-master --bind`) | **TCP 7778** (default) | inbound | Only if you run a master for other people (see [Master server](master-server.md)) |
| Master registration from a game server | TCP to the master's URL (HTTPS, and one WebSocket for the punch relay) | outbound | Outbound only |
| Router port mapping (UPnP, PCP, NAT-PMP) | UDP multicast 239.255.255.250:1900, HTTP to the router, UDP 5351 to the gateway | outbound, LAN only | No |
| STUN (public address of the game port) | UDP from the game port to the STUN servers (3478, 19302, 443) | outbound | Outbound only |
| RCON (`hsmp-server --rcon-bind`) | TCP, no default; these docs use **2345** | inbound, **loopback only** | **Never.** Bind `127.0.0.1:2345` and use an SSH tunnel (see [RCON](rcon.md)) |
| Local master of a listen host | TCP 7778 on `127.0.0.1` | loopback | No |

Notes:

- The master needs **no inbound UDP port**. To check that a registering server is reachable it sends
  one browser query from a temporary UDP socket to the server's game port and waits up to 500 ms for
  the answer on that same socket. The game server's UDP port must therefore be reachable from the
  master; a stateful firewall on the master lets the reply in.
- Players need no inbound ports. Their sidecar talks to the server from an ordinary outbound UDP socket.
- If you change `--bind` (for example to run two servers on one host), open and forward that port instead.
- On a host with more than one address players can reach (a floating or failover IP, a second
  public IP, two uplinks), bind the address players use: `--bind 203.0.113.7:7777`. With
  `0.0.0.0` the operating system picks the reply's source address by its routing table, which
  can be a different address from the one the player sent to. The player's NAT or the sidecar
  then drops every reply: the join times out, and the browser may show no ping.

## Linux firewall

### ufw (Debian, Ubuntu)

```bash
sudo ufw allow 7777/udp comment 'HSMP game server'
# Only if you run a master server for others:
sudo ufw allow 7778/tcp comment 'HSMP master server'
# Make sure SSH stays open before you enable ufw, then:
sudo ufw allow OpenSSH
sudo ufw enable
sudo ufw status verbose
```

### firewalld (Fedora, RHEL, Rocky, Alma)

```bash
sudo firewall-cmd --permanent --add-port=7777/udp
# Only if you run a master server for others:
sudo firewall-cmd --permanent --add-port=7778/tcp
sudo firewall-cmd --reload
sudo firewall-cmd --list-ports
```

Do **not** add a rule for the RCON port. It listens on `127.0.0.1` and is reached through SSH.

Cloud providers (AWS security groups, Hetzner, OVH, Oracle Cloud, Google Cloud and so on) often run
a firewall in front of the VPS as well. Allow UDP 7777 there too.

## Windows firewall

Hosting from the game menu needs nothing here: the launcher adds the rule "Half Sword MP server"
(inbound UDP for the installed `hsmp-server.exe`) at install. For a dedicated server, run in an
**administrator** PowerShell:

```powershell
New-NetFirewallRule -DisplayName "HSMP game server (UDP 7777)" `
  -Direction Inbound -Protocol UDP -LocalPort 7777 -Action Allow -Profile Any

# Only if you run a master server for others:
New-NetFirewallRule -DisplayName "HSMP master server (TCP 7778)" `
  -Direction Inbound -Protocol TCP -LocalPort 7778 -Action Allow -Profile Any

# Check, and remove later:
Get-NetFirewallRule -DisplayName "HSMP*" | Format-Table DisplayName, Enabled, Direction, Action
Remove-NetFirewallRule -DisplayName "HSMP game server (UDP 7777)"
```

To limit the rule to the program, add `-Program "C:\HSMP\server\hsmp-server.exe"` (use your path).
`scripts\install-service.ps1` does not add a firewall rule; add it yourself.

When you host from the in-game menu, Windows may ask once whether `hsmp-server.exe` may use the
network. Allow it for the network types you play on, or add the rule above with your HOST PORT.

## Allow only known players

The server has no join password. To keep a server private, allow the game port only from your
players' IP addresses:

```bash
# ufw: replace the open rule with per-player rules
sudo ufw delete allow 7777/udp
sudo ufw allow from 203.0.113.10 to any port 7777 proto udp
sudo ufw allow from 198.51.100.20 to any port 7777 proto udp
```

```bash
# firewalld
sudo firewall-cmd --permanent --remove-port=7777/udp
sudo firewall-cmd --permanent --add-rich-rule='rule family="ipv4" source address="203.0.113.10" port port="7777" protocol="udp" accept'
sudo firewall-cmd --reload
```

```powershell
# Windows
Set-NetFirewallRule -DisplayName "HSMP game server (UDP 7777)" -RemoteAddress 203.0.113.10,198.51.100.20
```

Home IP addresses change from time to time; update the list when a player cannot connect.

## Router port forwarding (home hosting)

### Automatic: UPnP, PCP, NAT-PMP

`hsmp-server` asks your router to open its UDP port when it starts: UPnP-IGD first, then PCP,
then NAT-PMP (most home routers speak at least one of them when "UPnP" is enabled in their
settings). The mapping has a one-hour lease that the server renews while it runs and removes
when it shuts down cleanly. A router that only accepts permanent mappings gets one; a port that
another device already holds moves the mapping to the next free port (7778, 7779, ...), and the
server lists the port it actually got.

When you host from the menu, the lobby shows the result:

| The lobby says | What it means |
|---|---|
| Router port opened automatically (UPnP) (or PCP, NAT-PMP) | Players outside your network can join. |
| This PC has a public address | No NAT in front of the PC; only the firewall matters. |
| Router port opened, but your router is behind another NAT | Your router's own WAN address is private (CGNAT, or a router behind another router): see [CGNAT](#cgnat). |
| Couldn't open your router port automatically ... Forward UDP 7777 to this PC | The router has UPnP off or does not support it. Forward the port by hand (below). |

Turn it off with **SETTINGS > HOSTING > ROUTER PORT: OFF** (menu hosting), or `--port-map off`
(`HSMP_PORT_MAP=off`) for `hsmp-server`. A server bound to `127.0.0.1` or to IPv6 never maps.
A datacenter VPS has no router to ask: the attempt fails quietly after a few seconds.

### By hand

If the server runs on a PC at home and the router port was not opened automatically:

1. Give the server PC a fixed LAN address (a DHCP reservation in the router, or a static IP).
2. In the router's "Port forwarding" (or "Virtual server", "NAT") page, forward **UDP 7777** from the
   WAN to that LAN address, port 7777. TCP is not needed for the game.
3. Allow the port in the PC's firewall (above).
4. Test from **outside** your network (a friend, or a phone hotspot). Many routers cannot reach their
   own public address from inside ("NAT loopback"), so a test from inside proves nothing.

### No forwarded port: NAT traversal

Even without a forwarded port many players can still join a listen host: the server learns its
public address with STUN (from its own game port), keeps its NAT's mapping alive, and holds a
relay socket to the server list. A joiner whose handshake gets no answer asks the list for a
"punch"; the host's server then sends a few small probes to the joiner's public address, which
opens the host's NAT for that joiner, and the normal handshake follows. The browser shows such a
server with **NAT** in the PING column.

This works when the host's NAT maps the port the same way for every destination (most home
routers). It does not work behind a symmetric NAT (some corporate networks, some mobile
providers, some CGNAT): the listing then says `symmetric`, and a forwarded port, a public IPv4
address or a VPS is needed. STUN goes to public servers (Cloudflare, Google, Nextcloud; change
them with `--stun host:port,...`, turn STUN and punching off with `--stun off`, punching alone
with `--punch off`).

### Testing reachability

`hsmp-query` (shipped next to `hsmp-server`) sends the same query the in-game browser sends. From a
machine outside your network:

```powershell
.\hsmp-query.exe --direct 203.0.113.5:7777
```

It prints its result when it finishes (a few seconds).

A reachable server shows up as a line starting with `S` whose `ping_ms` column is 0 or more and whose
`live` column (the 15th) is `1`. A `ping_ms` of `-2` means no answer: check the port forward, the
firewalls and CGNAT. With `--master <url>` the last two columns are the listing's `nat` and
`punch` (`1` = the list can relay a NAT punch to it right now).

To see what the server itself found, read its log at startup: `router port opened automatically`
(method, external port), `STUN: public endpoint of the game port` (address, `nat=cone` /
`symmetric` / `open`) and `punch relay connected`.

## CGNAT

Some ISPs (often mobile, fibre and cable providers) put customers behind carrier-grade NAT. You then
share a public IP address with other customers, and port forwarding on your router cannot work.

Signs: the WAN address shown in your router is in `100.64.0.0/10` (100.64.x.x to 100.127.x.x) or
another private range, or it differs from the address a "what is my IP" site shows.

What you can do:

- Ask your ISP for a public IPv4 address (some do this free or for a small fee).
- Rent a small VPS and run the server there (see [Linux](linux.md) or [Docker](docker.md)).
- If you and your players all have IPv6, bind the server to IPv6 (`--bind [::]:7777`) and open the
  port for IPv6 in your router's firewall. Whether an IPv6 socket also accepts IPv4 depends on the
  operating system (Linux usually yes, Windows no), so test both. Players join by a host name
  with an IPv6 (AAAA) record; the in-game address box takes host names and IPv4 addresses, not
  IPv6 literals.

Behind CGNAT the router port cannot be forwarded, but NAT traversal (above) still works when the
provider's NAT maps ports the same way for every destination; the server list shows the server
with `nat` = `cone` (or `double` when your router mapped the port behind it). HSMP has no relay that
carries game traffic: behind a symmetric CGNAT, use one of the options above.
