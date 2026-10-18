// SPDX-License-Identifier: GPL-3.0-or-later
// rain.frag — a wallpaper seen through a rainy window.
//
// Written from scratch for rainglass. The model, per pixel:
//   * fogged glass shows the frosted copy of the wallpaper;
//   * a drop is a small lens: it shows the sharp wallpaper, flipped and
//     shrunk the way a real drop inverts what is behind it;
//   * a drop running down the glass wipes a clear trail through the fog and
//     leaves a line of small droplets behind it.
// Resting droplets come from two grids at different sizes, running drops from
// two column grids at different speeds; keystrokes (u_impact) add splashes.
//
// Coordinates: uv is 0..1 with y DOWN (top-left origin, like the wallpaper);
// p is uv with x stretched by the aspect ratio, so one unit = the screen
// height and shapes stay round.
//
// The hashes are Dave Hoskins' "Hash without Sine" (MIT licence).

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

uniform vec2 u_res;          // buffer size, px
uniform float u_time;        // rain clock, seconds (slows while checking a password)
uniform sampler2D u_sharp;   // the wallpaper, cover-fitted
uniform sampler2D u_frost;   // the same, frosted (quarter resolution, blurred)
uniform float u_fog;         // 0 = clear glass, 1 = fully fogged between drops
uniform float u_density;     // 0..1: how many cells hold a drop
uniform float u_dim;         // overall brightness (text sits on top)
uniform float u_vignette;
uniform vec4 u_tint;         // rgb + amount: the wrong-password flash
uniform vec4 u_impact[8];    // keystroke drops: x, y (0..1, y down), landed, wiped (<0: not yet)
                             // landed < 0: an empty slot
uniform sampler2D u_text;    // clock and prompts, straight-alpha RGBA
uniform vec4 u_text_rect;    // where that band sits: x, y, w, h in buffer px, y down
uniform float u_has_text;

float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec3 hash32(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yxz + 33.33);
    return fract((p3.xxy + p3.yzz) * p3.zyx);
}

// Fine, stationary condensation between the larger drops. It is evaluated
// after the running drops so their clear trails can wipe it away as well.
float condensation(vec2 p) {
    vec2 q = p * 110.0;
    vec2 id = floor(q);
    vec3 h = hash32(id + vec2(29.0, 11.0));
    if (h.z > u_density * 0.18) return 0.0;
    vec2 d = fract(q) - 0.5 - (h.xy - 0.5) * 0.45;
    float r = mix(0.10, 0.19, h.x);
    return 1.0 - smoothstep(r * 0.5, r, length(d));
}

// What the glass does at one pixel, summed over every layer.
struct Glass {
    vec2 off;    // where to look instead (p units)
    float drop;  // coverage by water: shows the sharp image
    float clear; // wiped clear of fog (trails)
    vec2 n;      // drop surface slope, for the rim shading
};

// A drop of radius r centred at offset d from the pixel: lens + coverage.
// `units` converts the local lengths to p units.
void lens(vec2 d, float r, float a, float units, inout Glass g) {
    float dist = length(d);
    float m = (1.0 - smoothstep(r * 0.78, r, dist)) * a;
    // Sample from the far side of the drop's centre, three times as far out:
    // the inverted, shrunk view through a water lens.
    g.off -= d * 3.0 * units * m;
    g.drop += m;
    g.n += d / r * m;
}

// Resting droplets: at most one per cell, each gathering, sitting and
// evaporating on its own clock.
void dots(vec2 p, float cells, float t, float seed, inout Glass g) {
    vec2 q = p * cells;
    vec2 id = floor(q);
    vec2 f = fract(q) - 0.5;
    vec3 h = hash32(id + seed * 17.0);
    if (h.y > u_density) return;
    float r = mix(0.10, 0.24, h.z * h.z);
    vec2 c = (fract(h.xy * 7.31) - 0.5) * 2.0 * (0.48 - r);
    float life = fract(t * mix(0.02, 0.06, h.x) + h.z * 5.0);
    float a = smoothstep(0.0, 0.1, life) * (1.0 - smoothstep(0.75, 1.0, life));
    lens(f - c, r * mix(0.55, 1.0, a), a, 1.0 / cells, g);
}

