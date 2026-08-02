// The board, drawn as diodes.
//
// Canvas rather than eight thousand elements: at the reference geometry that is
// what the panel has, and a DOM node per LED would make scrubbing crawl for no
// gain.
(function () {
  "use strict";
  var B = window.BOARD;
  var at = 0;
  var playing = false;
  var timer = null;
  var $ = function (id) {
    return document.getElementById(id);
  };

  var canvas = $("panel");
  var ctx = canvas.getContext("2d");

  // Each diode is a disc on a pitch, with a gap, because a solid grid of squares
  // reads as a screen and the thing being drawn is not a screen.
  var PITCH = 8;
  var R = 3.1;
  canvas.width = B.w * PITCH;
  canvas.height = B.h * PITCH;

  function bitOf(hex, x, y) {
    var stride = Math.ceil(B.w / 8);
    var byte = parseInt(hex.substr((y * stride + (x >> 3)) * 2, 2), 16);
    return (byte & (0x80 >> x % 8)) !== 0;
  }

  function draw() {
    var hex = B.frames[at].bits;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, canvas.width, canvas.height);

    for (var y = 0; y < B.h; y++) {
      for (var x = 0; x < B.w; x++) {
        var on = bitOf(hex, x, y);
        var cx = x * PITCH + PITCH / 2;
        var cy = y * PITCH + PITCH / 2;
        // The dark diodes are drawn too. A board is a grid of LEDs that are
        // mostly off, and leaving them out turns a panel into floating text.
        ctx.beginPath();
        ctx.arc(cx, cy, R, 0, Math.PI * 2);
        ctx.fillStyle = on ? "#ff9d17" : "#141310";
        ctx.fill();
        if (on) {
          ctx.beginPath();
          ctx.arc(cx, cy, R * 2.1, 0, Math.PI * 2);
          ctx.fillStyle = "rgba(255,157,23,0.16)";
          ctx.fill();
        }
      }
    }
  }

  function show(i) {
    at = Math.max(0, Math.min(B.frames.length - 1, i));
    var f = B.frames[at];
    $("scrub").value = at;
    // textContent, not innerHTML. The value is a formatted number and could not
    // carry markup, but the habit is worth more than the exception.
    $("secs").textContent = (f.t / 1000).toFixed(1);
    var p = $("show");
    p.textContent = f.show;
    p.dataset.show = f.show;
    draw();
  }

  function play(on) {
    playing = on;
    $("play").textContent = on ? "pause" : "play";
    if (timer) clearInterval(timer);
    if (!on) return;
    timer = setInterval(function () {
      if (at >= B.frames.length - 1) return play(false);
      show(at + 1);
    }, 900);
  }

  $("scrub").max = B.frames.length - 1;
  $("scrub").addEventListener("input", function (e) {
    play(false);
    show(+e.target.value);
  });
  $("play").addEventListener("click", function () {
    if (at >= B.frames.length - 1) show(0);
    play(!playing);
  });
  document.addEventListener("keydown", function (e) {
    if (e.key === " ") {
      e.preventDefault();
      $("play").click();
    }
    if (e.key === "ArrowRight") show(at + 1);
    if (e.key === "ArrowLeft") show(at - 1);
  });

  show(B.frames.length - 1);
})();
