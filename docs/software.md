# Beam402 — Software Architecture

> Status: **design, not implemented.** No firmware and no race control code
> exist. This document says what will be built, and — more usefully — what each
> program is forbidden to do. Nothing here has run on hardware.

Software is not gated by **D15**: the gate stands on batch purchases, PCB
fabrication and public timelines. It does constrain what may be *claimed* —
every timing figure below is a design intent that **T3** has to confirm.

The wire contract between the programs (Modbus register map, mapping file
format) lives in [`protocol.md`](protocol.md). This document is the division of
labour; that one is the interface.

## 1. Three programs, and the boundary between them

| Program | Runs on | Language | Owns |
|---|---|---|---|
| **Node firmware** | ESP32-S3 | C / ESP-IDF (**D22**, *revisit*) | capturing instants, latching them, answering the bus |
| **Race control** | a small dedicated machine on the trunk (**D30**) | Rust (**D23**) | bus mastering, all race logic, UI, scoreboard, results |
| **Bench tooling** | developer machine | Python (§6) | reducing logic-analyzer captures to distributions |

The firmware language is settled only for the firmware that produces the **T3**
number, so that the project's first measurement of its own electronics has as
few unknowns in it as possible. A Rust node is admissible the moment it
reproduces that number on the same rig — **D22** carries the evidence and the
bar, including why the usual "the HAL doesn't support capture" argument does not
hold up.

The boundary that matters is between the first two, and it is deliberately
lopsided: **the node reports ticks, the master assigns meaning.** A node does
not know what an ET is, which lane it serves, or what a 60-foot split means.

That is not minimalism for its own sake. It is what keeps **D07** (one firmware
for every position) and **D08** (identity bound to track position, not silicon)
true once **D20** exists — see §2.

## 2. The node has no role

**D20** gives downstream nodes a per-lane MCPWM group binding and gives one
start-area node the job of capturing both start pulses on a single timer. Read
carelessly, that is position-specific behaviour — and **D08** says the DIP
address is a node's *only* configuration. Something has to give.

Nothing does, under one rule (**D24**): every node captures everything it can,
always, and publishes all of it. Concretely, every node — start, 60 ft, trap,
finish, spare on the shelf — runs the same loop:

- both edges of every populated input, on both lanes' capture groups;
- both lanes' start pulses, observed on a common timer, with their measured
  widths and their difference;
- telemetry, faults, live input state.

The master reads the registers it cares about for that address and ignores the
rest. "Which node's pulse difference is the launch margin" is a line in the
mapping file, not a mode in flash. Consequences worth stating:

- Firmware contains no `if (position == START)`. There is no start-node build.
- A spare node is a spare for any position with no reflash — **D11**'s promise
  extended to software.
- Adding the trap node (§12) costs one mapping-file line, as **D07** claims.
- Registers that are meaningless at a given position read as "not seen this
  run", not as an error. A finish node with no stage beam is not misconfigured;
  it simply never observes one.

The cost is a handful of registers per node that nobody reads. At two bytes
each, that is the cheapest thing in the system.

## 3. Node firmware

### The timing path, and what may not enter it

Everything in this subsection exists to keep firmware **out** of the interval
being measured. Per **D13** and **D16**:

- The 5 ms start pulse is produced by a monostable from the optocoupler output.
  Firmware observes it. Firmware never generates it.
- The pulse resets the capture timer through the MCPWM GPIO sync input
  (**D20**). Verified to exist as an API —
  `mcpwm_capture_timer_set_phase_on_sync()` with a GPIO sync source — but its
  latency and jitter are **T3**'s job, not the reference manual's word.
- Beam edges land in capture channel registers. No GPIO ISR is in the
  measurement path. A capture channel takes both edges (`pos_edge` and
  `neg_edge` together, edge reported in the event data), so one beam costs one
  channel for both its break and its make — which is what makes D20's
  "two channels of six" budget correct.
- Radios are disabled in build configuration, not at runtime.
- The Modbus task runs at a priority below capture handling and may be starved
  without consequence: results are latched, and a late poll loses nothing (§4).

### Width validation must not delay the counter

**D16**'s trap, restated because it is the one bug in this design that would
pass every test that does not look for it: the counter starts on the pulse's
**leading edge**. Width validation completes 5 ms later and, if the width is
wrong, sets an invalidation flag on a run that has already been timing. Waiting
for the full pulse before starting adds exactly 5 ms to every measurement and
looks perfectly normal in isolation.

The node therefore publishes the *measured* width alongside the flag, so the
master can see a pulse drifting toward the rejection threshold before it starts
throwing runs away — §11 #5 is about noise on 400 m of cable, and a margin
trending down is the early warning.

### What else the node does

- **Boot:** read DIP (address), report factory MAC as a serial number, publish
  capability bits (populated inputs, capture channels).
- **Telemetry:** battery millivolts, interior temperature, sensor bracket
  temperatures (**D19** requires the bracket, not the air), receiver
  self-diagnosis lines (§6), fault flags.
- **Raw edge log:** every edge, timestamped, to flash — dispute evidence per
  §6. On a coarse millisecond clock, deliberately: **D20** notes run timing
  needs no 64-bit accumulation, and evidence does not need 12.5 ns. Pulled
  after a round, never in the live poll loop.

  **Never written during a run.** Flash operations on this part run with
  interrupts disabled — `esp-storage` puts them in a critical section by
  default, and ESP-IDF's writes disable the cache — so a log flush mid-run can
  stall the very path being measured. Buffer edges in RAM, flush between rounds.
  The constraint is the silicon's, not the language's.

- **Self-verification:** on command, drive the injection GPIO with a known
  interval into the node's own input path and capture it. §3 asks the system to
  prove itself before a round for the cost of one GPIO; this is that GPIO.
