// A minimal W3C WebDriver client (Node 22 fetch), enough to drive the real Marsad app through
// tauri-driver + WebKitWebDriver: sessions, element lookup by CSS / XPath, click, keys, script,
// window size and screenshots. No dependencies.

const ELEMENT = "element-6066-11e4-a52e-4f735466cecf";

export class WebDriver {
  constructor(base = "http://127.0.0.1:4444") {
    this.base = base;
    this.sid = null;
  }

  async cmd(method, path, body) {
    const url = `${this.base}${path.replace(":sid", this.sid ?? "")}`;
    const r = await fetch(url, {
      method,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await r.text();
    let v;
    try {
      v = JSON.parse(text);
    } catch {
      throw new Error(`${method} ${path}: HTTP ${r.status} ${text.slice(0, 300)}`);
    }
    if (!r.ok || (v.value && v.value.error)) {
      const e = new Error(`${method} ${path}: ${v.value?.error ?? r.status}: ${v.value?.message ?? text.slice(0, 300)}`);
      e.wd = v.value?.error;
      throw e;
    }
    return v.value;
  }

  async start(capabilities) {
    const v = await this.cmd("POST", "/session", { capabilities: { alwaysMatch: capabilities } });
    this.sid = v.sessionId;
    return v;
  }

  async quit() {
    if (!this.sid) return;
    try {
      await this.cmd("DELETE", "/session/:sid");
    } finally {
      this.sid = null;
    }
  }

  async exec(fn, ...args) {
    const script = typeof fn === "function" ? `return (${fn}).apply(null, arguments);` : fn;
    return this.cmd("POST", "/session/:sid/execute/sync", { script, args });
  }

  /** Run an async function in the page; it gets the arguments and must resolve with the result. */
  async execAsync(fn, ...args) {
    const script = `const done = arguments[arguments.length - 1];
      Promise.resolve((${fn}).apply(null, Array.prototype.slice.call(arguments, 0, -1)))
        .then((v) => done({ ok: v === undefined ? null : v }), (e) => done({ err: String(e && (e.message || JSON.stringify(e)) || e) }));`;
    const r = await this.cmd("POST", "/session/:sid/execute/async", { script, args });
    if (r && r.err !== undefined) throw new Error(r.err);
    return r?.ok;
  }

  async find(using, value) {
    const v = await this.cmd("POST", "/session/:sid/element", { using, value });
    return v[ELEMENT];
  }

  async findAll(using, value) {
    const v = await this.cmd("POST", "/session/:sid/elements", { using, value });
    return v.map((e) => e[ELEMENT]);
  }

  async click(el) {
    return this.cmd("POST", `/session/:sid/element/${el}/click`, {});
  }

  async clear(el) {
    return this.cmd("POST", `/session/:sid/element/${el}/clear`, {});
  }

  async sendKeys(el, text) {
    return this.cmd("POST", `/session/:sid/element/${el}/value`, { text });
  }

  async text(el) {
    return this.cmd("GET", `/session/:sid/element/${el}/text`);
  }

  async setRect(width, height) {
    return this.cmd("POST", "/session/:sid/window/rect", { width, height, x: 0, y: 0 });
  }

  async screenshot() {
    return Buffer.from(await this.cmd("GET", "/session/:sid/screenshot"), "base64");
  }

  async url() {
    return this.cmd("GET", "/session/:sid/url");
  }
}

/** Keys for sendKeys (W3C code points). */
export const Keys = { ENTER: "", ESCAPE: "", TAB: "", BACKSPACE: "", CONTROL: "" };
