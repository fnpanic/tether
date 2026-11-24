# oidc-mock: a mock OpenID Connect provider for tests

**A test tool with no security. Never make it reachable from a network.** Anyone who can reach it
gets a signed token for any configured user, with any role, without a password. It listens on
loopback only and refuses any other address unless `--allow-network` is given; `/control` answers
loopback peers only, even then. Its signing keys are RSA 2048 keys generated at start (from
`--seed` or OS entropy), held in memory only and never written anywhere: nothing it signs may ever be
trusted outside a test. Release builds of the relay, the desktop and Tether Manage refuse an issuer
that is not `https://` or is a loopback or private address, so they never accept it.

It behaves like Microsoft Entra ID v2.0 as far as Tether relies on it (spec 12.2, 12.8): discovery,
JWKS, authorization code flow with PKCE S256 and a loopback redirect, refresh tokens, RS256 access
tokens with `iss`, `tid`, `aud`, `exp`, `nbf`, `iat`, `ver` 2.0, `oid`, `name`,
`preferred_username`, `roles`, `uti`, and on request every error shape the relay must refuse.

## Build and run

```
cd tools/oidc-mock
cargo build                       # Windows or Linux
cargo test                        # Windows; Linux/WSL also runs the LAN-peer test
target/debug/oidc-mock [--config users.json] [--listen 127.0.0.1:0] [--seed 42] [--now 1900000000] [--allow-network]
```

It prints two lines on stdout, then serves until killed:

```
listening on 127.0.0.1:53187
issuer http://127.0.0.1:53187/10000000-0000-4000-8000-000000000001/v2.0
```

A test starts it as a child process with `--listen 127.0.0.1:0` and reads the first line, like the
relay's tests do with `tether-api`. `--seed` makes keys, codes and token ids repeatable; `--now`
fixes the clock (unix seconds; `/control/clock` moves it).

As a library (Rust tests): `oidc-mock = { path = "../../tools/oidc-mock" }` as a dev-dependency,
then `Mock::start(Config { seed: Some(7), ..Config::default() }, "127.0.0.1:0", false)`; `base()`,
`issuer()`, `config()` (the start configuration), `token(user, shape)` for a token without the
flow, `control(call, json)` for the calls below. The mock stops when the `Mock` is dropped. Key
generation is slow without optimisation: a crate using it should add
`[profile.dev.package.num-bigint-dig] opt-level = 3` (this crate does).
`tests/flow.rs` holds `verify`, a reference relying-party check of these tokens.

## Configuration

`--config <file.json>`; every field is optional and defaults to the values shown:

```json
{
  "tenant": "10000000-0000-4000-8000-000000000001",
  "audience": "30000000-0000-4000-8000-000000000003",
  "client_id": "20000000-0000-4000-8000-000000000002",
  "scope": "api://tether-test/access",
  "lifetime": 3600,
  "seed": null,
  "now": null,
  "users": [
    {"oid": "00000000-0000-4000-8000-000000000001", "name": "Test Technician", "upn": "technician@tether.test", "roles": ["Tether.Technician"], "groups": [], "disabled": false}
  ]
}
```

