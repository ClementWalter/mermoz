/**
 * Mermoz landing animation: a paper plane loops through a sky of crumpled paper balls
 * and pencil-hatched clouds, leaving a dashed pencil trail. Everything is drawn as SVG
 * and moved with transforms only; motion stops for visitors who ask for less of it.
 */
(() => {
  const sky = document.getElementById("sky");
  const plane = document.getElementById("plane");
  const trailSvg = document.getElementById("trail");
  const trailPath = document.getElementById("trail-path");
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Seeded random so the sky looks the same on every visit.
  let seed = 7;
  const rand = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);

  /** A crumpled paper ball: a jittered outline, soft shading and a few creases. */
  function paperBall(r) {
    const pts = [];
    const n = 13;
    for (let i = 0; i < n; i++) {
      const a = (i / n) * Math.PI * 2;
      const rr = r * (0.84 + rand() * 0.22);
      pts.push([r + Math.cos(a) * rr, r + Math.sin(a) * rr]);
    }
    const creases = [];
    for (let i = 0; i < 6; i++) {
      const a = rand() * Math.PI * 2;
      const b = a + 0.8 + rand() * 1.6;
      const x1 = r + Math.cos(a) * r * 0.75, y1 = r + Math.sin(a) * r * 0.75;
      const mx = r + (rand() - 0.5) * r * 0.6, my = r + (rand() - 0.5) * r * 0.6;
      const x2 = r + Math.cos(b) * r * 0.7, y2 = r + Math.sin(b) * r * 0.7;
      creases.push(`M${x1.toFixed(1)} ${y1.toFixed(1)} L${mx.toFixed(1)} ${my.toFixed(1)} L${x2.toFixed(1)} ${y2.toFixed(1)}`);
    }
    const id = `g${Math.floor(rand() * 1e9)}`;
    const size = r * 2;
    return `<svg viewBox="-4 -4 ${size + 8} ${size + 8}" width="${size + 8}" height="${size + 8}">
      <defs><radialGradient id="${id}" cx="35%" cy="30%" r="75%">
        <stop offset="0%" stop-color="#FFFFFF"/><stop offset="65%" stop-color="#EEF5FC"/><stop offset="100%" stop-color="#CADCEC"/>
      </radialGradient></defs>
      <g filter="url(#pencil)">
        <polygon points="${pts.map((p) => p.map((v) => v.toFixed(1)).join(",")).join(" ")}" fill="url(#${id})" stroke="#8FA9BF" stroke-width="1.3" stroke-linejoin="round"/>
        <path d="${creases.join(" ")}" fill="none" stroke="#9DB6CB" stroke-width="1" stroke-linecap="round" opacity="0.9"/>
      </g>
    </svg>`;
  }

  /** A cloud outline filled with diagonal pencil hatching. */
  function hatchedCloud(w) {
    const h = w * 0.45;
    const id = `c${Math.floor(rand() * 1e9)}`;
    const cloud = `M${w * 0.12} ${h * 0.9} C ${w * 0.0} ${h * 0.9}, ${w * 0.0} ${h * 0.5}, ${w * 0.2} ${h * 0.52}
      C ${w * 0.22} ${h * 0.15}, ${w * 0.5} ${h * 0.05}, ${w * 0.56} ${h * 0.36}
      C ${w * 0.66} ${h * 0.18}, ${w * 0.92} ${h * 0.28}, ${w * 0.86} ${h * 0.56}
      C ${w * 1.0} ${h * 0.6}, ${w * 0.98} ${h * 0.92}, ${w * 0.84} ${h * 0.9} Z`;
    const lines = [];
    for (let x = -h; x < w; x += 9) lines.push(`M${x} ${h} L${x + h} 0`);
    return `<svg viewBox="0 0 ${w} ${h}" width="${w}" height="${h}">
      <defs><clipPath id="${id}"><path d="${cloud}"/></clipPath></defs>
      <g filter="url(#pencil)" fill="none" stroke="#9AB4CA" stroke-linecap="round">
        <g clip-path="url(#${id})" stroke-width="1.1">${lines.map((d) => `<path d="${d}"/>`).join("")}</g>
        <path d="${cloud}" stroke-width="1.6"/>
      </g>
    </svg>`;
  }

  // Populate the sky: far things small and slow, near things big and quick.
  const floaters = [];
  const add = (html, cls, depth) => {
    const el = document.createElement("div");
    el.className = cls;
    el.innerHTML = html;
    // Always behind the headline (z 4) so the text stays readable.
    el.style.zIndex = depth > 0.7 ? 2 : 1;
    sky.appendChild(el);
    floaters.push({ el, depth, x: rand(), y: 0.08 + rand() * 0.84, bob: rand() * Math.PI * 2, spin: cls === "ball" });
  };
  for (let i = 0; i < 5; i++) add(hatchedCloud(180 + rand() * 160), "hatch", 0.25 + rand() * 0.3);
  for (let i = 0; i < 12; i++) {
    const depth = 0.3 + rand() * 0.75;
    add(paperBall(10 + depth * 30), "ball", depth);
  }

  let W = 0, H = 0;
  const resize = () => {
    W = sky.clientWidth;
    H = sky.clientHeight;
    trailSvg.setAttribute("width", W);
    trailSvg.setAttribute("height", H);
  };
  resize();
  window.addEventListener("resize", resize);

  let pointerX = 0, pointerY = 0;
  window.addEventListener("pointermove", (e) => {
    pointerX = e.clientX / window.innerWidth - 0.5;
    pointerY = e.clientY / window.innerHeight - 0.5;
  });

  /** The plane's route: a slow figure-eight that loops around the headline. */
  const route = (t) => ({
    x: W * (0.5 + 0.44 * Math.sin(t)),
    y: H * (0.44 + 0.27 * Math.sin(2 * t + 0.6)),
  });

  const trail = [];
  let t = -1.2;
  let last = performance.now();

  function frame(now) {
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    t += dt * 0.21;
    const scroll = window.scrollY;

    // The plane follows the route, nose along its heading, never upside down.
    const p = route(t);
    const q = route(t + 0.01);
    const angle = Math.atan2(q.y - p.y, q.x - p.x);
    const flip = Math.cos(angle) < 0 ? -1 : 1;
    const bank = Math.sin(t * 2) * 6;
    plane.style.transform = `translate(${p.x - 60}px, ${p.y - 40}px) rotate(${angle}rad) scaleY(${flip}) rotate(${bank}deg)`;

    // Dashed pencil trail behind the plane, fading as it ages.
    trail.push([p.x, p.y]);
    if (trail.length > 260) trail.shift();
    trailPath.setAttribute("d", trail.length > 1 ? "M" + trail.filter((_, i) => i % 3 === 0).map((pt) => pt.map((v) => v.toFixed(1)).join(" ")).join(" L") : "");

    // Clouds and balls drift left, bob, and shift with scroll and pointer (parallax).
    for (const f of floaters) {
      f.x -= dt * 0.006 * (0.4 + f.depth);
      if (f.x < -0.15) { f.x = 1.1; f.y = 0.08 + rand() * 0.84; }
      const bob = Math.sin(now / 1000 * (0.4 + f.depth * 0.5) + f.bob) * 10 * f.depth;
      const px = f.x * W + pointerX * 30 * f.depth;
      const py = f.y * H + bob + pointerY * 20 * f.depth - scroll * 0.35 * f.depth;
      // Paper balls tumble; clouds stay level.
      const spin = f.spin ? (now / 1000 * 6 * f.depth + f.bob * 40) % 360 : Math.sin(now / 3000 + f.bob) * 2;
      f.el.style.transform = `translate(${px.toFixed(1)}px, ${py.toFixed(1)}px) rotate(${spin.toFixed(1)}deg)`;
    }
    requestAnimationFrame(frame);
  }

  if (reduced) {
    // A still sky: everything placed once, no flight.
    for (const f of floaters) f.el.style.transform = `translate(${f.x * W}px, ${f.y * H}px)`;
    const p = route(0.9);
    plane.style.transform = `translate(${p.x - 60}px, ${p.y - 40}px) rotate(-12deg)`;
  } else {
    requestAnimationFrame(frame);
  }

  // Sections rise into place as they scroll into view.
  const io = new IntersectionObserver(
    (entries) => entries.forEach((e) => e.isIntersecting && (e.target.classList.add("shown"), io.unobserve(e.target))),
    { threshold: 0.15 },
  );
  document.querySelectorAll(".reveal").forEach((el) => io.observe(el));
})();