- **Service:** per-channel alignment mode, `identify` blink, USB CLI.

### The seam that makes host testing possible

The register layer takes capture events *as data* — input index, tick count,
edge direction — and returns the register image. On the device those events come
from MCPWM; on a host they are constructed. So the register image is a pure
function of (events, config), and the part that cannot be tested without silicon
shrinks to the capture wiring itself — which is exactly what **T3** measures.
§7 turns that into a build order.

## 4. Race control

### Layers

```
serial ─▶ Modbus RTU transport ─▶ poller ─▶ event stream
                                                │
                              mapping file ─────┤
                                                ▼
                                          race logic  (pure)
                                                │
                        ┌───────────────────────┼──────────────┐
                        ▼                       ▼              ▼
                 results store (SQLite)    operator UI     scoreboard
```

**The race logic is a pure function of the event stream and the mapping.** No
serial handles, no clock reads, no file I/O below that line. Two payoffs, both
practical rather than aesthetic:

1. All race-logic work proceeds today, against synthetic runs, with no
   hardware — which is the whole reason this document exists now.
2. A recorded bus session replays deterministically. For a project whose claim
   is a trustworthy number, "here is the session, replay it and get the same
   ET" is the software half of D01's verifiability argument.

### Poll for change, read on change

The arithmetic is unforgiving: 19,200 bps 8N1 is 1,920 characters per second,
so **100 ms buys about 192 characters for the entire bus.** Every figure below
prices the *whole* exchange — request, response, and the 3.5-character silence
Modbus RTU requires before each frame (~1.8 ms), because on a half-duplex trunk
that gap is bus time like any other.

A full two-lane run record is ~28 registers — a 69-character exchange, ~40 ms
for one node. Seven devices of that is over half a second. So the steady-state
loop cannot fetch records:

| Traffic | Size | When |
|---|---|---|
| Digest — run generations, faults, live input state | 4 registers, 24.5 chars, ~12.8 ms/device | every cycle (~89 ms for 7 devices) |
| Full run record | ~28 registers, ~40 ms/device | only when that lane's generation changes |
| Telemetry — battery, temperatures | ~6 registers | one device per cycle, round-robin |
| Raw edge log | pages | after a round, on request |

An earlier revision of this table priced the digest at ~7 ms/device by counting
only the response frame. The corrected figure is ~1.8× that, and it is now
asserted against the poller rather than restated here — see
`a_digest_sweep_costs_what_the_arithmetic_says` in `software/crates/poller`.

This costs nothing, because records are latched and there is nowhere to be
late to: §3's quiet window stops polling from "both staged" until every node
has reported, a run lasts 10–20 s, and the next pair stages for minutes. The
unhurried moment to read results is exactly the moment after they exist.

The digest cycle therefore sizes only two things: liveness detection and
staging-lamp response. On staging: the beams are wired to the start node, the
lamps hang on the tree module, so a lamp change costs two poll hops — a full
cycle to see the beam and the top of the next to write the lamp, ~210 ms on a
seven-device bus. A driver creeping at 0.1 m/s covers ~2 cm in that time.

That is accepted for now with the number written down, rather than by wiring
staging beams to the tree and breaking §2's "beams land on nodes". It is not
accepted comfortably: 2 cm is visible to a driver inching onto the stage beam,
and §8 #9 carries the way out.

### Formats

Three shapes, and the rules that separate them live in one module because
**D23**'s promise is that a club changes a class rule without seeing a compiler:

| Format | Start | Breakout | Used for |
|---|---|---|---|
| Heads-up | together | none | grudge, heads-up classes, qualifying |
| Bracket | handicap from the two dial-ins | own dial | most club racing |
| Index | together | the class number | Super Comp and relatives |

Fouls are resolved **first or worst**: a driver who fouls loses; if both foul,
a red light outranks a breakout, two breakouts are separated by which ran
further under the dial, and two red lights by which car left first. That last
one is a question about the clock rather than about the two numbers, because
under a handicap the *smaller* red light can easily be the earlier one — the
tree answers it, since both greens and both pulses are its own registers.

### Race logic

- **Staging state machine:** idle → staging → ready → armed → quiet → running →
  complete, with a *blocked* state for anything the operator has to look at
  first. It reads three beams per lane out of the start node's live input state
  and decides four things: what the staging lamps show, when the tree may be
  armed, when the bus goes quiet, and when the round is over.
  Three cases are worth naming because each is a wrong race if it is missed.
  **Bodywork:** stage and guard broken together is a splitter, not a tire (§2),
  and lights nothing — so the driver sees the disagreement from the seat.
  **Deep staging** — stage broken with pre-stage made — is a class rule rather
  than a fault, so it blocks or does not depending on configuration.
  **A silent start node** is not an empty lane: one is a car that has not
  arrived, the other is a system that cannot see, and only the first is worth
  waiting through.
  Two timing rules fall out of the same place. The settle timer measures time
  *held* on the line, so a car crossing the beams on its way to the water box
  cannot bank it and arm the next pair early. And the quiet window opens with
  the **arm**, not with the green — the master cannot see the green — and has to
  outlast the handicap, or it ends before the second car has left.
- **ET assembly.** ET's zero is the launch instant, and the launch instant *is*
  the pulse — hardware-derived from the tire leaving the stage beam (**D16**),
  which under **D17** is a rising edge at the node. So ET is not assembled from
  two clocks: it is the finish node's own capture register, read directly.
- **Splits** — 60 ft, 1/8, trap entry and exit — likewise, each a single
  register from the node that owns that beam, each measured against that node's
  own timer, zeroed by the same pulse. **D04** in one sentence.
