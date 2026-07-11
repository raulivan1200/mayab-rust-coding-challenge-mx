if (window.matchMedia('(pointer: fine)').matches) {
  initCursor();
}

function initCursor() {
  const cursor = document.createElement('span');
  cursor.className = 'custom-cursor';
  cursor.setAttribute('aria-hidden', 'true');
  cursor.hidden = true;

  const cursorIcon = document.createElement('img');
  cursorIcon.src = '/icons/currency_bitcoin.svg';
  cursorIcon.alt = '';
  cursorIcon.draggable = false;
  cursor.appendChild(cursorIcon);
  document.body.appendChild(cursor);

  let mouseX = window.innerWidth / 2;
  let mouseY = window.innerHeight / 2;
  let hasMoved = false;
  let animationFrame = 0;

  window.addEventListener('pointermove', (e) => {
    if (e.pointerType === 'touch') return;
    mouseX = e.clientX;
    mouseY = e.clientY;
    if (!hasMoved) {
      hasMoved = true;
      cursor.hidden = false;
    }
    if (!animationFrame) animationFrame = requestAnimationFrame(updateCursor);
  }, { passive: true });

  const interactiveSelector = [
    'a[href]',
    'button:not(:disabled)',
    '[role="button"]:not([aria-disabled="true"])',
    'input:not(:disabled)',
    'select:not(:disabled)',
    'textarea:not(:disabled)',
    '[tabindex]:not([tabindex="-1"])',
    '.scroll-indicator',
    '.landing-brand',
  ].join(',');

  document.addEventListener('pointerover', (event) => {
    if (event.pointerType === 'touch') return;
    cursor.classList.toggle('is-clickable', Boolean(event.target.closest?.(interactiveSelector)));
  }, { passive: true });

  document.addEventListener('pointerdown', (event) => {
    if (event.pointerType !== 'touch') cursor.classList.add('is-pressed');
  }, { passive: true });
  document.addEventListener('pointerup', () => cursor.classList.remove('is-pressed'), { passive: true });
  window.addEventListener('blur', hideCursor);

  // Handle visibility transitions
  document.addEventListener('mouseleave', hideCursor);

  function hideCursor() {
    cursor.hidden = true;
    cursor.classList.remove('is-clickable', 'is-pressed');
  }

  document.addEventListener('mouseenter', () => {
    if (hasMoved) cursor.hidden = false;
  });

  function updateCursor() {
    animationFrame = 0;

    cursor.style.transform = `translate3d(${mouseX}px, ${mouseY}px, 0)`;
  }
}
