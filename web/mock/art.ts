// Procedural SVG pictures for the mock library, so screens can be judged with something that
// reads like real work. Deterministic per seed. Dev only: never shipped in the build.

type Rand = () => number;

function rng(seed: number): Rand {
  let state = seed >>> 0 || 1;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function hashString(text: string): number {
  let hash = 2166136261;
  for (let i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 16777619);
  return hash >>> 0;
}

interface Palette {
  sky: [string, string, string];
  sun: string;
  far: string;
  near: string;
  fog: string;
  accent: string[];
}

const palettes: Record<string, Palette> = {
  dawn: { sky: ["#f3d3c4", "#efb9a3", "#e49c80"], sun: "#fff1d9", far: "#c6a6b3", near: "#55445a", fog: "#fbefe7", accent: ["#f2693d", "#ad4527"] },
  golden: { sky: ["#f8dba6", "#f2a75f", "#de7442"], sun: "#fff3d0", far: "#c48b5f", near: "#45291f", fog: "#fbe6c8", accent: ["#f2693d", "#7a3b22"] },
  mist: { sky: ["#e2e9eb", "#c7d4d9", "#a8bcc4"], sun: "#f8f4ea", far: "#9cb0b7", near: "#2c3b40", fog: "#f4f6f4", accent: ["#e9b44c", "#3f5964"] },
  forest: { sky: ["#e6ecd9", "#c9d5b5", "#a5b88e"], sun: "#fbf6e0", far: "#8ea687", near: "#22362a", fog: "#f1f3e6", accent: ["#e7a54a", "#5b3b26"] },
  night: { sky: ["#0e1530", "#1c2352", "#3b2b64"], sun: "#f3eee0", far: "#2a2f58", near: "#0a0d1e", fog: "#3c3f6b", accent: ["#ff4fa3", "#33d1ff", "#ffd166"] },
  editorial: { sky: ["#f2ece2", "#efe7da", "#e9dfcf"], sun: "#f2693d", far: "#ad4527", near: "#25221e", fog: "#e0c9a6", accent: ["#f2693d", "#ad4527", "#25221e", "#d9bf98", "#5f7d8c"] },
};

function pickPalette(prompt: string, random: Rand): Palette {
  const text = prompt.toLowerCase();
  if (/night|neon|city|skyline|tokyo|moon/.test(text)) return palettes.night!;
  if (/poster|geometric|abstract|swiss|riso|arches/.test(text)) return palettes.editorial!;
  if (/fog|mist|snow|winter|blue hour|fjord|harbor|storm/.test(text)) return palettes.mist!;
  if (/forest|pine|green|tuscany|hills|autumn/.test(text)) return palettes.forest!;
  if (/golden|sunset|dusk|desert|dune|warm|terracotta/.test(text)) return palettes.golden!;
  if (/dawn|sunrise|morning/.test(text)) return palettes.dawn!;
  const keys = Object.keys(palettes).filter((key) => key !== "editorial" && key !== "night");
  return palettes[keys[Math.floor(random() * keys.length)]!]!;
}

function mix(a: string, b: string, t: number): string {
  const pa = [1, 3, 5].map((i) => parseInt(a.slice(i, i + 2), 16));
  const pb = [1, 3, 5].map((i) => parseInt(b.slice(i, i + 2), 16));
  return `#${pa.map((v, i) => Math.round(v + (pb[i]! - v) * t).toString(16).padStart(2, "0")).join("")}`;
}

const f = (value: number) => Math.round(value * 10) / 10;

function ridge(random: Rand, w: number, h: number, base: number, amplitude: number, steps = 26): string {
  let y = base + (random() - 0.5) * amplitude;
  const points = [`M0 ${f(h)}`, `L0 ${f(y)}`];
  for (let i = 1; i <= steps; i++) {
    y += (random() - 0.5) * amplitude * 0.9;
    y = Math.max(base - amplitude, Math.min(base + amplitude * 0.6, y));
    points.push(`L${f((w * i) / steps)} ${f(y)}`);
  }
  points.push(`L${f(w)} ${f(h)}Z`);
  return points.join("");
}

function sky(p: Palette, id: string) {
  return `<linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${p.sky[0]}"/><stop offset=".55" stop-color="${p.sky[1]}"/><stop offset="1" stop-color="${p.sky[2]}"/></linearGradient>`;
}

function glow(p: Palette, id: string) {
  return `<radialGradient id="${id}"><stop offset="0" stop-color="${p.sun}" stop-opacity=".9"/><stop offset=".35" stop-color="${p.sun}" stop-opacity=".35"/><stop offset="1" stop-color="${p.sun}" stop-opacity="0"/></radialGradient>`;
}

function landscape(random: Rand, w: number, h: number, p: Palette, prompt: string): string {
  const sx = w * (0.25 + random() * 0.5);
  const sy = h * (0.22 + random() * 0.2);
  const r = Math.min(w, h) * (0.05 + random() * 0.05);
  let body = `<defs>${sky(p, "s")}${glow(p, "g")}<linearGradient id="fog" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${p.fog}" stop-opacity="0"/><stop offset=".5" stop-color="${p.fog}" stop-opacity=".75"/><stop offset="1" stop-color="${p.fog}" stop-opacity="0"/></linearGradient></defs>`;
  body += `<rect width="${w}" height="${h}" fill="url(#s)"/><circle cx="${f(sx)}" cy="${f(sy)}" r="${f(r * 4)}" fill="url(#g)"/><circle cx="${f(sx)}" cy="${f(sy)}" r="${f(r)}" fill="${p.sun}"/>`;
  const layers = 4;
  for (let i = 0; i < layers; i++) {
    const t = i / (layers - 1);
    body += `<path d="${ridge(random, w, h, h * (0.46 + i * 0.12), h * (0.16 - i * 0.025))}" fill="${mix(p.far, p.near, t)}"/>`;
    if (i === 1) body += `<rect y="${f(h * 0.5)}" width="${w}" height="${f(h * 0.22)}" fill="url(#fog)"/>`;
  }
  if (/cabin|house|home|hut/.test(prompt)) {
    const x = w * (0.3 + random() * 0.35);
    const y = h * 0.8;
    const s = w * 0.07;
    body += `<path d="M${f(x)} ${f(y)}h${f(s)}v${f(-s * 0.62)}l${f(-s / 2)} ${f(-s * 0.45)}l${f(-s / 2)} ${f(s * 0.45)}z" fill="${mix(p.near, "#000000", 0.25)}"/><rect x="${f(x + s * 0.38)}" y="${f(y - s * 0.42)}" width="${f(s * 0.22)}" height="${f(s * 0.2)}" fill="#ffcf7a"/>`;
  }
  if (/pine|forest|trees|cypress/.test(prompt)) {
    for (let i = 0; i < 18; i++) {
      const x = random() * w;
      const y = h * (0.82 + random() * 0.14);
      const s = w * (0.018 + random() * 0.03);
      body += `<path d="M${f(x)} ${f(y - s * 3)}l${f(s)} ${f(s * 3)}h${f(-s * 2)}z" fill="${mix(p.near, "#000000", 0.3)}"/>`;
    }
  }
  return body;
}

function coast(random: Rand, w: number, h: number, p: Palette, prompt: string): string {
  const horizon = h * (0.56 + random() * 0.08);
  const sx = w * (0.3 + random() * 0.4);
  const r = Math.min(w, h) * 0.07;
  let body = `<defs>${sky(p, "s")}${glow(p, "g")}<linearGradient id="sea" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${mix(p.sky[2], p.near, 0.35)}"/><stop offset="1" stop-color="${mix(p.near, "#000000", 0.2)}"/></linearGradient></defs>`;
  body += `<rect width="${w}" height="${h}" fill="url(#s)"/><circle cx="${f(sx)}" cy="${f(horizon - r * 0.4)}" r="${f(r * 4.5)}" fill="url(#g)"/><circle cx="${f(sx)}" cy="${f(horizon - r * 0.4)}" r="${f(r)}" fill="${p.sun}"/>`;
  const side = random() > 0.5;
  body += `<path d="${side ? `M0 ${f(horizon)}L0 ${f(horizon - h * 0.16)}Q${f(w * 0.18)} ${f(horizon - h * 0.2)} ${f(w * 0.32)} ${f(horizon)}Z` : `M${w} ${f(horizon)}L${w} ${f(horizon - h * 0.14)}Q${f(w * 0.8)} ${f(horizon - h * 0.18)} ${f(w * 0.66)} ${f(horizon)}Z`}" fill="${mix(p.far, p.near, 0.6)}"/>`;
  body += `<rect y="${f(horizon)}" width="${w}" height="${f(h - horizon)}" fill="url(#sea)"/>`;
  for (let i = 0; i < 16; i++) {
    const y = horizon + (h - horizon) * (i / 16) + 2;
    const width = r * (2.2 - i * 0.06) * (0.6 + random() * 0.8);
    body += `<rect x="${f(sx - width / 2)}" y="${f(y)}" width="${f(width)}" height="${f(Math.max(1.5, h * 0.004))}" fill="${p.sun}" opacity="${f(0.75 - i * 0.04)}"/>`;
  }
  if (/lighthouse/.test(prompt)) {
    const x = side ? w * 0.12 : w * 0.86;
    body += `<rect x="${f(x)}" y="${f(horizon - h * 0.28)}" width="${f(w * 0.022)}" height="${f(h * 0.12)}" fill="#f4efe6"/><rect x="${f(x - w * 0.004)}" y="${f(horizon - h * 0.3)}" width="${f(w * 0.03)}" height="${f(h * 0.025)}" fill="${p.accent[0]}"/>`;
  }
  if (/boat|sail/.test(prompt)) {
    const x = w * (0.2 + random() * 0.6);
    body += `<path d="M${f(x)} ${f(horizon + h * 0.03)}l${f(w * 0.05)} 0l${f(-w * 0.008)} ${f(h * 0.012)}h${f(-w * 0.034)}z" fill="${p.near}"/><path d="M${f(x + w * 0.024)} ${f(horizon + h * 0.026)}v${f(-h * 0.09)}l${f(w * 0.022)} ${f(h * 0.085)}z" fill="${mix(p.sun, p.far, 0.2)}"/>`;
  }
  return body;
}

function editorial(random: Rand, w: number, h: number): string {
  const p = palettes.editorial!;
  const colors = p.accent;
  const pick = () => colors[Math.floor(random() * colors.length)]!;
  let body = `<rect width="${w}" height="${h}" fill="${p.sky[0]}"/>`;
  const unit = Math.min(w, h) / 6;
  body += `<circle cx="${f(w * (0.35 + random() * 0.3))}" cy="${f(h * (0.3 + random() * 0.2))}" r="${f(unit * (1.4 + random() * 0.6))}" fill="${colors[0]}"/>`;
  for (let i = 0; i < 5; i++) {
    const x = Math.floor(random() * 6) * unit;
    const y = Math.floor(random() * 6) * (h / 6);
    const kind = Math.floor(random() * 4);
    const color = pick();
    if (kind === 0) body += `<rect x="${f(x)}" y="${f(y)}" width="${f(unit * (1 + Math.floor(random() * 2)))}" height="${f(unit)}" fill="${color}"/>`;
    else if (kind === 1) body += `<path d="M${f(x)} ${f(y + unit)}a${f(unit)} ${f(unit)} 0 0 1 ${f(unit * 2)} 0z" fill="${color}"/>`;
    else if (kind === 2) body += `<path d="M${f(x)} ${f(y)}h${f(unit)}a${f(unit)} ${f(unit)} 0 0 1 ${f(-unit)} ${f(unit)}z" fill="${color}"/>`;
    else body += `<rect x="${f(x)}" y="${f(y + unit * 0.45)}" width="${f(unit * 2.4)}" height="${f(unit * 0.1)}" fill="${color}"/>`;
  }
  body += `<rect x="${f(w * 0.08)}" y="${f(h * 0.86)}" width="${f(w * 0.28)}" height="${f(h * 0.012)}" fill="${p.near}"/>`;
  return body;
}

function city(random: Rand, w: number, h: number, p: Palette): string {
  let body = `<defs>${sky(p, "s")}<linearGradient id="water" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#151a38"/><stop offset="1" stop-color="#07091a"/></linearGradient></defs><rect width="${w}" height="${h}" fill="url(#s)"/>`;
  for (let i = 0; i < 40; i++) body += `<circle cx="${f(random() * w)}" cy="${f(random() * h * 0.45)}" r="${f(0.6 + random() * 1.4)}" fill="#fff" opacity="${f(0.3 + random() * 0.6)}"/>`;
  body += `<circle cx="${f(w * (0.6 + random() * 0.3))}" cy="${f(h * 0.18)}" r="${f(Math.min(w, h) * 0.045)}" fill="${p.sun}"/>`;
  const ground = h * 0.72;
  let x = 0;
  while (x < w) {
    const bw = w * (0.05 + random() * 0.07);
    const bh = h * (0.12 + random() * 0.38);
    body += `<rect x="${f(x)}" y="${f(ground - bh)}" width="${f(bw + 1)}" height="${f(bh)}" fill="${mix(p.near, p.far, random() * 0.5)}"/>`;
    for (let wy = ground - bh + 8; wy < ground - 6; wy += h * 0.022) {
      for (let wx = x + 5; wx < x + bw - 5; wx += w * 0.016) {
        if (random() > 0.62) body += `<rect x="${f(wx)}" y="${f(wy)}" width="${f(w * 0.006)}" height="${f(h * 0.008)}" fill="${p.accent[Math.floor(random() * p.accent.length)]}" opacity=".85"/>`;
      }
    }
    x += bw;
  }
  body += `<rect y="${f(ground)}" width="${w}" height="${f(h - ground)}" fill="url(#water)"/>`;
  for (let i = 0; i < 26; i++) {
    const rx = random() * w;
    body += `<rect x="${f(rx)}" y="${f(ground + random() * (h - ground))}" width="${f(w * (0.01 + random() * 0.04))}" height="${f(h * 0.004)}" fill="${p.accent[Math.floor(random() * p.accent.length)]}" opacity=".5"/>`;
  }
  return body;
}

function stillLife(random: Rand, w: number, h: number, p: Palette, prompt: string): string {
  const wall = /dark|chiaroscuro|night/.test(prompt) ? ["#2a2522", "#1b1816"] : [mix(p.sky[0], "#ffffff", 0.3), p.sky[1]];
  const floorY = h * (0.66 + random() * 0.06);
  const cx = w * (0.42 + random() * 0.16);
  const bw = w * (0.13 + random() * 0.06);
  const bh = h * (0.32 + random() * 0.12);
  const top = floorY - bh;
  const color = /terracotta|teapot|clay/.test(prompt) ? "#b45a37" : /glass|perfume/.test(prompt) ? "#d9c7a8" : p.accent[0]!;
  let body = `<defs><linearGradient id="wall" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${wall[0]}"/><stop offset="1" stop-color="${wall[1]}"/></linearGradient><linearGradient id="body" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="${mix(color, "#ffffff", 0.35)}"/><stop offset=".45" stop-color="${color}"/><stop offset="1" stop-color="${mix(color, "#000000", 0.45)}"/></linearGradient><filter id="blur"><feGaussianBlur stdDeviation="${f(w * 0.012)}"/></filter></defs>`;
  body += `<rect width="${w}" height="${h}" fill="url(#wall)"/><rect y="${f(floorY)}" width="${w}" height="${f(h - floorY)}" fill="${mix(wall[1]!, "#000000", 0.12)}"/>`;
  body += `<path d="M${f(w * 0.62)} 0L${f(w)} 0L${f(w * 0.78)} ${f(h)}L${f(w * 0.38)} ${f(h)}Z" fill="#fff" opacity=".07"/>`;
  body += `<ellipse cx="${f(cx + bw * 0.4)}" cy="${f(floorY + h * 0.01)}" rx="${f(bw * 1.1)}" ry="${f(h * 0.025)}" fill="#000" opacity=".3" filter="url(#blur)"/>`;
  const neck = bw * (0.28 + random() * 0.12);
  body += `<path d="M${f(cx - neck / 2)} ${f(top)}C${f(cx - neck / 2)} ${f(top + bh * 0.2)} ${f(cx - bw)} ${f(top + bh * 0.35)} ${f(cx - bw)} ${f(top + bh * 0.68)}C${f(cx - bw)} ${f(floorY - bh * 0.05)} ${f(cx - bw * 0.6)} ${f(floorY)} ${f(cx)} ${f(floorY)}C${f(cx + bw * 0.6)} ${f(floorY)} ${f(cx + bw)} ${f(floorY - bh * 0.05)} ${f(cx + bw)} ${f(top + bh * 0.68)}C${f(cx + bw)} ${f(top + bh * 0.35)} ${f(cx + neck / 2)} ${f(top + bh * 0.2)} ${f(cx + neck / 2)} ${f(top)}Z" fill="url(#body)"/>`;
  body += `<ellipse cx="${f(cx)}" cy="${f(top)}" rx="${f(neck / 2)}" ry="${f(neck * 0.12)}" fill="${mix(color, "#000000", 0.5)}"/>`;
  return body;
}

function dunes(random: Rand, w: number, h: number, p: Palette): string {
  let body = `<defs>${sky(p, "s")}${glow(p, "g")}</defs><rect width="${w}" height="${h}" fill="url(#s)"/>`;
  const sx = w * (0.2 + random() * 0.6);
  body += `<circle cx="${f(sx)}" cy="${f(h * 0.32)}" r="${f(Math.min(w, h) * 0.3)}" fill="url(#g)"/><circle cx="${f(sx)}" cy="${f(h * 0.32)}" r="${f(Math.min(w, h) * 0.06)}" fill="${p.sun}"/>`;
  for (let i = 0; i < 5; i++) {
    const base = h * (0.5 + i * 0.1);
    const peak = w * (0.15 + random() * 0.7);
    const t = i / 4;
    body += `<path d="M0 ${f(base + h * 0.06)}C${f(peak * 0.5)} ${f(base + h * 0.02)} ${f(peak * 0.8)} ${f(base - h * 0.08)} ${f(peak)} ${f(base - h * 0.07)}C${f(peak + w * 0.2)} ${f(base - h * 0.03)} ${f(w * 0.85)} ${f(base + h * 0.07)} ${w} ${f(base + h * 0.04)}V${h}H0Z" fill="${mix(mix(p.far, p.sky[2], 0.3), p.near, t * 0.85)}"/>`;
  }
  return body;
}

/** An SVG document sized like the asset it stands in for. */
export function artSvg(seed: number, width: number, height: number, prompt: string): string {
  const random = rng(seed);
  const w = 1000;
  const h = Math.round((1000 * height) / width);
  const text = prompt.toLowerCase();
  const palette = pickPalette(text, random);
  let body: string;
  if (/poster|geometric|abstract|swiss|riso|arches/.test(text)) body = editorial(random, w, h);
  else if (/vase|teapot|perfume|bottle|product|still life|ceramic/.test(text)) body = stillLife(random, w, h, palette, text);
  else if (/city|neon|street|skyline|alley/.test(text)) body = city(random, w, h, palettes.night!);
  else if (/desert|dune/.test(text)) body = dunes(random, w, h, palette);
  else if (/sea|coast|ocean|beach|harbor|lighthouse|lake|fjord|boat|cliff/.test(text)) body = coast(random, w, h, palette, text);
  else body = landscape(random, w, h, palette, text);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${w} ${h}" preserveAspectRatio="xMidYMid slice">${body}</svg>`;
}
