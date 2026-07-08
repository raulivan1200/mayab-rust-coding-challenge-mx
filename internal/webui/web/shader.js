(() => {
  const canvas = document.getElementById("bg-shader");
  if (!canvas) return;

  const ctx = canvas.getContext("2d", { alpha: false });
  if (!ctx) return;

  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  const pointer = { x: 0.5, y: 0.5 };
  let width = 0;
  let height = 0;
  let frame = 0;
  let raf = 0;

  function resize() {
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    width = window.innerWidth;
    height = window.innerHeight;
    canvas.width = Math.max(1, Math.floor(width * ratio));
    canvas.height = Math.max(1, Math.floor(height * ratio));
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    draw();
  }

  function drawGrid(t) {
    const spacing = Math.max(42, Math.min(84, width / 18));
    const offset = (t * 18) % spacing;
    ctx.lineWidth = 1;
    ctx.strokeStyle = "rgba(32, 230, 154, 0.075)";
    ctx.beginPath();
    for (let x = -spacing + offset; x < width + spacing; x += spacing) {
      ctx.moveTo(x, 0);
      ctx.lineTo(x + height * 0.15, height);
    }
    for (let y = -spacing + offset; y < height + spacing; y += spacing) {
      ctx.moveTo(0, y);
      ctx.lineTo(width, y + width * 0.04);
    }
    ctx.stroke();
  }

  function drawWave(t) {
    const center = height * (0.36 + pointer.y * 0.16);
    const amplitude = Math.max(26, height * 0.055);
    const gradient = ctx.createLinearGradient(0, center - amplitude, width, center + amplitude);
    gradient.addColorStop(0, "rgba(32, 230, 154, 0)");
    gradient.addColorStop(0.45, "rgba(32, 230, 154, 0.28)");
    gradient.addColorStop(1, "rgba(34, 211, 238, 0)");

    ctx.lineWidth = 2;
    ctx.strokeStyle = gradient;
    ctx.shadowColor = "rgba(32, 230, 154, 0.28)";
    ctx.shadowBlur = 18;
    ctx.beginPath();
    for (let x = 0; x <= width; x += 18) {
      const y =
        center +
        Math.sin(x * 0.008 + t * 1.8) * amplitude +
        Math.sin(x * 0.017 - t * 1.1) * amplitude * 0.38;
      if (x === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
    ctx.shadowBlur = 0;
  }

  function drawParticles(t) {
    const total = Math.min(90, Math.max(34, Math.floor(width / 22)));
    for (let i = 0; i < total; i += 1) {
      const seed = i * 97.13;
      const x = (Math.sin(seed) * 10000 + width + t * (8 + (i % 5) * 3)) % width;
      const baseY = (Math.cos(seed * 1.7) * 10000 + height) % height;
      const y = (baseY + Math.sin(t * 0.9 + i) * 18) % height;
      const size = 1 + (i % 4) * 0.55;
      const alpha = 0.08 + (i % 7) * 0.018;
      ctx.fillStyle = i % 9 === 0 ? `rgba(247, 147, 26, ${alpha})` : `rgba(34, 211, 238, ${alpha})`;
      ctx.fillRect(x, y, size, size);
    }
  }

  function draw() {
    const t = frame / 60;
    const glowX = width * (0.15 + pointer.x * 0.7);
    const glowY = height * (0.2 + pointer.y * 0.5);

    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = "#070806";
    ctx.fillRect(0, 0, width, height);

    const radial = ctx.createRadialGradient(glowX, glowY, 0, glowX, glowY, Math.max(width, height) * 0.72);
    radial.addColorStop(0, "rgba(32, 230, 154, 0.16)");
    radial.addColorStop(0.34, "rgba(247, 147, 26, 0.08)");
    radial.addColorStop(1, "rgba(7, 8, 6, 0)");
    ctx.fillStyle = radial;
    ctx.fillRect(0, 0, width, height);

    drawGrid(t);
    drawWave(t);
    drawParticles(t);
  }

  function tick() {
    frame += 1;
    draw();
    if (!reducedMotion.matches && !document.hidden) {
      raf = window.requestAnimationFrame(tick);
    }
  }

  function start() {
    window.cancelAnimationFrame(raf);
    if (reducedMotion.matches || document.hidden) {
      draw();
      return;
    }
    raf = window.requestAnimationFrame(tick);
  }

  window.addEventListener("resize", resize, { passive: true });
  window.addEventListener(
    "pointermove",
    (event) => {
      pointer.x = width > 0 ? event.clientX / width : 0.5;
      pointer.y = height > 0 ? event.clientY / height : 0.5;
    },
    { passive: true },
  );
  document.addEventListener("visibilitychange", start);
  if (typeof reducedMotion.addEventListener === "function") {
    reducedMotion.addEventListener("change", start);
  } else if (typeof reducedMotion.addListener === "function") {
    reducedMotion.addListener(start);
  }

  resize();
  start();
})();
