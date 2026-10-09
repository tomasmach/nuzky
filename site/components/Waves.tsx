"use client";

import { useEffect, useRef } from "react";

// Slow silver swells behind a section. The same shader as nuzky-silk.glsl in the design file.
// It draws at reduced resolution (the light is soft, so nobody sees the difference), stops while
// the section is off screen or the tab is hidden, and holds one still frame with reduced motion.
//
// With `swellAt` the light spans several sections as one surface: the swells sit behind the element
// the selector finds, an optional soft glow sits behind `glowAt`, and the light fades out before the
// bottom edge, so nothing ends in a seam.

const vertex = `attribute vec2 a; void main() { gl_Position = vec4(a, 0.0, 1.0); }`;

const fragment = `
precision highp float;
uniform vec2 u_resolution;
uniform float u_time;
uniform float u_intensity;
uniform float u_base;
uniform float u_unit;
uniform float u_glow;
uniform float u_span;
const vec3 BASE = vec3(0.0314, 0.0314, 0.0392);
const vec3 COOL = vec3(0.788, 0.8, 0.827);
const vec3 DEEP = vec3(0.478, 0.49, 0.525);
const vec3 HIGHLIGHT = vec3(1.0);

float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
// pow() is undefined for a negative base, and some GPUs return NaN for it.
float sq(float x) { return x * x; }

void main() {
  vec2 uv = gl_FragCoord.xy / u_resolution;
  vec2 p = vec2(gl_FragCoord.x - 0.5 * u_resolution.x, gl_FragCoord.y - u_base) / u_unit;
  float t = u_time * 0.12;
  vec3 light = vec3(0.0);

  float wide = exp(-sq(p.y / 0.45)) * exp(-sq(p.x / 1.3));
  light += DEEP * wide * 0.07 * u_intensity;

  for (int i = 0; i < 3; i++) {
    float fi = float(i);
    float y = 0.07 * (fi - 1.0)
      + 0.06 * sin(p.x * (1.3 + 0.35 * fi) + t * (0.9 + 0.25 * fi) + fi * 2.1)
      + 0.025 * sin(p.x * (2.7 - 0.4 * fi) - t * (0.6 + 0.2 * fi) + fi * 0.7);
    float d = p.y - y;
    float below = smoothstep(0.0, -0.22, d) * exp(d / 0.18);
    float crest = exp(-sq(d / 0.03));
    float fade = exp(-sq(p.x / (0.85 + 0.2 * fi)));
    vec3 tone = mix(DEEP, COOL, 0.35 + 0.3 * fi);
    float s = (1.0 - 0.25 * fi) * fade * u_intensity;
    light += tone * below * 0.07 * s;
    light += mix(tone, HIGHLIGHT, 0.5) * crest * 0.09 * s;
  }

  vec3 col;
  if (u_span > 0.5) {
    // One surface over several sections: a broad glow behind the second anchor, a falloff into the
    // page colour at the sides, and a long fade before the bottom edge so the next section starts clean.
    float g = (gl_FragCoord.y - u_glow) / u_unit;
    light += mix(DEEP, COOL, 0.3) * exp(-sq(g / 0.55)) * exp(-sq(p.x / 1.1)) * 0.085 * u_intensity;
    light *= smoothstep(0.0, 0.55 * u_unit, gl_FragCoord.y);
    light *= mix(0.35, 1.0, exp(-sq(p.x / 1.5)));
    col = BASE + light;
  } else {
    col = BASE + light;
    float vig = smoothstep(1.25, 0.25, length((uv - vec2(0.5, 0.45)) * vec2(1.0, 1.3)));
    col *= mix(0.55, 1.0, vig);
  }
  // A trace of grain so the soft gradients never band.
  col += (hash(gl_FragCoord.xy) - 0.5) * 0.008;
  gl_FragColor = vec4(col, 1.0);
}`;

type Props = {
  /** Where the swells sit, 0 at the bottom of the section and 1 at the top. Ignored with `swellAt`. */
  height?: number;
  intensity?: number;
  /** Selector, within the parent, of the element the swells sit behind. */
  swellAt?: string;
  /** Selector, within the parent, of the element a soft glow sits behind. */
  glowAt?: string;
  className?: string;
};

// The span mode measures in CSS pixels so the swells keep the hero's proportions on a tall surface.
const SPAN_UNIT = 1000;

