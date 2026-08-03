# Running a receiver

`beam402 host` is the receiving end of **D33**. This directory is the reference
way to put it somewhere with an address — a club's own box, a league's VPS, a
Raspberry Pi at somebody's house.

**There is nothing here a drag strip depends on.** Cloud features are strictly
additive: a club with no internet loses a mirror and nothing else, and race
control never waits on any of this.

## What the receiver is and is not

It stores two files and a token per event and derives everything with the same
crate the tower uses, which is what stops an online ladder from ever
contradicting the one people are racing off. It **decides nothing** — no rules,
no scoring, no pairing.

Reading is public. Writing needs a token: the first writer claims an event with
a secret of its choosing and every later append has to present the same one.
That is the whole authorization model, and what it buys is that there is no
accounts system, no registry and nobody to email when a club loses a password. A
club that loses its token has lost the ability to add to *one* event, and the
fix is a new slug.

## What it does not do, and where that belongs

| Missing | Where it goes |
|---|---|
| TLS | in front, terminated by the reverse proxy below |
| who a writer *is* (officials, audit) | an accounts system in front, not inside |
| rate limiting, abuse handling | the reverse proxy |
| backups | `cp -r` on a timer; the state is files on purpose |
| standings across many events | a separate program, over many logs (**D33**) |

The last row is the only one that is a *program* rather than an operational
concern, and it is a genuinely new derivation rather than a second copy of an
existing one — which is why **D33** says it fits.

## The reference deployment

```
                        ┌── TLS, rate limits ──┐
  push ──── https ──────►  caddy / nginx        ├── http ──► beam402 host
                        └──────────────────────┘            (127.0.0.1:8403)
```

`Caddyfile` and `beam402-host.service` are that, with nothing in them that is
specific to any one instance. Both expect the binary at `/usr/local/bin/beam402`
and the events under `/var/lib/beam402`.

**Bind the host to loopback.** It has no TLS and it is not the thing facing the
internet. `-o 127.0.0.1:8403`, and the proxy is the only thing that reaches it.

## Pushing to it

Until the push client speaks TLS (`software.md` §4 — an open dependency
decision), a receiver behind `https` cannot be pushed to directly. Three ways
round it today, all of them fine:

- **The club's own network.** `beam402 host` on a box on the LAN, pushed to over
  `http`, and the public mirror gets a copy later.
- **A tunnel.** WireGuard or SSH to the VPS, push over `http` inside it.
- **By hand.** The day is two files. `scp` them, or `curl` the two requests —
  the API is three calls and they are documented in `software.md` §4.