- **Trap speed** = measured base ÷ (trap exit − trap entry), both on one node
  and one timer. The base comes from the mapping file, laser-measured (§2: 5 cm
  of error is 0.25 % of speed, which dwarfs the electronics).
- **Margin** — who won — is `(pulse₂ − pulse₁) + ET₂ − ET₁` per **D20**, with
  the first term read from whichever node the mapping file names as the margin
  source. Crossing order decides races, and ET alone cannot recover it.
- **Corrections** applied by the master, never by the node: per-MAC crystal ppm
  (**D13** — "passport, not job"), and temperature correction if **T4** finds a
  drift that is stable enough to calibrate (**D19**).
- **Event management:** registration, classes, qualifying, ladders, bye runs,
  time slips. A class is **data** — its format, how it qualifies, which ladder
  it runs, who gets lane choice, whether deep staging is allowed — because
  **D23** promises a club changes a class rule without seeing a compiler.

  Two ladder shapes are built and a third is a table. **Pro** pairs 1 v 16 and
  **re-pairs every round**: best surviving qualifier against the worst.
  **Sportsman** splits the field — 1 v 9 — and is a fixed bracket after that,
  arranged so the top two seeds can only meet in the final. `Style::Table` takes a first round
  transcribed from a rulebook, because sanctioning bodies publish their own and
  they are not all the same; `beam402 ladder 13 --format sportsman` prints one
  to check against.

  Three details that are rules rather than arithmetic, and are therefore written
  down rather than assumed. A short field puts the **byes in different places**
  by style — Pro on the top seeds, Sportsman in the middle — because seed 1
  faces the empty slot in one and seed 9 in the other. A **bye still has to be
  run**: most rulebooks require a full pass to advance, so it is recorded, not
  granted. And **lane choice with no previous round is not guessed** — awarding
  it on no basis hands somebody an advantage they did not earn, so the operator
  decides.

  The event around the ladder is two files and no stored state. An **entry
  sheet** in TOML beside the mapping file carries the classes and the entries —
  **D23** again, a club changes a class rule without a compiler — and everything
  else is **derived by replaying an append-only log** of what was recorded: who
  qualified where, which round each class is on, who is still in it. That is the
  same argument **D26** makes about a bus session: a ladder rebuilt from the
  results that produced it can be checked, while one held in a file that is
  rewritten as it goes can only be trusted.

  It decides the failure mode too. Race control loses power mid-eliminator and
  comes back exactly where it was, because the last line written was about a
  round that *finished* rather than a snapshot of one in progress. A torn final
  line costs one record and is counted rather than swallowed — one is a power
  cut, a hundred is a file somebody has to look at.

  Everything that can be wrong is wrong at **load**: a bracket entry with no
  dial, an entry in a class the sheet does not declare, two cars with the same
  number, an index class with no index, a drawn ladder with no seed to re-derive
  it from. A sheet that is wrong in the morning is an inconvenience; one that is
  wrong at the semi-final is a protest.

  `beam402 event <sheet> [--log <results.log>]` shows a meeting: fields, rounds,
  what is on deck and who has lane choice. `--draw <class>` closes qualifying
  and draws that class's ladder, which is a deliberate act with a class named
  rather than something that happens the first time a round is asked for.

  Seeding is the other place a class rule hides. A heads-up field orders by
  quickest ET; a bracket field orders by **closest to the dial**, because a
  bracket racer's ET is their prediction rather than their speed and ranking
  them by it would sort the fast cars to the top for no reason connected to the
  racing.

#### From the ladder to the bus and back

`beam402 serve --event <sheet> --log <file>` runs the meeting rather than one
pairing. The join is deliberately one-directional and narrow:

- **Out.** The pair on deck becomes the `Pairing` the round runs with — lanes
  from the operator's exercise of lane choice, dials from the entry sheet,
  format from the class. So a bracket's handicap is a consequence of the entry
  sheet, never a number typed at a prompt.
- **Back.** A finished round becomes one line in the log, and the ladder
  advances. The only translation this makes is *winning lane → winning seed*,
  because the bus can say "lane 1" and nothing more. That correspondence is
  decided in exactly one place, since two functions agreeing about it would
  eventually stop, and the round they stopped on would advance the wrong car.

**The operator records; the machine proposes.** Same rule as arming, for the
same reason: the timing system can say which lane took the stripe but not that
the car in it was in the right class, or that a protest is standing. So a
completed round sits with its result showing until somebody holding control says
to write it down — and what gets written is what the beams measured, never a
re-derivation of it. A result the timing system *cannot* decide is not written
at all: nobody's day ends because a poll cycle came back empty, and the log is
a text file precisely so that the answer in that case is a human appending a
line to it.

Two interlocks, both of them things that went wrong before they were there.
"Next pair" **refuses to discard an unrecorded result**, so the button that
brings up the next car cannot lose the last one. And a bye is asked a different
question: `completed` means the car made a full pass, which the beams answer,
not whether the pass was clean — you cannot lose to nobody, so a bye that broke
out advances instead of stalling the class, and whether that costs the round is
a class rule rather than something this plumbing decides.

What is **not** wired is qualifying over the bus. Eliminations have a ladder to
take a pair from; time trials are a queue of single cars an operator picks,
which is a different mode and not a smaller version of this one. Until it
exists, qualifying attempts are `Q` lines put into the log by whatever recorded
them.

### Before the day, and after it

Two ends of the same file. **D34** puts registration outside this project —
every league does it differently and several already do it in a spreadsheet
they like — so the entry sheet is the interface and anything that can produce
one is a valid front end. **D33** does the same at the other end: the result
log is the wire format, and a receiver stores it verbatim rather than modelling
it.

