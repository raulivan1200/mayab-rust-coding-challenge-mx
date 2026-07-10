const gridContainer = document.getElementById("bg-grid");

if (gridContainer) {
  let resizeTimer = 0;
  let tiles = [];
  let cols = 0;
  let rows = 0;
  let hoverTimers = new Map();
  
  const colors = [
    "var(--blue)",
    "var(--green)",
    "var(--orange)",
    "var(--yellow)",
    "var(--purple)"
  ];
  
  function createGrid() {
    gridContainer.innerHTML = "";
    tiles = [];
    hoverTimers.clear();
    
    const tileSize = 52; 
    cols = Math.ceil(window.innerWidth / tileSize);
    rows = Math.ceil(window.innerHeight / tileSize);
    
    gridContainer.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
    gridContainer.style.gridTemplateRows = `repeat(${rows}, 1fr)`;
    
    const numTiles = cols * rows;
    
    for (let i = 0; i < numTiles; i++) {
      const tile = document.createElement("div");
      tile.className = "grid-tile";
      
      const hoverColor = colors[Math.floor(Math.random() * colors.length)];
      tile.style.setProperty("--hover-c", hoverColor);
      
      gridContainer.appendChild(tile);
      tiles.push(tile);
    }
  }

  createGrid();

  window.addEventListener("resize", () => {
    window.clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(createGrid, 200);
  }, { passive: true });

  window.addEventListener("mousemove", (e) => {
    const col = Math.floor((e.clientX / window.innerWidth) * cols);
    const row = Math.floor((e.clientY / window.innerHeight) * rows);
    const index = row * cols + col;
    
    if (tiles[index]) {
      const tile = tiles[index];
      tile.classList.add("hovered");
      
      if (hoverTimers.has(index)) {
        clearTimeout(hoverTimers.get(index));
      }
      
      hoverTimers.set(index, setTimeout(() => {
        tile.classList.remove("hovered");
        hoverTimers.delete(index);
      }, 150));
    }
  }, { passive: true });
}

import { ShaderMount, paperTextureFragmentShader, getShaderColorFromString } from 'https://esm.sh/@paper-design/shaders@0.0.76';

const paperOverlay = document.getElementById('paper-texture-overlay');
if (paperOverlay) {
  new ShaderMount(
    paperOverlay,
    paperTextureFragmentShader,
    {
      u_colorBack: getShaderColorFromString('#ffffff'),
      u_colorFront: getShaderColorFromString('#9fadbc'),
      u_contrast: 0.3,
      u_roughness: 0.4,
      u_fiber: 0.3,
      u_fiberSize: 0.2,
      u_crumples: 0.3,
      u_crumpleSize: 0.35,
      u_folds: 0.65,
      u_foldCount: 5,
      u_drops: 0.2,
      u_fade: 0,
      u_seed: 5.8
    },
    undefined,
    0
  );
}
