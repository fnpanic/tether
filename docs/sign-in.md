# Sign-in with an identity provider

From 0.7.0, support staff can sign in to the Tether desktop app and to Tether Manage with their
organisation's account. The directory then decides who is a team member and what each person may
do, through **app roles**; the relays log every access under the person's name
([team audit log](api.md#team-audit-log)). Microsoft Entra ID is the documented provider.

Nothing changes until an administrator publishes a signed team document with sign-in settings
(its `oidc` part): before that, team access works with app keys on the team list exactly as
before ([management.md](management.md#team)).

**Status.** Everything Entra-specific on this page was tested against the mock provider
`tools/oidc-mock` only, never against a real Entra tenant. The set-up steps below follow
Microsoft's documentation and the facts checked in it; the list of what a real tenant still has to
show is at the end ([What was checked](#what-was-checked)).

## How it works

Tether uses OpenID Connect, the standard sign-in protocol on top of OAuth 2.0:

1. The app reads the sign-in settings from every relay (`GET /api/team/config`). They are part of
   the admin-signed team document, and the app checks the signature against the organisation's
   admin key, not against the relay's word. Every relay that answers must name the same settings;
   one relay with sign-in and one without stops the sign-in ("… Your administrator's team document
   has not reached every server yet; try again later.").
2. **Sign in** opens the system browser at the provider (authorization code flow with PKCE S256).
   The app waits for the answer on `http://localhost:<random port>/`, listening on `127.0.0.1` and,
   where the system allows it, `[::1]`, for at most five minutes. It asks for the scope
   `<API scope> openid offline_access`. The person signs in there, with whatever the provider
   requires (password, MFA, a compliant device).
3. The app exchanges the code for an access token and a refresh token. It uses no ID token: the
   person's name, roles and permissions come from the relays' answers.
4. The app posts the access token to **every relay at once** (`POST /api/team/session`), signed
   with its own team key and naming that key (`kid`). Each relay checks the token itself
   ([api.md](api.md#staff-sign-in-sessions-postdelete-apiteamsession)), maps the token's `roles`
   through the signed team document and opens a **session bound to that app's key**: from then on
   the app's ordinary signed requests are this person's, with this person's permissions.
5. A session lasts until the token expires or `TEAM_SESSION_MAX_MINUTES` (default 60), whichever
   comes first. The app renews it five minutes before the earliest relay session ends, with the
   refresh token and no browser, and posts the new token to every relay.
6. **Sign out** deletes the stored refresh token, ends the session on every relay
   (`DELETE /api/team/session`) and opens the provider's sign-out page.

The access token is held in memory only while it is posted. The refresh token is kept only in the
operating system's credential store (Windows Credential Manager, macOS Keychain; service
"Tether sign-in", for Tether Manage "Tether Manage sign-in"; entries `refresh-token <client_id> <n>`),
never in a file or a log. The entry is named by the client id only, not by the Tether build or
instance: two Tether builds (or two organisations' Tether) on one PC that use the same client id
overwrite each other's stored sign-in. That is one more reason for **one registration pair per
Tether deployment**.

**What the relay relies on from the provider.** A discovery document at
`<issuer>/.well-known/openid-configuration` whose `issuer` is exactly the configured one, and from it
only `jwks_uri`, which must be on the issuer's scheme, host and port. Keys: RSA, 2048 to 4096 bits,
RS256. Access tokens: version 2.0 (`ver`), signed RS256 with a `kid`, carrying `iss`, `tid`, `aud`
(the API's client id), `azp` (the client's id), `exp`, `nbf`, `iat`, `scp`, `oid`, `uti` and the
person's app roles in `roles`, with a lifetime of at most 28 hours and no groups overage. These are
Entra's claim names: a provider that does not issue them is refused. Tether never reads the
`groups` claim, the ID token, the userinfo endpoint or Microsoft Graph, and holds no client secret.

## Set-up in Microsoft Entra ID (tenant administrator)

Two app registrations in the organisation's tenant: **Tether API**, the resource the tokens are for
(it carries the app roles), and **Tether apps**, the public client the desktop app and Tether
Manage sign in with. All relays of one Tether deployment share them.

### 1. The API registration ("Tether API")

1. **Microsoft Entra admin center › App registrations › New registration.** Name: `Tether API`.
   Supported account types: *Accounts in this organizational directory only* (single tenant;
   tokens of other tenants and personal accounts are refused anyway). No redirect URI.
   Note the **Application (client) ID** (the *audience*) and the **Directory (tenant) ID**.
2. **Expose an API.** Set the **Application ID URI**; the proposed `api://<API client id>` is fine.
   **Add a scope**: name `access`, state *Enabled*. The full scope, `<Application ID URI>/access`
   (for example `api://<API client id>/access`), is the *scope* of the sign-in settings.
3. **App roles › Create app role**, once per value below. *Allowed member types*: *Users/Groups*.
   The **Value** must be exactly as listed (it is what the token carries and what the team document
   maps); the display name and description are free.
4. **Manifest**: set the access-token version to 2:

   ```json
   "api": { "requestedAccessTokenVersion": 2 }
   ```

   Microsoft: "The values of `null` and `1` result in v1.0 tokens, and the value of `2` results in
   v2.0 tokens"; an organisation-only API gets v1.0 by default. **Without this setting every token
   is refused** (`401 v1-token`; the app says "The server takes version 2.0 access tokens only: your
   administrator must set requestedAccessTokenVersion to 2 on the Tether API registration."). The
   older manifest format names the setting `accessTokenAcceptedVersion` (from memory; Microsoft's
   current page names only `requestedAccessTokenVersion`).

The app role values Tether's standard set uses (Tether Manage › Team › **Add the standard roles**
fills them in with these grants; the administrator can change what each grants):

| App role value | Grants (standard set) |
|---|---|
| `Tether.Technician` | Team member: host list, health summary, reports and history, shared tags (read) |
| `Tether.Technician.Write` | `write`: change shared tags, link and unlink agents, allow a claim, remove a device |
| `Tether.Software` | `sbom`: software lists and software search |
| `Tether.Agent` | `agent`: install or remove the health monitoring agent in a session |
| `Tether.SessionAudit` | `audit`: read one computer's session log |
| `Tether.Customer.<reference>` (e.g. `Tether.Customer.4711`) | Not in the standard set: add it in Tether Manage (**Add role**) with its customers, e.g. `4711` or `47*` |
| `Tether.Manage.Auditor` | Tether Manage role Auditor |
| `Tether.Manage.Operator` | Tether Manage role Operator |
| `Tether.Manage.PolicyEditor` | Tether Manage role Policy editor |
| `Tether.Manage.Publisher` | Tether Manage role Publisher |
| `Tether.Manage.KeyHolder` | Tether Manage role Key holder |

A person's roles add up. Any mapped team role makes the person a team member; the customers are the
union of the person's customer roles, and every customer (`*`) when none of them names one (so
`Tether.Technician` with `Tether.Customer.4711` sees only customer 4711's computers). A Tether
Manage role gives no access in the desktop app. What each Manage role may do:
[management.md](management.md#sign-in-and-roles).

### 2. The client registration ("Tether apps")

1. **App registrations › New registration.** Name: `Tether apps`. Single tenant.
2. **Authentication › Add a platform › Mobile and desktop applications**, custom redirect URI
   exactly `http://localhost`. Entra ignores the port of a `localhost` redirect URI, so the random
   port the apps use matches; host and path must match, so do not register `http://127.0.0.1`
   instead (the apps send `http://localhost:<port>/`).
3. No client secret and no certificate: the apps are public clients and send none. *Allow public
   client flows* enables flows Tether does not use (device code, username and password); Tether
   does not need it.
4. **API permissions › Add a permission › My APIs › Tether API › Delegated permissions › `access`.**
   Then **Grant admin consent** for the tenant, so that staff are not asked to consent.
5. Note this registration's **Application (client) ID** (the *client id*).

### 3. Who gets which role

**Enterprise applications › Tether API › Users and groups › Add user/group**: choose a group and
one app role; repeat per role. The token then carries the role values in `roles`, whatever other
groups the person is in. Tether uses roles and never the `groups` claim: above 200 groups Entra puts
only a pointer into the token (overage), which the relay refuses (`overage`).

- **Licence.** Assigning **groups** to app roles needs Microsoft Entra ID P1 or higher (Microsoft's
  documentation; not checked against the current licence terms). Without it, assign **users** to
  the roles one by one; Tether sees no difference.
- **Assignment required.** **Enterprise applications › Tether API › Properties › Assignment
  required?** = *Yes*: people without a role should then not get a token at all. Tether refuses
  a token without a mapped role in any case (`403 no-role`; the app says "Your account has no
  Tether role. Ask your administrator to assign one.").

### 4. MFA and Conditional Access

Both are Entra's: Tether neither adds nor checks a second factor. Require MFA, compliant devices or
locations with a Conditional Access policy for the Tether API (the resource of the token) or the
Tether apps client. The sign-in runs in the system browser, not in a window of Tether. When a renewal is refused because the person must sign in again (`interaction_required`,
`login_required`, `consent_required`, `invalid_grant`), the app ends the session with the reason and
the person signs in again with **Sign in**.

### 5. Token lifetimes and leavers

Entra's access tokens live 60 to 90 minutes (randomised); refresh tokens up to 90 days, and they stop
working when the account is disabled or its password reset (Microsoft's documentation). A Tether
session lasts until min(token expiry, `TEAM_SESSION_MAX_MINUTES`), so **at most 60 minutes** with the
default.

- **Disabling the account** ends access at the next renewal: at most `TEAM_SESSION_MAX_MINUTES`
  (60) after the last one. Continuous access evaluation does not reach Tether's relays.
- **At once:** add the person's object id (`oid`) to the **deny list** in Tether Manage › Team and
  publish. Each relay ends that person's sessions as soon as it gets the document. If the person's
  app key is also a key entry of the team document, remove that entry too (with `oidc_only` off, the
  key would otherwise fall back to it).
- **Shorter bound:** `TEAM_SESSION_MAX_MINUTES` (1 to 1440) in `/etc/tether/api.env` on every relay,
  for example `15`: more renewals, and a sign-in prompt sooner when Entra is slow.

### 6. What to send to the Tether administrator

- the **Directory (tenant) ID**;
- the **Tether API** Application (client) ID (the audience);
- the **Tether apps** Application (client) ID (the client id);
- the **Application ID URI** of the Tether API and so the full scope (`<Application ID URI>/access`);
- the app role values created, and which groups have which (for the role table and for support).

The issuer is then `https://login.microsoftonline.com/<tenant id>/v2.0`.

## Set-up in Tether (Tether administrator)

### Release order

1. **Every relay first**: tether-api 0.7.0. A relay before it refuses the signed team document,
   and a desktop that finds one relay with sign-in and one without does not sign in.
2. **Tether Manage 0.7.0.** Its Team page shows the plain roster as the first signed document it
   becomes, every entry keeping today's rights. Publishing it switches the relays to signed
   documents for good (they refuse plain rosters from then on); people keep working with their
   keys.
3. **Desktop apps 0.7.0.** Older desktops keep working with their key entries as long as
   `oidc_only` is off.
4. **The `oidc` part last**: the Entra set-up above, then the sign-in settings and the role table in
   Tether Manage › Team, signed and published to every relay. From then on the desktops offer
   **Sign in**.
5. When everyone signs in: delete the people's key entries (keep service and break-glass entries),
   then, if wanted, `oidc_only`.

### The team document in Tether Manage

Tether Manage › **Team** is the editor of the signed team document
([management.md](management.md#team)). Its **Staff sign-in** card ("Staff sign in with an identity
provider"):

| Field | Value |
|---|---|
| Issuer | `https://login.microsoftonline.com/<tenant id>/v2.0` |
| Tenant (directory) id | the Directory (tenant) ID |
| Audience: the Tether API registration's client id | the Tether API's Application (client) ID (not its App ID URI: a v2 token's `aud` is always the client id) |
| Client id of the Tether desktop app registration | the Tether apps' Application (client) ID (Tether Manage signs in with it too) |
| Scope | `<Application ID URI>/access`; `openid` and `offline_access` are added |

The relays refuse a document whose issuer is not `https://` in normal form or names `localhost` or
a loopback, private or link-local address (`400`).

**Directory roles**: one row per app role value with what it grants: team permissions (`write`,
`audit`, `sbom`, `agent`, customers: references, `47*` or `*`; empty: every customer) or a Tether
Manage role ("no team access with it"). **Add the standard roles** adds the table of
[step 1](#1-the-api-registration-tether-api); **Add role** adds one (e.g. `Tether.Customer.4711`). A
role value the table does not list gives nothing; "No roles: nobody can sign in."

**Deny list**: object ids refused a sign-in; "their sessions end on each relay as soon as it gets
this document".

**Key entries**: apps that work with their own key: services (the bridge, scripts) and break-glass
technicians, and until everyone signs in the technicians of the old roster.

**Sign and publish…** (a key holder with the keys unlocked) or **Propose…** (a policy editor). The
review lists security-relevant changes first, and a change of the sign-in settings or of a KeyHolder
mapping needs the tick "I checked the sign-in settings and the KeyHolder mapping above: they decide
who can sign in and who signs for the organisation."

### Sign-in only (`oidc_only`)

"Sign-in only: keys work only for service entries." With it, every key entry not marked as a
service loses team access on every relay that gets the document; people must sign in. Tether
Manage names them before publishing: "Every key entry that is not a service loses team access on
every relay that gets this document: … They must sign in instead. Keep at least one break-glass way
in (a key holder's keys)." It needs the `oidc` part.

### Break-glass

- **Tether Manage without sign-in**: a key holder unlocks the vault with its passphrase and works as
  before 0.7.0, signing and publishing everything. Such a publish is recorded in the team audit log
  under the admin key only, with no person named. While someone is signed in to Tether Manage
  without the KeyHolder role, the vault does not sign ("… to use the keys alone (break-glass), sign
  out first.").
- **Desktop team access without the provider**: a key entry in the team document. With
  `oidc_only` it must be marked as a service to keep working.
- Keep the vault, its passphrase and its backup with the key holders; the vault never leaves their
  PCs.

### What staff see

- **Desktop app**: the **Team** button in the host list opens the Team dialog, whose sign-in part
  appears only while the relays offer sign-in. It shows "Signed in as <name> (<user name>)", the
  roles, what the person may and may not do, "Session until HH:MM; renewed automatically.", and
  **Sign out**; a refusal by a relay as a sentence (for example "Your account is blocked for Tether by
  your administrator."). The app's viewer name in remote sessions becomes the signed-in person's
  name; the remote computer still shows it as not verified. Without `sbom` the Software part and the
  software search say "Your team role does not include software lists (sbom)."; without `agent` the
  session's agent menu item is disabled.
- **Tether Manage**: "Sign in with your organisation" on the unlock screen ("Without the keys: what
  your Tether Manage role allows (read, operate, propose). Signing and publishing stay with a key
  holder's keys."); the person, the roles and **Sign out** at the foot of the navigation.

### The relays' connection to the provider

Every relay fetches the provider's discovery document and keys itself: **outbound HTTPS from every
relay to `login.microsoftonline.com`** (port 443).

- It trusts the web PKI roots compiled into tether-api, not the system's store, and uses **no
  proxy** (proxy environment variables are ignored). A TLS-inspecting proxy, or any middlebox that
  presents its own certificate, between a relay and Microsoft breaks sign-in.
- 5 s connect and request timeouts, at most 64 KiB, no redirect followed. Keys are kept 24 h and
  fetched again earlier only for an unknown `kid`, at most once a minute; one fetch at a time per
  relay; none of it holds up heartbeats or reports.
- When the keys cannot be fetched, new sign-ins get `503 provider-unavailable` ("The server cannot
  reach the sign-in provider right now; try again in a minute."); sessions already open keep working
  until they expire; key entries are not affected. A production build also refuses a provider name
  that resolves to a loopback or private address (`503 provider-address`).
- The desktop apps and Tether Manage need the same host through the browser, and their token
  requests go through the proxy Tether is set to use, else direct.

## Residual risks

- **A token is accepted by every relay of the deployment** within its lifetime. All relays share
  one API registration. Whoever obtains a person's valid access token (a relay that received it, a
  log, a compromised desktop) can open a session as that person, with a key of their own, at a relay
  that has not seen the token yet, for at most the token's lifetime and the session bound (60
  minutes). Accepted for this release, because one organisation runs all relays of its deployment.
  Mitigation: the app posts the token to all relays at once, so a replay meets `409 token-used` on
  any relay that already has it, and the app raises an alarm: "This sign-in was already used at
  <relay>: someone may have your sign-in. Sign out and tell your administrator." It also posts a
  `token-used` record to the audit log. A later step would be a key-bound token or one API
  registration per relay.
- **A role removed in Entra** takes effect at the person's next renewal or sign-in: at most 60
  minutes with the default session bound. Only the deny list acts at once.
- **After 24 hours without a successful key refresh** a relay no longer uses the keys it has, and
  every new sign-in there fails (`503 provider-unavailable`) until it reaches the provider again.
- **A relay not reached at sign-in** gets the person's session only at the next renewal (up to
  about 55 minutes): until then it treats the app's key as its key entry, if there is one, or
  refuses it.
- **The person on the remote computer** is still not verified: the viewer name is the signed-in
  person's, shown as not verified; the relay's audit log, not the remote computer, records who
  started a session (reported by the person's app).
- **A compromised relay** can treat anyone as a member on itself (it holds that relay's data
  anyway), but cannot give anyone rights at another relay through the team document, which is
  admin-signed. It can replay a token it received (above). See
  [SECURITY.md](../SECURITY.md#team-access).
- The app's team key is still a plain file in the user's profile. A stolen key file alone has no
  rights once people's key entries are removed; a stolen key together with a valid token has the
  person's rights until the session ends.

## Testing with the mock provider

`tools/oidc-mock` is a small OpenID provider for tests that behaves like Entra ID v2.0 as far as
Tether relies on it: discovery and keys on Entra's paths, the code flow with PKCE S256 and a
registered `http://localhost` redirect URI, refresh tokens, v2.0 access tokens with Entra's claims,
configurable users and roles, and on request every token shape the relay must refuse (expired,
other audience, tenant or issuer, v1.0, a Microsoft Graph token, groups overage, unknown key, bad
signature, key rotation). Build, run and the control calls: [tools/oidc-mock/README.md](../tools/oidc-mock/README.md).

It has **no security**: anyone who reaches it gets a signed token for any configured user. It
listens on loopback only and serves plain `http`. Release builds of the relay, the desktop app and
Tether Manage refuse an issuer that is not `https://` or names a loopback or private address, so
they never accept it; only debug builds (and relays built with the `test-hooks` feature) take
`http` on loopback.

How it differs from Entra (the README lists all): no password, consent, MFA or Conditional Access;
one fixed token lifetime (default 3600 s) and refresh tokens that never expire; only the disabled
flag refuses a refresh; keys change only when told to; no optional claims; error codes close to
Entra's AADSTS codes but not checked against the real service.

## What was checked

- **Against the mock only**: the whole Entra-specific path. The relay's token checks with every
  refused shape and its reason, sessions, the deny list, a new document ending sessions, `oidc_only`,
  expiry and the session bound, sessions surviving a restart, a provider that is down or hangs, Tether
  Manage's roles at the relay's admin routes (relay tests with the mock in the same process). The
  desktop's sign-in, renewal, a disabled account, sign-out and the `token-used` alarm (Rust tests
  against the mock and two stand-in relays). One run by hand against the mock and two real relays
  (debug builds under WSL): sign-in at both, the host list, a software list readable only with
  `sbom`, a person without a role refused.
- **The TLS path once by hand**: a relay fetched the discovery document and keys of Microsoft's own
  public tenant over HTTPS (issuer equal, `jwks_uri` on `login.microsoftonline.com`, keys accepted,
  a forged token refused as `unknown-kid`). No token from a real tenant was ever checked.
- **Not run at all**: the Windows Credential Manager and the macOS Keychain (tests use an in-memory
  store); the desktop's sign-in in the real app with a real browser; Tether Manage's sign-in in a
  real window; a load run with signed-in sessions.

**What a real tenant must still show** (before production):

- [ ] The two registrations as described, and app roles assigned to groups (licence) and to users.
- [ ] A token with `roles` and without overage for a member of more than 200 groups.
- [ ] `requestedAccessTokenVersion` 2 gives v2.0 tokens with `aud` = the API's client id.
- [ ] `http://localhost` matches the apps' `http://localhost:<port>/` with a random port, and the
      browser reaching it over IPv4 or IPv6; the public client works without *Allow public client
      flows*.
- [ ] Admin consent: nobody is asked to consent; "Assignment required" refuses a person without a role.
- [ ] MFA and a Conditional Access policy in the system browser, also on a locked-down PC.
- [ ] Real token lifetimes and silent renewal over a working day; the real size of refresh tokens
      in the credential store.
- [ ] Disabling an account and resetting a password end access within the session bound; the deny
      list ends it at once.
- [ ] Key rollover (watch for weeks, or force it with a test registration).
- [ ] Sign-out, including the provider's sign-out page.
- [ ] The system browser behind a TLS-inspecting proxy; the apps' token requests through the proxy.
- [ ] Outbound HTTPS from every relay to `login.microsoftonline.com`.
- [ ] Tether Manage's five roles with real groups.
- [ ] Entra's real error descriptions in the apps' messages.