**The desk.** `beam402 sheet <entries.csv> --event <season.toml>` takes the
classes from a skeleton the club keeps across a season and the entries from
whatever the registration desk actually has. Semicolon files with decimal
commas work, because that is what a Russian-locale spreadsheet exports; columns
the club needs and this does not — paid, tech, phone — are ignored. Every error
names the row it came from, which is the whole reason to check here: the person
who can fix it is still standing at the desk, and the same fact discovered in a
semi-final is a protest. `beam402 sheet <sheet.toml>` prints the entry list for
somebody who does not read TOML.

**Carrying the day.** `beam402 push <sheet> --log <file> --to <url>` sends the
sheet once and then result lines from wherever the receiver is; `beam402 serve
--to <url>` does the same on a timer while the event runs. The two are the same
call, which is the point — a club with signal pushes after every pair, a club
without pushes that evening, and the receiver cannot tell.

The pusher is deliberately **not** wired into the bus thread. It needs nothing
from the runtime except the log file, which is on disk before anything else
knows a result exists, so it cannot delay a poll cycle, cannot lose a result if
the network fails, and retries by simply running again.

**The receiver.** `beam402 host <directory>` is this same binary run somewhere
with an address — a club's own box, not a service to depend on. It stores
`<slug>/sheet.toml` and `<slug>/results.log` per event and derives everything
with the same crate the tower uses, which is what stops an online ladder from
ever contradicting the one people are racing off. It serves a page per event, a
JSON view for anyone building their own, and the log itself, because a mirror
that cannot be copied onward is not much of a mirror.

**Reading is public; writing needs a token.** The first writer claims an event
with a secret of its choosing, and every later append has to present the same
one. That is the whole authorization model, and what it buys is the absence of
an accounts system: a club that loses its token has lost the ability to add to
one event, and the fix is a new slug. It proves only that this is the same
writer as last time — a league that must know *which official* filed a result
wants accounts, and those go in front of this. `BEAM402_TOKEN` is preferred
over `--token`, because a secret on a command line is a secret in every
process listing on the machine.

Three refusals carry the correctness, and each one hands back the fact needed
to continue rather than just failing:

| Refused | Because | The client's move |
|---|---|---|
| the offset does not match | a retried upload would append twice | resume from the count it was given |
| the prefix digest does not match | two writers are forking one event id | stop — this is a mistake, not a retry |
| a sheet would orphan a recorded result | somebody has already been told that result | fix the sheet, not the log |

A late entry added at ten in the morning is ordinary, so the sheet *is*
replaceable — only never in a way that leaves a written result meaning nothing.
And an unparseable line is mirrored and counted rather than rejected: a batch
refused for one torn line is a day that can never be uploaded at all.

**No TLS on the push client.** **D32** established there is none in the server;
this leaves the same hole at the other end, and it is named rather than papered
over. Plain HTTP is right for a club hosting its own receiver and wrong for
pushing across the open internet, so the fix — a TLS crate in *this one file*,
which never runs on a tree — is a dependency decision to make against a real
server rather than in advance. Until then a receiver behind `https` is reached
over the club's own network, a tunnel, or by hand: the day is two files and the
API is three calls.

**Where a public receiver runs**, and it is not a second program: `deploy/`
holds the reference — a reverse proxy for TLS and rate limiting, a unit file,
and the receiver itself bound to loopback because it is not the thing facing
the internet. The one piece that would justify a program of its own is a
standing across many events, which is a new derivation over many logs rather
than a second derivation of one (**D33**).

**A facade is somebody else's** (**D35**). `/event/<slug>` here is the reference
view and the fallback for a club with no web developer; it stays deliberately
plain, because the moment it grows accounts or a theme system it stops being a
reference and becomes the thing a league would rather replace but cannot. What a
league builds on instead is the read API, and reads carry
`Access-Control-Allow-Origin` for exactly that reason — without it a site on the
league's own domain could not fetch any of this from a browser, and "build your
own" would mean proxying everything server-side. **Writes carry no CORS**: a
token in a cross-origin request is a different threat model and no browser needs
to make one.

| Read | Is |
|---|---|
| `GET /api/events` | every event held here — a calendar, with no ladders in it |
| `GET /api/event/<slug>` | `lines` and the sheet digest: where an uploader continues |
| `GET /api/event/<slug>/state` | the derived day: fields, rounds, winners |
| `GET /api/event/<slug>/log` | the result log itself, so a mirror can be copied on |

Two rules about those shapes, because they are somebody else's dependency now:
**fields get added and never change meaning**, and `ref` is carried through
untouched. `ref` is a league's own key on an event and on an entry — a licence
number, a row id, a UUID — which this project never reads, compares or requires
a shape of. It rides the registration CSV into the sheet and the sheet into the
API, which is what lets a facade show a racer's history across a season
**without this project owning a database of people**.

Payment, accounts, licences and eligibility are not on a later roadmap; they are
a category this project does not enter (**D35**). A league takes money the way
it already takes money, and the only artefact that crosses over is a row with a
competitor on it.

### What ships as data, not code

**D23** buys a single static binary at the cost of a narrower contributor pool
than Python would have. That cost is paid down by keeping everything a club
would plausibly want to change out of the language entirely:

- class and bracket rules, dial-ins, breakout handling — configuration;
- tree modes and delays — configuration, pushed to the tree at arm time;
- scoreboard and time-slip layout — templates and CSS;
- the mapping file — the only source of truth for track meaning (**D08**).

A club changing a class rule or a slip layout should never see a compiler. If
that stops being true, D23 is the decision to revisit.

### Four roles, and which device holds each

