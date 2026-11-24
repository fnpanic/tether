# Running Tether for several organisations

A service provider supports several organisations whose technicians must not see each other's
machines. Tether has no tenants inside a relay: for that separation, run one deployment per
organisation.

## One deployment per organisation

A deployment is everything [getting-started.md](getting-started.md) sets up:

| Part | Why it cannot be shared |
|---|---|
| Relay (hbbs, hbbr, tether-api) | Everything below is stored once per relay, for all callers. |
| Admin key | Signs policies, release indexes, admin requests (publishing the team roster is one) and enrolment tokens. Every key in a relay's `ADMIN_PUB_KEY` is an admin of all of it; a second organisation's key in that list is a second admin of the first organisation. |
| Updater key | Signs the update files. An update signed with it is accepted by every app built with its public key. |
| Team roster | One per relay. Every member can read the device list, the shared tags and the health of every device on that relay. |
| Builds | The relay's name and the public keys are compiled into the apps, so each organisation gets its own builds. |

Several relays for one organisation ([relay-ha.md](relay-ha.md)) stay one deployment: they share
its keys and roster. Moving an organisation to new keys later is a key rotation
([key-rotation.md](key-rotation.md)).

## Why there are no tenants inside a relay

The roster, the device list, the shared tags, the session audit log, the policy, the release
indexes and the health store are each one per relay. None of them has a field that says which
organisation a record belongs to, and no route filters by one. Separating organisations inside a
relay would mean adding that to each of them and to every check; Tether does not do that.

## The customer reference

Inside one deployment, an administrator can group the devices that report their health by a
customer reference (1 to 32 characters `A-Z a-z 0-9 . _ -`, for example `4711`), set by the
enrolment token ([health.md](health.md#customer-reference)). The host list shows it in a Customer
column and finds it with `customer:4711`. It is a label for sorting and searching, not a boundary:
every team member still sees every device, and it applies only to devices with a monitoring agent.
