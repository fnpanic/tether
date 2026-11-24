# Device health

Tether can show the health of the machines it supports in the host list: a dot per host, the
failing checks first, and a built-in view of the hosts that need attention. The data comes from a
monitoring agent, a separate program on each machine that enrols once and then posts signed
reports to the relay. This page is for relay operators, administrators, technicians and anyone
who wants to write an agent. The wire details are in [api.md](api.md#device-status-health-reports).

## What it is, and what it is not

- A store on the relay for the last report, the recent changes and the last contact of each
  enrolled device, and a health view in the technician app that reads it.
- Read-only. Nothing in this feature runs a command on a device, changes a device's settings or
  opens a connection to it. A device never listens: the agent only sends reports to the relay,
  over the same outgoing TLS connection (TCP 443) the other Tether apps use.
- Not part of the remote session. Seeing a host's health needs no session, and a session needs no
  agent.
- Not end-to-end encrypted: the relay stores the reports in readable form, and its operator and
  the team members can read them ([Privacy and security](#privacy-and-security)).
- Not a monitoring system by itself: no alert mail, no graphs. The relay's own gauges feed
  Prometheus ([Metrics](#metrics)); the technician app notifies on Windows
  ([Notifications](#notifications)).

The agent is not part of this repository. Any program can be an agent: the enrolment and the
report format below are open and checked by the relay, which does not interpret the checks.

## How it fits together

```
 device                         relay (tether-api)                     technician PC
 ┌──────────────┐  enrol once   ┌────────────────────────────┐        ┌──────────────────┐
 │ monitoring   │ ────────────► │ status/keys.jsonl (pins)   │        │ Tether host list │
 │ agent        │  signed       │ status/<uuid>.json         │ ◄───── │ Health column,   │
 │ (own key)    │  reports ───► │ status/<uuid>.history…     │ member │ Health section   │
 └──────────────┘               │ status/summary.json        │ signed └──────────────────┘
        ▲                       └────────────────────────────┘ reads
        │ agent and check-pack updates:  download/<channel>/monitor.json (signed index)
```

1. An administrator creates an **enrolment token** (valid 1 to 30 days) and gives it to the
   agent's installer.
2. The agent generates its own Ed25519 key and enrols: `POST /api/status/enroll` with the token,
   its uuid, its public key and, when Tether is installed, its Tether ID. The relay checks the
   token, pins the key to the uuid and answers how long to wait before the next post. The Tether
   ID the agent names is only a **claim**: it becomes the device's Tether ID when the token was
   issued for that ID, or when a team member confirms it ([Linking an agent to its
   host](#linking-an-agent-to-its-host)).
3. The agent posts a **full report** (everything) and, between changes, **heartbeats** (the state
   in a few fields). Each post is signed with the device key. The relay sets the pace.
4. Technicians that are in the team roster read the summary of all hosts and one host's report
   through their own member key ([api.md](api.md#team-technician-endpoints)). The app merges the
   summary into the host list by the verified Tether ID only. A machine with an agent but without
   Tether, or whose claim nobody confirmed yet, appears as a row of its own without a connect action.
5. Agent and check-pack updates come through a second signed release index per channel,
   `monitor.json`. Installed Tether clients never read it.

Status is stored per relay. An agent should enrol on every relay of its list and post to one at a
time. The technician app asks every relay and shows the newest answer for each device.

## For the relay operator

### Enabling and switching off

It is on by default and needs `ADMIN_PUB_KEY` (the enrolment token is signed with the admin key).
Without it `POST /api/status/enroll` answers `503 status enrollment needs ADMIN_PUB_KEY`. It also
needs `RELAY_HOST` (the relay's own host names, as for the team routes): devices sign each post for
the relay's host name, and without `RELAY_HOST` every device post and removal is refused with
`401 this relay does not know its own host name …`. Settings
are environment variables of tether-api, normally in `/etc/tether/api.env`, read at start:

| Setting | Default | Meaning |
|---|---|---|
| `STATUS` | on | `STATUS=off` switches the whole feature off (below). Any other value leaves it on. |
| `STATUS_TARGET_RPS` | `10` | Posts per second the relay paces its enrolled devices to (at least 1). |
| `STATUS_MAX_DEVICES` | `20000` | Enrolled devices at most. A new device beyond it gets `507 too many devices (STATUS_MAX_DEVICES)`. Devices already enrolled are not affected. |
| `STATUS_TRACE_LOCK` | off | `1`: log every hold of tether-api's state lock over 20 ms with the route that held it (`lock held <ms> ms: …`). A diagnosis setting for load tests; leave it off. |

A value that is not a number stops tether-api from starting, as for the other numeric settings.
The default of 20 000 is the size the relay was load-tested with inside `MemoryMax=64M`; a larger
fleet needs a larger `MemoryMax` ([scaling.md](scaling.md)).

`STATUS=off` makes every status route answer `503 status reporting is off` with
`Retry-After: 3600`: enrolment, posts, a device's own removal; the team routes `GET /api/team/status`,
`GET /api/team/status/<uuid>`, `POST` and `DELETE /api/team/status/<uuid>/link`,
`POST /api/team/status/<uuid>/allow-claim` and `DELETE /api/team/status/<uuid>` (after the
member's signature, and for the changing routes the `write` check); and the admin routes
`DELETE /api/admin/status/<uuid>`, `POST /api/admin/status/revoke`, `GET /api/admin/status/revoked`
and `GET /api/admin/status/devices`. Posts are refused before their body is read. Nothing else on the relay changes, and the
technician app says that health reporting is switched off on this server. The files in `status/` stay and
come back when the setting is removed. Pins that have not seen a post for 180 days are still
dropped while it is off.

Enrolment is refused when the same Tether ID or uuid is pinned to another key, and limited to 30
new pins or refused tokens per minute per IP (an IPv6 client counts as its /64).

### Storage and backup

Everything is in `/var/lib/tether-api/status/` (directory `0700`, files `0600`):

| File | Content | If it is lost |
|---|---|---|
| `keys.jsonl` | The pins, one line per change: `set` (enrolment, link, unlink, allowing a new claim, a claim taken from an accepted report: uuid, device public key, the token's label and id, time of enrolment, the verified Tether ID, the claimed one, who linked it and when, `no_claim`, the customer reference), `used` (a token issued for one Tether ID was used, with its expiry), `dismissed` and `forgotten` (the key of a device whose link was dismissed released its own pin; kept 180 days, at most 10 000: when full, the oldest goes with a `forgotten` line, also when the file is read at start) and `unpin` (removal); `pin` lines from before are still read. Appended and synced for each change, compacted in the background. | Every device must enrol again, with a valid token ([Locked state](#the-locked-state)). **Back this one up.** |
| `revoked.json` | Revoked tokens: `{"<token id>": <its expiry>}`, at most 10 000; an entry is dropped once the token would have expired anyway. Written by the status writer, synced before a revocation is answered. | Revoked tokens that have not expired work again. **Back this one up.** |
| `<uuid>.json` | The last full report, byte for byte as the agent sent it. | Shown as "No full report yet" until the device posts a full one. |
| `<uuid>.history.jsonl` | One line per change of a check (`id`, `from`, `to`, `time`, and `at`, the relay's receive time). Kept 90 days and at most 2000 lines. | The history of that device. |
| `seen.json` | Last post of each device: time, `seq`, the `next` it was told and the summary entry (at most 2 KiB). Written at most every 5 minutes when something changed, and at a normal stop; a hard kill loses up to 5 minutes of last-seen times. | The hosts leave the summary until each posts a full report again (a heartbeat is answered `409 send a full report`). |
| `summary.json` | The cached team summary, rebuilt at most every 30 s. | Rebuilt on the next request. |

A heartbeat writes no file, with one exception: the first post that names a Tether ID for a device
with neither an ID nor a claim (and whose claims are allowed) appends that claim to `keys.jsonl`,
once per device. Otherwise it updates `seen.json`'s content in memory. A full report is written
and synced before the relay answers. Writing `seen.json`, the summary, compacting `keys.jsonl` and
trimming history files run away from the request path, so a flush of thousands of devices does not
hold up heartbeats and policy requests. A failed background write is counted
(`tether_api_status_write_errors`) and logged at most once a minute.

A device whose pin has had no post for 180 days (never posted: 180 days since enrolment) is
removed with its files, as in the device list.

Back up `keys.jsonl` and `revoked.json` with the rest of `/var/lib/tether-api` ([deployment.md](deployment.md)).
Reports and history are worth keeping only if you want the history across a restore.

### Load behaviour

The relay also carries session signalling and Tether's updates, so health reporting is built to
cost little and to give way:

- **The relay sets the pace.** Every answer carries `next`, the seconds until the device posts
  again: `min(3600, max(300, ceil(enrolled / STATUS_TARGET_RPS)))`. With the defaults that is 300 s
  up to 3 000 devices and 2 000 s at 20 000. Raising `STATUS_TARGET_RPS` shortens it; lowering it
  stretches it. Above `3600 * STATUS_TARGET_RPS` devices the cap applies and the real rate
  exceeds the budget.
- **Cheap checks first.** A post is refused for size (64 KiB, from `Content-Length`, before the
  body is read), a missing or malformed header, a second post from the same uuid within 60 s,
  too many refused posts from that uuid (10 per minute) and too many failed signatures from the
  IP (60 per minute), all before any signature is verified and before the body is parsed. A forged
  post that names a device's uuid does not use up that device's allowance.
- **Heartbeats stay in memory.** Only full reports touch the disk, apart from a device's first
  claim (one `keys.jsonl` line, once per device).
- **The summary is cached.** `GET /api/team/status` is built from a snapshot outside the state
  lock at most every 30 s and sent from a file. Clients send `If-None-Match` and get `304`
  while nothing changed. A host that is removed disappears from it within 30 s. Each host's
  entry holds at most 8 products and 10 failing checks (with `more_products` and `more_faults`
  giving what was left out); the host's report has them all.
- **Staleness is computed on read.** The relay keeps no timers: a device is stale when
  `now > last_seen + 3 × next`, using the `next` it last told that device.
- **Load test.** `tools/load/status-run.sh` posts for simulated devices at the paced rate while it
  measures heartbeat and policy latency and the relay's memory. The numbers are in
  [scaling.md](scaling.md).

### The locked state

`keys.jsonl` is the only record of which key belongs to which uuid, and `revoked.json` of which
tokens no longer count. When tether-api starts and cannot read one of them (a line or document
that is not valid, or the file is unreadable), or a status file cannot be written while the store
opens (a full or read-only disk during a migration), it does not guess: an empty list would hand every
uuid to whoever holds a token, or make revoked tokens valid again. Instead all status routes answer
`503 status store unavailable` with `Retry-After: 3600`, nothing under `status/` is written, and
tether-api logs the file name at start and then at most once an hour. The rest of the relay runs normally:
sessions, heartbeats, policy and updates do not depend on the status store. A last line cut short
by a crash is not an error: it is dropped and logged.

Recovery, then restart tether-api (`sudo systemctl restart tether-api`):

1. Restore the file the log names from a backup, or repair it.
2. Without a backup of `keys.jsonl`: move it away. Every device then has to enrol again, with a
   token that is still valid, and Tether IDs must be linked again. Remove the stale `<uuid>.json`
   and `<uuid>.history.jsonl` files of devices that do not come back.
3. Without a backup of `revoked.json`: move it away and revoke again the tokens that have not
   expired yet (`monitor-publish tokens` lists the ones this PC revoked).

While the store is locked, the 503 answers count in the `ApiErrors` alert
([monitoring.md](monitoring.md#alerts)) like any other 503.

### Metrics

On tether-api's `/metrics` ([monitoring.md](monitoring.md)):

| Metric | Meaning |
|---|---|
| `tether_api_status_hosts{state="ok\|warn\|crit\|stale"}` | Hosts with a full report by state. `stale` takes precedence over the reported state; `unknown` counts as `ok`. Counted at each scrape. |
| `tether_api_status_write_errors` | Failed writes of the status store since start (counter): `seen.json`, `summary.json`, `revoked.json`, history trimming, the `keys.jsonl` compaction, syncs and expiry, and a claim line whose sync failed. |
| `tether_api_release_index_expires_timestamp{channel,index="monitor"}` | When the channel's `monitor.json` expires (unix seconds); the series without `index` are Tether's `releases.json`. |
| `tether_api_requests_total{endpoint="status"}` | Posts, enrolments and removals by HTTP code (`/api/status`, `/api/status/*`). |
| `tether_api_requests_total{endpoint="team"}` | The member routes (`/api/team/*`), the summary included. They used to count as `other`. |

Ideas for alert rules (examples, not part of the shipped rules):

```
# hosts critical (with `for: 15m` in the rule)
tether_api_status_hosts{state="crit"} > 0
# many hosts stopped reporting at once: usually the network or the relay, not the machines
tether_api_status_hosts{state="stale"} > 0.2 * sum(tether_api_status_hosts)
# the status store is locked or switched off
sum(rate(tether_api_requests_total{endpoint="status",code="503"}[10m])) > 0
# background writes failing
increase(tether_api_status_write_errors[15m]) > 0
# monitor.json expires within 7 days: run monitor-publish refresh
tether_api_release_index_expires_timestamp{index="monitor"} - time() < 7 * 86400
```

With several relays a device posts to one at a time, so each relay's gauges count the devices that
post to it.

## For the administrator

### Creating an enrolment token

A token is signed with the admin key, names a label (1 to 64 characters, for your own records),
has a random id and expires after 1 to 30 days. Two kinds:

- **For one Tether ID** (`--tether-id`): the device that enrols with it gets exactly that Tether
  ID, without anyone confirming it, and the token works for one device **per relay**. Each relay
  keeps its own record of the token's use, so a copy of the token can still link a second
  device on a relay the first one has not reached yet; the host list then shows a [link
  conflict](#link-conflicts). Give such a token only to the agent installer on that machine, with
  a short lifetime (a day). Use it when you install the agent on a known machine, for example from
  a remote session. Only the token's first use links: after a member unlinks the device, the same
  token enrolling again changes nothing.
- **Without a Tether ID**: any number of devices can enrol with it until it expires or is revoked.
  A Tether ID they name is only a claim, shown in the host list until a team member links it
  ([Linking an agent to its host](#linking-an-agent-to-its-host)).

Either kind can also carry a **customer reference** (`--customer`, 1 to 32 characters
`A-Z a-z 0-9 . _ -`): the relay sets it on every device that enrols with the token
([Customer reference](#customer-reference)). Use one token per customer.

```bash
bash scripts/status-token.sh office 7                 # label, days (1 to 30, default 7)
bash scripts/status-token.sh front-desk 1 123456789   # for that Tether ID only
bash scripts/status-token.sh example 30 "" 4711       # for customer 4711 ("" = no Tether ID)
```

The script builds `tools/monitor-publish` when it is missing and reads the admin key from Tether
Manage's own key file (`MANAGE_KEYS`, default `%APPDATA%\Tether-Manage\keys.json`). It asks for
Tether Manage's passphrase, or takes it from `MANAGE_PASSPHRASE`; the passphrase is never an
argument. The token is printed on standard output, its id, label, expiry and customer on standard error.
Give the token to the agent's installer. Treat it like a password for as long as it is valid: see
[what a token holder can do](../SECURITY.md#device-status-health-reports). The tool keeps a
journal of the tokens it issued (id, label, expiry, Tether ID, customer; never the token itself) in
`monitor-tokens.json` next to the key file; `monitor-publish tokens` lists it and marks expired,
revoked and partly revoked tokens.

### Revoking a token

```bash
monitor-publish revoke --id <token id> --relay https://relay.example.com,https://relay2.example.com
monitor-publish revoke --token-file token.txt --relay …     # or --token - (the token on standard input)
```

With `--id` the expiry comes from the journal. For a token issued on another PC, give the token
itself (`--token-file`, `--token -`): the tool checks it with the admin key and takes id and
expiry from it. `--exp <unix seconds>` is accepted only for an id the journal does not know, or
later than the journal's expiry: a revocation is kept until the expiry it names, so an earlier one
would end it too soon. A token that has expired is refused anyway; the tool and the relay
(`400 exp has passed`) say so instead of storing nothing. Every relay gets an admin-signed
`POST /api/admin/status/revoke` and refuses the token from then on (`401 token revoked`). The
output names each relay and ends with "revoked on N of M relays"; the journal marks the token
revoked only when every relay accepted, else "partly revoked" with the relays that failed, and the
tool exits non-zero (run it again for those). **Revoking a token does not remove the devices that
already enrolled with it.** List them and remove the ones you do not want:

```bash
monitor-publish devices --token <token id> --relay https://relay.example.com
monitor-publish remove --uuid <uuid> --relay https://relay.example.com,https://relay2.example.com
```

### Publishing an agent or a check-pack update

The agent (`monitor/windows-x86_64`, file `tether-monitor-<version>.exe`) and its check packs
(`monitor-packs/any`, file `monitor-packs-<version>.json`) are published to
`download/<channel>/monitor.json`, a signed index with its own serial per channel, in the same
format and with the same staged rollout as the Tether release index. The file must lie next to its
`.sig`, signed with the updater key; it is checked against that key before anything is sent.
The commands and the relay routes are in
[updates.md](updates.md#device-status-agent-monitorjson-and-enrolment-tokens):

```bash
bash scripts/monitor-publish.sh stable monitor 0.1.0 dist/tether-monitor-0.1.0.exe \
    https://relay.example.com,https://relay2.example.com standard
```

`monitor.json` expires 30 days after the last publish or refresh. Run
`bash scripts/monitor-refresh.sh <channel> <relays>` (`monitor-publish refresh`) about once a
month, a manual step from the admin's own machine: it signs the published index again, unchanged
but for serial + 1 and a new expiry, and prints the new expiry. The metric
`tether_api_release_index_expires_timestamp{index="monitor"}` shows it on each relay. What an agent
does with an expired index is defined by the agent, not here.

Publishing to `monitor.json` does not move Tether's release generation and never reaches installed
Tether clients. Every status answer carries `releases`, an opaque hint that changes when a
`monitor.json` on that relay changes; an agent compares it for inequality and then fetches its
channel's `monitor.json`.

### Removing a device

A device removes itself on uninstall (`DELETE /api/status`, signed with its own key). To remove
one yourself, for a lost machine, a conflicting enrolment (`409 uuid is enrolled with another key`
or `409 tether_id is enrolled with another key`) or a reused ID:

- `monitor-publish remove --uuid <uuid> --relay <relays>` sends an admin-signed
  `DELETE /api/admin/status/<uuid>` to every relay, prints the result per relay ("removed", "not
  enrolled there" or the error) and exits non-zero if one failed. `monitor-publish devices
  --relay <relays>` lists the enrolled devices (uuid, Tether ID, claimed ID, host name, customer,
  enrolment and last report): all of them, with `--token <id>` those of one token, with
  `--customer <ref>` those of one customer.
- A team member with the `write` permission can do the same from the host list: **Remove from
  health monitoring…** in the host's Health section
  ([Removing a device from the host list](#removing-a-device-from-the-host-list)).

Both delete the pin with its link or claim, the report and the history. The line `status: pin of
<uuid> removed by …` goes to the journal. If the agent on that computer still holds a valid
enrolment token, it enrols again by itself and comes back as an agent that is not linked to any
host (with a claim if it names a Tether ID). Uninstall the agent there, or revoke the token. A
token issued for one Tether ID stays used on that relay: it does not enrol a device there again
after the removal.

**The record of team actions.** Linking, unlinking, allowing a new claim and removing write one
line each to tether-api's journal on stderr (`status: <uuid> linked to <id> by team member
<name>`, `… dismissed by team member <name>`, `… removed by team member <name>`), and the pin
keeps `linked_by` and `linked_at` of the current link. There is no separate audit log of these
actions: keep the journal (systemd's) as long as you need that record.

### Customer reference

A device can carry a customer reference, for example `4711`: a label that groups the devices of
one customer inside this deployment. Only an enrolment token sets it:

- A device that enrols with a token naming a customer gets that customer. The relay keeps it with
  the pin in `keys.jsonl`.
- **Changing it:** the same device (same uuid and key) enrolling again with a token that names a
  customer other than the one it has (or any customer, if it had none) gets the new one only when
  the enrolment carries the device's own signature (the `Tether-Sig: device-v1` header of a
  status post). Without it the relay answers `401 changing the customer needs the device's
  signature` and the device keeps its customer. When it changes, tether-api writes `status: customer of <uuid> set to
  <customer> by token <id>` to its journal. The pin keeps the token it first enrolled with, so
  `monitor-publish devices --token` still lists it under that one; only the journal line names the
  token that changed it. While the device's pin exists, knowing its uuid and public key is not
  enough to move it (the public key is on the device and in the relay's `status/keys.jsonl`; no
  route returns it). Where a relay has no pin for that uuid (the device was removed, the pin was
  dropped after 180 days without a report, or the device never enrolled on that relay), a first
  enrolment is accepted unsigned (old agents) or signed with the key in the request, so a token
  holder who also knows that uuid and public key could create the pin there under the token's
  customer.
- A token without a customer changes nothing: a device that has one keeps it.
- To take a customer away, remove the device ([Removing a device](#removing-a-device)) and let it
  enrol again with a token without one.
- **A device cannot set or change it.** Neither the enrolment request nor a report has a field for
  it; a request or report that names one is refused (`400`, unknown field).

Like everything in the status store it is kept per relay: a device that enrols again on one relay
only has the new customer there. `monitor-publish devices --customer 4711 --relay <relays>` lists
one customer's devices. The relay sends every device and the tool keeps those whose customer is
exactly `4711`.

A relay from before the customer reference refuses a token that names one
(`401 enrollment token: not an enrollment token`). Update tether-api on every relay before you
issue such tokens.

The customer reference is not a security boundary: every team member sees every device, whatever
its customer, and whoever holds a valid token can enrol new devices under that token's customer.
An existing device moves to another customer only with its own key
([SECURITY.md](../SECURITY.md#device-status-health-reports)). For
organisations whose technicians must not see each other's machines, see
[several-organisations.md](several-organisations.md).

## For the technician

Health needs the technician to be in the team roster ([management.md](management.md)), because
the reads are member-signed; no `write` or `audit` permission is needed to read. Linking,
unlinking and removing an agent need `write`. The host list reads the
summary from every relay when it opens and every 5 minutes while it is open, and **Sync now** in
the team dialog reads it too. Search terms and the Needs attention view are also described in
[advanced.md](advanced.md#query-syntax).

### The Health column

The list shows a dot and a short text per host; the words are there so a state never rests on
colour alone. The column appears as soon as any host has health data and gets wider with the
window. Products, Agent and Customer are added in a wide window; Customer shows the host's
[customer reference](#customer-reference), or "—" without one. Click the **Health** header to sort
the worst hosts first, the **Customer** header to sort by customer.

| Shown | Meaning |
|---|---|
| Critical, Warning (with counts, for example "1 critical, 2 warnings") | The agent's `overall` is `crit` or `warn`. |
| No health report for 25 min | Stale: no post for more than three times the interval the relay told the device. |
| Setup needed | Nothing failing, but checks are waiting to be configured (the report's `setup` count). |
| OK | Nothing failing. Checks in state `unknown` are not a fault. |
| No agent | The host has no health data. |
| Link conflict | Two devices are linked to this host on different relays ([Link conflicts](#link-conflicts)). |

Machines with an agent but without Tether, and agents whose claim to a Tether ID nobody confirmed
yet, show as rows of their own without a Tether ID. Connecting to such a row says that Tether is
not installed on the machine.

### Linking an agent to its host

Nothing links an agent to a host row automatically. An agent enrolled with a token issued for its
Tether ID is linked from the start. Any other agent that names a Tether ID only claims it: its
row's Health section says "Claims to be 123456789 (FRONT-DESK-1). Confirm only if this is that
computer.", with the agent's own host name and when it enrolled. If that is the machine, a member
with the `write` permission chooses **Link to this host** and confirms once. The app asks every
relay; each pins that Tether ID to the agent (`409` when another agent has it already), records
who linked it and when, and the agent's health shows on the host row. Without `write` the button
is disabled.

A linked host's Health section says who linked it and when ("Health linked by Anna Berger, …", or
by the token issued for the host). **Unlink** (members with `write`) removes the link and the
agent's claim; the agent becomes a row of its own that shows "Link dismissed". It cannot claim a
Tether ID again, neither by enrolling again (also not with the token issued for the host, whose
first use is past) nor by its reports, which are still accepted with the field ignored. A member
with `write` can choose **Allow a new claim**: the agent's next enrolment or report names its
claim again, and the claim waits to be linked like any other. Only the first use of a token issued
for that ID, or a member's Allow a new claim followed by a Link, can link it again. A dismissed
device's key claims nothing under any uuid: enrolling another uuid with the same key, while the
dismissed pin is still there or after the device released it itself (`DELETE /api/status`), gives a
pin with `no_claim` and no claim, and its reports claim nothing either. Allow a new claim on every
dismissed pin of that key, or the first use of a token issued for its ID, ends this. A pin removed
by an admin or a member leaves no such record. A device cannot link, unlink or change its Tether ID itself: a
report naming another ID is refused (`409`).

**The boundary.** Whoever holds a valid enrolment token that is not issued for a Tether ID can
always enrol a *new* device (a new key) that names any Tether ID as its claim. A claim joins
nothing, and it stays fixed for that device until a technician acts. `no_claim` stops the same
device (the same key) from asking again, also when it drops its own pin and enrols anew. To stop
claim rows from an unwanted source, revoke the token and remove the devices enrolled with it
(`monitor-publish devices --token <id>`, then `remove`).

**Link to this host** is disabled when the claimed host already has a linked health agent (from
any relay): "This host already has a linked health agent (FRONT-DESK-1, linked …). Unlink that
one first if this is its replacement." Linking a second one would only make a [link
conflict](#link-conflicts) that hides that host's health. Each relay refuses it on its own too
(`409 tether_id is pinned to another device`), but only for the devices it knows.

The Link dialog shows the agent's own host name; when it differs from the name of the host it
claims to be, the dialog says so in a highlighted line.

### Link conflicts

A token issued for one Tether ID is single-use per relay, not across relays, so two devices can
end up linked to the same Tether ID on different relays (a copied token, or a reinstalled machine
that enrolled on another relay first). The host list never guesses which one is right: nothing is
shown on that host, its Health column says **Link conflict**, its detail says "Health link
conflict: two devices are linked to this host on different relays", and both agents appear as
rows of their own (search `id:m:`), each marked as conflicting, with **Unlink** and **Remove from
health monitoring…**. Unlink or remove the one that is not that computer (compare the host name
and enrolment time in its Health section). A conflict counts as needing attention.

### Removing a device from the host list

**Remove from health monitoring…** in the Health section (members with `write`) asks once, naming
the host, then removes the agent's enrolment, link and stored reports on every relay. As the
dialog says: if the agent on that computer still holds a valid enrolment token it enrols again by
itself, and comes back as an agent that is not linked to any host. Uninstall the agent there, or
ask an admin to revoke the token.

### Search terms

The terms work in the search box and in saved searches. In a tag rule they are plain words, as
they were before health data existed, so a shared rule tags the same hosts on every desktop of the
team, older ones included; the Rules dialog says so in one line.

| Term | Matches |
|---|---|
| `health:crit` `warn` `stale` `setup` `ok` `none` `conflict` | Hosts in that health state; `none` = no agent, `conflict` = two devices linked to the host on different relays |
| `product:records` | A detected product whose name contains the text |
| `version:26.3` | A product version starting with the text; next to `product:` the same product must match both |
| `role:server` `client` `single` | A detected product role |
| `check:backup` | A check whose id contains the text is not OK |
| `agent:0.1` `agent:none` | Agent version starting with the text; `none` = no agent |
| `customer:47` `customer:none` | A [customer reference](#customer-reference) starting with the text, in any case (`customer:47` finds `4711`, `customer:711` does not); `none` = a host that reports health but has no customer reference (hosts without an agent are `health:none` and `agent:none`). |
| `since:3d` | Warning or critical for at least that long (earliest failing check), units `m h d w` |

Examples: `health:crit -is:offline` (critical and online, so a connection is possible),
`check:backup since:2d` (a backup check failing for two days or more),
`product:records version:26.3 health:warn`, `agent:none os:win` (Windows hosts without an agent).
In the search box `role:` used to be a plain word that found tags; `#role:server` still finds the
tag. In tag rules `role:server` is still that plain word.

### Needs attention

In the library on the left, **Needs attention** lists the hosts that are critical, stale or in a
link conflict; it is the search `health:crit | health:stale | health:conflict`, built into the app
rather than a saved search, because
saved searches are shared with the whole team. It appears when the list has health data.

### The Health section

Select a host: the detail panel has a Health section, loaded from the relay when the host is
selected and again when a newer report arrives. Under the line with the report time and the agent
version it says "Customer 4711" when the device has a customer reference.

- The failing checks come first, grouped by their group (critical before warning), each with the
  value, since when it fails and the hint on how to fix it.
- **Setup** lists checks that are only not configured. Other checks in state unknown are folded
  and not counted as faults.
- **Passed** shows each group's passing checks as a count, expandable.
- **History (90 days)** lists the state changes, newest first, filterable by check.
- **All checks** and **Raw report**, the report exactly as the relay holds it, with a copy button.

Titles, hints and groups come from the report in German and English; the app uses the language of
the system and falls back to the other one. Everything is shown as plain text, shortened to fit
(titles 120, hints 600, values 80 characters). In a session window, a **Health** button in the
toolbar opens the same information for the connected host.

### Export

**Export** in the host list: with health data, the JSON export gets a `health` object per host
(state, counts, products, agent, customer when there is one, time of last report, failing checks)
and the CSV the columns `health, crit, warn, unknown, setup, products, agent, customer, reported`. The format **Failing checks
(CSV)** writes one row per host and failing check (`id,name,uuid,check,state,since`) to
`tether-health-faults-<date>.csv`. The default scope with health data is the hosts shown.
The exports hold names and product names from your fleet; handle them like the host list.

### Notifications

On Windows, Tether shows a notification when a host turns critical or stops reporting. At most one
per host per hour, and when more than three come in one refresh, one summary instead. They are
sent only while the host list is open, because the list does the refreshing; the first refresh
after the app starts only records the states. The switch is in the team dialog ("Notify me when a
host turns critical or stops reporting"), on by default and kept on this PC only. There are no
notifications on macOS.

## The report format

The agent sends JSON, at most 65 536 bytes, unknown top-level fields refused. Two shapes. A **full
report** has every field. A **heartbeat** leaves out `products`, `changes` and `body` (all three
or none), and means "unchanged since the last full report". A post with some of the three is
`400`. The first post after the agent starts, and any post answered `409 send a full report`, must
be a full report.

| Field | Rule |
|---|---|
| `v` | `1` |
| `uuid` | The enrolled uuid, 8 to 64 characters `[A-Za-z0-9-]`; equal to the `uuid` in the signature header |
| `tether_id` | Absent, or equal to the device's Tether ID or to its claim (`409` otherwise). A report never sets, links or changes a Tether ID. When the device has neither, the field becomes its claim, unless a member dismissed its link (then the field is ignored until a member allows a new claim). |
| `host` | The machine's name, 1 to 64 characters, no control characters |
| `seq` | Integer, strictly greater than the last accepted `seq` (full report or heartbeat). Recommended: the larger of last + 1 and the corrected time in milliseconds. |
| `time` | When the report was made, RFC 3339 UTC, at most 40 characters. Informative: the relay uses its own receive time. |
| `next` | Seconds the agent currently plans between posts, 60 to 3600. Informative: the relay's answer is what counts. |
| `agent` | The agent's version, as a Tether version (`0.1.0`, `2026.11.23.2`) |
| `packs` | Object, at most 32 entries, names `[a-z0-9-]` 1 to 32 characters, value an integer: the installed check packs and their version |
| `overall` | `ok`, `warn`, `crit` or `unknown`: the agent's verdict, used as sent |
| `counts` | `{"ok","warn","crit","unknown","setup"}`, all required, non-negative integers. `setup` counts the unknown checks that are only not configured. |
| `faults` | At most 50 of `{"id","state","since"?}`: the checks that are not OK. `id` is `[a-z0-9._/-]` 1 to 64 characters, `state` is `warn`, `crit` or `unknown`, `since` at most 40 characters |
| `products` | Full report. At most 32 of `{"name","version"?,"role"?}`: `name` 1 to 64 characters, `version` at most 32, `role` `server`, `client` or `single` |
| `changes` | Full report. At most 200 of `{"id","from","to","time"}`, the changes since the last accepted full report; states `ok`, `warn`, `crit` or `unknown`. The relay appends them to the history. |
| `body` | Full report. A JSON object. The relay never reads it: it stores it and returns it byte for byte. |

If a report would exceed 64 KiB, an agent should leave the OK checks out of `body` first; `counts`
stays accurate. The relay answers `413` over the limit.

`body.checks[]` is what the technician app reads, a contract between agent and app, not the
relay: `{"id","group","state","since"?,"value"?,"title":{"de","en"},"hint"?:{"de","en"},"setup"?:true}`.
`state` is `ok`, `warn`, `crit` or `unknown`; `setup: true` marks an unknown check that is only
not configured (listed under Setup). Anything in `body` that the app cannot read is skipped.

A full report:

```json
{"v":1,"uuid":"3f2a9c4e-0b1d-4e57-9d1a-6c2f8a7b5e10","tether_id":"123456789","host":"FRONT-DESK-1",
 "seq":1795000123456,"time":"2026-11-23T08:15:00Z","next":300,"agent":"0.1.0","packs":{"system":9},
 "overall":"crit","counts":{"ok":31,"warn":2,"crit":1,"unknown":3,"setup":1},
 "faults":[{"id":"app/db-service","state":"crit","since":"2026-11-23T07:55:00Z"},
           {"id":"disk/free-c","state":"warn","since":"2026-11-21T12:00:00Z"}],
 "products":[{"name":"Example App","version":"26.3.2","role":"server"}],
 "changes":[{"id":"app/db-service","from":"ok","to":"crit","time":"2026-11-23T07:55:00Z"}],
 "body":{"checks":[
   {"id":"app/db-service","group":"Example App","state":"crit","since":"2026-11-23T07:55:00Z",
    "value":"stopped","title":{"de":"Datenbankdienst läuft","en":"Database service is running"},
    "hint":{"de":"Dienst neu starten.","en":"Restart the service."}},
   {"id":"disk/free-c","group":"System","state":"warn","since":"2026-11-21T12:00:00Z",
    "value":"8 % free","title":{"de":"Freier Platz auf C:","en":"Free space on C:"}}]}}
```

The heartbeat that follows leaves out the last three fields and raises `seq`:

```json
{"v":1,"uuid":"3f2a9c4e-0b1d-4e57-9d1a-6c2f8a7b5e10","tether_id":"123456789","host":"FRONT-DESK-1",
 "seq":1795000423456,"time":"2026-11-23T08:20:00Z","next":300,"agent":"0.1.0","packs":{"system":9},
 "overall":"crit","counts":{"ok":31,"warn":2,"crit":1,"unknown":3,"setup":1},
 "faults":[{"id":"app/db-service","state":"crit","since":"2026-11-23T07:55:00Z"},
           {"id":"disk/free-c","state":"warn","since":"2026-11-21T12:00:00Z"}]}
```

Enrolment, the signature, the answers and the status codes: [api.md](api.md#device-status-health-reports).

## Privacy and security

- **Who can read a report:** the relay operator (the files are readable by the tether-api user
  and root) and every member of the team roster. Reports are not encrypted end to end. They
  contain the machine's name, the names and versions of installed products, and the titles, values
  and hints the agent writes; keep personal data out of them. See [PRIVACY.md](../PRIVACY.md).
- **Who can write:** only a device with a key that was pinned with a valid enrolment token. Each
  post is signed; a device key is never accepted as an admin or member key and the reverse.
- **The token** allows enrolling new devices until it expires or is revoked; one issued for a
  Tether ID enrols one device per relay, with that ID (two relays, two devices: a [link
  conflict](#link-conflicts) the app shows instead of choosing). It cannot read anything or
  remove a device. A token that names a customer sets that customer also on a device that is
  already enrolled, when that device enrols with it again and signs that enrolment with its own key
  ([Customer reference](#customer-reference)). A device's Tether ID is shown only when a token was issued for it or a team member
  confirmed it. Both are the word of a trusted person, not a proof by Tether on that machine:
  [SECURITY.md](../SECURITY.md#device-status-health-reports).
- **Who linked a host** (the member's roster name and the time) is stored with the pin on the
  relay and shown to the team.
- **The relay never runs anything from a report:** `body` is stored and returned as text, and the
  app shows it as plain text.