"Who uploads the results" and "who owns the runs" are different questions, and
running them together is what makes the deployments look more different than
they are. There are four roles, and both **D30** and **D31** are the same table
with the devices moved around.

| Role | Owns | D30 — full | D31 — tree-hosted |
|---|---|---|---|
| **Bus master** | polling, arming, assembling results, assigning run identity | the machine on the trunk | the tree |
| **Store of record** | the day's results | same machine | the tree |
| **Control client** | arm, abort, advance — one at a time, by token | a phone, tablet or browser | a phone |
| **Relay** | carrying results to a server | the machine, or any client | a client |

A master may **host** one of those devices itself: **D31**'s tree is the bus
master *and* a device on the bus, read through a `Local` implementation that
serves one address from memory and forwards the rest. Nothing above the seam
learns which. That is what keeps one poller, one mapping file and one session
log across both deployments, instead of a second data path for the device that
happens to be running the code.

**The master is always the device on the bus, and a phone is never it.** Not by
preference — a phone cannot sit on RS-485, and **D05** allows exactly one master
on the copper regardless. So a phone is a client in every deployment and is
never the authority on anything.

**The relay needs no application, and this is the part worth knowing.** The
mixed-content rule that forbids a hosted site from calling a local address works
in *one* direction only: it blocks an `https` page loading an `http` resource.
The reverse — the tree's `http` page calling `https://a-server` — is an upgrade
and is not blocked. CORS in that direction is the remote server's to grant, and
Private Network Access restricts public→private, not private→public.

So the page that shows a run also uploads it, from the browser tab somebody
already has open. An application stays optional forever; if one is ever written
it uses the same API.

And because the store of record is always the master, **nothing waits on the
relay**. No signal at the track loses nothing; the first client to reach a
network carries the day.

### The scoreboard is a frame, not a document (D29)

**D23** puts it on a LAN page reached by a QR code, and that stands. **D29**
adds the shape: race control renders the board to a **monochrome frame of
pixels** at a declared resolution, the page draws that frame as diodes on the
diode pitch, and an LED panel — if one is ever built — takes the same bytes.

The reference geometry is **128 × 32 per lane**, a whole number of the 32 × 16
module LED signs are assembled from. Nothing has been bought; **D15** gates
that. The number is there so that "does it fit" is a test rather than a
discovery.

What the constraint immediately bought:

- A band spends **7 + 14 + 7** rows on the dial line, the ET at double size and
  the reaction-and-speed line, plus one for the separator. That is 29 of 32 —
  **there is no fourth line**, and 60 ft or a driver's name costs a taller band
  and therefore more panels.
- The bottom row is 108 px of 128 with reaction on the left and speed on the
  right. There is no room for a third field, and the test says so by name.
- While a round is running the board shows **RUN**, never the last pair's
  numbers. A spectator who reads a stale ET as this round's is certain of the
  wrong thing, and certainty is what a scoreboard is for.
- Who won is a bar down the edge of the band rather than a word, because it
  reads from a distance where three letters do not and costs no characters in a
  row that has none to spare.

The board decides nothing. Winner, breakout, missing splits all arrive settled
from the race logic — a board that reasoned would be a second implementation of
the rules.

`beam402 scoreboard <scenario>` writes the page. When race control grows an HTTP
surface, the same bytes go out of it.

### Looking at it: `beam402 scope`

Three words in this document already mean specific things, and the thing you
point at a round is none of them. **Bench** is the physical T1–T5 rig
([`bench-validation.md`](bench-validation.md)); **console** is the operator's
box at the start line (**D07**); **scoreboard** is the spectator display this
binary serves in-process (**D23**). The fourth is `scope`, named for the
evidence bar [`CONTRIBUTING.md`](../CONTRIBUTING.md) sets — a datasheet, a
**scope trace**, a field failure.

`beam402 scope <scenario>` runs the same round `sim` does and writes one
self-contained page: the strip with the beams on it, the tree, live node state,
the poller's event stream, the bus tape and the slip, all on one scrubbable
timeline. No server and no CDN — it opens from a `file://` URL and can be
committed beside the round it is evidence about. The loop cannot tell it is
being watched; an observed round that ran differently would not be the round
the page is of.

Two things it draws are **not** measurements, and it says so on the page rather
than in a footnote nobody reads:

- A car's position *between* two beams is interpolated. The crossings are
  registers; the line between them is a drawing.
- The trap's marks are placed by the drawing. The mapping file records the
  trap's **base**, because that is all trap speed needs (§2), and never where
  the pair sits.

The most useful thing it shows is an absence: across the quiet window the bus
tape goes silent, because the master transmits nothing while the cars leave.

That absence also produced the page's worst bug, and the shape of it is worth
keeping. Since the master never watches the cascade, the first version drew the
ambers and the green as *unknown* — and the result was two cars leaving a tree
that never went green, which reads as a monumental false start. **A picture
asserting a foul the record denies is a worse failure than one that draws
less.** The cascade is now reconstructed, and the reconstruction is built so
that exactly one number in it is approximate:

```text
base         = finish crossing − ET − reaction − handicap   ← one poll cycle
green[lane]  = base + handicap[lane]                        ← tree register
launch[lane] = green[lane] + reaction[lane]                 ← tree register
position     = launch[lane] + crossings                     ← node registers
```

Anchoring each lane on its own finish instead put the greens 4.900 s apart when
the handicap register said 4.840; averaging the two anchors instead moved the
reaction times to 0.47 and 0.57 when the slip said 0.500 and 0.540. Both are
tests now. The whole picture can still be shifted bodily by up to a poll cycle;
nothing inside it disagrees with the slip.

### Serving it (D32)

