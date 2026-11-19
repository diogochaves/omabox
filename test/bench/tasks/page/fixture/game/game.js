// A tiny game: rocks drift across the screen; the score starts from a bonus worked out at load.
const c = document.getElementById("c");
const g = c.getContext("2d");
const rocks = Array.from({ length: 12 }, (_, i) => ({ x: (i * 97) % 800, y: (i * 53) % 420 + 40, r: 8 + (i % 4) * 5 }));
const bonus = [37, 41].reduce((a, b) => a * b);
let score = bonus;

function frame() {
  g.fillStyle = "#111a33";
  g.fillRect(0, 0, c.width, c.height);
  g.fillStyle = "#94a3b8";
  for (const r of rocks) {
    r.x = (r.x + 0.6) % 820;
    g.beginPath();
    g.arc(r.x, r.y, r.r, 0, Math.PI * 2);
    g.fill();
  }
  g.fillStyle = "#facc15";
  g.font = "bold 44px sans-serif";
  g.fillText("SCORE " + score, 24, 60);
  requestAnimationFrame(frame);
}
frame();
