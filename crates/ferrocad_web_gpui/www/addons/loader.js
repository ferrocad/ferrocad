// FerroCAD · the wasm bundle loader.
//
// A wasm bundle is an ordinary `WebAssembly` module with two conventions:
//
//   * it imports `ferrocad.call(ptr, len) -> i64`: the document API. The loader
//     implements it — read the request out of the addon's memory, call the host, and
//     write the answer back into a buffer the addon's own `ferrocad_alloc` returned;
//   * it exports `memory`, `ferrocad_alloc`/`ferrocad_dealloc`, and the entry points
//     this class calls.
//
// Only text crosses a memory boundary, in either direction, and the document stays the
// host's: a bundle holds no `Document`, so it cannot grow a second registry. Nothing
// here touches the DOM, which is why `loader.test.mjs` can run it under Node.
//
// **The capability.** A bundle is read-only unless its manifest (`<stem>.json`, which
// `run.sh` copies beside it) says `{"document": "read-write"}`. The refusal happens in
// the document API below, so a refused request never reaches the host — the same rule the
// desktop applies with `ferrocad_shell_api::document_api::is_write`, and `WRITE_VERBS`
// mirrors that list (it is three verbs; there is no cross-language check).

export const WRITE_VERBS = ["create", "set", "recompute"];

const REFUSAL = JSON.stringify({
  ok: false,
  error:
    "read-only: this bundle was not granted document writes " +
    '(its manifest does not say {"document": "read-write"})',
});

/** Whether a document API request changes the document. */
export function isWrite(request) {
  try {
    return WRITE_VERBS.includes(JSON.parse(request)?.call);
  } catch {
    return false;
  }
}

/** Instantiate `bytes` with `call` as the document API, and return the bundle. */
export async function instantiate(bytes, call, options = {}) {
  const writes = options.document === "read-write";
  const encoder = new TextEncoder();
  let instance;

  const imports = {
    ferrocad: {
      // `(ptr, len)` points into the addon's memory. The answer goes back into a
      // buffer **the addon** allocated, packed as `len << 32 | ptr`.
      call(ptr, len) {
        const request = decode(instance, ptr, len);
        // A read-only bundle's writes are answered here, not forwarded: it sees the
        // ordinary failure envelope and cannot learn anything it was not granted.
        const answer = encoder.encode(writes || !isWrite(request) ? call(request) : REFUSAL);
        const out = instance.exports.ferrocad_alloc(answer.length);
        new Uint8Array(instance.exports.memory.buffer, out, answer.length).set(answer);
        return (BigInt(answer.length) << 32n) | BigInt(out >>> 0);
      },
    },
  };

  ({ instance } = await WebAssembly.instantiate(bytes, imports));
  return new WasmBundle(instance);
}

/** The addon functions the page calls, over the packed-string ABI. */
export class WasmBundle {
  #instance;
  #decoder = new TextDecoder();
  #encoder = new TextEncoder();

  constructor(instance) {
    this.#instance = instance;
  }

  /** `[{ id, label, tooltip }]`, as the addon serialized it. */
  commands() {
    return this.#take(this.#instance.exports.ferrocad_commands());
  }

  /** `true` when the command ran; `false` when it is not the bundle's or it failed. */
  run(id) {
    const { ptr, len } = this.#give(id);
    try {
      return this.#instance.exports.ferrocad_run(ptr, len) === 1;
    } finally {
      this.#instance.exports.ferrocad_dealloc(ptr, len);
    }
  }

  /** The display hook: `have` as JSON in, a JSON array of meshes out. */
  stageMeshes(haveJson) {
    const { ptr, len } = this.#give(haveJson);
    try {
      return this.#take(this.#instance.exports.ferrocad_stage_meshes(ptr, len));
    } finally {
      this.#instance.exports.ferrocad_dealloc(ptr, len);
    }
  }

  /** Copy a string into the addon's memory. */
  #give(text) {
    const bytes = this.#encoder.encode(text);
    const ptr = this.#instance.exports.ferrocad_alloc(bytes.length);
    if (bytes.length > 0) {
      new Uint8Array(this.#instance.exports.memory.buffer, ptr, bytes.length).set(bytes);
    }
    return { ptr, len: bytes.length };
  }

  /** Copy a packed `(len << 32 | ptr)` answer out, then free it. */
  #take(packed) {
    const ptr = Number(packed & 0xffffffffn);
    const len = Number((packed >> 32n) & 0xffffffffn);
    if (len === 0) {
      return "";
    }
    const bytes = new Uint8Array(this.#instance.exports.memory.buffer, ptr, len).slice();
    this.#instance.exports.ferrocad_dealloc(ptr, len);
    return this.#decoder.decode(bytes);
  }
}

function decode(instance, ptr, len) {
  if (len === 0) {
    return "";
  }
  const bytes = new Uint8Array(instance.exports.memory.buffer, ptr, len).slice();
  return new TextDecoder().decode(bytes);
}