The HTTP server is written rather than depended on, and the reason that is a
decision rather than a preference is **D31**: the same server has to run on a
small machine on the trunk *and* on an ESP32-S3 inside a tree. Blocking
`std::net` exists on both; a framework and its async runtime are a different
proposition on the second. And **D05** already makes the bus loop synchronous by
discipline, so there is nothing to overlap.

It is not a web server. No filesystem, no TLS, no keep-alive, no chunked bodies.
Routes are **matched** rather than mapped onto paths, so traversal has nowhere
to occur; every response carries `Content-Length` and closes, which removes
request smuggling along with the state machine it lives in; and every read is
bounded before it is parsed, so an oversized anything is a status code and not
an allocation.

`beam402 serve <scenario>` runs a **live** round. The bus is on its own thread
and is the only thing that touches the bus — **D05** allows exactly one master,
and a mutex is not a master. Clients never poll a node, never write a register
and never call the race logic; they post an *intent*, the bus thread drains it on
its next cycle, and everything anybody reads is a snapshot that thread published
as a string. A handler that formatted JSON under the lock would hold the bus up
for as long as it took, and the bus is the thing with a deadline.

Both pages render from **one** endpoint, `/api/state`, so the operator's screen
and the spectator's cannot disagree about the round. Polling rather than a
socket, because the bus already paces everything at roughly ten hertz and there
is nothing to push faster than it changes.

**The operator arms, not the machine.** The staging machine reaching `Ready`
means the tree *may* be armed; nothing leaves the master until a client holding
control says so. Control is **D30**'s token: one holder at a time, visible on
every screen, and it **expires** if it stops being renewed — a token that never
expired would strand an event the moment somebody closed a laptop and drove
home. Claiming and renewing are the same call, so holding control means coming
back for it.

An intent from a client that does not hold it is a **409**, not a 403: the
request is well formed and the caller is not forbidden, somebody else is simply
holding the start.

### Storage and offline

One SQLite file per event, plus the raw bus session log beside it for replay
and disputes. No network dependency anywhere in the path from beam to time
slip: the scoreboard is served from the same process on the LAN, reachable by
QR code, and cloud features remain strictly additive.

**D31** is where "strictly additive" gets tested. Its tree-hosted deployment
lets a phone carry a day's results away and upload them, so others can see the
racing — and a venue with no signal loses nothing, because the results are in
the tree either way. Two design rules are stated there before any uploading
exists, because both are cheap now and expensive later.

A run's identity is assigned by the **tree**, survives it restarting, and is
**derived rather than random** — `MAC : session : run`. A random identifier
cannot be re-derived, and **D26**'s "replay the session and get the same ET"
ought to extend to *and the same run number*.

And whatever serves the numbers serves the page from the **same origin**. A
hosted site calling a local address is the one arrangement browsers prevent
outright, so the device that holds the results is the device that serves the
interface — which is what **D23** already had race control doing.

**The session log exists** — `beam402 sim … --record`, replayed by
`beam402 replay`. It is text, one line per transaction, because a session is
dispute evidence and the other driver does not have this program:

```text
beam402-session 1
M [venue]
M name = "Sim Strip"
T 100
R 1 0000 4 0007 0000 0001 000f
X 3 0000 timeout
W 10 0100 0010 0000 02bc 0001
```

Replay is a third implementation of the same `Bus` trait, so it drives the real
poller and the real race logic rather than a harness that agrees with itself,
and a request the recording does not have **stops the replay and names the
divergence** instead of serving the nearest match. The mapping and the pairing
ride in the file, so a session answers "what was this a race between" on its
own. The SQLite half is not written.

## 5. Reaction time and red light belong to the tree

The tree module is a bus device like any other (**D07** keeps it off the
universal board), and it owns something no other device does: **the instant the
green lit.**

Reaction time is the interval between green and launch. Assembling it from the
tree's green and the start node's pulse means subtracting two clocks — exactly
what **D04** forbids. But the tree sits in the start area on the trunk, so it
sees both start pulse pairs, and under **D24** it observes them like everyone
else. So:

```
reaction_time = t_pulse − t_green        both on the tree's own clock
red light     = reaction_time < 0        the driver left before green
```

One clock, no bus latency in the number, and a foul is not a special case —
it is a negative RT.

Two consequences that are easy to miss:

- **The green instant must be captured in hardware too**, from the lamp driver
  output looped back into a capture input — not taken at the moment firmware
  calls the LED write. Otherwise firmware latency lands in a number handed to
  the driver, which is D16's mistake in a different device. §8 already requires
  calibrating sequence delays to *include* LED turn-on time; this measures it
  instead of trusting the calibration.
- The master arms the sequence; the tree runs it. AutoStart's random delay
  lives in the tree with bounds pushed at arm time — volatile per-round
  settings, not flash configuration, so **D08**'s rule survives.

### One cascade per lane (D28)

Bracket racing holds the quicker car's cascade back by the difference between
the two dial-ins, so the tree runs **two independent sequences** and there are
two green instants rather than one. The handicap is written with
`tree_handicap` before `tree_arm`, which latches it, and the tree echoes the
armed value so the master can verify it before a car stages.

What is worth stating is how little of the timing model this touches, because
that is the part that says the model was right:

| | Heads-up | Handicap |
|---|---|---|
| ET's zero | that car's launch pulse | unchanged — the four seconds spent waiting are not in it |
| Reaction time | `t_pulse − t_green` | unchanged — against *that lane's* green |
| Launch margin (**D20**) | `pulse₂ − pulse₁` | unchanged — the handicap *is* part of that difference |
| Lamps | shared column | per lane, because the lanes are genuinely in different places |

