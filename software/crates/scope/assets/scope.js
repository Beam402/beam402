// The capture is embedded above as `window.CAPTURE`. Everything here is a
// function of it plus one integer: which frame is showing.
(function () {
  "use strict";
  var C = window.CAPTURE;
  var frames = C.frames;
  var at = 0;
  var playing = false;
  var timer = null;

  var $ = function (id) {
    return document.getElementById(id);
  };

  // A venue name and a node label come out of a mapping file, and a mapping file
  // is shared: `protocol.md` §5 keeps one per venue in the club's own repository.
  // A scope page is meant to be emailed as evidence about a disputed round, so
  // nothing that arrived in a file gets to be markup on the way out.
  function esc(v) {
    return String(v).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  // --- the strip's broken scale -----------------------------------------
  //
  // A quarter mile is 402 m and everything that decides a race happens in the
  // first 20 of them. A linear axis would stack pre-stage, stage, guard and the
  // 60 ft beam into four pixels, so the first 25 m get a third of the width and
  // the break is drawn and labelled rather than hidden.
  var NEAR_M = 25,
    NEAR_W = 0.34,
    W = 1000,
    PAD = 34;

  function x(m) {
    var span = W - PAD * 2;
    var f =
      m <= NEAR_M
        ? (m / NEAR_M) * NEAR_W
        : NEAR_W + ((m - NEAR_M) / (C.finish_m - NEAR_M)) * (1 - NEAR_W);
    return PAD + f * span;
  }

  // --- what the car did, from what was measured -------------------------
  //
  // Crossings are measurements. The line between two of them is a drawing, and
  // the caption under the strip says so.
  function positionAt(lane, tRun) {
    var pts = C.crossings
      .filter(function (c) {
        return c.lane === lane;
      })
      .sort(function (a, b) {
        return a.t - b.t;
      });
    if (!pts.length || tRun == null) return null;
    if (tRun <= 0) return 0;
    var last = pts[pts.length - 1];
    if (tRun >= last.t) return last.m;
    var prev = { t: 0, m: 0 };
    for (var i = 0; i < pts.length; i++) {
      if (tRun < pts[i].t) {
        var f = (tRun - prev.t) / (pts[i].t - prev.t || 1);
        return prev.m + f * (pts[i].m - prev.m);
      }
      prev = pts[i];
    }
    return last.m;
  }

  // --- the strip --------------------------------------------------------

  function drawStrip() {
    var laneY = [52, 110];
    var H = 162;
    var s = ['<svg viewBox="0 0 ' + W + " " + H + '" role="img" aria-label="the strip">'];
    s.push(
      '<rect x="' +
        PAD +
        '" y="32" width="' +
        (W - PAD * 2) +
        '" height="' +
        (H - 62) +
        '" fill="var(--asphalt)"/>'
    );
    // Lane paint, and the centre line the trunk runs down (D21).
    [32, 81, H - 30].forEach(function (y) {
      s.push(
        '<line x1="' +
          PAD +
          '" y1="' +
          y +
          '" x2="' +
          (W - PAD) +
          '" y2="' +
          y +
          '" stroke="var(--paint)" stroke-width="1"/>'
      );
    });
    // The scale break.
    var bx = x(NEAR_M);
    s.push(
      '<line x1="' +
        bx +
        '" y1="26" x2="' +
        bx +
        '" y2="' +
        (H - 24) +
        '" stroke="var(--faint)" stroke-width="1" stroke-dasharray="2 4"/>'
    );
    s.push(
      '<text x="' +
        (bx + 4) +
        '" y="24" fill="var(--faint)" font-size="10">scale break \u00b7 25 m</text>'
    );

    // Four of these beams live inside half a metre of each other and the trap's
    // exit sits on the finish line, so labels are placed rather than emitted:
    // anything within 8 px of a label already drawn is dropped, and anything
    // within 34 px goes on the second row. Overlapping text is not a legend.
    var labelled = [];
    function labelRow(px) {
        var near = labelled.filter(function (l) {
            return Math.abs(l - px) < 34;
        });
        if (near.some(function (l) { return Math.abs(l - px) < 8; })) return null;
        labelled.push(px);
        return near.length % 2;
    }

    // Priority, so the beam that matters wins a contested slot: the finish line
    // is not going to lose its label to a trap mark that shares its position.
    var RANK = { finish: 0, interval_60: 1, interval_660: 1, stage: 2, trap_exit: 3, trap_entry: 3, prestage: 4, guard: 5 };
    function rank(name) {
      return name in RANK ? RANK[name] : 9;
    }
    C.beams
      .filter(function (b) { return b.lane === 1; })
      .slice()
      // `RANK[x] || 9` would rank the finish line last, because its rank is 0
      // and zero is falsy. The one beam nobody would agree to drop.
      .sort(function (a, b) { return rank(a.beam) - rank(b.beam); })
      .forEach(function (b) {
        var row = labelRow(x(b.m));
        if (row === null) return;
        s.push(
          '<text x="' + x(b.m) + '" y="' + (row === 0 ? 22 : 12) +
            '" fill="var(--dim)" font-size="9" text-anchor="middle">' +
            esc(short(b.beam)) + (b.assumed ? "*" : "") + "</text>"
        );
      });

    C.beams.forEach(function (b) {
      var y0 = b.lane === 1 ? 34 : 83,
        y1 = b.lane === 1 ? 79 : H - 32;
      var st = beamState(b);
      var col =
        st === "broken"
          ? "var(--red)"
          : st === "unknown"
            ? "var(--faint)"
            : "var(--green)";
      s.push(
        '<line x1="' +
          x(b.m) +
          '" y1="' +
          y0 +
          '" x2="' +
          x(b.m) +
          '" y2="' +
          y1 +
          '" stroke="' +
          col +
          '" stroke-width="' +
          (st === "broken" ? 2.5 : 1.5) +
          '" opacity="' +
          (st === "unknown" ? 0.5 : 1) +
          '"/>'
      );
    });

    [1, 2].forEach(function (lane) {
      if (lane > C.lanes) return;
      var m = carAt(lane);
      if (m == null) return;
      var cx = x(m),
        cy = laneY[lane - 1];
      s.push(
        '<path d="M' +
          (cx - 9) +
          " " +
          (cy - 6) +
          "L" +
          (cx + 7) +
          " " +
          cy +
          "L" +
          (cx - 9) +
          " " +
          (cy + 6) +
          'Z" fill="var(--accent)"/>'
      );
    });
    s.push(
      '<text x="' +
        PAD +
        '" y="' +
        (H - 8) +
        '" fill="var(--faint)" font-size="10">0 m</text>'
    );
    s.push(
      '<text x="' +
        (W - PAD) +
        '" y="' +
        (H - 8) +
        '" fill="var(--faint)" font-size="10" text-anchor="end">' +
        C.finish_m.toFixed(0) +
        " m</text>"
    );
    s.push("</svg>");
    $("strip").innerHTML = s.join("");
  }

  function short(name) {
    return { prestage: "pre", interval_60: "60ft", interval_660: "1/8", trap_entry: "trap", trap_exit: "trap", finish: "finish", stage: "stage", guard: "guard" }[name] || name;
  }

  function beamState(b) {
    var f = frames[at];
    for (var i = 0; i < f.nodes.length; i++) {
      if (f.nodes[i].a === b.a) {
        if (f.nodes[i].silent) return "unknown";
        // D17: a set bit is an intact beam. The polarity is the opposite of the
        // intuitive one, and it is the whole reason a cut cable is loud.
        return f.nodes[i].in & (1 << b.i) ? "intact" : "broken";
      }
    }
    return "unknown";
  }

  function carAt(lane) {
    var l = C.launch[lane - 1];
    if (l == null || frames[at].t < l) return null;
    return positionAt(lane, (frames[at].t - l) / 1000);
  }

  // --- panels -----------------------------------------------------------

  var LAMPS = ["prestage", "stage", "amber1", "amber2", "amber3", "green", "red"];

  function drawTree() {
    var f = frames[at];
    // The staging bits are the master's — it wrote them. Everything the cascade
    // lights belongs to the tree, and the master knows those only when it has
    // read the block. Across the quiet window it has not, so those bulbs are
    // drawn as *unknown* rather than guessed: the green genuinely happens while
    // nobody is looking (`architecture.md` §3), and the reaction times are read
    // out of the tree afterwards.
    var stale = f.tree_lamps === null || f.tree_age > 400;
    var out = ['<div class="tree">'];
    for (var lane = 1; lane <= C.lanes; lane++) {
      out.push('<div class="column"><div class="who">LANE ' + lane + "</div>");
      LAMPS.forEach(function (lamp, i) {
        var bit = 1 << (i + 7 * (lane - 1));
        var master = i < 2;
        var word = master ? f.lamps : f.tree_lamps;
        var unknown = !master && stale;
        var on = !unknown && word !== null && word & bit ? " on" : "";
        out.push(
          '<div class="bulb' +
            (master ? " small" : "") +
            (unknown ? " unknown" : "") +
            on +
            '" data-lamp="' +
            lamp +
            '"></div>'
        );
      });
      out.push("</div>");
    }
    out.push("</div>");
    if (stale) {
      out.push(
        '<div class="spot">the cascade is the tree\u2019s \u2014 the master is not polling it</div>'
      );
    }
    var spot = C.handicap[0] || C.handicap[1];
    out.push(
      '<div class="spot">' +
        (spot
          ? "handicap <b>" +
            (spot / 1000).toFixed(3) +
            " s</b> on lane " +
            (C.handicap[0] ? 1 : 2)
          : "heads-up — both cascades together") +
        "</div>"
    );
    $("tree").innerHTML = out.join("");
  }

  function drawNodes() {
    var f = frames[at];
    var rows = [
      "<table><tr><th>addr</th><th>node</th><th>beams</th><th>state</th></tr>",
    ];
    f.nodes.forEach(function (n) {
      var label = "";
      C.labels.forEach(function (l) {
        if (l.a === n.a) label = l.label;
      });
      var bits = ['<span class="beambits">'];
      var mapped = C.beams.filter(function (b) {
        return b.a === n.a;
      });
      if (!mapped.length) bits.push('<span class="bit unknown"></span>');
      mapped.forEach(function (b) {
        var st = beamState(b);
        bits.push(
          '<span class="bit ' +
            (st === "broken" ? "broken" : st === "unknown" ? "unknown" : "") +
            '" title="lane ' +
            b.lane +
            " " +
            esc(b.beam) +
            '"></span>'
        );
      });
      bits.push("</span>");
      rows.push(
        '<tr class="' +
          (n.silent ? "silent" : "") +
          '"><td class="num">' +
          n.a +
          "</td><td>" +
          esc(label || "tree") +
          "</td><td>" +
          bits.join("") +
          "</td><td>" +
          (n.silent ? "silent" : n.id ? "ok" : "unknown") +
          "</td></tr>"
      );
    });
    rows.push("</table>");
    $("nodes").innerHTML = rows.join("");
  }

  function tape(el, lines, empty) {
    if (!lines.length) {
      el.innerHTML = '<div class="none">' + empty + "</div>";
      return;
    }
    el.innerHTML = lines.join("");
    el.scrollTop = el.scrollHeight;
  }

  // Both tapes show everything up to the current frame, so scrubbing back is a
  // rewind rather than a different program.
  function drawTapes() {
    var bus = [],
      ev = [];
    for (var i = 0; i <= at; i++) {
      var now = i === at ? " now" : "";
      frames[i].txns.forEach(function (t) {
        bus.push(
          '<div class="' +
            (t.ok ? "" : "bad") +
            now +
            '">' +
            pad(frames[i].t / 1000, 6) +
            (t.w ? ' <span class="w">W</span>' : " R") +
            " " +
            pad(t.a, 3) +
            " " +
            esc(padr(t.b, 14)) +
            " " +
            pad(t.n, 3) +
            (t.ok ? "" : "  no answer") +
            "</div>"
        );
      });
      frames[i].events.forEach(function (e) {
        ev.push(
          '<div class="' + now + '">' + pad(frames[i].t / 1000, 6) + "  " + esc(e) + "</div>"
        );
      });
    }
    tape($("bus"), bus, "the bus is quiet");
    tape($("events"), ev, "nothing yet");
  }

  function pad(v, n) {
    var s = typeof v === "number" ? (n === 6 ? v.toFixed(2) : String(v)) : String(v);
    while (s.length < n) s = " " + s;
    return s;
  }
  function padr(s, n) {
    s = String(s);
    while (s.length < n) s += " ";
    return s;
  }

  function drawRun() {
    if (!C.crossings.length) {
      $("run").innerHTML = '<div class="caption">no crossing was measured</div>';
      return;
    }
    var max = 0;
    C.crossings.forEach(function (c) {
      if (c.t > max) max = c.t;
    });
    var out = [];
    for (var lane = 1; lane <= C.lanes; lane++) {
      var pts = C.crossings.filter(function (c) {
        return c.lane === lane;
      });
      out.push('<div class="axis"><span class="lane">L' + lane + "</span>");
      var placed = [];
      pts.forEach(function (c) {
        var pct = (c.t / max) * 96;
        // Trap entry and exit are 0.19 s apart on a 12 s axis. Two labels there
        // are one unreadable smear, so the second gets the upper row.
        var near = placed.filter(function (p) {
          return Math.abs(p - pct) < 9;
        }).length;
        placed.push(pct);
        out.push(
          '<span class="mark" style="left:' +
            pct.toFixed(2) +
            '%"><span' +
            (near ? ' class="r' + Math.min(near, 3) + '"' : "") +
            ">" +
            esc(short(c.beam)) +
            " " +
            c.t.toFixed(3) +
            "</span></span>"
        );
      });
      out.push("</div>");
    }
    out.push(
      '<div class="caption">seconds from each car’s own launch pulse — one register from one node, on that node’s own timer (D04)</div>'
    );
    $("run").innerHTML = out.join("");
  }

  // --- transport --------------------------------------------------------

  function show(i) {
    at = Math.max(0, Math.min(frames.length - 1, i));
    var f = frames[at];
    $("scrub").value = at;
    $("clock").innerHTML =
      (f.t / 1000).toFixed(1) + "<small> s</small>";
    var p = $("phase");
    p.textContent = f.phase;
    p.dataset.phase = f.phase.split(":")[0];
    $("where").textContent =
      "lane 1 " + f.pos[0] + (C.lanes > 1 ? "  ·  lane 2 " + f.pos[1] : "");
    $("cost").textContent = f.bus_ms.toFixed(0) + " ms of bus so far";
    drawStrip();
    drawTree();
    drawNodes();
    drawTapes();
  }

  function play(on) {
    playing = on;
    $("play").textContent = on ? "pause" : "play";
    if (timer) clearInterval(timer);
    if (!on) return;
    timer = setInterval(function () {
      if (at >= frames.length - 1) return play(false);
      show(at + 1);
    }, 60);
  }

  $("scrub").max = frames.length - 1;
  $("scrub").addEventListener("input", function (e) {
    play(false);
    show(+e.target.value);
  });
  $("play").addEventListener("click", function () {
    if (at >= frames.length - 1) show(0);
    play(!playing);
  });
  $("first").addEventListener("click", function () {
    play(false);
    show(0);
  });
  $("last").addEventListener("click", function () {
    play(false);
    show(frames.length - 1);
  });
  document.addEventListener("keydown", function (e) {
    if (e.key === " ") {
      e.preventDefault();
      $("play").click();
    }
    if (e.key === "ArrowRight") show(at + 1);
    if (e.key === "ArrowLeft") show(at - 1);
  });

  drawRun();
  show(0);
})();