- `tenant`: the `tid`, and part of the issuer `http://<addr>/<tenant>/v2.0`.
- `audience`: the `aud` of access tokens (the *Tether API* registration's client id).
- `client_id`: the only client accepted at `/authorize` and `/token` (the *Tether apps* public client).
- `scope`: its last segment is the token's `scp`.
- `lifetime`: access and id token lifetime in seconds.
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
| 10 | `noroles` | none (no `roles` claim) |
| 11 | `disabled` | `Tether.Technician`, disabled |

## Endpoints

Under the issuer `I = http://<addr>/<tenant>/v2.0`:

- `GET I/.well-known/openid-configuration` (also at `/.well-known/openid-configuration`).
- `GET I/jwks`: `{"keys":[{"kty":"RSA","use":"sig","kid","n","e"}]}`.
- `GET I/authorize?client_id&response_type=code&redirect_uri&scope&state&nonce&code_challenge&code_challenge_method=S256[&login_hint]`.
  `redirect_uri` must be `http://` on `127.0.0.1`, `localhost` or `[::1]` (any port, no fragment,
  no user info), else `400` and no redirect. PKCE S256 is required. The user is `login_hint`, else
  the one set with `/control/login`, else a page lists the users to click (local development). A
  known user is sent at once with `302` to `redirect_uri?code=…&state=…`; a disabled one to
  `redirect_uri?error=access_denied&error_description=AADSTS50057…&state=…`. Codes are single use
  (the first redemption attempt uses them up, even a failed one) and live 60 seconds.
- `POST I/token` (form): `grant_type=authorization_code&client_id&code&redirect_uri&code_verifier`
  or `grant_type=refresh_token&client_id&refresh_token`. Answer
  `{"token_type":"Bearer","scope","expires_in","ext_expires_in","access_token","id_token"?,"refresh_token"?}`:
  the id token when the scope has `openid` (with the `nonce`), a refresh token when it has
  `offline_access`. Errors `400 {"error","error_description":"AADSTSn: …","error_codes":[n]}`:
  `invalid_client` 700016, `invalid_grant` 54005 (unknown or used code), 70008 (code expired),
  50011 (other `redirect_uri`), 501481 (wrong verifier), 50057 (user disabled), 9002313 (unknown
  refresh token), `invalid_request` 50148 (verifier missing or not 43 to 128 characters),
  `unsupported_grant_type` 70003.
- `GET I/logout[?post_logout_redirect_uri]`: `302` to a loopback `post_logout_redirect_uri`, else a
  page.

## `/control` (loopback peers only, `POST`, JSON body; `403` from any other peer)

Each answers `200` (`{"ok":true}` unless noted) or `400 {"error"}`. `M=http://127.0.0.1:<port>`.

| Call | Body | Effect |
|---|---|---|
| `force` | `{"shape": "<shape>"}` or `{"shape": null}` | every access token from `/token` has that shape until cleared |
| `token` | `{"user": "<upn or oid>", "shape": "<shape>"?}` | `{"access_token"}` without the flow (refused for a disabled user) |
| `rotate` | `{"keep_old": true}` | new signing key, the old ones stay in the JWKS (overlap); answers `{"kid","kids"}` |
| `rotate` | `{"keep_old": false}` | new signing key, only it is in the JWKS: older tokens carry an unknown `kid` |
| `lifetime` | `{"seconds": 120}` | lifetime of tokens issued from now on |
| `user` | `{"upn": "technician@tether.test", "disabled": true}` | changes the given fields of the user named by `oid` (else `upn`); disabled: sign-in and refresh refused |
| `user` | `{"oid": "…", "upn": "new@tether.test", "name": "New", "roles": ["Tether.Agent"]}` | adds a user (an unknown `oid`) |
| `login` | `{"user": "writer@tether.test"}` or `{"user": null}` | the user `/authorize` picks without `login_hint` |
| `clock` | `{"now": 1900000000}`, `{"now": null}` or `{"advance": 61}` | fixed clock, system clock, or move it; answers `{"now"}` |

Shapes, each one fault in an otherwise good token:

| Shape | What differs |
|---|---|
| `expired` | `exp` 600 s in the past (beyond a 300 s skew) |
| `not_yet_valid` | `nbf` 600 s in the future |
| `wrong_aud` | `aud` = `90000000-0000-4000-8000-0000000000aa` |
| `wrong_iss` | `iss` names tenant `90000000-0000-4000-8000-000000000009`, `tid` unchanged |
| `wrong_tid` | `tid` = `90000000-0000-4000-8000-000000000009`, `iss` unchanged |
| `wrong_ver` | `ver` = `1.0` |
| `overage` | `_claim_names: {"groups":"src1"}` and `_claim_sources: {"src1":{"endpoint":"https://graph.microsoft.com/v1.0/users/<oid>/getMemberObjects"}}`, no `groups` |
| `unknown_kid` | header `kid` not in the JWKS (signature by the current key) |
| `bad_signature` | one signature byte changed |

Examples:

```
curl -s -XPOST $M/control/force -d '{"shape":"expired"}'
curl -s -XPOST $M/control/token -d '{"user":"software@tether.test","shape":"overage"}'
curl -s -XPOST $M/control/rotate -d '{"keep_old":true}'
curl -s -XPOST $M/control/lifetime -d '{"seconds":120}'
curl -s -XPOST $M/control/user -d '{"upn":"technician@tether.test","disabled":true}'
curl -s -XPOST $M/control/login -d '{"user":"writer@tether.test"}'
curl -s -XPOST $M/control/clock -d '{"advance":3600}'
```

## How its tokens differ from real Entra ID tokens

- Issuer `http://127.0.0.1:<port>/<tenant>/v2.0` over plain HTTP; Entra's is
  `https://login.microsoftonline.com/<tenant>/v2.0`. Endpoint paths differ too (Entra:
  `…/oauth2/v2.0/authorize`, `…/oauth2/v2.0/token`, `…/discovery/v2.0/keys`,
  `…/oauth2/v2.0/logout`): use the discovery document, never build the paths.
- JWKS entries have only `kty`, `use`, `kid`, `n`, `e`: no `x5t`, `x5c`, `issuer` or
  `cloud_instance_name`; the token header has no `x5t`. Entra rotates keys on its own schedule and
  publishes new ones in advance; here only `/control/rotate` changes them.
- Claims: no `aio`, `rh`, `idp`, `wids`, `xms_*` or other optional claims; `sub` is a hash of
  client id and oid (Entra: an opaque pairwise value); `uti` is random base64url, not Entra's
  format; `azpacr` is always `0`; `scp` is the last segment of `scope`, whatever was requested.
  Empty `roles` are left out, as Entra does.
- Lifetimes: one fixed lifetime (default 3600 s); Entra randomises access tokens between about 60
  and 90 minutes. Refresh tokens never expire and stay valid after use (Entra: up to 90 days,
  revoked on disable or password reset; here only `disabled` refuses them). Codes live 60 s
  (Entra about 10 minutes).
- No password, consent, MFA or Conditional Access; no client authentication beyond the `client_id`
  match; scopes are not checked, only echoed; no app registration list: any loopback redirect is
  accepted for the one client. Only the code flow with PKCE S256 and refresh: no implicit, device
  code, client credentials or on-behalf-of.
- Authorize errors other than a disabled user are a `400` page, not a redirect with `error`. Error
  codes are close to Entra's AADSTS codes but not checked against the real service.
- Groups overage is emitted from more than 200 configured groups or on request; Entra decides it from
  real memberships. The discovery document lists `code_challenge_methods_supported`, which Entra's
  does not.
- One request at a time, all state in memory, lost on exit.