export function Waves({ height = 0.4, intensity = 1, swellAt, glowAt, className = "" }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const gl = canvas.getContext("webgl", { antialias: false, alpha: false, powerPreference: "low-power" });
    if (!gl) return;

    const compile = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      return s;
    };
    const program = gl.createProgram()!;
    gl.attachShader(program, compile(gl.VERTEX_SHADER, vertex));
    gl.attachShader(program, compile(gl.FRAGMENT_SHADER, fragment));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return;
    gl.useProgram(program);

    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(program, "a");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    const u = (name: string) => gl.getUniformLocation(program, name);
    const uRes = u("u_resolution");
    const uTime = u("u_time");
    const uBase = u("u_base");
    const uUnit = u("u_unit");
    const uGlow = u("u_glow");
    gl.uniform1f(u("u_intensity"), intensity);
    gl.uniform1f(u("u_span"), swellAt ? 1 : 0);

    const scale = Math.min(window.devicePixelRatio || 1, 2) * 0.5;
    const root = canvas.parentElement?.parentElement;
    const anchors = [swellAt, glowAt].map((s) => (s && root ? root.querySelector<HTMLElement>(s) : null));

    // GL counts y from the bottom; anchors are measured from the top of the page.
    const anchorY = (el: HTMLElement | null, rect: DOMRect, fallback: number) => {
      if (!el) return fallback;
      const r = el.getBoundingClientRect();
      return (rect.bottom - (r.top + r.height / 2)) * scale;
    };
    const measure = () => {
      const w = Math.max(1, Math.round(canvas.clientWidth * scale));
      const h = Math.max(1, Math.round(canvas.clientHeight * scale));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
      }
      // Always, not only on a resize: a remount (React runs effects twice in development) reuses the
      // sized canvas with a new program whose resolution would otherwise stay 0 and pin the light to the left edge.
      gl.viewport(0, 0, w, h);
      gl.uniform2f(uRes, w, h);
      if (swellAt) {
        const rect = canvas.getBoundingClientRect();
        gl.uniform1f(uBase, anchorY(anchors[0], rect, h * height));
        gl.uniform1f(uGlow, anchorY(anchors[1], rect, -1e5));
        gl.uniform1f(uUnit, SPAN_UNIT * scale);
      } else {
        gl.uniform1f(uBase, h * height);
        gl.uniform1f(uUnit, h);
      }
    };

    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    let visible = false;
    let frame = 0;
    // Start a little into the motion so the first frame already has shape.
    const start = performance.now() - 8000;

    const draw = (now: number) => {
      gl.uniform1f(uTime, (now - start) / 1000);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    const loop = (now: number) => {
      draw(now);
      frame = visible && !still.matches && !document.hidden ? requestAnimationFrame(loop) : 0;
    };
    const kick = () => {
      if (!frame && visible) frame = requestAnimationFrame(loop);
    };

    const io = new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
      kick();
    });
    io.observe(canvas);
    // Anchors only move when something resizes, so the layout is read here and never per frame.
    const ro = new ResizeObserver(() => {
      measure();
      draw(performance.now());
    });
    ro.observe(canvas);
    for (const a of anchors) if (a) ro.observe(a);
    measure();
    document.addEventListener("visibilitychange", kick);
    still.addEventListener("change", kick);
    canvas.dataset.ready = "true";

    return () => {
      cancelAnimationFrame(frame);
      io.disconnect();
      ro.disconnect();
      document.removeEventListener("visibilitychange", kick);
      still.removeEventListener("change", kick);
      // The context is left to the garbage collector: losing it here would hand a dead context to
      // the next mount of the same canvas (React runs effects twice in development).
      delete canvas.dataset.ready;
    };
  }, [height, intensity, swellAt, glowAt]);

  // Without WebGL the canvas stays empty and the radial light underneath shows instead.
  return (
    <div aria-hidden className={`pointer-events-none absolute inset-0 overflow-hidden ${className}`}>
      <div
        className="absolute inset-0"
        style={{ background: `radial-gradient(70% 28% at 50% ${100 - height * 100}%, rgb(201 204 211 / 0.13), transparent 70%), var(--color-site)` }}
      />
      <canvas ref={ref} className="absolute inset-0 size-full opacity-0 transition-opacity duration-700 data-[ready=true]:opacity-100" />
    </div>
  );
}