The winner is whoever crosses the stripe first, which the margin gives directly:
`(pulse₂ − pulse₁) + ET₂ − ET₁`. In a bracket the quicker ET usually belongs to
the car that lost, and a system that recorded only ET would confidently print
the wrong name.

## 6. Bench tooling

`bench-validation.md` §5 requires a data-reduction script, and `BOM.md` is
blunt about the deadline: it has to exist **before the first serious
measurement.** *It exists*, in [`bench/`](../bench). Given a capture, it reports
pass count, mean, σ, peak-to-peak and 99th percentile of Δt, split by edge
direction, with the run's speed and body temperature — the same numbers for
every run so different days and different sensors compare.

The input is **VCD**, not CSV. §5 does the arithmetic: 300 passes at creep speed
is 667 million samples and some thirteen gigabytes of text, so sample-level
export is not merely wasteful there but impossible. CSV is accepted as the
fallback §5 keeps it as, for short bursts.

This is Python, not Rust, and deliberately not an ADR: nothing downstream
depends on it, it never leaves a developer's machine, and it will be rewritten
against the first real capture that surprises us. Standard library only, so it
runs on any laptop at the bench.

Two things it does that are not arithmetic, and they are why it is a script
rather than a spreadsheet:

- **An edge with no partner is counted and shouted about.** A capture where a
  third of the passes did not pair is a rig problem, and a tidy σ over the two
  thirds that worked is the shape of a wrong answer.
- **T1 is judged on the worse single edge direction, never on the pooled
  spread.** §1 calls conflating jitter with offset the classic mistake, and
  pooling commits it: the make/break asymmetry is a shift of two means, so a
  sensor with 100 µs of jitter and a 350 µs offset pools to 450 and would be
  failed for a fault it does not have. The offset is calibratable and **T2** is
  where it is judged.

It is tested against synthetic captures whose jitter and asymmetry are known —
`synth.py` generates them — so the answer is checked against the truth rather
than against something plausible. That generator earns its place at the bench
too: a capture is unrepeatable, and finding out the channel names were wrong
after the run is expensive.

## 7. Build order

Firmware splits into three tiers by what is capable of testing it, and the split
decides the order:

| Tier | What | Tested by |
|---|---|---|
| **1 — pure core** | register image, run-record snapshot, generation semantics, wrap accumulation, DIP decode, flag words, width-validation *logic*, log paging | an ordinary host compiler. No ESP-IDF dependency, so there is nothing to mock and no mocking framework needed |
| **2 — orchestration** | FreeRTOS tasks, Modbus UART transport, response timing | CMock, or the ESP-IDF Linux target, where `driver` and `esp_hw_support` are mocked |
| **3 — silicon** | MCPWM capture, GPIO-sync latency, ISR timing, the 5 ms width in real time | **T3**, and nothing else |

**No emulator covers tier 3.** Wokwi's feature table lists MCPWM as not
implemented and its RMT as transmit-only; Espressif's QEMU does not list MCPWM
among emulated peripherals. Neither claims cycle accuracy. A green test suite on
an emulator before T3 would be the worst available outcome, because it would
look like evidence.

The rule that follows: push everything that fits into tier 1, because the size
of tier 3 *is* the project's risk surface.

Order, chosen to need no hardware until tier 3:

1. **Bench reduction script**, with synthetic captures as its tests. Blocks T1.
   *Done* — [`bench/`](../bench). What T1 waits on now is the rig and a sensor.
2. **The register map**, as the `beam402-protocol` crate — see **D27** and
   [`protocol.md`](protocol.md) §0. *Done.* `no_std` and dependency-free so both
   halves share it verbatim; `registers.toml` is generated from it and §3 is
   checked against it. Layout was the cheap part: what it mainly buys is that
   **D25**'s generation rule, **D16**'s `valid`/`invalidated` pair, **D17**'s
   polarity and "never observed ≠ zero" are enforced rather than remembered.
3. **Tier 1 core and its host tests**, driven by synthetic capture events.
   Deliberately language-independent in shape — that is what keeps **D22** cheap
   to reverse.
4. **Node simulator**: a Modbus RTU slave replaying scripted runs, including the
   ugly ones — invalid pulse width, a node rebooting mid-run, a silent node, a
   beam that breaks and never makes again, two cars leaving 3 ms apart.
5. **Race logic** against the simulator: staging, ET, splits, margin, fouls.
   *Done.* Formats (heads-up, bracket, index), the staging machine, run
   assembly with a named reason for every absence, and first-or-worst.
6. **Scoreboard and time slips** from recorded sessions. *Time slips, session
   replay, `scope` and the scoreboard frame done*, as `beam402 sim`,
   `beam402 replay`, `beam402 scope` and `beam402 scoreboard` — the whole path
   from beams to a printed winner over the same seam a serial port will sit
   behind, the same slip again from the recording, one page showing every layer
   of it at once, and the spectator board at the resolution a panel would have
   (**D29**). What is left is serving it over HTTP and the results database.
7. **T3 harness** when the DevKits land — capture, sync, marker output, nothing
   else. The first real number this project produces about its own electronics.
8. **Tier 2, then the node firmware proper**, then the parking-lot demo.

Step 3 does not wait for steps 4–6; it is the firmware half and runs in
parallel. Step 4 is the load-bearing one for race control: a simulator that
replays only clean runs validates nothing, and the failures listed there are
the specification.

**The order actually being worked, and what it costs.** Steps 3, 7 and 8 are
deferred: no firmware is being written until there is silicon to write it
against. That leaves the simulator as the only executable model of node
semantics — latching, generation wrap, snapshot atomicity, the **D16** rule —
so it is built as a *node model* with its own API rather than as a test double.
Under **D27**'s shared crate that model is tier 1: the same code compiled for
the host in tests and for xtensa in firmware, which returns most of step 3 as a
by-product rather than losing it.

