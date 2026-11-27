# oidc-mock: a mock OpenID Connect provider for tests

**A test tool with no security. Never make it reachable from a network.** Anyone who can reach it
gets a signed token for any configured user, with any role, without a password. It listens on
loopback only and refuses any other address unless `--allow-network` is given; `/control` answers
loopback callers only, even then. Its signing keys are RSA 2048 keys generated at start (from
`--seed` or OS entropy), held in memory only and never written anywhere: nothing it signs may ever be
trusted outside a test. Release builds of the relay, the desktop and Tether Manage refuse an issuer
that is not `https://` or is a loopback or private address, so they never accept it.

It behaves like Microsoft Entra ID v2.0 as far as Tether relies on it (spec 12.2, 12.8): discovery,
JWKS, authorization code flow with PKCE S256 and a registered loopback redirect URI, refresh tokens,
RS256 access tokens with `iss`, `tid`, `aud`, `azp`, `exp`, `nbf`, `iat`, `ver` 2.0, `oid`, `name`,
`preferred_username`, `roles`, `uti`, and on request every error shape the relay must refuse,
including the v1 token and the Microsoft Graph token a misconfigured registration or client gets.

## Build and run

```
cd tools/oidc-mock
cargo build                       # Windows or Linux
cargo test                        # Windows; Linux/WSL also runs the LAN-peer test
target/debug/oidc-mock [--config users.json] [--listen 127.0.0.1:0] [--seed 42] [--now 1900000000] [--allow-network [--issuer-host <host>]]
```

It prints two lines on stdout, then serves until killed:

```
listening on 127.0.0.1:53187
issuer http://127.0.0.1:53187/10000000-0000-4000-8000-000000000001/v2.0
```

A test starts it as a child process with `--listen 127.0.0.1:0` and reads the first line, like the
relay's tests do with `tether-api`. `--seed` makes keys, codes and token ids repeatable; `--now`
fixes the clock (unix seconds; `/control/clock` moves it). `--issuer-host` puts that host name in
the issuer and endpoint URLs instead of the listen address; listening on `0.0.0.0` or `::` (only
with `--allow-network`) is refused without it, because the issuer would be unusable.

As a library (Rust tests): `oidc-mock = { path = "../../tools/oidc-mock" }` as a dev-dependency,
then `Mock::start(Config { seed: Some(7), ..Config::default() }, "127.0.0.1:0", false)`; `base()`,
`issuer()`, `discovery()` (the endpoint URLs), `config()` (the start configuration),
`token(user, shape)` for an API token without the flow, `control(call, json)` for the calls below.
The mock stops when the `Mock` is dropped. Key generation is slow without optimisation: a crate
depending on it must add both of these to its own manifest (profiles are not inherited from a
dependency):

```toml
[profile.dev.package.num-bigint-dig]
opt-level = 3
[profile.dev.package.rsa]
opt-level = 3
```

`tests/flow.rs` holds `verify`, a reference relying-party check of these tokens, marked as test
code: the checks are the ones the relay needs, the plumbing (no JWKS fetch or cache) is not.

## Configuration

`--config <file.json>`; every field is optional and defaults to the values shown:

```json
{
  "tenant": "10000000-0000-4000-8000-000000000001",
  "audience": "30000000-0000-4000-8000-000000000003",
  "client_id": "20000000-0000-4000-8000-000000000002",
  "scope": "api://tether-test/access",
  "redirect_uris": ["http://localhost"],
  "lifetime": 3600,
  "issuer_host": null,
  "seed": null,
  "now": null,
  "users": [
    {"oid": "00000000-0000-4000-8000-000000000001", "name": "Test Technician", "upn": "technician@tether.test", "roles": ["Tether.Technician"], "groups": [], "disabled": false}
  ]
}
```

