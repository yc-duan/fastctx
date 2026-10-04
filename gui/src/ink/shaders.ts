export const vertex = `#version 300 es
precision highp float;
precision highp int;
layout(location=0) in vec2 a_position;
layout(location=1) in vec2 a_normal;
layout(location=2) in vec2 a_path;
uniform vec2 u_resolution;
uniform vec3 u_transform;
uniform vec4 u_brush;
uniform vec4 u_motion;
uniform vec4 u_path;
uniform uint u_seed;
uniform float u_time;
uniform float u_pixel;
out float v_distance;
out float v_side;
out float v_half;
out float v_ink;
uint mixBits(uint x) {
  x ^= x >> 16; x *= 0x7feb352du; x ^= x >> 15; x *= 0x846ca68bu; return x ^ (x >> 16);
}
float random(uint x) { return float(mixBits(x) >> 8) / 16777216.0; }
void main() {
  uint identity = u_seed + uint(gl_InstanceID) * 747796405u;
  float r = random(identity) * 2.0 - 1.0;
  float phase = random(identity + 1u) * 6.283185307;
  float phase2 = random(identity + 2u) * 6.283185307;
  float s = a_path.x;
  float period = u_path.x;
  // Integer spatial periods join exactly at the seam of a closed contour.
  float f = u_path.y > 0.5 ? 6.283185307 * max(1.0, round(period / 240.0)) / period : 0.026;
  float detail = u_path.y > 0.5 ? 6.283185307 * max(1.0, round(period / 53.0)) / period : 0.119;
  float displacement = sign(r) * pow(abs(r), u_brush.z) * u_brush.y * 0.5;
  displacement += u_motion.x * (0.7 * sin(s*f + phase) + 0.3 * sin(s*detail + phase2));
  float t = u_time * u_motion.z * 6.283185307;
  displacement += u_motion.y * (0.65*sin(s*f + phase + t) + 0.35*sin(s*f*2.0 + phase2 - t*0.61));
  float halfWidth = u_brush.x * 0.5 * (0.8 + 0.2*sin(s*detail + phase));
  float outer = halfWidth + u_pixel;
  vec2 p = a_position + a_normal * (displacement + a_path.y * outer);
  vec2 screen = p * u_transform.z + u_transform.xy;
  gl_Position = vec4(screen / u_resolution * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0);
  v_distance = s;
  v_side = a_path.y * outer;
  v_half = halfWidth;
  v_ink = (0.62 + 0.38*random(identity+3u)) * (0.8 + 0.2*sin(s*detail + phase2));
}`;

export const fragment = `#version 300 es
precision highp float;
uniform vec4 u_color;
uniform vec4 u_dash;
uniform vec4 u_path;
uniform float u_pixel;
in float v_distance;
in float v_side;
in float v_half;
in float v_ink;
out vec4 outColor;
void main() {
  float coverage = clamp((v_half - abs(v_side)) / u_pixel + 0.5, 0.0, 1.0);
  if (u_dash.x > 0.0 && u_dash.y > 0.0) {
    float cycle = u_dash.x + u_dash.y;
    float p = mod(v_distance - u_dash.z + u_dash.x*0.5, cycle) - u_dash.x*0.5;
    float edge = abs(p) - u_dash.x*0.5;
    coverage *= 1.0 - smoothstep(-u_pixel*0.5, u_pixel*0.5, edge);
  }
  coverage *= 1.0 - smoothstep(u_path.z-u_pixel*0.5, u_path.z+u_pixel*0.5, v_distance);
  float alpha = u_color.a * v_ink * coverage * u_path.w;
  outColor = vec4(u_color.rgb * alpha, alpha);
}`;
