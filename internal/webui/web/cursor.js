if (window.matchMedia('(pointer: fine)').matches) {
  initCursor();
}

function initCursor() {
  const dot = document.createElement('div');
  dot.className = 'custom-cursor-dot';
  dot.innerHTML = '<span class="custom-cursor-icon" aria-hidden="true"></span>';

  const ring = document.createElement('div');
  ring.className = 'custom-cursor-ring';

  document.body.appendChild(dot);
  document.body.appendChild(ring);

  let mouseX = window.innerWidth / 2;
  let mouseY = window.innerHeight / 2;
  let ringX = mouseX;
  let ringY = mouseY;
  let hasMoved = false;

  window.addEventListener('mousemove', (e) => {
    mouseX = e.clientX;
    mouseY = e.clientY;
    if (!hasMoved) {
      hasMoved = true;
      dot.style.opacity = '1';
      ring.style.opacity = '1';
    }
  });

  // Target interactives to grow the cursor
  const hoverSelector = 'a, button, [role="button"], input, select, textarea, .tab-btn, .btn-link, .scroll-indicator, .landing-cta, .landing-nav-link, .landing-brand';

  document.addEventListener('mouseover', (e) => {
    const target = e.target.closest(hoverSelector);
    if (target && !target.disabled) {
      dot.classList.add('hovering');
      ring.classList.add('hovering');
    } else {
      dot.classList.remove('hovering');
      ring.classList.remove('hovering');
    }
  });

  // Handle click animations
  document.addEventListener('mousedown', () => {
    dot.classList.add('clicking');
    ring.classList.add('clicking');
  });

  document.addEventListener('mouseup', () => {
    dot.classList.remove('clicking');
    ring.classList.remove('clicking');
  });

  // Handle visibility transitions
  document.addEventListener('mouseleave', () => {
    dot.style.opacity = '0';
    ring.style.opacity = '0';
  });

  document.addEventListener('mouseenter', () => {
    if (hasMoved) {
      dot.style.opacity = '1';
      ring.style.opacity = '1';
    }
  });

  function updateCursor() {
    // Dot stays attached directly to mouse coordinates
    dot.style.transform = `translate3d(${mouseX}px, ${mouseY}px, 0)`;
    
    // Ring trails behind with lerp
    ringX += (mouseX - ringX) * 0.15;
    ringY += (mouseY - ringY) * 0.15;
    ring.style.transform = `translate3d(${ringX}px, ${ringY}px, 0)`;

    requestAnimationFrame(updateCursor);
  }

  requestAnimationFrame(updateCursor);
}