Two consequences worth stating rather than discovering. A simulator speaking
decoded blocks rather than RTU frames tests nothing about framing — §8 #5 is a
measurement against real adapters, and no amount of PTY work substitutes. And a
node model that is *also* the firmware's tier 1 must be held to that standard
from the first commit: it is the reference the silicon will be compared against,
not a convenience for the master's tests.

## 8. Open questions (software)

Ranked, in the spirit of `architecture.md` §11, with the test that settles
each:

1. **MCPWM GPIO-sync latency and jitter.** The mechanism under every number in
   this document. **T3**; **D20** carries the fallback if it jitters.
2. **Run-record atomicity.** A capture can land while the Modbus task is
   assembling a response. The record must be snapshotted whole, or the master
   will occasionally read a split from one run with a generation from the next.
   Settled by construction plus a deliberate test: capture at maximum rate
   while polling continuously, and assert every record read is self-consistent.
3. **Digest poll cycle on the real trunk.** ~89 ms for 7 devices is
   arithmetic; at 450 m with retries it is a measurement. Same soak test as
   §11 #10. The arithmetic also ignores what a *silent* node costs, and that
   term dominates: one dead device burns the response timeout once per attempt,
   300 ms at the current `retries = 2`, more than three times the whole healthy
   sweep. Whether that is tolerable or the timeout needs shortening once a
   device is known to be dead is a measurement, not a preference.
4. **Tree reaction-time path.** Requires both pulse pairs and the looped-back
   green at the tree, and a hardware capture on each. Verifiable on the bench
   with a logic analyzer before any tree exists at a track.
5. **USB-RS485 adapter framing.** Modbus RTU needs 3.5-character inter-frame
   silence (~1.8 ms at 19,200). Cheap adapters with large latency timers break
   framing in ways that look like bus noise. Measure before blaming the cable.
6. **Whether the capture interrupt actually fires.** Binding a handler compiles
   and links (**D22**, B1), and `INT_ST` reports which channel fired — but
   nothing has run. This decides how a second edge arriving before the first is
   read gets detected, which is what sets `run_flags.overflow` honestly rather
   than by assumption. Same silicon session as **T3**.
7. **Which `(input, lane)` pairs a node can capture, and how the master learns
   it.** §2 says every node captures every populated input "on both lanes'
   capture groups". Read literally that is four inputs × two groups = **eight**
   capture channels; `architecture.md` §6 has two MCPWM groups of three, so
   **six** exist. Some allocation policy therefore has to live in firmware, and
   the protocol does not publish it: `capture_channels` (0x0018) reports a
   count, not a map, while the register layout offers four input slots in
   *each* lane's run record — so it can express combinations the silicon cannot
   produce. The cost lands in [`protocol.md`](protocol.md) §5, where load-time
   validation cannot check that a mapped `(address, input, lane)` is
   capturable at all. A lane typo then
   reads "not seen this run" — which is data, not an error — and the run quietly
   loses a split instead of failing to load. That is the failure class this
   project refuses, arriving through the cheapest route again. Settled by
   publishing the allocation as a per-lane input bitmap in the identity block's
   reserved space — additive, so `protocol_version` does not move — plus a tenth
   validation rule. The allocation *rule* is a firmware decision and waits on
   firmware; the register to publish it in does not.
   One consequence is already load-bearing: because the node cannot tell which
   input belongs to which lane, `run_flags.complete` and
   `status_flags.run_active` never set on a node shared between lanes, so
   neither can serve as "the record is ready". **D25**'s amendment routes around
   that — the run generation now moves on every capture — so this gap no longer
   blocks results. It still leaves two flags in the map that are unusable at
   most positions, which is its own reason to close it.
8. ~~**What the master does with a partial set of run records.**~~ **Settled,
   in code.** The rule: an ET that is present and timing-valid **is** a run.
   Intermediate splits that produced nothing print as "—" with a named reason
   attached — node silent, node restarted, run disowned, beam not broken — and
   are never dropped silently and never invented. No ET is **no time**, whatever
   else the round produced.
   Refusing the whole round for a missing 60 ft split would throw away the
   number the driver came for because of a beam that does not decide anything;
   printing a zero would be a lie with a plausible shape. Naming the absence is
   the only option that leaves the slip both complete and honest.
   Implemented as `Missing` and `Gap` in `software/crates/race`, with
   `a_missing_split_names_its_reason_and_the_slip_is_still_issued` as the test.
9. **Nothing in the digest says the tree moved.** The tree block carries a
   `sequence_gen` and [`protocol.md`](protocol.md) schedules it "on generation
   change", but the four-register digest has no bit that carries it — so the
   cheap read a master does every cycle cannot tell it the green lit. The poller
   works around this by reading the tree's 13 registers outright from the arm
   onward, which costs ~9 ms on one device and only during a round. Two ways to
   settle it: spend a reserved `status_flags` bit on "tree sequence advanced",
   or accept the workaround and say so in the contract. The first is additive
   and does not move `protocol_version`.
10. **Whether every device deserves a digest every cycle.** The staging lamps
   are the only thing the cycle time actually gates, and they depend on the
   *start* nodes alone: a beam read from the start node in one cycle becomes a
   lamp written to the tree at the top of the next, ~210 ms on a seven-device
   bus, ~2 cm of creep. Polling the start nodes and the tree every cycle and the
   downstream nodes every third would cut that to ~40 ms while leaving liveness
   detection at three cycles — around a second, which is far inside what an
   operator notices. It is a tiering rule, not a protocol change, but it makes
   the cycle non-uniform and liveness detection position-dependent, so it is
   written down as a choice rather than taken quietly.
