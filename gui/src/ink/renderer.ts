import { vertex, fragment } from './shaders';
import type { Mesh } from './path';
import type { InkStyle } from './types';

export interface Geometry { vao: WebGLVertexArrayObject; buffer: WebGLBuffer; count: number; bytes: number; }
export interface DrawStroke {
  geometry: Geometry; length: number; closed: boolean; seed: number;
  style: InkStyle; color: readonly number[]; reveal: number; alpha: number;
}

export class Renderer {
  readonly gl: WebGL2RenderingContext;
  private program: WebGLProgram;
  private uniforms: Record<string, WebGLUniformLocation | null> = {};

  constructor(canvas: HTMLCanvasElement) {
    const gl = canvas.getContext('webgl2', { alpha: true, premultipliedAlpha: true, antialias: false, depth: false, stencil: false, preserveDrawingBuffer: false });
    if (!gl) throw new Error('Ink requires WebGL2; no substitute renderer is installed');
    this.gl = gl;
    const shaders = [vertex, fragment].map((source, i) => {
      const shader = gl.createShader(i === 0 ? gl.VERTEX_SHADER : gl.FRAGMENT_SHADER)!;
      gl.shaderSource(shader, source); gl.compileShader(shader); return shader;
    });
    this.program = gl.createProgram()!;
    shaders.forEach(shader => gl.attachShader(this.program, shader));
    gl.linkProgram(this.program);
    const linked = gl.getProgramParameter(this.program, gl.LINK_STATUS);
    const errors = linked ? '' : [gl.getProgramInfoLog(this.program), ...shaders.map(s => gl.getShaderInfoLog(s))].join('\n');
    shaders.forEach(shader => gl.deleteShader(shader));
    if (!linked) { gl.deleteProgram(this.program); throw new Error(`Ink shader compilation failed: ${errors}`); }
    for (const name of ['resolution', 'transform', 'brush', 'motion', 'path', 'seed', 'time', 'pixel', 'color', 'dash'])
      this.uniforms[name] = gl.getUniformLocation(this.program, `u_${name}`);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  }

  upload(mesh: Mesh): Geometry {
    const gl = this.gl;
    const buffer = gl.createBuffer()!, vao = gl.createVertexArray()!;
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, mesh.vertices, gl.STATIC_DRAW);
    for (let i = 0; i < 3; i++) {
      gl.enableVertexAttribArray(i);
      gl.vertexAttribPointer(i, 2, gl.FLOAT, false, 24, i * 8);
    }
    return { vao, buffer, count: mesh.points * 2, bytes: mesh.vertices.byteLength };
  }

  release(geometry: Geometry): void {
    this.gl.deleteVertexArray(geometry.vao); this.gl.deleteBuffer(geometry.buffer);
  }

  begin(width: number, height: number, cssWidth: number, cssHeight: number, box: readonly [number, number] | undefined, time: number): void {
    const gl = this.gl, u = this.uniforms;
    gl.viewport(0, 0, width, height);
    gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT);
    gl.useProgram(this.program);
    const scale = box ? Math.min(cssWidth / box[0], cssHeight / box[1]) : 1;
    gl.uniform2f(u.resolution, cssWidth, cssHeight);
    gl.uniform3f(u.transform, box ? (cssWidth - box[0] * scale) / 2 : 0, box ? (cssHeight - box[1] * scale) / 2 : 0, scale);
    gl.uniform1f(u.pixel, 1 / (width / cssWidth * scale));
    gl.uniform1f(u.time, time);
  }

  draw(stroke: DrawStroke, time: number): void {
    const gl = this.gl, u = this.uniforms, s = stroke.style, c = stroke.color;
    gl.bindVertexArray(stroke.geometry.vao);
    gl.uniform1ui(u.seed, stroke.seed);
    gl.uniform4f(u.brush, s.width, s.spread, s.density, 0);
    gl.uniform4f(u.motion, s.roughness, s.jitter, s.jitterSpeed, 0);
    gl.uniform4f(u.path, stroke.length, stroke.closed ? 1 : 0, stroke.reveal * stroke.length, stroke.alpha);
    gl.uniform4f(u.color, c[0], c[1], c[2], c[3] * s.opacity);
    const dash = s.dash;
    // Reduce only the dash phase modulo its known period, preserving precision over long sessions.
    const phase = dash ? ((time * dash.speed + (dash.offset ?? 0)) % (dash.length + dash.gap)) : 0;
    gl.uniform4f(u.dash, dash?.length ?? 0, dash?.gap ?? 0, phase, 0);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, stroke.geometry.count, s.fibers);
  }

  dispose(): void { this.gl.deleteProgram(this.program); }
}