- `tenant`: the `tid`, and part of the issuer `http://<addr>/<tenant>/v2.0`.
- `audience`: the `aud` of v2 access tokens (the *Tether API* registration's client id).
- `client_id`: the only client accepted at `/authorize` and `/token` (the *Tether apps* public client).
- `scope`: the API scope `<App ID URI>/<name>`. Only a sign-in whose `scope` contains it exactly
  gets an access token for the API (`scp` = `<name>`, `aud` = `audience`, `roles`); any other
  request gets a Microsoft Graph token (see `graph` below). `<App ID URI>` is the `aud` of the `v1`
  and `aud_app_id_uri` shapes.
- `redirect_uris`: the client's registered redirect URIs (design 12.2 registers `http://localhost`).
  Matched like Entra: scheme, host and path (with query) exactly, paths case-sensitive, no path the
  same as `/`; the port is ignored for `localhost` and `127.0.0.1` only; `http` only for those two
  hosts. `localhost` and `127.0.0.1` are different hosts: a client registered on one and listening
  on the other is refused. `[::1]` matches only if listed (Entra does not support it). A mismatch is
  a `400` page `AADSTS50011` and no redirect. The same list decides `post_logout_redirect_uri`.
- `lifetime`: access and id token lifetime in seconds.
- `issuer_host`: as `--issuer-host`.
- `users`: `upn` is the `preferred_username` and what `login_hint` names (the `oid` works too);
  `groups`, when not empty, are emitted as a `groups` claim, and more than 200 become Entra's
  overage pointer; `disabled` refuses sign-in and refresh.

Default users (when the file has no `users`), all `<name>@tether.test`, oid
`00000000-0000-4000-8000-0000000000NN`:

| NN | upn | roles |
|---|---|---|
| 01 | `technician` | `Tether.Technician` |
| 02 | `software` | `Tether.Technician`, `Tether.Software` |
| 03 | `writer` | `Tether.Technician`, `Tether.Technician.Write`, `Tether.Software`, `Tether.Agent`, `Tether.SessionAudit` |
| 04 | `customer` | `Tether.Technician`, `Tether.Customer.4711` |
| 05 | `auditor` | `Tether.Manage.Auditor` |
| 06 | `operator` | `Tether.Manage.Operator` |
| 07 | `editor` | `Tether.Manage.PolicyEditor` |
| 08 | `publisher` | `Tether.Manage.Publisher` |
| 09 | `keyholder` | `Tether.Manage.KeyHolder` |
| 10 | `noroles` | none (no `roles` claim; refused at sign-in with `/control/assignment`) |
| 11 | `disabled` | `Tether.Technician`, disabled |

## Endpoints

Laid out like Entra's and found through the discovery document only. With `B = http://<addr>/<tenant>`
and the issuer `I = B/v2.0`:

- `GET I/.well-known/openid-configuration`: the only URL a client derives from the issuer. Not at
  `/.well-known/…` or `B/.well-known/…`.
- `GET B/discovery/v2.0/keys` (`jwks_uri`): `{"keys":[{"kty":"RSA","use":"sig","kid","n","e"}]}`.
- `GET B/oauth2/v2.0/authorize?client_id&response_type=code&redirect_uri&scope&state&nonce&code_challenge&code_challenge_method=S256[&login_hint]`.
  Unknown client: `400` page `AADSTS700016`. `redirect_uri` not registered: `400` page `AADSTS50011`,
  no redirect. PKCE S256 is required. The user is `login_hint`, else the one set with
  `/control/login`, else a page lists the users to click (local development). A known user is sent
  at once with `302` to `redirect_uri?code=…&state=…`; a disabled one to
  `redirect_uri?error=access_denied&error_description=AADSTS50057…&state=…`; with "assignment
  required" on, a user without any role to `…error=access_denied&error_description=AADSTS50105…`.
  Codes are single use (the first redemption attempt uses them up, even a failed one) and live 60
  seconds.
- `POST B/oauth2/v2.0/token` (form): `grant_type=authorization_code&client_id&code&redirect_uri&code_verifier`
  (the `redirect_uri` exactly as at authorize) or `grant_type=refresh_token&client_id&refresh_token`.
  Answer `{"token_type":"Bearer","scope","expires_in","ext_expires_in","access_token","id_token"?,"refresh_token"?}`:
  the id token when the scope has `openid` (with the `nonce`), a refresh token when it has
  `offline_access`. Errors `400 {"error","error_description":"AADSTSn: …","error_codes":[n]}`:
  `invalid_client` 700016, `invalid_grant` 54005 (unknown or used code), 70008 (code expired),
  50011 (other `redirect_uri`), 501481 (wrong verifier), 50057 (user disabled), 9002313 (unknown
  refresh token), `invalid_request` 50148 (verifier missing or not 43 to 128 characters),
  `unsupported_grant_type` 70003.
- `GET B/oauth2/v2.0/logout[?post_logout_redirect_uri]`: `302` to a registered redirect URI, else a page.

Anything else, including `I/token`, `I/jwks` or `/jwks`, is `404`: a client that builds endpoint
URLs instead of reading discovery fails here as it would against Entra.

## `/control` (loopback callers only, `POST`, JSON)

At the root (`M/control/<call>`, `M=http://127.0.0.1:<port>`), not under the issuer. A request is
refused with `403` unless its peer is a loopback address **and** its `Host` header names loopback
(`localhost`, `127.x.x.x`, `[::1]`, any port): a DNS-rebound name sends its own `Host`. It must
send `Content-Type: application/json` (else `415`): a web page cannot send that cross-origin without
a CORS preflight, which the mock never answers. **A reverse proxy on the same host defeats both
checks** (its connection comes from loopback and it may rewrite `Host`): never put one in front.

Each answers `200` (`{"ok":true}` unless noted) or `400 {"error"}`.

| Call | Body | Effect |
|---|---|---|
| `force` | `{"shape": "<shape>"}` or `{"shape": null}` | every API access token from `/token` has that shape until cleared |
| `token` | `{"user": "<upn or oid>", "shape": "<shape>"?}` | `{"access_token"}` for the API without the flow (refused for a disabled user) |
| `rotate` | `{"keep_old": true}` | sign with a new key (or the staged one), the old ones stay in the JWKS (overlap); answers `{"kid","signing","kids"}` |
| `rotate` | `{"keep_old": false}` | as above, but only the signing key stays in the JWKS: older tokens carry an unknown `kid` |
| `rotate` | `{"stage": true}` | publish a new key in the JWKS but keep signing with the current one (a real rollover); the next `rotate` signs with it |
| `lifetime` | `{"seconds": 120}` | lifetime of tokens issued from now on |
| `user` | `{"upn": "technician@tether.test", "disabled": true}` | changes the given fields of the user named by `oid` (else `upn`); disabled: sign-in and refresh refused |
| `user` | `{"oid": "…", "upn": "new@tether.test", "name": "New", "roles": ["Tether.Agent"]}` | adds a user (an unknown `oid`) |
| `login` | `{"user": "writer@tether.test"}` or `{"user": null}` | the user `/authorize` picks without `login_hint` |
| `assignment` | `{"required": true}` or `false` | Entra's "Assignment required?": a user without any role is refused at authorize (`AADSTS50105`); default off |
| `clock` | `{"now": 1900000000}`, `{"now": null}` or `{"advance": 61}` | fixed clock, system clock, or move it; answers `{"now"}` |

Shapes, each one fault (or one alternative form) in an otherwise good token:

| Shape | What differs |
|---|---|
| `expired` | `exp` 600 s in the past (beyond a 300 s skew) |
| `not_yet_valid` | `nbf` 600 s in the future |
| `wrong_aud` | `aud` = `90000000-0000-4000-8000-0000000000aa` |
| `wrong_iss` | `iss` names tenant `90000000-0000-4000-8000-000000000009`, `tid` unchanged |
| `wrong_tid` | `tid` = `90000000-0000-4000-8000-000000000009`, `iss` unchanged |
| `wrong_ver` | `ver` = `1.0`, otherwise a v2 token |
| `wrong_azp` | `azp` = `90000000-0000-4000-8000-0000000000cc` (issued to another client) |
| `overage` | `_claim_names: {"groups":"src1"}` and `_claim_sources: {"src1":{"endpoint":"https://graph.microsoft.com/v1.0/users/<oid>/getMemberObjects"}}`, no `groups` |
| `hasgroups` | `hasgroups: true`, no `groups` (the other overage marker) |
| `unknown_kid` | header `kid` not in the JWKS (signature by the current key) |
| `bad_signature` | one signature byte changed |
| `v1` | a complete, validly signed v1.0 token, what a *Tether API* registration left at the default token version issues: `iss` `https://sts.windows.net/<tenant>/`, `ver` `1.0`, `aud` the App ID URI (`api://tether-test`), `appid`/`appidacr` instead of `azp`/`azpacr`, `upn` and `unique_name` instead of `preferred_username`, `acr`, `amr`, header `x5t` |
| `aud_app_id_uri` | a v2.0 token whose `aud` is the App ID URI instead of the client id (Microsoft documents v2 `aud` as always the client id; this shape tests a verifier that would accept either) |
| `graph` | what a client that did not request the API scope gets: a v1 Microsoft Graph token, `aud` `00000003-0000-0000-c000-000000000000`, `iss` `https://sts.windows.net/<tenant>/`, `scp` `openid profile email`, no `roles`; a `nonce` in the header and a signature that does not verify against the JWKS (Graph's own format). Issued automatically when the API scope is missing |

Examples:

```
J='Content-Type: application/json'
curl -s -H "$J" $M/control/force -d '{"shape":"expired"}'
curl -s -H "$J" $M/control/token -d '{"user":"software@tether.test","shape":"v1"}'
curl -s -H "$J" $M/control/rotate -d '{"stage":true}'
curl -s -H "$J" $M/control/rotate -d '{"keep_old":true}'
curl -s -H "$J" $M/control/lifetime -d '{"seconds":120}'
curl -s -H "$J" $M/control/user -d '{"upn":"technician@tether.test","disabled":true}'
curl -s -H "$J" $M/control/login -d '{"user":"writer@tether.test"}'
curl -s -H "$J" $M/control/assignment -d '{"required":true}'
curl -s -H "$J" $M/control/clock -d '{"advance":3600}'
```

## What a tenant administrator must set

For the later documentation task (spec 12.2), the settings without which real tokens fail the
relay's checks although every test here passes:

- *Tether API* registration, **access token version 2**: in the app manifest
  `"requestedAccessTokenVersion": 2` (in the older Azure AD Graph manifest format the same setting is
  `accessTokenAcceptedVersion`; that name is from memory, the current documentation names only
  `requestedAccessTokenVersion`). Microsoft: "The values of `null` and `1` result in v1.0 tokens, and
  the value of `2` results in v2.0 tokens", and the default for organisation-only applications is
  v1.0. Without it every token has the shape `v1` and is refused.
- *Tether API*: Application ID URI `api://<name>`, the delegated scope `access`, the app roles, and
  on its enterprise application "Assignment required?" = Yes (users without a role are then refused
  at sign-in, `/control/assignment`).
- *Tether apps* (public client, "Mobile and desktop applications"): the redirect URI must have the
  host and path the programs actually listen on. Entra ignores only the port, and only for
  localhost: `http://localhost` does not match `http://127.0.0.1:<port>/`. Microsoft recommends
  `127.0.0.1`, but an `http://127.0.0.1` redirect URI cannot be entered in the portal's text box;
  it has to be added in the manifest (`replyUrlsWithType`). Register exactly what the clients use.
- *Tether apps* › API permissions: the *Tether API* `access` scope, with admin consent. Clients must
  request `api://<name>/access` in the `scope`; with only `openid profile offline_access` Entra
  returns a Microsoft Graph token (shape `graph`).

## How it differs from the real service

- Issuer `http://127.0.0.1:<port>/<tenant>/v2.0` over plain HTTP; Entra's is
  `https://login.microsoftonline.com/<tenant>/v2.0`. The endpoint paths are Entra's. No v1 metadata
  (`B/.well-known/openid-configuration` with issuer `sts.windows.net`) is served, so the `v1` shape
  can only be refused, not verified end to end.
- JWKS entries have only `kty`, `use`, `kid`, `n`, `e`: no `x5t`, `x5c`, `issuer` or
  `cloud_instance_name`; the v2 token header has no `x5t`. Entra rotates keys on its own schedule;
  here only `/control/rotate` changes them.
- Claims: no `aio`, `rh`, `idp`, `wids`, `xms_*` or other optional claims; `sub` is a hash of
  client id and oid (Entra: an opaque pairwise value); `uti` is random base64url, not Entra's
  format; `azpacr` is always `0`. Empty `roles` are left out, as Entra does. The `graph` token's
  claims and header are modelled from memory, not from documentation (only "you can't validate
  tokens for Microsoft Graph … due to their proprietary format" is documented).
- `scope` in the token answer echoes the requested scope string; Entra answers the granted scopes
  (typically only the API scope, e.g. `api://…/access`, without `openid profile offline_access`).
  A request naming two resources (Graph scopes and the API scope) gets an API token here; Entra
  refuses it. `scope` on a refresh request is ignored: the refresh repeats the original scope.
- PKCE: only `S256` is accepted; Entra also accepts `plain`. The discovery document lists
  `code_challenge_methods_supported`, which Entra's does not.
- Lifetimes: one fixed lifetime (default 3600 s); Entra randomises access tokens between 60 and 90
  minutes. Refresh tokens never expire and stay valid after use (Entra: up to 90 days, rotated,
  revoked on disable or password reset; here only `disabled` refuses them). Codes live 60 s
  (Entra about 10 minutes).
- No password, consent, MFA or Conditional Access; no client authentication beyond the `client_id`
  match; scopes other than the API scope are not checked. Only the code flow with PKCE S256 and
  refresh: no implicit, device code, client credentials or on-behalf-of.
- Authorize errors other than a disabled or unassigned user are a `400` page, not a redirect with
  `error`. Error codes are close to Entra's AADSTS codes; AADSTS50011 is documented by Microsoft,
  the others (and whether AADSTS50105 comes as a redirect or as Entra's own error page) are not
  checked against the real service. That `127.0.0.1` also has its port ignored is inferred from
  Microsoft's text (it recommends `127.0.0.1` as the loopback redirect URI), not stated outright.
- Groups overage is emitted from more than 200 configured groups or on request; Entra decides it from
  real memberships.
- One request at a time, all state in memory, lost on exit.
