// covenant.mjs — the covenant command as a JavaScript API.
//
// It runs covenant.wasm, the command built for wasm32-wasip1 (CI holds that
// build byte for byte to the native one), over an in-memory filesystem: the
// files a call passes are all the command can read, what it writes comes back
// in the result, and nothing touches a disk or leaves the page. No
// dependencies; browsers and Node 20+.
//
//   import { load } from "./covenant.mjs";
//   const covenant = await load(new URL("./covenant.wasm", import.meta.url));
//   const { code, report } = await covenant.check({ contract: yaml, data: csv });
//   // code: 0 passed, 1 violated, 2 the run failed (report is null; see stderr)

/** Compile covenant.wasm once; each call then runs a fresh instance of it. */
export async function load(wasm) {
  return new Covenant(await compile(wasm));
}

export class Covenant {
  #module;

  constructor(module) {
    this.#module = module;
  }

  /**
   * Run the command. `args` leave out the program name; `files` maps paths
   * (relative, like "contracts/orders.yaml") to text or bytes, and a path
   * ending in "/" to an empty directory. Returns the exit code, stdout and
   * stderr as text, and every file after the run.
   */
  async run(args, { files = {}, stdin = "" } = {}) {
    const fs = new MemFs(files);
    const wasi = new Wasi(["covenant", ...args], fs, bytes(stdin));
    const instance = await WebAssembly.instantiate(this.#module, {
      wasi_snapshot_preview1: wasi.imports(),
    });
    wasi.memory = instance.exports.memory;
    let code = 0;
    try {
      instance.exports._start();
    } catch (e) {
      if (!(e instanceof Exit)) {
        e.stderr = text(wasi.stderr);
        throw e;
      }
      code = e.code;
    }
    return { code, stdout: text(wasi.stdout), stderr: text(wasi.stderr), files: fs.snapshot() };
  }

  /**
   * `covenant check` one data file against one contract: the JSON report as
   * an object (null when the run failed), with the exit code and output.
   * `dataName` picks the reader by extension: .csv, .ndjson/.jsonl/.json.
   */
  async check({
    contract,
    data,
    contractName = "contract.yaml",
    dataName = "data.csv",
    model,
    allowUnenforced = false,
  }) {
    const args = ["check", dataName, "--contract", contractName, "--format", "json"];
    if (model) args.push("--model", model);
    if (allowUnenforced) args.push("--allow-unenforced");
    const run = await this.run(args, { files: { [contractName]: contract, [dataName]: data } });
    return { ...run, report: run.code === 2 ? null : JSON.parse(run.stdout)[0] };
  }
}

