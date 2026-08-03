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
accounts system, no registry and nobody to email when a club loses a password.
A club that loses its token has lost the ability to add to *one* event, and the
fix is a new slug.

## What it does not do, and where that belongs

| Missing | Where it goes |
|---|---|
| TLS | in front, terminated by the reverse proxy below |
| who a writer *is* (officials, audit) | an accounts system in front, not inside |
| rate limiting, abuse handling | the reverse proxy |
| backups | `beam402-backup.timer` — a `tar` on a timer; the state is files on purpose |
| standings across many events | a separate program, over many logs (**D33**) |

The last row is the only one that is a *program* rather than an operational
concern, and it is a genuinely new derivation rather than a second copy of an
existing one — which is why **D33** says it fits.

## There is no database

The receiver's entire state is, per event, two text files and a token:

```
/var/lib/beam402/<slug>/sheet.toml     the entry sheet
/var/lib/beam402/<slug>/results.log    the result log
/var/lib/beam402/<slug>/token          write authority
```

About **12 KB an event**. A twenty-round season is a quarter of a megabyte. The
receiver derives everything else — fields, ladders, winners — with the same
crate race control uses, and stores none of it.

**A managed database here would be worse than unnecessary.** Putting results in
one means a schema, and a schema is a second model of a ladder; sooner or later
a query computes a round slightly differently and the online bracket
contradicts the one the tower raced off. That is the failure **D33** exists to
prevent, and a managed service does not remove it — it pays for it.

Measured, and measured at a scale nothing here will reach for years — a store of
**401 events**, one of them a national-scale day of 256 entries across 8 classes
with every ladder drawn:

| | |
|---|---|
| an ordinary club day, rendered | **0.3 ms** |
| the 256-entry day, 46 KB of page | **0.7 ms** |
| its `/state` JSON, 27 KB | **0.6 ms** |
| `GET /api/events` over all 401 | **21 ms** |
| resident memory, whole store | **4.5 MB** |
| binary | 3.5 MB |

An event is flat: replaying its log costs about the same whether the day is ten
runs or a thousand. **The one thing that grows with the store is the events
index**, which opens and parses every sheet — roughly 52 µs an event, so four
thousand events is a fifth of a second, and that is the point at which it wants
an index of its own.

When that day comes the answer is a **cache, not a database of results**: a
derived index that can be deleted and rebuilt from the files, with the logs
still authoritative. The moment results themselves live in a database there are
two representations of a ladder, and one of them is wrong on some Saturday.

## Load, and why it is not answered with a bigger machine

Reads do not wait for each other — the lock serializes the append path and
nothing else — so the receiver uses the cores it is given. On one laptop core,
against the 256-entry day:

| Concurrent readers | Requests a second | p95 |
|---|---|---|
| 1 | 1,390 | 0.9 ms |
| 8 | 6,227 | 1.7 ms |
| 32 | 7,461 | 7.6 ms |

**But the throughput is not the point, the cache window is.** A results page
changes when somebody records a result — a few times an hour — so every read
carries `Cache-Control`, and a proxy or CDN in front serves a stampede from one
render:

- **5 seconds** for a day in progress: short enough that a page watched during a
  round still feels live, long enough that ten thousand people refreshing cost
  two renders a minute instead of twenty thousand.
- **300 seconds** for a day where every class is settled, because it will never
  change again and the receiver is the only thing that knows that.
- **60 seconds** for the calendar, the least live endpoint here — it changes
  when an event is *added*, not when a result is recorded.
- **`no-store`** on what an uploader reads and writes. A cached cursor hands out
  a stale offset; a cached refusal sends the client round the same loop.

The weak spot is `GET /api/events`, which parses every sheet: **52 req/s** at
this store size, degrading with the number of events rather than with traffic.
Which is exactly why the calendar has the longest window — one render a minute,
whatever the store grows to. An index inside the receiver is the fix if that
stops being enough, and it would be a *cache* of the sheets rather than a second
place results live.

## When to add a machine, and in what order

In order of capacity bought per unit of complexity. **Each step should be
triggered by a measurement rather than a guess** — the rule **D15** applies to
hardware, applied here.

1. **Put something in front that actually caches.** Caddy does not on its own; a
   cache module, nginx `proxy_cache` or a CDN all do. This is configuration, and
   it is worth about four orders of magnitude: a five-second window turns a page
   that can render 7,000 times a second into one that renders twelve times a
   minute.
2. **Watch two numbers** — renders reaching the origin, and p95 on
   `/api/events`. Nothing below is worth doing until one of them moves.
3. **Read replicas**, if it ever comes to that. One writer, N readers, the store
   copied one way by `rsync` or a filesystem send. Lag of a few seconds is
   nothing for a mirror. Reads are the load; writes are one machine *by
   construction*, because **D33** allows one writer per event anyway.
4. **Static export**, the end of the road: the receiver renders to files and a
   CDN serves them. Nothing in the design forbids it.

**Not a shared volume between two instances.** It looks like the obvious answer
and is not. The lock in the receiver is process-local, so two processes over one
filesystem have no mutual exclusion — an atomic rename keeps each write whole
but does not serialize two of them. What saves that today is a property to rely
on deliberately rather than to discover: the store is partitioned by event, and
**D33** permits one writer per event. And `rename` and `fsync` do not mean over
a network filesystem what they mean on a local disk, which is the one guarantee
this store is built on. One-way replication raises none of those questions,
because readers never write.

## The reference deployment

```
                        ┌── TLS, rate limits ──┐
  push ──── https ──────►  caddy / nginx        ├── http ──► beam402 host
                        └──────────────────────┘            (127.0.0.1:8403)
```

`Caddyfile` and `beam402-host.service` are that, with nothing in them that is
specific to any one instance. Both expect the binary at
`/usr/local/bin/beam402` and the events under `/var/lib/beam402`.

The binary is a **static** release artifact — copy it and run it, no toolchain
and no build, which is what **D23** promised. Releases carry
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.

```sh
install -m0755 beam402 /usr/local/bin/
install -m0644 deploy/beam402-host.service /etc/systemd/system/
install -m0644 deploy/beam402-backup.{service,timer} /etc/systemd/system/
systemctl enable --now beam402-host beam402-backup.timer
```

Then a firewall that lets in 80 and 443 and nothing else, and
`unattended-upgrades` for the OS — which is the only recurring maintenance a
box running this actually has.

**Bind the host to loopback.** It has no TLS and it is not the thing facing the
internet. `-o 127.0.0.1:8403`, and the proxy is the only thing that reaches it.

## Pushing to it

`beam402 push --to https://results.example` works directly: the client speaks
TLS and verifies certificates (**D36**). There is no flag to skip that
verification and there will not be one — it is the sort of thing that gets
pasted into a club's script once and stays there, and what it would be
protecting is a season of results.

A receiver with a **self-signed** certificate is therefore not pushable over
`https`. Reach it over plain `http` on a network the club controls instead,
which is at least honest about what it is:

- **The club's own network.** `beam402 host` on a box on the LAN, pushed to over
  `http`, and the public mirror gets a copy later.
- **A tunnel.** WireGuard or SSH to the VPS, push over `http` inside it.
- **By hand.** The day is two files. `scp` them, or `curl` the three requests.

And a mirror can be copied onward without touching the machine at all, because
`GET /api/event/<slug>/log` is public — so an off-site backup can be a `curl`
loop from anywhere.

For a build with no TLS at all — a machine that never leaves the track — `cargo
build --no-default-features`. `https` then fails by saying so rather than by
failing to connect.
