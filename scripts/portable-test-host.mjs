const REQUIRED_EXPORTS = [
  'memory',
  'rh_abi_version',
  'rh_alloc',
  'rh_dealloc',
  'rh_init',
  'rh_dispatch',
  'rh_output_ptr',
  'rh_output_len'
]

const MAX_MEMORY_BYTES = 64 * 1024 * 1024
const MAX_JSON_BYTES = 2 * 1024 * 1024
const MAX_SCENE_BYTES = 2 * 1024 * 1024
const MAX_DRAW_OPS = 4096
const MAX_PATH_POINTS = 8192
const MAX_TEXT_CHARS = 131072
const MAX_SURFACE_SIZE = 8192

function finite(value) {
  const number = Number(value)
  return Number.isFinite(number) ? number : 0
}

export function validateScene(scene) {
  if (!scene || typeof scene !== 'object') throw new Error('missing portable scene')
  if (JSON.stringify(scene).length > MAX_SCENE_BYTES) {
    throw new Error('portable scene exceeds 2 MB')
  }
  const width = finite(scene.width)
  const height = finite(scene.height)
  if (width < 1 || height < 1 || width > MAX_SURFACE_SIZE || height > MAX_SURFACE_SIZE) {
    throw new Error('invalid portable scene dimensions')
  }
  if (!Array.isArray(scene.draw) || scene.draw.length > MAX_DRAW_OPS) {
    throw new Error('invalid portable draw list')
  }
  let pathPoints = 0
  let textChars = 0
  for (const item of scene.draw) {
    if (!item || typeof item !== 'object' || typeof item.op !== 'string') {
      throw new Error('invalid portable draw operation')
    }
    if (item.op === 'line') {
      if (!Array.isArray(item.points)) throw new Error('line points are required')
      pathPoints += item.points.length
    }
    if (item.op === 'text') textChars += String(item.text || '').length
  }
  if (pathPoints > MAX_PATH_POINTS) throw new Error('portable paths exceed point limit')
  if (textChars > MAX_TEXT_CHARS) throw new Error('portable text exceeds character limit')
  return scene
}

function validatePortableApi(api) {
  for (const name of REQUIRED_EXPORTS) {
    if (!(name in (api || {}))) throw new Error(`missing portable ABI export: ${name}`)
  }
  if (!(api.memory instanceof WebAssembly.Memory)) {
    throw new Error('portable ABI memory is not exported')
  }
  if (Number(api.rh_abi_version()) !== 3) {
    throw new Error('unsupported portable ABI version')
  }
  if (api.memory.buffer.byteLength > MAX_MEMORY_BYTES) {
    throw new Error('portable WASM memory limit exceeded')
  }
  return api
}

export class PortableWasmRuntime {
  constructor() {
    this.api = null
    this.encoder = new TextEncoder()
    this.decoder = new TextDecoder()
  }

  checkMemory() {
    if (!this.api || this.api.memory.buffer.byteLength > MAX_MEMORY_BYTES) {
      throw new Error('portable WASM memory limit exceeded')
    }
  }

  callJson(name, value) {
    const bytes = this.encoder.encode(JSON.stringify(value ?? {}))
    if (!bytes.byteLength || bytes.byteLength > MAX_JSON_BYTES) {
      throw new Error('portable WASM JSON input is invalid')
    }
    const pointer = Number(this.api.rh_alloc(bytes.byteLength))
    this.checkMemory()
    if (pointer <= 0 || pointer + bytes.byteLength > this.api.memory.buffer.byteLength) {
      throw new Error('invalid portable WASM allocation')
    }
    new Uint8Array(this.api.memory.buffer, pointer, bytes.byteLength).set(bytes)
    try {
      this.api[name](pointer, bytes.byteLength)
    } finally {
      this.api.rh_dealloc(pointer, bytes.byteLength)
    }
    this.checkMemory()
    const outputPointer = Number(this.api.rh_output_ptr())
    const outputLength = Number(this.api.rh_output_len())
    if (
      outputLength < 0 ||
      outputLength > MAX_JSON_BYTES ||
      outputPointer < 0 ||
      outputPointer + outputLength > this.api.memory.buffer.byteLength
    ) {
      throw new Error('invalid portable WASM JSON output')
    }
    if (!outputLength) return {}
    return JSON.parse(
      this.decoder.decode(
        new Uint8Array(this.api.memory.buffer, outputPointer, outputLength)
      )
    )
  }

  async load(bytes, context) {
    const module = await WebAssembly.compile(bytes)
    if (WebAssembly.Module.imports(module).length) {
      throw new Error('portable WASM imports are forbidden')
    }
    const instance = await WebAssembly.instantiate(module, {})
    this.api = validatePortableApi(instance.exports)
    return this.callJson('rh_init', context || {})
  }

  dispatch(message) {
    if (!this.api) throw new Error('portable WASM is not initialized')
    return this.callJson('rh_dispatch', message)
  }
}
