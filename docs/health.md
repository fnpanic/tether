# Device health

Tether can show the health of the machines it supports in the host list: a dot per host, the
failing checks first, and a built-in view of the hosts that need attention. The data comes from a
monitoring agent, a separate program on each machine that enrols once and then posts signed
reports to the relay. This page is for relay operators, administrators, technicians and anyone
who wants to write an agent. The wire details are in [api.md](api.md#device-status-health-reports).

## What it is, and what it is not

- A store on the relay for the last report, the recent changes and the last contact of each
  enrolled device, and a health view in the technician app that reads it.
- Read-only towards the device, with one exception: on Windows, Tether on that computer can
  install, update and remove the agent itself when the organisation's signed policy or a
  technician in a session asks for it ([Installing the agent through
  Tether](#installing-the-agent-through-tether)); it runs nothing but the organisation's signed
  agent. Nothing else in this feature runs a command on a device, changes a device's settings or
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
report format below are open and checked by the relay, which does not interpret the checks. The
install through Tether expects the commands of the organisation's agent (0.2.0 or later), which
this page names but does not define.

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

1. The agent gets an **enrolment token** in one of two ways. Through Tether: the administrator
   puts a **grant** (a token for agents Tether installs, made by Tether Manage, valid 365 days)
   into the signed policy, and Tether on the machine hands it to the agent it installs together
   with a **Tether proof**, a signature by Tether's identity key that names this agent
   ([Installing the agent through Tether](#installing-the-agent-through-tether)). By hand: an
   administrator creates an enrolment token (valid 1 to 30 days) and gives it to the agent's
   installer ([Without Tether: the manual path](#without-tether-the-manual-path)).
2. The agent generates its own Ed25519 key and enrols: `POST /api/status/enroll` with the token,
   its uuid, its public key and, when Tether is installed, its Tether ID, signed with that key
   (the `Tether-Sig: device-v1` header of a status post; every enrolment needs it). The relay
   checks the token and the signature, pins the key to the uuid and answers how long to wait
   before the next post. The Tether
   ID the agent names is only a **claim**: it becomes the device's Tether ID when a grant comes
   with a Tether proof from Tether on that machine (each relay checks the proof against the key
   hbbs holds for that ID, [api.md](api.md#post-apistatusenroll-token-and-device-signature)), when
   the token was issued for that ID, or when a team member confirms it ([Linking an agent to its
   host](#linking-an-agent-to-its-host)).
3. The agent posts a **full report** (everything) and, between changes, **heartbeats** (the state
   in a few fields). Each post is signed with the device key. The relay sets the pace.
4. Technicians that are in the team roster read the summary of all hosts and one host's report
   through their own member key ([api.md](api.md#team-technician-endpoints)). The app merges the
   summary into the host list by the verified Tether ID only. A machine with an agent but without
   Tether, or whose claim nobody confirmed yet, appears as a row of its own without a connect action.
5. Agent and check-pack updates come through a second signed release index per channel,
   `monitor.json`. The agent updates itself from it. Tether never takes a Tether update from it;
   on Windows Tether reads it only to install the agent and to raise an agent it installed to the
   index's minimum version ([What Tether's service does by
   itself](#what-tethers-service-does-by-itself)).

Status is stored per relay. An agent should enrol on every relay of its list and post to one at a
time. The technician app asks every relay and shows the newest answer for each device.

## Installing the agent through Tether

On Windows, Tether can install the health monitoring agent, keep it at the published minimum
version and remove it, when the organisation's signed policy or a technician in a session asks for
it. Nobody carries a token, copies a file, types at a console or links the host afterwards: the
agent is linked to its host by a **Tether proof**, a signature by the identity key of Tether on that
computer over the statement "this agent (uuid and key) belongs to this Tether ID", which each relay
checks against the key hbbs holds for that ID. The host's Health section then says "Health linked
by Tether on that computer".

There are two ways, and Tether picks the one that fits the computer:

- **With Tether's service** (Tether installed with unattended access, [unattended.md](unattended.md)):
  the service's `--server` process, running as LocalSystem, does everything itself, by policy or on
  a technician's request in a session, without asking anyone at the computer.
- **Without the service** (an installed Tether without unattended access, and TetherQS): only when
  someone asks, a technician in a session or the person at the computer, and then through the
  Windows elevation prompt that the person at the computer answers. A policy alone never starts a
  prompt.

On macOS the two policy features are ignored and a Mac host answers "not supported"; a Mac viewer
can use the session menu on Windows hosts. Agents installed by hand ([the manual
path](#without-tether-the-manual-path)) stay as they are: Tether touches them only for the policy
value `remove` and a technician's explicit **Remove…**.

Nothing of this has been run on a real Windows machine yet: see [What was checked](#what-was-checked).

### Setting it up (administrator)

1. **Relays first.** tether-api and hbbs 0.7.0 on every relay: tether-api checks the proof, hbbs
   answers its key lookup (`/peer-pk`). On a relay where both run on one server nothing else is
   needed; otherwise set `HBBS_PEER_URL` and `PEER_LOOKUP_FROM` ([For the relay
   operator](#enabling-and-switching-off), [kubernetes.md](kubernetes.md)). A relay before 0.7.0
   refuses a policy that uses the new features (`400 unknown feature "health.agent"`; Tether Manage
   says "This relay runs an older tether-api that does not know a feature of this policy. Update
   the relay first, then publish again."), refuses the grant (`401 enrollment token: not an
   enrollment token`) and an enrolment with a proof (`400`, unknown field). So: update every relay,
   then publish the policy.
2. **The agent in `monitor.json`.** Publish the agent, version 0.2.0 or later (the first that
   knows the commands Tether uses), as before ([Publishing an agent or a check-pack
   update](#publishing-an-agent-or-a-check-pack-update)). Tether installs only what is published
   there, on its `update.channel` (`stable` or `test`; any other channel installs nothing).
3. **The policy.** In Tether Manage › Policy, for the technician profile (every installed Tether),
   the customer profile (TetherQS) or a group, set `health.agent` and create the layer's grant
   (`health.enrol-grant`) in the Health monitoring area
   ([management.md](management.md#health-monitoring-agent)); publish. For a group, assign the
   machines under **Devices** and publish again. Tether's service sees a machine's group only after
   a user has opened Tether on it once ([unattended.md](unattended.md#policy-in-the-service));
   before that the profile and the machine policy apply.

| `health.agent` | What happens |
|---|---|
| `manual` (default) | Nothing, exactly as before: Tether does not touch the agent, and a technician's request is refused. |
| `on-request` | Nothing by itself. A technician in a session can install or remove the agent. Tether's service keeps an agent it installed at the minimum version. |
| `install` | Tether's service installs the agent at its next policy check (every `policy.refresh-minutes`, default 15; every minute with `policy.fast-apply`), following the staged rollout of the published version, keeps it at the minimum version and enrols it again when the relays forgot it. A Tether without its service shows the person at the computer an **Install…** card. A technician can install or remove it. |
| `remove` | Tether's service runs the agent's uninstall, also of an agent installed by hand. A technician can remove it, not install it. |

Switching from `install` to `manual` or `on-request` removes nothing.

**The grant.** A grant is an enrolment token with the flag `tether_proof`, made by Tether Manage for
one layer with the vault's admin key: label `policy:<layer>`, valid 365 days, with an optional
customer reference that the relay sets on every agent enrolled with it. It travels in the policy,
which anyone who reaches the relay can read: it is public by design. Alone it enrols nothing: a
relay accepts it only together with a proof signed by the identity key hbbs holds for the Tether ID
named, so it links each agent only to the machine whose Tether signed for it, and it is not
single-use. Its states in Manage: **Valid**; "Expires in N days: renew" (fewer than 60 days left;
Manage reminds you after unlocking); "Replaced by a newer grant (not published)"; **Expired**;
**Revoked**; "Does not verify with this app's admin key: renew" (another or an older admin key made
it); "Not valid". Manage refuses to publish a profile, or a group over its profile, whose
`health.agent` is `on-request` or `install` without a valid grant. **Renew** puts a new grant into
the draft; after you publish, the old one is revoked on every relay once all of them serve a policy
without it. **Revoke** revokes it on every relay at once: new installs and re-enrolments with it
stop (`401 token revoked`), installed agents keep reporting, and the draft sets that layer's
`health.agent` to `manual`, which you then publish. With an expired grant new installs and
re-enrolments fail the same way (`401 enrollment token: expired`) until you renew and publish.

### What Tether's service does by itself

Only the service's `--server` process running as LocalSystem acts, after each policy check; a
user-mode `--server`, the window, macOS and TetherQS never run this part. With `manual` it does
nothing at all: no file, no request, no process.

- **Install** (`install`, the agent's service `TetherMonitor` absent, no removal that sticks, see
  below): it reads `download/<channel>/monitor.json` from its relays and opens it as it opens its
  own release index (admin signature, channel, serial floor `monitor-<channel>.serial`); it refuses
  an expired or halted index, an entry for another file name, and a computer the entry's staged
  rollout has not reached yet. It downloads the `monitor/windows-x86_64` artifact and verifies its
  size, SHA-256, minisign signature (the compiled-in updater key) and trusted comment (file and
  version) into `%ProgramData%\Tether\health-agent\` (`%ProgramData%\Tether-<instance>\…` for an
  instance build), a folder only SYSTEM and Administrators can write (owner checked, no junction).
  It opens the file with a handle that denies writing and deleting, verifies it again through that
  handle, and runs it while the handle is held: `identity` (the agent's uuid and public key), then,
  with the proof signed by the service's own identity key (the one registered with hbbs for this
  computer's Tether ID), `install --from-tether -` with one line on standard input (grant, Tether
  ID, proof, channel), at most 3 minutes. The file is deleted afterwards.
- **Minimum version** (`install` and `on-request`, for an agent Tether installed): it reads the index
  at most every 6 hours. When the agent's service program or its worker has been below the entry's
  `minimumVersion` for 48 hours, it downloads the entry's version the same way and runs its
  `upgrade` (data, key, enrolment and the agent's own switches stay). It never installs a version
  that is not above both.
- **Enrol again** (`install` only, for an agent Tether installed): when the agent reports that no
  relay knows it any more, at most once a day, and once whenever the policy's grant changes (for
  example a group moved to another customer): `identity`, a new proof, `enrol --from-tether -` of the
  installed agent.
- **Remove** (`remove`): `uninstall --by-tether` of the installed agent (plain `uninstall` for one
  older than 0.2.0), also of an agent installed by hand. The agent deletes its enrolment on every
  relay, its service and its folders.
- Failures are retried after 1 hour, 6 hours, then every 24 hours. The state is kept in
  `%ProgramData%\Tether\health-agent.json` (never the grant or the proof, only the grant's id), so a
  restart changes nothing; a read-only copy for the window's Settings line is
  `health-agent-status.json` beside it.

What it never does:

- **Install again after a removal that sticks.** When the agent was removed on the computer
  (*Settings › Apps*, `tether-monitor uninstall`; Tether sees the agent's `RemovedAt` value under
  `HKLM\SOFTWARE\Tether Monitor`, or the service it installed is gone) or by a technician in a
  session, the policy never installs it again. Only a technician's install in a session clears that.
- Touch an agent installed by hand (service present, no state of Tether's), except for `remove` and
  a technician's **Remove…**.
- Change the agent's own switches: it passes only the channel at install.
- Run any other program, pass a command line from the network, or run a file from a folder a user
  can write. Besides the verified download it runs only the installed agent in
  `%ProgramFiles%\Tether Monitor` (`identity`, `enrol`, `uninstall`, `--version`).

A standard user who created `%ProgramData%\Tether` before Tether's service did blocks the feature on
that computer: the folder is refused, not taken over, and the state says why. When Tether's service
is removed, nothing more happens; the agent keeps running and updating itself.

### In a session (technician)

Connect with control of mouse and keyboard, open **Remote actions** › **Health monitoring agent…**.
The item is disabled in a view-only session ("Not in a view-only session: it needs control of the
remote computer.") and when the person at the computer has turned off mouse and keyboard; the remote
computer accepts the request only from a logged-in remote-control connection whose control is on.
Every team member sees the item; the remote computer's policy decides. The dialog first asks the
remote computer and shows its answer; under the sentence a line "The remote computer says: “…”" may
give the remote computer's own words (for `busy` and `not-allowed` always its reason).

| The dialog says | Do |
|---|---|
| "The health monitoring agent is not installed on the remote computer." | **Install…** |
| "Installed <version> by Tether on <date> (policy \| technician in a session)." | Nothing; **Remove…** to remove it. A line "It is linked to another ID (<id>), so its health shows on that host's row." means it belongs to another Tether ID (TetherQS of another user, an earlier install). |
| "Installed by hand <version> (managed outside Tether)." | Tether leaves it alone; **Remove…** removes it. |
| "Removed on the remote computer on <date> by someone at that computer. Tether does not install it again by itself." | **Install again…** only if they agree: the confirmation names the date, and they see the session. |
| "Removed by Tether on <date>." | **Install…** if wanted; the policy does not install it again. |
| "The last action on the remote computer did not succeed." | Read the remote computer's line; **Ask again** or **Install…**. |
| "Still running on the remote computer. Ask again in a moment to see the result." | **Ask again**. |
| "Tether on the remote computer is busy with the agent right now. Ask again in a moment." | **Ask again** later. |
| "Not allowed by the remote computer's policy. The person or administrator of that computer decides (machine policy)." | The layer says `manual`, a machine policy locks it, TetherQS's `customer.elevate` is off, or Windows there would not prompt: change the policy, or ask the person at the computer. |
| "Needs Tether's service on the remote computer (Tether installed as a service)." | Install Tether's service there. |
| "The remote computer does not support this: the health monitoring agent is for Windows computers with Tether installed." | Nothing (a Mac). |
| "Tether on the remote computer is too old (needs 0.7.0)." | Update Tether there (no answer within 10 s). |

**Install…** asks once ("Install the health monitoring agent?", naming that it runs as a Windows
service, stays after the session and reports to your organisation's servers), then shows "Installing
on the remote computer… This can take up to three minutes. Without Tether's service there, the
person at the remote computer is asked first." With the service the result is "Installed
<version>, linked to this host; first report within five minutes." The host waits at most 3
minutes for the outcome; an install that takes longer goes on and the dialog says "Still running…".
A remote computer takes at most one install or remove per connection every 10 s, and one action at
a time. **Remove…** asks once and runs the agent's uninstall ("Removed: the agent's service,
folders and enrolment are gone."); Tether does not install it again by policy afterwards.

The person at the computer sees a line in Tether's connection window ("The technician installed the
health monitoring agent.", "… removed …", or "The technician asked to install …" when the outcome
was another). Each install or remove the host acted on is posted to the session audit log as an
`alarm` record of type 30 with the action and its result ([api.md](api.md#post-apiauditconn-post-apiauditfile-post-apiauditalarm-session-audit-log)).

**On a host without Tether's service** the same item asks the person at the computer first (next
section). A no holds further requests from the session off for 10 minutes ("The person at the
computer said no a moment ago; ask again in N minutes."). Where Windows would not show an
elevation prompt (Tether started as administrator, UAC switched off, administrators elevated without
asking, TetherQS running elevated or with its elevated service), a technician's install or removal
is refused before anything is shown: "Windows would not ask for confirmation on this computer; the
person at the computer can install it there." An agent older than 0.2.0 cannot be removed this way.

**A lost or reimaged machine.** **Remove from health monitoring…** in the host list deletes the
device on the relays only. Where Tether on that computer has the policy `install`, it enrols the
agent again within a day; to remove the agent, use the session menu or change the policy. A new
agent on a machine whose Tether ID another device still holds gets `409 tether_id is enrolled with
another key`: remove the old device first.

### At the computer (the person there)

**With Tether's service** nobody is asked: the organisation decided by policy, or the technician in
the session did. The person sees:

- in Tether's connection window during a session, the line above;
- in Tether's **Settings**, when Tether installed the agent: "Health monitoring agent: installed by
  Tether on <date> at your organisation's request (policy | technician in a session). What it
  reports: the health of this computer's software and, if your organisation collects it, the list
  of installed programs (see Tether's privacy policy). Remove it in Settings › Apps (Tether
  Monitor).";
- in Windows *Settings › Apps*: **Tether Monitor** (written by the agent from 0.2.0 on).

**Without Tether's service**, Tether's (or TetherQS's) own window comes to the front with **Install
the health monitoring program?**: "A technician (support of <program>) asks to install Tether
Monitor." (on the card's path: "Your organisation (<program>) asks …"); in TetherQS, when the policy
or the custom app sets a title, "Your support organisation: <title>"; "Name given by the technician
(not verified): <name>"; "It runs in the background as a Windows service, stays on this computer
after this session, and sends information about the computer's state and installed software to
<relays>."; where it is listed (this repository's [PRIVACY.md](../PRIVACY.md)); how to remove it;
"Windows will ask for administrator approval next."; buttons **Don't install** (selected) and
**Install**. <program> is Tether or TetherQS: an installed Tether has no organisation name to show.
**Don't install**, closing it, or 2 minutes without an answer: nothing is downloaded or started
("The person at the computer declined." / "… did not answer." for the technician). After
**Install**, Tether downloads and verifies the agent (the same index rules as the service, without
the staged rollout) into the user's own `%LOCALAPPDATA%\<program>\health-agent\` and starts it with the Windows elevation prompt; the prompt
names the publisher Windows verified on the agent's file. A standard user must type an
administrator's name and password. Declined or timed out: "Administrator approval was not given
(declined, or no administrator's name and password)." and nothing is installed. Approved: the
elevated agent copies itself into `%ProgramFiles%\Tether Monitor`, checks that copy against the
signed index Tether hands it, and only then creates its service and enrols; it is linked to the
Tether ID of the program that asked (for TetherQS, TetherQS's own ID). Removal from a session works
the same way ("Remove the health monitoring program?", then the prompt for the installed
`%ProgramFiles%\Tether Monitor\tether-monitor.exe uninstall --by-tether`).

With the policy value `install`, an installed Tether without its service shows on its home view
"Your organisation asks to install its health monitoring program on this computer." with
**Install…** (the same question, then the prompt) and **Not now** (hidden until Tether starts
again). TetherQS has no card.

Without Tether's service only the agent keeps itself current; Tether does not upgrade it.

**Removing it.** *Settings › Apps › Tether Monitor › Uninstall* (Windows asks for administrator
approval), or `tether-monitor uninstall` in an administrator's command prompt. The agent removes its
service, folders and enrolment on every relay, and Tether never installs it again by policy.

**Refusing it for good.** The machine policy `{"health.agent":{"state":"locked","value":"manual"}}`
as the REG_SZ value `PolicyJson` under `HKLM\SOFTWARE\Policies\Tether` (or by Group Policy;
`Policies\Tether-<instance>` for an instance build) blocks the install by policy and every
technician's request on that computer. Tether without its service and TetherQS refuse every request
then before any question, with "This computer's administrator has switched this off (machine
policy)."; TetherQS reads only this one rule, and only from `Policies\Tether`.

The question before the prompt is not a security boundary (whoever controls the desktop can operate
it, for example through the On-Screen Keyboard); the Windows elevation prompt is
([SECURITY.md](../SECURITY.md#device-status-health-reports)).

### Without Tether: the manual path

For a machine without Tether, or where Tether cannot do it (an older Tether, a server that is not
supported), the agent is installed by hand as before: an administrator creates an enrolment token
([Creating an enrolment token](#creating-an-enrolment-token)), the agent's installer is run with
it, and a team member links the host with **Link to this host** when the token was not issued for
its Tether ID ([Linking an agent to its host](#linking-an-agent-to-its-host)). The agent's own
documentation has the commands.

### What was checked

The parts were checked with unit and integration tests only: the token flag and the proof
(`tether-trust`), the key lookup in hbbs and the proof enrolment against a stub hbbs (tether-api,
on Linux under WSL), the service's manager and the elevated path with fake processes, services and
downloads (desktop and TetherQS backends), the session message's logic (from the desktop crate,
because the vendored core's own tests cannot be built here), the dialogs (vitest) and the host-list
text (mock end-to-end test, Chromium). **Not run yet:** any of it on a real Windows machine, so no
real service install, upgrade or removal, no real Windows elevation prompt, no TetherQS install, no
session between two machines, no round trip against a real relay with a real hbbs, and no macOS or
Linux build of the new code.

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
| `HBBS_PEER_URL` | `http://127.0.0.1:21122` | hbbs health ports (comma separated, tried in order, 2 s for all) asked for the key of a Tether ID when a device enrols with a grant and a Tether proof ([api.md](api.md#post-apistatusenroll-token-and-device-signature)). Only `http://<host>:<port>`; anything else stops tether-api at start. hbbs answers only loopback and its `PEER_LOOKUP_FROM`. `off`: such enrolments get `503 tether proof needs HBBS_PEER_URL`; when no hbbs answers, `503 tether proof unavailable` (the device tries again later; it is never linked without the proof). |
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
new pins or refused tokens (wrong Tether proofs and Tether IDs hbbs does not know included) per
minute per IP (an IPv6 client counts as its /64).

**Enrolment with a Tether proof** needs hbbs 0.7.0 beside tether-api 0.7.0. hbbs answers
`GET /peer-pk/<id>` on its health port (21122) with the key it holds for that ID, only to loopback
and to the addresses in its `PEER_LOOKUP_FROM` (comma-separated CIDRs, default empty; a bad entry
stops hbbs at start), and only for IDs of 6 to 16 characters `A-Z a-z 0-9 _ -` (every Tether ID
today is numeric); everyone else gets `404`. The health port listens on IPv4 only, so an IPv6 range
in `PEER_LOOKUP_FROM` never matches. On a relay where tether-api and hbbs run on one server, the
defaults work. Where they run apart, point `HBBS_PEER_URL` at hbbs and put tether-api's address
into hbbs's `PEER_LOOKUP_FROM`; a relay outside it gets `404` and every proof enrolment there
answers `409 tether id unknown to this relay`. An hbbs that does not know the ID yet (the host never
registered on that relay) gives the same `409`; the agent is then enrolled only on the relays that
accepted it, which is enough for its reports. When no hbbs answers within 2 s, `503 tether proof
unavailable`: the agent is never linked without the proof, and Tether tries again with its backoff.

### Storage and backup

Everything is in `/var/lib/tether-api/status/` (directory `0700`, files `0600`):

| File | Content | If it is lost |
|---|---|---|
| `keys.jsonl` | The pins, one line per change: `set` (enrolment, link, unlink, allowing a new claim, a claim taken from an accepted report: uuid, device public key, the token's label and id, time of enrolment, the verified Tether ID, the claimed one, who linked it and when (a link by a Tether proof as `linked_token` `proof:<grant id>`), `no_claim`, the customer reference), `used` (a token issued for one Tether ID was used, with its expiry), `dismissed` and `forgotten` (the key of a device whose link was dismissed released its own pin; kept 180 days, at most 10 000: when full, the oldest goes with a `forgotten` line, also when the file is read at start) and `unpin` (removal); `pin` lines from before are still read. Appended and synced for each change, compacted in the background. | Every device must enrol again, with a valid token ([Locked state](#the-locked-state)). **Back this one up.** |
| `revoked.json` | Revoked tokens: `{"<token id>": <its expiry>}`, at most 10 000; an entry is dropped once the token would have expired anyway. Written by the status writer, synced before a revocation is answered. | Revoked tokens that have not expired work again. **Back this one up.** |
| `<uuid>.json` | The last full report, byte for byte as the agent sent it. | Shown as "No full report yet" until the device posts a full one. |
| `<uuid>.sbom.json` | Only with `STATUS_SBOM=on` ([The software list](#the-software-list)): the device's software list, byte for byte as it was uploaded. | The list of that device, until the device uploads one again. |
| `<uuid>.history.jsonl` | One line per change of a check (`id`, `from`, `to`, `time`, and `at`, the relay's receive time). Kept 90 days and at most 2000 lines. | The history of that device. |
| `seen.json` | Last post of each device: time, `seq`, the `next` it was told, the summary entry (at most 2 KiB) and, when the device has a software list, its time, size and the first 16 hex digits of its SHA-256. Written at most every 5 minutes when something changed, and at a normal stop; a hard kill loses up to 5 minutes of last-seen times. | The hosts leave the summary until each posts a full report again (a heartbeat is answered `409 send a full report`). |
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

### The software list

A device can also upload a list of the software installed on it, as one JSON document (the
monitoring agent that supports it sends CycloneDX 1.6). The relay stores it and hands it to team
members; it reads no field of it. **Off by default.** `STATUS_SBOM=on` in `api.env` enables it
(any other value leaves it off). While it is off, the upload and the team members' read answer
`404` and the answer to a post has no `sbom` field; a device's own signed
`DELETE /api/status/sbom` is still carried out (`204`; it has no budget and no hourly limit, so this
adds no load). `STATUS=off` wins over it: with `STATUS=off` all three answer
`503 status reporting is off`. Files already stored stay when the feature is switched off, and
come back when it is switched on again, except those their device deleted meanwhile. A relay
before this change answered that `DELETE` with `404` and kept the file.

| Setting | Default | Meaning |
|---|---|---|
| `STATUS_SBOM` | off | `on` enables the software list. |
| `STATUS_SBOM_MAX_KB` | `512` | The largest document, in KiB. A larger `Content-Length` is `413`, answered before the body is read. |
| `STATUS_SBOM_RPS` | `2` | Uploads the relay takes up per second, all devices together (at least 1). More in the same second is `429 busy` with `Retry-After: 60`. |
| `STATUS_SBOM_MAX_TOTAL_MB` | `2048` | The sum of all stored lists, in MiB. An upload that would exceed it is `507`; a device that already has a list can still replace it with one that is not larger. |

A value that is not a number stops tether-api from starting, as for the other numeric settings.

**Routes** (every status code: [api.md](api.md#put-apistatussbom-device-signed)):

- `PUT /api/status/sbom`, signed by the device like a post, stores the document: `204`.
- `DELETE /api/status/sbom`, signed by the device, removes it: `204`, also when there is none.
- `GET /api/team/status/<uuid>/sbom`, signed by a team member, returns the document byte for byte
  with `Content-Type: application/vnd.cyclonedx+json` and an `ETag`; `404` when the device has none.

The answer to a post (`POST /api/status`) carries `"sbom": true` beside `releases` and `next`
when the feature is on, so an agent uploads only to a relay that takes lists. The team summary
gives each host that has a list `"sbom":{"at":<unix time of the file>,"sha":"<16 hex>"}`, so a client can see
whether the list changed without fetching it.

**What the relay checks.** Only that the body is a JSON object. A document that is valid JSON but
deeply nested, or not CycloneDX at all, is stored as it is; there is no depth limit apart from the
size limit. Whatever reads the stored document (your own scripts, the technician app) must not
assume it is CycloneDX or that it is shallow. The file is written and synced to
`status/<uuid>.sbom.json` before the `204`.

**Limits, in the order the relay applies them to an upload:** `STATUS=off` (`503`), the switch
(`404`), a `Content-Length` (`411 length required` without one) of at most `STATUS_SBOM_MAX_KB`
(`413`), a locked status store (`503`), the header, the per-IP limit on failed requests
(`429 too many failed requests`), the key lookup (`401 unknown device`), one accepted upload per
device per hour (`429 upload too frequent`, `Retry-After` is the time left), the global budget
(`429 busy`), then the body, then the signature and the JSON-object check. Everything before the
body is cheap and happens before the global budget is touched, so a request for a device that is
not enrolled here never uses it.

- A request refused for its signature, or whose body did not arrive (`400 request body
  incomplete`, `400 could not read body`), gives its slot in the budget back and counts as a failed
  request for its IP. It does not use up the device's hour.
- A request with a valid signature that is refused for the device's own fault, `400 not a JSON
  object` or `507 sbom store full`, uses the device's hour: its next upload within the hour,
  also a replay of the same request, is `429 upload too frequent` before it reaches the budget.
  One device cannot keep the global budget busy with refused uploads. `507 insufficient storage`
  (the relay's own disk) uses no hour.
- **The per-IP limit.** Sixty failed requests in a minute from one address (IPv6: one /64) make
  `429 too many failed requests` for the rest of the minute. It is the same counter the other
  device-signed status routes use, so while it is tripped every status post, removal and upload
  from that address is refused, not only uploads. For a site behind one address that takes sixty
  failed requests in a minute (bad signatures, unknown uuids, bodies that do not arrive), for
  example a broken or misconfigured agent on many machines at once; the minute ends it.
- The hour also holds after a `DELETE`, so deleting and uploading again does not get around it. For
  a device that has no document it is kept in memory only, so a restart of tether-api ends it.
- A request that declares a body of up to 1 KiB and then sends fewer bytes may get no answer at all
  (the HTTP server drops it); nothing is stored. A longer one is `400 request body incomplete`.

**Lifetime.** The file goes with the device's pin: when the device removes itself, when an
administrator or a team member removes it ([Removing a device](#removing-a-device)) and when the
pin expires after 180 days without a report. A device that has not posted a report yet can upload
before its first report; it is then shown without a report and removed 180 days after its
enrolment if it never posts. Unlinking a host or dismissing its claim leaves the list in place.
At each start and then at most once an hour tether-api matches the records and the files, on
a thread of its own so that nothing else waits for it (it matters after a hard stop, which loses up
to 5 minutes of `seen.json`): a list without a record, or one that was replaced shortly before the
stop (its size or modification time differs from the record), is read again and recorded; a record
without a file is dropped; a file whose device is no longer pinned is deleted; and a file whose
content still matches its record but has another modification time (a restore or a copy) gets the
recorded time back. Each pass writes one line to the journal: `status: software lists checked: N
files, R read again, B records rebuilt`. A record's time is the file's modification time.

**Disk.** *Estimated*, not measured at scale: a list is 40 to 80 KiB per machine, about 1.2 GB at
20 000 machines. The default total of 2 048 MiB leaves room for that; the files are replaced in
place, not added to, so the total does not grow with the number of uploads. A load test with
uploads: [scaling.md](scaling.md). Back up `status/` with the rest of `/var/lib/tether-api`
([Storage and backup](#storage-and-backup)); a lost list returns when the device uploads it again.

**Who can read it.** Every member of the team roster (no `write` or `audit` flag needed) and the
relay operator, who can read `status/`. It is not encrypted end to end. A list names the installed
programs, their versions and publishers and the machine's hardware identity
([PRIVACY.md](../PRIVACY.md)). A device that is enrolled can store one document of its own choosing
there, at most `STATUS_SBOM_MAX_KB`, once an hour, and nothing in it is ever run
([SECURITY.md](../SECURITY.md#the-software-list)).

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
  measures heartbeat and policy latency and the relay's memory; with `STATUS_SBOM=on` the devices
  also upload 60 KiB software lists. The numbers are in [scaling.md](scaling.md).

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
| `tether_api_status_sbom_files`, `tether_api_status_sbom_bytes` | Software lists stored and their total size in bytes (the figure `STATUS_SBOM_MAX_TOTAL_MB` limits). Counted from the records, also while the feature is off. |
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

For agents Tether installs you need no token of your own: Tether Manage makes the layer's grant
([Setting it up](#setting-it-up-administrator)). The tokens below are for the manual path.

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
does with an expired index is defined by the agent, not here. Tether installs and upgrades nothing
from an expired index: when it expires, installs through Tether stop until the next refresh.

**The minimum version.** An entry's `minimumVersion` is set when you publish it
(`monitor-publish index … --min-version <v>`; the wrapper script does not pass it). Tether's service
raises an agent it installed that stays below it for 48 hours to the entry's version with the
agent's `upgrade` ([What Tether's service does by itself](#what-tethers-service-does-by-itself)).
To make every machine move to a new version: publish that version with `--min-version` set to it.
Agents installed by hand are never upgraded by Tether.

Publishing to `monitor.json` does not move Tether's release generation and is never taken as a
Tether update. Every status answer carries `releases`, an opaque hint that changes when a
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

Both delete the pin with its link or claim, the report, the history and the software list. The line `status: pin of
<uuid> removed by …` goes to the journal. If the agent on that computer still holds a valid
enrolment token, it enrols again by itself and comes back as an agent that is not linked to any
host (with a claim if it names a Tether ID). Uninstall the agent there, or revoke the token. A
token issued for one Tether ID stays used on that relay: it does not enrol a device there again
after the removal. An agent Tether installed is enrolled again, and linked again by its proof,
within a day while that computer's policy says `install`; to remove it, use the session menu or the
policy value `remove`.

**The record of team actions.** Linking, unlinking, allowing a new claim and removing write one
line each to tether-api's journal on stderr (`status: <uuid> linked to <id> by team member
<name>`, `… dismissed by team member <name>`, `… removed by team member <name>`), and the pin
keeps `linked_by` and `linked_at` of the current link. There is no separate audit log of these
actions: keep the journal (systemd's) as long as you need that record.

### Software lists

A relay stores software lists only when its operator set `STATUS_SBOM=on`
([The software list](#the-software-list)); a device uploads one only to a relay whose answer to
its posts says `"sbom": true`. To remove the list of one device without removing the device, that
device sends `DELETE /api/status/sbom` (an agent does so when its owner switches the list off);
removing the device removes the list with it. A list is read through the member routes: any
member of the roster can fetch any device's list, whatever its customer.

### Customer reference

A device can carry a customer reference, for example `4711`: a label that groups the devices of
one customer inside this deployment. Only an enrolment token sets it:

- A device that enrols with a token naming a customer gets that customer. The relay keeps it with
  the pin in `keys.jsonl`.
- **Changing it:** the same device (same uuid and key) enrolling again with a token that names a
  customer other than the one it has (or any customer, if it had none) gets the new one. Every
  enrolment is signed with the device's own key (without the signature the relay answers `401
  enrolment needs the device's signature`), so only the device itself can do this: a token holder
  who knows a device's uuid and public key can neither move its pin nor create one for it where
  none exists. When it changes, tether-api writes `status: customer of <uuid> set to
  <customer> by token <id>` to its journal. The pin keeps the token it first enrolled with, so
  `monitor-publish devices --token` still lists it under that one; only the journal line names the
  token that changed it.
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

An agent is linked to a host row in three ways only. An agent Tether installed is linked from the
start by its Tether proof, checked by each relay: its Health section says "Health linked by Tether
on that computer" (`linked_proof` in the relay's answer). An agent enrolled with a token issued for
its Tether ID is linked from the start too. Any other agent that names a Tether ID only claims it: its
row's Health section says "Claims to be 123456789 (FRONT-DESK-1). Confirm only if this is that
computer.", with the agent's own host name and when it enrolled. If that is the machine, a member
with the `write` permission chooses **Link to this host** and confirms once. The app asks every
relay; each pins that Tether ID to the agent (`409` when another agent has it already), records
who linked it and when, and the agent's health shows on the host row. Without `write` the button
is disabled.

A linked host's Health section says who linked it and when ("Health linked by Anna Berger, …", by
Tether on that computer, or by the token issued for the host; a desktop before 0.7.0 shows a proof
link as the token's). **Unlink** (members with `write`) removes the link and the
agent's claim; the agent becomes a row of its own that shows "Link dismissed". It cannot claim a
Tether ID again, neither by enrolling again (also not with the token issued for the host, whose
first use is past) nor by its reports, which are still accepted with the field ignored. A member
with `write` can choose **Allow a new claim**: the agent's next enrolment or report names its
claim again, and the claim waits to be linked like any other. Only the first use of a token issued
for that ID, or a member's Allow a new claim followed by a Link, can link it again; an enrolment
with a Tether proof does not (the Unlink holds against it too). A dismissed
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
ask an admin to revoke the token. And: "Tether on that computer enrols it again while its policy
says install; to remove the agent, use the session menu or change the policy." (an agent Tether
installed comes back linked, by its proof).

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

The device's software list is stored on the relay too, when the operator enabled it
([The software list](#the-software-list)); the app shows it under [Software](#software).

Titles, hints and groups come from the report in German and English; the app uses the language of
the system and falls back to the other one. Everything is shown as plain text, shortened to fit
(titles 120, hints 600, values 80 characters). In a session window, a **Health** button in the
toolbar opens the same information for the connected host.

### Software

When a host's agent has sent a software list and its relay has the feature on
([The software list](#the-software-list)), the Health section has a **Software** part, folded
until you open it. The header says how many programs the list holds and when it is from; the list
is loaded from the relay when you open it, and again when the agent sends a new one. A host
without a list has no Software part: the feature is off by default on the relay, and an agent
sends a list only to a relay that has it on (and only when its owner has not switched it off).

- The table has the columns Name, Version and Publisher. Products the agent detected are marked
  and come first, then the programs by name. At most 200 rows are drawn at first; **Show all**
  draws the rest.
- The filter box finds text in the name or the publisher, in any case, as plain text (no
  wildcards).
- **Device** names the manufacturer, model, serial number (in full) and memory, when the agent
  reported them. A line "12 more programs are not in the list" says that the agent left programs
  out (it sends at most 2000).
- **Export SBOM** saves the document exactly as the relay holds it, as
  `<host>-<yyyy-mm-dd>.cdx.json` (CycloneDX 1.6; only letters, digits, `.`, `_` and `-` of the
  host name are kept in the file name). If the app cannot read a document, it says so and still
  offers the export.
- The app shows at most 5000 programs and cuts names and publishers to 128 characters and versions
  to 64. A document larger than 1 MiB is not shown ("too large to show") and cannot be exported
  from the app. If the relay no longer has the list, the part says "No software list on the
  server".

There is no search across all hosts for a program in the app: the software lists are not part of
the team summary the host list is built from, so a search such as "which hosts have Java 8" is not
possible there. Everything in the table is plain text.

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
- **Who can write:** only a device with a key that was pinned with a valid enrolment token (a grant
  only together with a Tether proof), by an enrolment signed with that key. Each post is signed; a
  device key is never accepted as an admin or member key and the reverse.
- **Who can install the agent through Tether:** the administrator by policy, a technician in a
  session with control (where the policy allows it), and the person at the computer through the
  card; without Tether's service always through the Windows elevation prompt. Tether runs nothing
  but the organisation's signed agent from `monitor.json`. Whoever administers the computer removes it in *Settings ›
  Apps* and refuses it for good by machine policy ([At the computer](#at-the-computer-the-person-there),
  [SECURITY.md](../SECURITY.md#device-status-health-reports)).
- **The token** allows enrolling new devices until it expires or is revoked; one issued for a
  Tether ID enrols one device per relay, with that ID (two relays, two devices: a [link
  conflict](#link-conflicts) the app shows instead of choosing). It cannot read anything or
  remove a device. A token that names a customer sets that customer also on a device that is
  already enrolled, when that device enrols with it again (like every enrolment, signed with its own key)
  ([Customer reference](#customer-reference)). A device's Tether ID is shown only when Tether on
  that machine proved it (a grant with a Tether proof, checked against hbbs), when a token was
  issued for it, or when a team member confirmed it. The proof rests on hbbs's record of the ID's
  key; the other two are the word of a trusted person:
  [SECURITY.md](../SECURITY.md#device-status-health-reports). A grant is public (it travels in the
  policy) and enrols nothing without a proof.
- **The software list** (only when the relay has `STATUS_SBOM=on`) is readable by the same people
  and is as little end-to-end encrypted. A device can store one JSON document of its own choosing
  there, never run by anything ([SECURITY.md](../SECURITY.md#the-software-list)).
- **Who linked a host** (the member's roster name and the time) is stored with the pin on the
  relay and shown to the team.
- **The relay never runs anything from a report:** `body` is stored and returned as text, and the
  app shows it as plain text.
