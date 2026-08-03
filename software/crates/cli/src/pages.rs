//! The two live pages, served by the process that holds the numbers.
//!
//! Both render from **one** endpoint — `/api/state` — so they cannot disagree
//! about what the round currently is. Polling rather than a socket, because the
//! bus already paces everything at roughly ten hertz (**D32**) and there is
//! nothing to push faster than it changes.
//!
//! Self-contained, like every other page here: no CDN, no framework, nothing
//! fetched. They have to work on a phone joined to a tree's own network, where
//! there is no internet to fetch anything from.

/// The operator's screen: where the cars are, what may be done, and by whom.
pub fn operator(venue: &str) -> String {
    format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{venue} — beam402 operator</title>
<style>
:root{{--ink:#e7e4dd;--dim:#78848d;--faint:#4a555c;--line:#232b30;--panel:#131719;
--accent:#5aa9e6;--amber:#ffa92b;--green:#38d26b;--red:#ff4438;
--mono:ui-monospace,SFMono-Regular,"Cascadia Mono",Menlo,monospace}}
*{{box-sizing:border-box}}
body{{background:#0b0d0e;color:var(--ink);font:13px/1.5 var(--mono);margin:0;padding:20px;
max-width:900px;font-variant-numeric:tabular-nums}}
h1{{font-size:15px;font-weight:600;letter-spacing:.14em;text-transform:uppercase;margin:0}}
h1 span{{color:var(--accent)}}
.top{{display:flex;gap:8px 18px;align-items:baseline;flex-wrap:wrap;
border-bottom:1px solid var(--line);padding-bottom:12px;margin-bottom:16px}}
.meta{{color:var(--dim);font-size:12px}}
.warn{{margin-left:auto;color:var(--amber);font-size:11px;letter-spacing:.08em;
text-transform:uppercase;border:1px solid currentColor;border-radius:3px;padding:2px 8px}}
.card{{background:var(--panel);border:1px solid var(--line);border-radius:3px;
padding:12px;margin-bottom:12px}}
h2{{font-size:11px;font-weight:600;letter-spacing:.14em;text-transform:uppercase;
color:var(--dim);margin:0 0 10px}}
.phase{{font-size:26px;font-weight:600;letter-spacing:-.01em}}
.row{{display:flex;gap:12px;align-items:center;flex-wrap:wrap}}
button{{font:inherit;color:var(--ink);background:#171c1f;border:1px solid var(--line);
border-radius:3px;padding:7px 16px;cursor:pointer}}
button:hover:not(:disabled){{border-color:var(--accent);color:var(--accent)}}
button:disabled{{opacity:.35;cursor:not-allowed}}
button.go{{border-color:var(--green);color:var(--green)}}
button.stop{{border-color:var(--red);color:var(--red)}}
button:focus-visible{{outline:2px solid var(--accent);outline-offset:2px}}
table{{border-collapse:collapse;width:100%;font-size:12px}}
th{{text-align:left;font-weight:400;color:var(--faint);font-size:11px;letter-spacing:.06em;
text-transform:uppercase;padding:0 8px 4px 0;border-bottom:1px solid var(--line)}}
td{{padding:4px 8px 4px 0;border-bottom:1px solid #1a2126;white-space:nowrap}}
td.num{{text-align:right}}
.pill{{font-size:11px;letter-spacing:.1em;text-transform:uppercase;border:1px solid var(--line);
border-radius:3px;padding:3px 9px;color:var(--dim)}}
.pill.on{{color:var(--green);border-color:currentColor}}
.pill.off{{color:var(--amber);border-color:currentColor}}
pre{{margin:0;font-size:12px;line-height:1.6;overflow-x:auto;white-space:pre}}
.note{{color:var(--amber);font-size:12px;min-height:1.5em}}
.deck{{display:flex;gap:10px 22px;align-items:baseline;flex-wrap:wrap;margin-bottom:10px}}
.deck b{{font-size:15px;font-weight:600}}
.deck .seed{{color:var(--accent)}}
.choice{{color:var(--amber);font-size:11px;letter-spacing:.08em;text-transform:uppercase}}
.bracket{{display:flex;flex-direction:column;gap:4px}}
.pair{{display:flex;gap:10px;align-items:center;font-size:12px;color:var(--dim)}}
.pair .who{{color:var(--ink)}} .pair.done{{color:var(--faint)}}
.pair.done .who{{color:var(--dim)}} .pair.now{{color:var(--ink)}}
.pair .mark{{width:1.2em;color:var(--green)}}
/* The qualifying queue calls a car into a lane: any car, not only the next. */
.pair .runs{{margin-left:auto}}
.pair button,.deck button{{font:inherit;font-size:11px;padding:1px 6px;
min-width:2.4em;background:transparent;color:var(--dim);
border:1px solid var(--line);border-radius:3px;cursor:pointer}}
.pair button:hover,.deck button:hover{{color:var(--ink);border-color:currentColor}}
.pair button.on,.deck button.on{{color:var(--green);border-color:currentColor}}
/* The car that could not make the call, still named because it is still drawn. */
.deck .gone,.deck .gone b{{color:var(--faint);text-decoration:line-through}}
.hide{{display:none}}
footer{{color:var(--faint);font-size:11px;border-top:1px solid var(--line);
padding-top:10px;margin-top:14px}}
a{{color:var(--accent)}}
</style></head><body>
<div class="top">
  <h1>beam402 <span>operator</span></h1>
  <div class="meta">{venue}</div>
  <div class="meta"><a href="/board">scoreboard</a></div>
  <div class="warn">simulated · no hardware exists</div>
</div>

<div class="card">
  <div class="row">
    <div class="phase" id="phase">…</div>
    <div class="pill" id="held">control: nobody</div>
    <div class="pill" id="win"></div>
  </div>
  <div class="note" id="note"></div>
</div>

<div class="card">
  <h2>Control</h2>
  <div class="row">
    <button id="take" type="button">take control</button>
    <button id="arm" class="go" type="button" disabled>arm</button>
    <button id="abort" class="stop" type="button" disabled>abort</button>
    <button id="swap" type="button" disabled>swap lanes</button>
    <button id="record" type="button" disabled>record result</button>
    <button id="next" type="button" disabled>next pair</button>
    <button id="draw" type="button" disabled>close qualifying</button>
  </div>
</div>

<div class="card hide" id="deckcard">
  <h2 id="decktitle">On deck</h2>
  <div class="deck" id="deck"></div>
  <div class="bracket" id="bracket"></div>
</div>

<div class="card">
  <h2>Lanes</h2>
  <table id="lanes"><tr><th>lane</th><th>where</th><th>dial</th><th>reaction</th>
  <th>ET</th><th>speed</th></tr></table>
</div>

<div class="card">
  <h2>Nodes</h2>
  <div id="nodes" class="row"></div>
</div>

<div class="card">
  <h2>Time slip</h2>
  <pre id="slip"></pre>
</div>

<footer id="foot"></footer>

<script>
(function(){{
  "use strict";
  var token = null;
  var $ = function(id){{ return document.getElementById(id); }};

  function post(path){{
    // `/api/call` carries the entry number, so the token joins whatever is there.
    var url = path + (token === null ? ""
      : (path.indexOf("?") < 0 ? "?" : "&") + "token=" + token);
    return fetch(url, {{method:"POST"}}).then(function(r){{ return r.json(); }});
  }}

  $("take").addEventListener("click", function(){{
    post("/api/control").then(function(j){{
      // Refused means somebody else is holding it and still coming back. That
      // is the answer, not an error to retry through.
      if (j.token !== null && j.token !== undefined) token = j.token;
      draw();
    }});
  }});
  [["arm","/api/arm"],["abort","/api/abort"],["next","/api/next"],
   ["record","/api/record"],["swap","/api/swap"],["draw","/api/draw"]].forEach(function(p){{
    $(p[0]).addEventListener("click", function(){{ post(p[1]).then(draw); }});
  }});

  // A car that could not make the call. Its opponent runs alone; pressing it
  // again puts the car back, because crews fix things in the lanes.
  $("deck").addEventListener("click", function(e){{
    var b = e.target.closest ? e.target.closest("[data-single]") : null;
    if (b) post("/api/single?lane=" + b.getAttribute("data-single")).then(draw);
  }});

  // Calling a car into a lane. Delegated, because the queue is rebuilt on every
  // poll and a listener per row would be a listener per poll.
  $("bracket").addEventListener("click", function(e){{
    var b = e.target.closest ? e.target.closest("[data-call]") : null;
    if (!b) return;
    post("/api/call?n=" + b.getAttribute("data-call") +
         "&lane=" + b.getAttribute("data-lane")).then(draw);
  }});

  function esc(v){{
    return String(v).replace(/[&<>"']/g, function(c){{
      return {{"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}}[c];
    }});
  }}
  function num(v){{ return v === null || v === undefined ? "—" : (+v).toFixed(4); }}

  // The ladder panel. Everything in it is seeds and names the server derived
  // from the result log, so this cannot show a pairing the log disagrees with.
  function deck(ev){{
    var card = $("deckcard");
    if (!ev) {{ card.className = "card hide"; return; }}
    card.className = "card";
    if (!ev.on) {{
      $("decktitle").textContent = "Event";
      $("deck").innerHTML = (ev.champions || []).length
        ? ev.champions.map(function(c){{
            return "<b>" + esc(c.who) + "</b><span>" + esc(c["class"]) + "</span>";
          }}).join("")
        : "<span>nothing on deck</span>";
      $("bracket").innerHTML = "";
      return;
    }}

    // Qualifying: one car and the queue behind it. No seeds, because a seed is
    // what this produces — and any row may be clicked, because cars arrive at the
    // lanes in the order they arrive rather than the order a list computed.
    if (ev.phase === "qualifying") {{
      $("decktitle").textContent = esc(ev["class"]) + " — qualifying";
      $("deck").innerHTML = (ev.cars || []).map(function(c){{
        return "<div>lane " + c.lane + " &nbsp;<b>" + esc(c.who) + "</b>" +
          (c.dial === null || c.dial === undefined ? ""
            : ' <span class="choice">dial ' + num(c.dial) + "</span>") + "</div>";
      }}).join("") + (ev.recorded ? '<div class="choice">recorded</div>' : "");
      $("bracket").innerHTML = (ev.queue || []).map(function(q){{
        // A lane button each, because a practice pass can hold two cars and the
        // operator is the only one who can see which lane each rolled into.
        var lane = function(n){{
          return '<button type="button" data-call="' + q.number + '" data-lane="' + n +
            '"' + (q.lane === n ? ' class="on"' : "") + ">L" + n + "</button>";
        }};
        return '<div class="pair' + (q.lane === null ? "" : " now") + '">' +
          '<span class="mark">' + (q.lane === null ? "" : "▸") + "</span>" +
          '<span class="who">' + esc(q.who) + "</span>" +
          '<span class="runs">' + q.runs + (q.runs === 1 ? " pass" : " passes") +
          "</span><span>" + (q.best === null ? "—" : num(q.best)) + "</span>" +
          lane(1) + lane(2) + "</div>";
      }}).join("");
      return;
    }}

    $("decktitle").textContent = esc(ev["class"]) + " — " + esc(ev.round) +
      (ev.bye ? " — bye" : "");
    var single = (ev.single === undefined) ? null : ev.single;
    $("deck").innerHTML = (ev.cars || []).map(function(c){{
      // A single: the other car could not make the call. Shown on both cars, so
      // the one that is not running reads as out rather than as missing.
      var alone = single === c.lane;
      return '<div' + (single !== null && !alone ? ' class="gone"' : "") + ">" +
        '<span class="seed">#' + c.seed + "</span> lane " + c.lane +
        " &nbsp;<b>" + esc(c.who) + "</b>" +
        (c.choice ? ' <span class="choice">lane choice</span>' : "") +
        (ev.bye ? "" : ' <button type="button" data-single="' + c.lane + '"' +
          (alone ? ' class="on"' : "") + ">runs alone</button>") + "</div>";
    }}).join("") + (ev.recorded ? '<div class="choice">recorded</div>' : "");

    var seat = {{}};
    (ev.field || []).forEach(function(f){{ seat[f.seed] = f.who; }});
    $("bracket").innerHTML = (ev.pairs || []).map(function(p){{
      var here = p.position === ev.position;
      var side = function(s){{
        if (s === null) return "<span>bye</span>";
        return '<span class="seed">' + s + "</span> " +
          '<span class="who">' + esc(seat[s] || "") + "</span>";
      }};
      return '<div class="pair' + (p.won !== null ? " done" : (here ? " now" : "")) + '">' +
        '<span class="mark">' + (p.won !== null ? "✓" : (here ? "▸" : "")) + "</span>" +
        side(p.left) + "<span>v</span>" + side(p.right) + "</div>";
    }}).join("");
  }}

  var last = {{}};
  function draw(){{
    var s = last;
    $("phase").textContent = s.phase || "…";
    $("note").textContent = s.note || "";
    $("win").textContent = s.winner ? "WIN " + s.winner : "";
    $("win").className = "pill" + (s.winner ? " on" : "");

    var mine = token !== null && s.holder === token;
    $("held").textContent = mine ? "you have control"
      : (s.held ? "control: another client" : "control: nobody");
    $("held").className = "pill " + (mine ? "on" : (s.held ? "off" : ""));
    $("take").disabled = mine;
    // Arm is offered only when the staging machine says the tree may be armed
    // *and* this client holds control. Neither alone is enough.
    $("arm").disabled = !(mine && s.ready && !s.armed);
    $("abort").disabled = !mine;
    $("next").disabled = !mine;
    // Recording is offered only when there is a round to record and an event to
    // record it into. Swapping stops being offered once the pair has been raced.
    var ev = s.event;
    $("record").disabled = !(mine && ev && ev.on && s.phase === "complete" && !ev.recorded);
    $("swap").disabled = !(mine && ev && ev.on && !s.armed && !ev.recorded);
    // Closing qualifying is offered only while a class is in it. How many passes
    // is a club's business, so nothing here decides the moment is right.
    var qualifying = !!(ev && ev.on && ev.phase === "qualifying");
    $("draw").disabled = !(mine && qualifying);
    $("next").textContent = qualifying ? "next car" : "next pair";
    $("swap").textContent = qualifying ? "other lane" : "swap lanes";
    deck(ev);

    var rows = ["<tr><th>lane</th><th>where</th><th>dial</th><th>reaction</th>" +
                "<th>ET</th><th>speed</th></tr>"];
    (s.lanes || []).forEach(function(l){{
      rows.push("<tr><td>" + l.lane + "</td><td>" + esc(l.where) + "</td>" +
        "<td class=num>" + num(l.dial) + "</td><td class=num>" + num(l.reaction) +
        "</td><td class=num>" + num(l.et) + "</td><td class=num>" +
        (l.kmh === null ? "—" : (+l.kmh).toFixed(1) + " km/h") + "</td></tr>");
    }});
    $("lanes").innerHTML = rows.join("");

    $("nodes").innerHTML = (s.nodes || []).map(function(n){{
      var cls = n.silent ? "off" : (n.known ? "on" : "");
      return '<span class="pill ' + cls + '">' + n.a + " " +
        (n.silent ? "silent" : (n.known ? "ok" : "unknown")) + "</span>";
    }}).join("");

    $("slip").textContent = s.slip || "";
    $("foot").textContent = (s.cycles || 0) + " poll cycles · " +
      (s.bus_ms || 0) + " ms of bus this cycle · nothing here has run against hardware";
  }}

  function tick(){{
    fetch("/api/state").then(function(r){{ return r.json(); }})
      .then(function(j){{ last = j; draw(); }})
      .catch(function(){{}});
    // Renewing control is what the poll is for as much as the state is: a token
    // that is not renewed expires, so a closed laptop frees the event instead of
    // stranding it.
    if (token !== null) post("/api/control");
  }}
  setInterval(tick, 500);
  tick();
}})();
</script>
</body></html>"##
    )
}

/// The spectator board, rendered live from the same snapshot.
pub fn board(venue: &str) -> String {
    format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{venue} — beam402 scoreboard</title>
<style>
*{{box-sizing:border-box}}
body{{background:#08080a;color:#d8d3c9;margin:0;padding:22px;
font:13px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;max-width:1180px}}
.top{{display:flex;gap:8px 18px;align-items:baseline;flex-wrap:wrap;
border-bottom:1px solid #23211e;padding-bottom:12px;margin-bottom:20px}}
h1{{font-size:15px;font-weight:600;letter-spacing:.14em;text-transform:uppercase;margin:0}}
h1 span{{color:#ff9d17}} .meta{{color:#6e6a62;font-size:12px}}
.warn{{margin-left:auto;color:#ff9d17;font-size:11px;letter-spacing:.08em;
text-transform:uppercase;border:1px solid currentColor;border-radius:3px;padding:2px 8px}}
.panel{{background:#000;border:10px solid #17181a;border-radius:6px;padding:14px;
box-shadow:inset 0 0 0 1px #2a2c2f,0 18px 60px -30px #ff9d17;overflow-x:auto}}
canvas{{display:block;width:100%;min-width:520px;image-rendering:pixelated}}
footer{{color:#46433d;font-size:11px;margin-top:16px}}
</style></head><body>
<div class="top"><h1>beam402 <span>scoreboard</span></h1>
<div class="meta">{venue}</div>
<div class="warn">simulated · no board exists</div></div>
<div class="panel"><canvas id="p" aria-label="the scoreboard"></canvas></div>
<footer>Live from the same snapshot the operator reads — one endpoint, so the two
screens cannot disagree about the round.</footer>
<script>
(function(){{
  "use strict";
  var c = document.getElementById("p"), ctx = c.getContext("2d");
  var PITCH = 8, R = 3.1, sized = false;
  function bit(hex, w, x, y){{
    var stride = Math.ceil(w / 8);
    var b = parseInt(hex.substr((y * stride + (x >> 3)) * 2, 2), 16);
    return (b & (0x80 >> x % 8)) !== 0;
  }}
  function draw(b){{
    if (!b) return;
    if (!sized) {{ c.width = b.w * PITCH; c.height = b.h * PITCH; sized = true; }}
    ctx.fillStyle = "#000"; ctx.fillRect(0, 0, c.width, c.height);
    for (var y = 0; y < b.h; y++) for (var x = 0; x < b.w; x++) {{
      var on = bit(b.bits, b.w, x, y);
      var cx = x * PITCH + PITCH / 2, cy = y * PITCH + PITCH / 2;
      // The dark diodes are drawn too: a board is a grid of LEDs that are
      // mostly off, and leaving them out turns a panel into floating text.
      ctx.beginPath(); ctx.arc(cx, cy, R, 0, Math.PI * 2);
      ctx.fillStyle = on ? "#ff9d17" : "#141310"; ctx.fill();
      if (on) {{
        ctx.beginPath(); ctx.arc(cx, cy, R * 2.1, 0, Math.PI * 2);
        ctx.fillStyle = "rgba(255,157,23,0.16)"; ctx.fill();
      }}
    }}
  }}
  setInterval(function(){{
    fetch("/api/state").then(function(r){{ return r.json(); }})
      .then(function(j){{ draw(j.board); }}).catch(function(){{}});
  }}, 500);
}})();
</script>
</body></html>"##
    )
}