// Drops running down the glass. Rows of `cells` per screen height, each row
// split into columns a third as wide; a cell holds one drop that slides down
// it in stick-slip bursts, wiping a trail and shedding droplets behind it.
void runs(vec2 p, float cells, float t, float seed, inout Glass g) {
    const float COLS = 3.0;
    // Each column's cells start at their own height, so drops never line up
    // in rows across the glass.
    float qx = p.x * cells * COLS;
    vec2 q = vec2(qx, p.y * cells + hash12(vec2(floor(qx), seed * 7.0)));
    vec2 id = floor(q);
    vec3 h = hash32(id + seed * 31.0);
    if (h.x > u_density * 0.8) return;
    vec2 u = fract(q);
    float lx = (u.x - 0.5) / COLS;   // row units, centred on the column
    float ly = u.y;                   // 0 at the top of the cell

    // One run down the cell per cycle, in three bursts with pauses between.
    float cyc = fract(t * mix(0.05, 0.12, h.y) + h.z);
    float s = cyc * 3.0;
    float yh = 0.06 + 0.86 * (floor(s) + smoothstep(0.35, 1.0, fract(s))) / 3.0;
    float a = smoothstep(0.0, 0.04, cyc) * (1.0 - smoothstep(0.92, 1.0, cyc));

    // The path wanders a little from side to side: one slow swing, plus a
    // small fast jitter where the drop catches on the glass.
    float x0 = (h.y - 0.5) * 0.2 / COLS;
    float wob = 0.06 / COLS;
    float ph = h.z * 6.283;
    float xh = x0 + wob * (sin(yh * 4.0 + ph) + 0.15 * sin(yh * 37.0 + ph * 3.0));
    float xl = x0 + wob * (sin(ly * 4.0 + ph) + 0.15 * sin(ly * 37.0 + ph * 3.0));
    float rh = mix(0.05, 0.075, h.y);

    // The head: a little flatter on top, where it drags.
    vec2 d = vec2(lx - xh, (ly - yh) * (ly < yh ? 1.35 : 1.0));
    lens(d, rh, a, 1.0 / cells, g);

    // The trail above it: clear glass, narrowing and fogging back over.
    float above = yh - ly;
    if (above > 0.0) {
        float age = above / 0.8;
        float tw = rh * 0.5 * (1.0 - 0.7 * age);
        float trail = (1.0 - smoothstep(tw * 0.2, tw * 1.1, abs(lx - xl))) * (1.0 - smoothstep(0.0, 1.0, age));
        g.clear += trail * a * 0.85;

        // Droplets left in the trail, smaller the older they are.
        float j = floor(ly * 9.0);
        float cy = (j + 0.5) / 9.0;
        float hj = hash12(vec2(j, id.x * 3.7 + seed));
        if (cy < yh - 0.05 && hj < 0.75) {
            float cx = x0 + wob * (sin(cy * 4.0 + ph) + 0.15 * sin(cy * 37.0 + ph * 3.0)) + (hj - 0.4) * 0.02;
            float rd = rh * mix(0.25, 0.5, hj) * (1.0 - 0.6 * age);
            lens(vec2(lx - cx, ly - cy), rd, a * (1.0 - age), 1.0 / cells, g);
        }
    }
}

// Keystrokes: a drop lands, with a brief ring as it hits. Backspace wipes
// the newest one away (its fourth component: when).
void impacts(vec2 p, float aspect, inout Glass g) {
    for (int i = 0; i < 8; i++) {
        vec4 k = u_impact[i];
        float age = u_time - k.z;
        if (k.z < 0.0 || age < 0.0) continue;
        float grow = smoothstep(0.0, 0.12, age);
        // Gone within two seconds: a glance at the glass later mustn't count
        // the keystrokes (the password's length) from the drops left behind.
        float a = 1.0 - smoothstep(1.0, 2.0, age);
        if (k.w >= 0.0) a *= 1.0 - smoothstep(0.0, 0.25, u_time - k.w);
        vec2 c = vec2(k.x * aspect, k.y);
        float r = mix(0.012, 0.022, hash12(c * 91.0)) * grow;
        vec2 d = p - c;
        if (r > 0.0) lens(d, r, a, 1.0, g);
        if (age < 0.35 && a > 0.0) {
            float ring = 1.0 - smoothstep(0.0, 0.006, abs(length(d) - age * 0.25));
            float fade = 1.0 - age / 0.35;
            g.clear += ring * fade * 0.6;
            g.off += normalize(d + 1e-5) * ring * fade * 0.004;
        }
    }
}

void main() {
    vec2 uv = vec2(gl_FragCoord.x, u_res.y - gl_FragCoord.y) / u_res;
    float aspect = u_res.x / u_res.y;
    vec2 p = vec2(uv.x * aspect, uv.y);

    Glass g;
    g.off = vec2(0.0);
    g.drop = 0.0;
    g.clear = 0.0;
    g.n = vec2(0.0);
    dots(p, 34.0, u_time, 1.0, g);
    dots(p, 15.0, u_time, 2.0, g);
    runs(p, 2.3, u_time, 3.0, g);
    runs(p, 3.7, u_time * 1.15, 4.0, g);
    impacts(p, aspect, g);

    vec2 off = vec2(g.off.x / aspect, g.off.y);
    float drop = clamp(g.drop, 0.0, 1.0);
    float clear = clamp(max(drop, g.clear), 0.0, 1.0);
    float micro = condensation(p) * (1.0 - smoothstep(0.15, 0.75, clear)) * clamp(u_fog, 0.0, 1.0);
    float fog = clamp(u_fog * (1.0 - clear) - micro * 0.22, 0.0, 1.0);

    vec3 sharp = texture2D(u_sharp, uv + off).rgb;
    vec3 frost = texture2D(u_frost, uv + off * 0.25).rgb;
    // Fogged glass scatters light: a touch brighter and greyer than behind it.
    float lum = dot(frost, vec3(0.299, 0.587, 0.114));
    vec3 glass = mix(frost, vec3(lum), 0.2) * 0.9 + 0.035;
    vec3 col = mix(sharp, glass, fog);
    col += micro * 0.035;

    // Water shading: darker towards each drop's rim, a highlight from the top left.
    float slope = clamp(length(g.n), 0.0, 1.0);
    col *= 1.0 - 0.4 * drop * smoothstep(0.55, 1.0, slope);
    float spec = pow(max(dot(normalize(g.n + 1e-5), normalize(vec2(-0.55, -0.8))), 0.0), 8.0);
    col += spec * drop * smoothstep(0.3, 0.8, slope) * 0.22;

    vec2 c = uv - 0.5;
    col *= u_dim * (1.0 - u_vignette * dot(c, c) * 1.6);
    col = mix(col, u_tint.rgb, u_tint.a);

    // Text sits on the glass, not behind it: no refraction.
    if (u_has_text > 0.5) {
        vec2 tp = (vec2(gl_FragCoord.x, u_res.y - gl_FragCoord.y) - u_text_rect.xy) / u_text_rect.zw;
        if (tp.x >= 0.0 && tp.y >= 0.0 && tp.x <= 1.0 && tp.y <= 1.0) {
            vec4 t = texture2D(u_text, tp);
            col = mix(col, t.rgb, t.a);
        }
    }
    gl_FragColor = vec4(col, 1.0);
}