/** `wasm`: the module, its bytes, a Response, or where to find it (a URL; in Node, a path). */
async function compile(wasm) {
  if (wasm instanceof WebAssembly.Module) return wasm;
  if (wasm instanceof ArrayBuffer || ArrayBuffer.isView(wasm)) return WebAssembly.compile(wasm);
  if (globalThis.Response && wasm instanceof Response) {
    return WebAssembly.compile(await wasm.arrayBuffer());
  }
  let url;
  if (wasm instanceof URL) url = wasm;
  else if (globalThis.location) url = new URL(String(wasm), globalThis.location.href);
  // A URL only in a scheme this can load: `C:\covenant.wasm` parses as a URL
  // whose scheme is the drive letter, and is a Windows path.
  else if (/^(?:file|https?|data|blob):/i.test(String(wasm))) url = new URL(String(wasm));
  else url = (await import("node:url")).pathToFileURL(String(wasm));
  if (url.protocol === "file:") {
    const { readFile } = await import("node:fs/promises");
    return WebAssembly.compile(await readFile(url));
  }
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return WebAssembly.compile(await response.arrayBuffer());
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function bytes(content) {
  return typeof content === "string" ? encoder.encode(content) : new Uint8Array(content);
}

function text(chunks) {
  return decoder.decode(concat(chunks));
}

function concat(chunks) {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

// --- An in-memory filesystem: files by normalized relative path -----------

class MemFs {
  constructor(files) {
    this.files = new Map();
    this.dirs = new Set([""]);
    this.inodes = new Map();
    for (const [path, content] of Object.entries(files)) {
      const at = this.resolve("", path);
      if (at === null || at === "") throw new Error(`not a file path: ${path}`);
      this.addParents(at);
      if (path.endsWith("/")) this.dirs.add(at);
      else this.files.set(at, new Blob(bytes(content)));
    }
  }

  /** `path` relative to directory `base`; null when it climbs out of the root. */
  resolve(base, path) {
    const parts = base ? base.split("/") : [];
    for (const part of path.split("/")) {
      if (part === "" || part === ".") continue;
      if (part === "..") {
        if (parts.length === 0) return null;
        parts.pop();
      } else {
        parts.push(part);
      }
    }
    return parts.join("/");
  }

  addParents(path) {
    const parts = path.split("/");
    for (let i = 1; i < parts.length; i++) this.dirs.add(parts.slice(0, i).join("/"));
  }

  parent(path) {
    const cut = path.lastIndexOf("/");
    return cut < 0 ? "" : path.slice(0, cut);
  }

  kind(path) {
    if (this.files.has(path)) return FILETYPE.REGULAR_FILE;
    if (this.dirs.has(path)) return FILETYPE.DIRECTORY;
    return null;
  }

  inode(path) {
    if (!this.inodes.has(path)) this.inodes.set(path, BigInt(this.inodes.size + 1));
    return this.inodes.get(path);
  }

  /** A directory's entries, sorted: [name, filetype]. */
  list(dir) {
    const prefix = dir ? `${dir}/` : "";
    const entries = new Map();
    for (const [set, type] of [
      [this.dirs, FILETYPE.DIRECTORY],
      [this.files.keys(), FILETYPE.REGULAR_FILE],
    ]) {
      for (const path of set) {
        if (path === dir || !path.startsWith(prefix)) continue;
        const rest = path.slice(prefix.length);
        if (!rest.includes("/")) entries.set(rest, type);
      }
    }
    return [...entries].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  }

  rename(from, to) {
    if (this.files.has(from)) {
      if (this.dirs.has(to)) return ERRNO.ISDIR;
      if (!this.dirs.has(this.parent(to))) return ERRNO.NOENT;
      this.files.set(to, this.files.get(from));
      this.files.delete(from);
      return ERRNO.SUCCESS;
    }
    if (!this.dirs.has(from) || from === "") return ERRNO.NOENT;
    if (to === from || to.startsWith(`${from}/`)) return ERRNO.INVAL;
    if (this.files.has(to) || !this.dirs.has(this.parent(to))) return ERRNO.NOTDIR;
    if (this.list(to).length > 0) return ERRNO.NOTEMPTY;
    const moved = (path) => to + path.slice(from.length);
    for (const path of [...this.dirs]) {
      if (path === from || path.startsWith(`${from}/`)) {
        this.dirs.delete(path);
        this.dirs.add(moved(path));
      }
    }
    for (const [path, blob] of [...this.files]) {
      if (path.startsWith(`${from}/`)) {
        this.files.delete(path);
        this.files.set(moved(path), blob);
      }
    }
    return ERRNO.SUCCESS;
  }

  /** Every file's bytes, by path. */
  snapshot() {
    return Object.fromEntries([...this.files].map(([path, blob]) => [path, blob.bytes()]));
  }
}

/** A growable file body. */
class Blob {
  constructor(initial) {
    this.buf = initial;
    this.len = initial.length;
  }

  bytes() {
    return this.buf.slice(0, this.len);
  }

  truncate() {
    this.buf = new Uint8Array(0);
    this.len = 0;
  }

  write(at, data) {
    const end = at + data.length;
    if (end > this.buf.length) {
      const grown = new Uint8Array(Math.max(end, this.buf.length * 2, 64));
      grown.set(this.buf.subarray(0, this.len));
      this.buf = grown;
    }
    this.buf.set(data, at);
    this.len = Math.max(this.len, end);
  }
}

// --- WASI preview 1, as much as the command uses ---------------------------

const ERRNO = {
  SUCCESS: 0,
  BADF: 8,
  EXIST: 20,
  INVAL: 28,
  ISDIR: 31,
  NOENT: 44,
  NOTDIR: 54,
  NOTEMPTY: 55,
  SPIPE: 70,
};
const FILETYPE = { UNKNOWN: 0, DIRECTORY: 3, REGULAR_FILE: 4 };
const OFLAGS = { CREAT: 1, DIRECTORY: 2, EXCL: 4, TRUNC: 8 };
const FDFLAGS_APPEND = 1;
const ALL_RIGHTS = 0x1fffffffn;
const PREOPEN = 3;

class Exit {
  constructor(code) {
    this.code = code;
  }
}

class Wasi {
  constructor(args, fs, stdin) {
    this.args = args.map((a) => encoder.encode(`${a}\0`));
    this.fs = fs;
    this.stdin = { data: stdin, pos: 0 };
    this.stdout = [];
    this.stderr = [];
    this.memory = null;
    // fd 3 is the one preopened directory, "/", which is the filesystem's root.
    this.fds = new Map([[PREOPEN, { dir: "" }]]);
    this.nextFd = PREOPEN + 1;
  }

  view() {
    return new DataView(this.memory.buffer);
  }

  mem() {
    return new Uint8Array(this.memory.buffer);
  }

  str(ptr, len) {
    return decoder.decode(this.mem().subarray(ptr, ptr + len));
  }

  /** The path a call names, relative to directory fd `fd`. */
  path(fd, ptr, len) {
    const handle = this.fds.get(fd);
    if (!handle || handle.dir === undefined) return { errno: ERRNO.BADF };
    const at = this.fs.resolve(handle.dir, this.str(ptr, len));
    return at === null ? { errno: ERRNO.NOENT } : { at };
  }

  filestat(ptr, type, size, ino) {
    const v = this.view();
    for (let i = 0; i < 64; i += 8) v.setBigUint64(ptr + i, 0n, true);
    v.setBigUint64(ptr + 8, ino, true);
    v.setUint8(ptr + 16, type);
    v.setBigUint64(ptr + 24, 1n, true);
    v.setBigUint64(ptr + 32, BigInt(size), true);
  }

  statPath(at, ptr) {
    const type = this.fs.kind(at);
    if (type === null) return ERRNO.NOENT;
    const size = type === FILETYPE.REGULAR_FILE ? this.fs.files.get(at).len : 0;
    this.filestat(ptr, type, size, this.fs.inode(at));
    return ERRNO.SUCCESS;
  }

  imports() {
    const w = this;
    return {
      args_sizes_get(argc, bufSize) {
        const v = w.view();
        v.setUint32(argc, w.args.length, true);
        v.setUint32(bufSize, w.args.reduce((n, a) => n + a.length, 0), true);
        return ERRNO.SUCCESS;
      },
      args_get(argv, buf) {
        const v = w.view();
        const m = w.mem();
        for (const [i, a] of w.args.entries()) {
          v.setUint32(argv + i * 4, buf, true);
          m.set(a, buf);
          buf += a.length;
        }
        return ERRNO.SUCCESS;
      },
      environ_sizes_get(count, bufSize) {
        const v = w.view();
        v.setUint32(count, 0, true);
        v.setUint32(bufSize, 0, true);
        return ERRNO.SUCCESS;
      },
      environ_get() {
        return ERRNO.SUCCESS;
      },
      clock_time_get(id, _precision, time) {
        const ns =
          id === 0
            ? BigInt(Date.now()) * 1_000_000n
            : BigInt(Math.round(globalThis.performance.now() * 1e6));
        w.view().setBigUint64(time, ns, true);
        return ERRNO.SUCCESS;
      },
      random_get(buf, len) {
        for (let at = 0; at < len; at += 65536) {
          globalThis.crypto.getRandomValues(w.mem().subarray(buf + at, buf + Math.min(len, at + 65536)));
        }
        return ERRNO.SUCCESS;
      },
      fd_close(fd) {
        if (fd <= PREOPEN || !w.fds.delete(fd)) return ERRNO.BADF;
        return ERRNO.SUCCESS;
      },
      fd_fdstat_get(fd, ptr) {
        const handle = fd <= 2 ? {} : w.fds.get(fd);
        if (!handle) return ERRNO.BADF;
        const type =
          fd <= 2 ? FILETYPE.UNKNOWN : handle.dir !== undefined ? FILETYPE.DIRECTORY : FILETYPE.REGULAR_FILE;
        const v = w.view();
        v.setUint8(ptr, type);
        v.setUint16(ptr + 2, handle.append ? FDFLAGS_APPEND : 0, true);
        v.setBigUint64(ptr + 8, ALL_RIGHTS, true);
        v.setBigUint64(ptr + 16, ALL_RIGHTS, true);
        return ERRNO.SUCCESS;
      },
      fd_filestat_get(fd, ptr) {
        if (fd <= 2) {
          w.filestat(ptr, FILETYPE.UNKNOWN, 0, 0n);
          return ERRNO.SUCCESS;
        }
        const handle = w.fds.get(fd);
        if (!handle) return ERRNO.BADF;
        return w.statPath(handle.dir ?? handle.file, ptr);
      },
      fd_prestat_get(fd, ptr) {
        if (fd !== PREOPEN) return ERRNO.BADF;
        const v = w.view();
        v.setUint8(ptr, 0);
        v.setUint32(ptr + 4, 1, true);
        return ERRNO.SUCCESS;
      },
      fd_prestat_dir_name(fd, ptr, len) {
        if (fd !== PREOPEN) return ERRNO.BADF;
        if (len < 1) return ERRNO.INVAL;
        w.mem()[ptr] = "/".charCodeAt(0);
        return ERRNO.SUCCESS;
      },
      fd_read(fd, iovs, count, nread) {
        let source;
        if (fd === 0) {
          source = w.stdin;
        } else {
          const handle = w.fds.get(fd);
          if (!handle || fd <= 2) return ERRNO.BADF;
          if (handle.dir !== undefined) return ERRNO.ISDIR;
          const blob = w.fs.files.get(handle.file);
          source = { data: blob.buf.subarray(0, blob.len), handle };
          source.pos = handle.pos;
        }
        const v = w.view();
        const m = w.mem();
        let total = 0;
        for (let i = 0; i < count; i++) {
          const ptr = v.getUint32(iovs + i * 8, true);
          const len = v.getUint32(iovs + i * 8 + 4, true);
          const chunk = source.data.subarray(source.pos, source.pos + len);
          m.set(chunk, ptr);
          source.pos += chunk.length;
          total += chunk.length;
          if (chunk.length < len) break;
        }
        if (source.handle) source.handle.pos = source.pos;
        v.setUint32(nread, total, true);
        return ERRNO.SUCCESS;
      },
      fd_write(fd, iovs, count, nwritten) {
        const v = w.view();
        const m = w.mem();
        const chunks = [];
        for (let i = 0; i < count; i++) {
          const ptr = v.getUint32(iovs + i * 8, true);
          const len = v.getUint32(iovs + i * 8 + 4, true);
          chunks.push(m.slice(ptr, ptr + len));
        }
        const data = concat(chunks);
        if (fd === 1 || fd === 2) {
          (fd === 1 ? w.stdout : w.stderr).push(data);
        } else {
          const handle = w.fds.get(fd);
          if (!handle || fd === 0) return ERRNO.BADF;
          if (handle.dir !== undefined) return ERRNO.ISDIR;
          const blob = w.fs.files.get(handle.file);
          const at = handle.append ? blob.len : handle.pos;
          blob.write(at, data);
          handle.pos = at + data.length;
        }
        v.setUint32(nwritten, data.length, true);
        return ERRNO.SUCCESS;
      },
      fd_seek(fd, offset, whence, newOffset) {
        if (fd <= 2) return ERRNO.SPIPE;
        const handle = w.fds.get(fd);
        if (!handle) return ERRNO.BADF;
        if (handle.dir !== undefined) return ERRNO.ISDIR;
        const len = BigInt(w.fs.files.get(handle.file).len);
        const base = [0n, BigInt(handle.pos), len][whence];
        if (base === undefined) return ERRNO.INVAL;
        const pos = base + offset;
        if (pos < 0n) return ERRNO.INVAL;
        handle.pos = Number(pos);
        w.view().setBigUint64(newOffset, pos, true);
        return ERRNO.SUCCESS;
      },
      fd_readdir(fd, buf, len, cookie, used) {
        const handle = w.fds.get(fd);
        if (!handle) return ERRNO.BADF;
        if (handle.dir === undefined) return ERRNO.NOTDIR;
        const entries = w.fs.list(handle.dir);
        const out = [];
        for (let i = Number(cookie); i < entries.length; i++) {
          const [name, type] = entries[i];
          const nameBytes = encoder.encode(name);
          const header = new DataView(new ArrayBuffer(24));
          header.setBigUint64(0, BigInt(i + 1), true);
          header.setBigUint64(8, w.fs.inode(handle.dir ? `${handle.dir}/${name}` : name), true);
          header.setUint32(16, nameBytes.length, true);
          header.setUint8(20, type);
          out.push(new Uint8Array(header.buffer), nameBytes);
        }
        const all = concat(out);
        const n = Math.min(all.length, len);
        w.mem().set(all.subarray(0, n), buf);
        w.view().setUint32(used, n, true);
        return ERRNO.SUCCESS;
      },
      path_filestat_get(fd, _flags, ptr, len, out) {
        const { errno, at } = w.path(fd, ptr, len);
        return errno ?? w.statPath(at, out);
      },
      path_open(fd, _dirflags, ptr, len, oflags, _base, _inheriting, fdflags, opened) {
        const { errno, at } = w.path(fd, ptr, len);
        if (errno) return errno;
        const type = w.fs.kind(at);
        let handle;
        if (type === FILETYPE.DIRECTORY) {
          if (oflags & (OFLAGS.CREAT | OFLAGS.EXCL)) return ERRNO.EXIST;
          if (oflags & OFLAGS.TRUNC) return ERRNO.ISDIR;
          handle = { dir: at };
        } else if (oflags & OFLAGS.DIRECTORY) {
          return type === null ? ERRNO.NOENT : ERRNO.NOTDIR;
        } else if (type === FILETYPE.REGULAR_FILE) {
          if (oflags & OFLAGS.CREAT && oflags & OFLAGS.EXCL) return ERRNO.EXIST;
          if (oflags & OFLAGS.TRUNC) w.fs.files.get(at).truncate();
          handle = { file: at, pos: 0, append: Boolean(fdflags & FDFLAGS_APPEND) };
        } else {
          if (!(oflags & OFLAGS.CREAT)) return ERRNO.NOENT;
          if (!w.fs.dirs.has(w.fs.parent(at))) return ERRNO.NOENT;
          w.fs.files.set(at, new Blob(new Uint8Array(0)));
          handle = { file: at, pos: 0, append: Boolean(fdflags & FDFLAGS_APPEND) };
        }
        const newFd = w.nextFd++;
        w.fds.set(newFd, handle);
        w.view().setUint32(opened, newFd, true);
        return ERRNO.SUCCESS;
      },
      path_rename(fd, fromPtr, fromLen, toFd, toPtr, toLen) {
        const from = w.path(fd, fromPtr, fromLen);
        if (from.errno) return from.errno;
        const to = w.path(toFd, toPtr, toLen);
        if (to.errno) return to.errno;
        return w.fs.rename(from.at, to.at);
      },
      proc_exit(code) {
        throw new Exit(code);
      },
    };
  }
}
