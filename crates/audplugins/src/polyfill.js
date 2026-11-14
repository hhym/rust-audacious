/*
 * MusicFree 插件运行时垫片（Rust 版）
 * ------------------------------------
 * 目标：让按照 MusicFreePlugins 标准写的 JS 插件，能在 rust-audacious 里跑起来。
 *
 * 宿主注入的原生函数：
 *   __http(cfgJson)        -> 响应 JSON 字符串 {status, headers, body, url}
 *   __hash(algo, s, key)   -> JSON 字符串 {hex, b64}
 *   __storage(key, value)  -> 读/写本地存储，value 省略时是读
 *   __log(msg)
 *   __platform / __appVersion / __now_ms
 *
 * 这里实现的是插件常用的库子集：axios、cheerio（load/选择器）、crypto-js、
 * dayjs、qs、he、big-integer、musicfree/storage。
 */
(function (global) {
  "use strict";

  function str(v) {
    if (v === undefined) return "undefined";
    if (v === null) return "null";
    if (typeof v === "object") {
      try {
        return JSON.stringify(v);
      } catch (e) {
        return String(v);
      }
    }
    return String(v);
  }

  // ---------------------------------------------------------------- console
  global.console = {
    log: function () {
      __log(Array.prototype.map.call(arguments, str).join(" "));
    },
    info: function () {
      __log(Array.prototype.map.call(arguments, str).join(" "));
    },
    warn: function () {
      __log("[warn] " + Array.prototype.map.call(arguments, str).join(" "));
    },
    error: function () {
      __log("[error] " + Array.prototype.map.call(arguments, str).join(" "));
    },
  };

  // ---------------------------------------------------------------- axios
  function mergeHeaders(base, extra) {
    var out = {};
    var k;
    for (k in base || {}) out[k] = base[k];
    for (k in extra || {}) out[k] = extra[k];
    if (!out["User-Agent"] && !out["user-agent"]) {
      out["User-Agent"] =
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";
    }
    return out;
  }

  function encodeParams(params) {
    if (!params) return "";
    var parts = [];
    for (var key in params) {
      if (!Object.prototype.hasOwnProperty.call(params, key)) continue;
      var v = params[key];
      if (v === undefined || v === null) continue;
      var values = Array.isArray(v) ? v : [v];
      for (var i = 0; i < values.length; i++) {
        parts.push(encodeURIComponent(key) + "=" + encodeURIComponent(values[i]));
      }
    }
    return parts.join("&");
  }

  function doRequest(config) {
    var url = config.url || "";
    var method = (config.method || "get").toUpperCase();
    var params = config.params;
    var qsText = encodeParams(params);
    if (qsText) url += (url.indexOf("?") >= 0 ? "&" : "?") + qsText;

    var headers = mergeHeaders(axios.defaults.headers, config.headers);
    var body = config.data;
    if (body !== undefined && typeof body !== "string") {
      if (headers["Content-Type"] === undefined && headers["content-type"] === undefined) {
        headers["Content-Type"] = "application/json";
      }
      body = JSON.stringify(body);
    }

    var payload = {
      method: method,
      url: url,
      headers: headers,
      body: body === undefined ? null : body,
      timeout: config.timeout || axios.defaults.timeout || 15000,
    };

    var raw = __http(JSON.stringify(payload));
    var resp = JSON.parse(raw);
    if (resp.error) {
      var err = new Error("Request failed: " + resp.error);
      err.config = config;
      throw err;
    }
    if (resp.status >= 400) {
      var e = new Error(
        "Request failed with status code " + resp.status + " (" + url + ")"
      );
      e.response = {
        status: resp.status,
        data: tryParseJson(resp.body),
        headers: resp.headers,
        config: config,
      };
      throw e;
    }
    return {
      data: config.responseType === "text" ? resp.body : tryParseJson(resp.body),
      status: resp.status,
      statusText: "",
      headers: resp.headers,
      config: config,
      request: {},
    };
  }

  function tryParseJson(text) {
    if (typeof text !== "string") return text;
    try {
      return JSON.parse(text);
    } catch (e) {
      return text;
    }
  }

  function settle(config) {
    // 宿主是同步 HTTP，这里包成 Promise，插件的 await 才能拿到值
    return new Promise(function (resolve, reject) {
      try {
        resolve(doRequest(config));
      } catch (err) {
        reject(err);
      }
    });
  }

  function axios(configOrUrl, maybeConfig) {
    var cfg = typeof configOrUrl === "string" ? maybeConfig || {} : configOrUrl || {};
    if (typeof configOrUrl === "string") cfg.url = configOrUrl;
    if (!cfg.url && cfg.baseURL) cfg.url = cfg.baseURL + (cfg.url || "");
    return settle(cfg);
  }
  axios.defaults = { headers: {}, timeout: 15000 };
  axios.get = function (url, config) {
    return settle(Object.assign({}, config || {}, { url: url, method: "get" }));
  };
  axios.delete = function (url, config) {
    return settle(Object.assign({}, config || {}, { url: url, method: "delete" }));
  };
  axios.head = function (url, config) {
    return settle(Object.assign({}, config || {}, { url: url, method: "head" }));
  };
  axios.options = function (url, config) {
    return settle(Object.assign({}, config || {}, { url: url, method: "options" }));
  };
  axios.post = function (url, data, config) {
    return settle(Object.assign({}, config || {}, { url: url, data: data, method: "post" }));
  };
  axios.put = function (url, data, config) {
    return settle(Object.assign({}, config || {}, { url: url, data: data, method: "put" }));
  };
  axios.patch = function (url, data, config) {
    return settle(Object.assign({}, config || {}, { url: url, data: data, method: "patch" }));
  };
  axios.request = axios;
  axios.create = function (defaults) {
    var instance = function (cfg) {
      var merged = Object.assign({ headers: mergeHeaders(axios.defaults.headers, (defaults && defaults.headers) || {}) }, cfg || {});
      return settle(merged);
    };
    instance.get = function (url, config) {
      return instance(Object.assign({}, config || {}, { url: url, method: "get" }));
    };
    instance.post = function (url, data, config) {
      return instance(Object.assign({}, config || {}, { url: url, data: data, method: "post" }));
    };
    instance.defaults = Object.assign({ headers: {} }, defaults || {});
    return instance;
  };

  // ---------------------------------------------------------------- DOM 解析
  var VOID_TAGS = {
    hr: 1, br: 1, img: 1, input: 1, link: 1, meta: 1, area: 1, base: 1, col: 1,
    embed: 1, source: 1, track: 1, param: 1, wbr: 1, script: 0,
  };
  var RAW_TEXT_TAGS = { script: 1, style: 1, textarea: 1 };

  function parseHtml(html) {
    var root = { type: "root", tag: "#root", attrs: {}, children: [], parent: null };
    var stack = [root];
    var current = root;
    var i = 0;
    var len = html.length;

    function pushText(text) {
      if (!text) return;
      var last = current.children[current.children.length - 1];
      if (last && last.type === "text") {
        last.text += text;
      } else {
        current.children.push({ type: "text", text: text, parent: current });
      }
    }

    while (i < len) {
      var lt = html.indexOf("<", i);
      if (lt < 0) {
        pushText(html.slice(i));
        break;
      }
      if (lt > i) pushText(html.slice(i, lt));

      if (html.startsWith("<!--", lt)) {
        var endComment = html.indexOf("-->", lt + 4);
        i = endComment < 0 ? len : endComment + 3;
        continue;
      }
      if (html.startsWith("<!", lt) || html.startsWith("<?", lt)) {
        var endDecl = html.indexOf(">", lt);
        i = endDecl < 0 ? len : endDecl + 1;
        continue;
      }

      var gt = html.indexOf(">", lt);
      if (gt < 0) break;
      var raw = html.slice(lt + 1, gt);

      if (raw[0] === "/") {
        var closeName = raw.slice(1).trim().toLowerCase();
        for (var d = stack.length - 1; d >= 1; d--) {
          if (stack[d].tag === closeName) {
            stack.length = d;
            current = stack[stack.length - 1];
            break;
          }
        }
        i = gt + 1;
        continue;
      }

      // 开标签：拆出标签名与属性
      var nameMatch = /^([a-zA-Z0-9:_-]+)/.exec(raw);
      if (!nameMatch) {
        i = gt + 1;
        continue;
      }
      var tag = nameMatch[1].toLowerCase();
      var attrs = {};
      var attrRe = /([^\s=/>]+)(?:\s*=\s*("([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
      var rest = raw.slice(nameMatch[1].length);
      var m;
      while ((m = attrRe.exec(rest))) {
        var attrName = m[1].toLowerCase();
        var attrValue = m[3] !== undefined ? m[3] : m[4] !== undefined ? m[4] : m[5] !== undefined ? m[5] : "";
        attrs[attrName] = decodeEntities(attrValue);
      }

      var node = { type: "tag", tag: tag, attrs: attrs, children: [], parent: current };
      current.children.push(node);
      i = gt + 1;

      if (RAW_TEXT_TAGS[tag]) {
        var closeTag = "</" + tag + ">";
        var closeIdx = html.toLowerCase().indexOf(closeTag, i);
        var rawContent = closeIdx < 0 ? html.slice(i) : html.slice(i, closeIdx);
        if (rawContent) {
          node.children.push({ type: "text", text: rawContent, parent: node });
        }
        i = closeIdx < 0 ? len : closeIdx + closeTag.length;
        continue;
      }

      if (VOID_TAGS[tag] || raw.slice(-1) === "/") {
        continue;
      }
      stack.push(node);
      current = node;
    }
    return root;
  }

  var ENTITIES = {
    amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ",
    copy: "©", reg: "®", trade: "™", hellip: "…", mdash: "—", ndash: "–",
    ldquo: "“", rdquo: "”", lsquo: "‘", rsquo: "’",
  };

  function decodeEntities(text) {
    if (typeof text !== "string" || text.indexOf("&") < 0) return text;
    return text.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, function (all, code) {
      if (code[0] === "#") {
        var num = code[1] === "x" || code[1] === "X" ? parseInt(code.slice(2), 16) : parseInt(code.slice(1), 10);
        if (!isNaN(num)) return String.fromCharCode(num);
        return all;
      }
      var lowered = code.toLowerCase();
      return ENTITIES[lowered] !== undefined ? ENTITIES[lowered] : all;
    });
  }

  // ---------------------------------------------------------------- 选择器
  // 支持：tag、*、.class、#id、[attr]、[attr=value]、后代（空格）、子代（>）、逗号分组
  function parseCompound(part) {
    var out = { tag: null, id: null, classes: [], attrs: [] };
    var re = /([#.]?[a-zA-Z0-9_*-]+)|(\[[^\]]+\])|(\*)/g;
    var m;
    while ((m = re.exec(part))) {
      var token = m[0];
      if (token === "*") continue;
      if (token[0] === ".") out.classes.push(token.slice(1));
      else if (token[0] === "#") out.id = token.slice(1);
      else if (token[0] === "[") {
        var inner = token.slice(1, -1);
        var eq = inner.indexOf("=");
        if (eq < 0) out.attrs.push({ name: inner.trim(), value: null });
        else {
          var name = inner.slice(0, eq).trim();
          var value = inner.slice(eq + 1).trim().replace(/^["']|["']$/g, "");
          out.attrs.push({ name: name, value: value });
        }
      } else out.tag = token.toLowerCase();
    }
    return out;
  }

  function parseSelector(selector) {
    return selector.split(",").map(function (group) {
      var steps = [];
      var token = "";
      var combinator = " ";
      for (var i = 0; i < group.length; i++) {
        var ch = group[i];
        if (ch === ">" || ch === " ") {
          if (token.trim()) {
            steps.push({ combinator: steps.length ? combinator : " ", compound: parseCompound(token.trim()) });
            token = "";
          }
          if (ch === ">") combinator = ">";
          else if (!token) combinator = " ";
        } else {
          token += ch;
          if (ch !== " ") {
            // 下一个组合符生效前保持当前值
          }
        }
        if (ch !== " " && ch !== ">") combinator = i === 0 ? " " : combinator === ">" ? ">" : " ";
      }
      if (token.trim()) {
        steps.push({ combinator: steps.length ? combinator : " ", compound: parseCompound(token.trim()) });
      }
      return steps;
    });
  }

  function matchCompound(node, compound) {
    if (node.type !== "tag") return false;
    if (compound.tag && node.tag !== compound.tag) return false;
    if (compound.id && node.attrs.id !== compound.id) return false;
    for (var i = 0; i < compound.classes.length; i++) {
      var cls = node.attrs.class;
      if (!cls || (" " + cls + " ").indexOf(" " + compound.classes[i] + " ") < 0) return false;
    }
    for (var j = 0; j < compound.attrs.length; j++) {
      var spec = compound.attrs[j];
      if (node.attrs[spec.name] === undefined) return false;
      if (spec.value !== null && node.attrs[spec.name] !== spec.value) return false;
    }
    return true;
  }

  function matchSteps(node, steps, index) {
    if (index < 0) return true;
    if (!matchCompound(node, steps[index].compound)) return false;
    if (index === 0) return true;
    var combinator = steps[index].combinator;
    var parent = node.parent;
    if (combinator === ">") {
      return parent && parent.type === "tag" ? matchSteps(parent, steps, index - 1) : false;
    }
    while (parent) {
      if (parent.type === "tag" && matchSteps(parent, steps, index - 1)) return true;
      parent = parent.parent;
    }
    return false;
  }

  function walk(node, visit) {
    var children = node.children || [];
    for (var i = 0; i < children.length; i++) {
      var child = children[i];
      if (visit(child) === false) return false;
      if (child.children && walk(child, visit) === false) return false;
    }
    return true;
  }

  function selectNodes(rootNode, selector) {
    var groups = parseSelector(selector);
    var out = [];
    walk(rootNode, function (node) {
      if (node.type !== "tag") return;
      for (var g = 0; g < groups.length; g++) {
        var steps = groups[g];
        if (!steps.length) continue;
        if (matchSteps(node, steps, steps.length - 1)) {
          out.push(node);
          return;
        }
      }
    });
    return out;
  }

  function textOf(node) {
    if (node.type === "text") return node.text;
    var out = "";
    var children = node.children || [];
    for (var i = 0; i < children.length; i++) out += textOf(children[i]);
    return out;
  }

  function htmlOf(node) {
    if (node.type === "text") return node.text;
    var out = "";
    var children = node.children || [];
    for (var i = 0; i < children.length; i++) {
      var child = children[i];
      if (child.type === "text") out += child.text;
      else {
        var attrs = "";
        for (var k in child.attrs) {
          if (Object.prototype.hasOwnProperty.call(child.attrs, k)) {
            attrs += " " + k + '="' + child.attrs[k] + '"';
          }
        }
        var inner = htmlOf(child);
        out += "<" + child.tag + attrs + ">" + inner + "</" + child.tag + ">";
      }
    }
    return out;
  }

  function Collection(nodes, root) {
    this.nodes = nodes || [];
    this.root = root || null;
    this.length = this.nodes.length;
    for (var i = 0; i < this.nodes.length; i++) this[i] = this.nodes[i];
  }

  Collection.prototype.each = function (fn) {
    for (var i = 0; i < this.nodes.length; i++) {
      if (fn(i, this.nodes[i]) === false) break;
    }
    return this;
  };
  Collection.prototype.map = function (fn) {
    var out = [];
    for (var i = 0; i < this.nodes.length; i++) out.push(fn(i, this.nodes[i]));
    return new Collection(out, this.root);
  };
  Collection.prototype.get = function (index) {
    return this.nodes[index];
  };
  Collection.prototype.eq = function (index) {
    return new Collection(this.nodes[index] ? [this.nodes[index]] : [], this.root);
  };
  Collection.prototype.first = function () {
    return this.eq(0);
  };
  Collection.prototype.last = function () {
    return this.eq(this.nodes.length - 1);
  };
  Collection.prototype.find = function (selector) {
    var out = [];
    for (var i = 0; i < this.nodes.length; i++) {
      var found = selectNodes(this.nodes[i], selector);
      for (var j = 0; j < found.length; j++) out.push(found[j]);
    }
    return new Collection(out, this.root);
  };
  Collection.prototype.parent = function () {
    var out = [];
    for (var i = 0; i < this.nodes.length; i++) {
      if (this.nodes[i].parent && this.nodes[i].parent.type === "tag") out.push(this.nodes[i].parent);
    }
    return new Collection(out, this.root);
  };
  Collection.prototype.children = function (selector) {
    var out = [];
    for (var i = 0; i < this.nodes.length; i++) {
      var children = this.nodes[i].children || [];
      for (var j = 0; j < children.length; j++) {
        if (children[j].type !== "tag") continue;
        if (selector && !matchCompound(children[j], parseCompound(selector))) continue;
        out.push(children[j]);
      }
    }
    return new Collection(out, this.root);
  };
  Collection.prototype.filter = function (selector) {
    return new Collection(
      selectNodes(this.root || { children: this.nodes }, selector).filter(function (n) {
        return this.nodes.indexOf(n) >= 0;
      }, this),
      this.root
    );
  };
  Collection.prototype.text = function (value) {
    if (value !== undefined) return this;
    var out = "";
    for (var i = 0; i < this.nodes.length; i++) out += textOf(this.nodes[i]);
    return out;
  };
  Collection.prototype.html = function (value) {
    if (value !== undefined) return this;
    var out = "";
    for (var i = 0; i < this.nodes.length; i++) out += htmlOf(this.nodes[i]);
    return out;
  };
  Collection.prototype.attr = function (name, value) {
    if (value === undefined) {
      return this.nodes.length ? this.nodes[0].attrs[name] : undefined;
    }
    for (var i = 0; i < this.nodes.length; i++) this.nodes[i].attrs[name] = String(value);
    return this;
  };
  Collection.prototype.removeAttr = function (name) {
    for (var i = 0; i < this.nodes.length; i++) delete this.nodes[i].attrs[name];
    return this;
  };
  Collection.prototype.data = function (name, value) {
    if (value === undefined) {
      if (!this.nodes.length) return undefined;
      var raw = this.nodes[0].attrs["data-" + name];
      if (raw === undefined) raw = this.nodes[0].attrs[name];
      if (raw === undefined) return undefined;
      var asNumber = Number(raw);
      if (raw !== "" && !isNaN(asNumber)) return asNumber;
      if (raw === "true") return true;
      if (raw === "false") return false;
      if ((raw[0] === "{" && raw[raw.length - 1] === "}") || (raw[0] === "[" && raw[raw.length - 1] === "]")) {
        try {
          return JSON.parse(raw);
        } catch (e) {
          return raw;
        }
      }
      return raw;
    }
    return this.attr("data-" + name, value);
  };
  Collection.prototype.val = function () {
    return this.nodes.length ? this.nodes[0].attrs.value : undefined;
  };
  Collection.prototype.hasClass = function (name) {
    return this.nodes.length ? matchCompound(this.nodes[0], { classes: [name], attrs: [], tag: null }) : false;
  };
  Collection.prototype.is = function (selector) {
    if (!this.nodes.length) return false;
    var steps = parseSelector(selector)[0];
    return matchSteps(this.nodes[0], steps, steps.length - 1);
  };
  Collection.prototype.remove = function () {
    for (var i = 0; i < this.nodes.length; i++) {
      var node = this.nodes[i];
      if (node.parent) {
        var idx = node.parent.children.indexOf(node);
        if (idx >= 0) node.parent.children.splice(idx, 1);
      }
    }
    return this;
  };
  Collection.prototype.toString = function () {
    return this.text();
  };

  function cheerioLoad(html) {
    var root = parseHtml(String(html === undefined || html === null ? "" : html));
    var $ = function (selector, context) {
      if (selector === undefined || selector === null) return new Collection([], root);
      if (typeof selector === "string") {
        var trimmed = selector.trim();
        if (trimmed[0] === "<" && trimmed[trimmed.length - 1] === ">") {
          var fragment = parseHtml(trimmed);
          return new Collection(fragment.children.filter(function (n) {
            return n.type === "tag";
          }), fragment);
        }
        return new Collection(selectNodes(context && context.nodes ? context.nodes[0] : root, trimmed), root);
      }
      if (Array.isArray(selector)) return new Collection(selector, root);
      if (selector.type) return new Collection([selector], root);
      return new Collection([], root);
    };
    $.root = function () {
      return new Collection([root], root);
    };
    $.html = function () {
      return htmlOf(root);
    };
    $.load = cheerioLoad;
    $.text = function () {
      return textOf(root);
    };
    return $;
  }

  // ---------------------------------------------------------------- crypto-js
  var encHex = { __enc: "hex" };
  var encBase64 = { __enc: "base64" };

  function HashResult(algo, data, key) {
    this.__algo = algo;
    this.__data = data;
    this.__key = key;
    this.__cache = null;
  }
  HashResult.prototype.__compute = function () {
    if (!this.__cache) {
      this.__cache = JSON.parse(__hash(this.__algo, String(this.__data), this.__key === undefined || this.__key === null ? null : String(this.__key)));
    }
    return this.__cache;
  };
  HashResult.prototype.toString = function (encoder) {
    var result = this.__compute();
    var enc = encoder && encoder.__enc ? encoder.__enc : "hex";
    return enc === "base64" ? result.b64 : result.hex;
  };
  HashResult.prototype.__valueOf = function () {
    return this.toString();
  };

  var CryptoJS = {
    enc: {
      Hex: encHex,
      Base64: encBase64,
      Utf8: { stringify: function (v) { return String(v); }, parse: function (v) { return String(v); } },
      Latin1: { stringify: function (v) { return String(v); }, parse: function (v) { return String(v); } },
    },
    SHA256: function (data) { return new HashResult("sha256", data); },
    SHA1: function (data) { return new HashResult("sha1", data); },
    MD5: function (data) { return new HashResult("md5", data); },
    HmacSHA1: function (data, key) { return new HashResult("hmac-sha1", data, key); },
    HmacSHA256: function (data, key) { return new HashResult("hmac-sha256", data, key); },
    HmacMD5: function (data, key) { return new HashResult("hmac-md5", data, key); },
  };
  CryptoJS.SHA256.prototype = HashResult.prototype;
  CryptoJS.HmacSHA1.prototype = HashResult.prototype;

  // ---------------------------------------------------------------- dayjs（子集）
  function pad2(n) {
    return n < 10 ? "0" + n : String(n);
  }
  function Dayjs(value) {
    if (value instanceof Dayjs) this.date = new Date(value.date.getTime());
    else if (value === undefined || value === null) this.date = new Date();
    else if (typeof value === "number") this.date = new Date(value < 1e12 ? value * 1000 : value);
    else if (typeof value === "string" && /^\d+$/.test(value)) this.date = new Date(Number(value) * 1000);
    else this.date = new Date(value);
  }
  Dayjs.prototype.unix = function () {
    return Math.floor(this.date.getTime() / 1000);
  };
  Dayjs.prototype.valueOf = function () {
    return this.date.getTime();
  };
  Dayjs.prototype.format = function (fmt) {
    if (!fmt) fmt = "YYYY-MM-DDTHH:mm:ssZ";
    var d = this.date;
    return fmt
      .replace(/YYYY/g, d.getFullYear())
      .replace(/MM/g, pad2(d.getMonth() + 1))
      .replace(/DD/g, pad2(d.getDate()))
      .replace(/HH/g, pad2(d.getHours()))
      .replace(/mm/g, pad2(d.getMinutes()))
      .replace(/ss/g, pad2(d.getSeconds()))
      .replace(/M/g, d.getMonth() + 1)
      .replace(/D/g, d.getDate())
      .replace(/H/g, d.getHours())
      .replace(/m/g, d.getMinutes())
      .replace(/s/g, d.getSeconds());
  };
  Dayjs.prototype.add = function (n, unit) {
    var d = new Date(this.date.getTime());
    if (unit === "day" || unit === "days") d.setDate(d.getDate() + n);
    else if (unit === "month" || unit === "months") d.setMonth(d.getMonth() + n);
    else if (unit === "year" || unit === "years") d.setFullYear(d.getFullYear() + n);
    else d.setSeconds(d.getSeconds() + n);
    return new Dayjs(d.getTime());
  };
  Dayjs.prototype.toDate = function () {
    return this.date;
  };
  function dayjs(value) {
    return new Dayjs(value);
  }
  dayjs.unix = function (seconds) {
    return new Dayjs(Number(seconds) * 1000);
  };
  dayjs.extend = function () {};
  dayjs.prototype = Dayjs.prototype;

  // ---------------------------------------------------------------- qs
  var qs = {
    stringify: function (obj, options) {
      var sep = (options && options.delimiter) || "&";
      var parts = [];
      function add(name, value) {
        if (value === undefined) return;
        if (value === null) value = "";
        if (Array.isArray(value)) {
          for (var i = 0; i < value.length; i++) {
            add(name + (options && options.indices === false ? "[]" : "[" + i + "]"), value[i]);
          }
        } else {
          parts.push(encodeURIComponent(name) + "=" + encodeURIComponent(value));
        }
      }
      for (var key in obj) {
        if (Object.prototype.hasOwnProperty.call(obj, key)) add(key, obj[key]);
      }
      return parts.join(sep);
    },
    parse: function (text) {
      var out = {};
      if (!text) return out;
      var pairs = String(text).replace(/^\?/, "").split("&");
      for (var i = 0; i < pairs.length; i++) {
        if (!pairs[i]) continue;
        var eq = pairs[i].indexOf("=");
        var key = decodeURIComponent(eq < 0 ? pairs[i] : pairs[i].slice(0, eq));
        var value = eq < 0 ? "" : decodeURIComponent(pairs[i].slice(eq + 1).replace(/\+/g, " "));
        if (out[key] === undefined) out[key] = value;
        else if (Array.isArray(out[key])) out[key].push(value);
        else out[key] = [out[key], value];
      }
      return out;
    },
  };

  // ---------------------------------------------------------------- he
  var he = {
    decode: function (text) {
      return decodeEntities(String(text));
    },
    encode: function (text) {
      return String(text)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
    },
  };

  // ---------------------------------------------------------------- big-integer
  function BigInteger(value) {
    this.value = BigInt(typeof value === "string" ? value : Math.floor(Number(value)));
  }
  BigInteger.prototype.toString = function (radix) {
    return radix && radix !== 10 ? this.value.toString(radix) : this.value.toString();
  };
  BigInteger.prototype.valueOf = function () {
    return Number(this.value);
  };
  BigInteger.prototype.add = function (other) {
    return new BigInteger((this.value + BigInteger.prototype.__toBig(other)) .toString());
  };
  BigInteger.prototype.__toBig = function (other) {
    return other instanceof BigInteger ? other.value : BigInt(other);
  };
  function bigInt(value) {
    return new BigInteger(value);
  }
  bigInt.zero = new BigInteger(0);
  bigInt.one = new BigInteger(1);

  // ---------------------------------------------------------------- storage
  var storage = {
    getItem: function (key) {
      return Promise.resolve(JSON.parse(__storage(String(key), null)));
    },
    setItem: function (key, value) {
      __storage(String(key), JSON.stringify(value === undefined ? null : value));
      return Promise.resolve();
    },
    removeItem: function (key) {
      __storage(String(key), JSON.stringify(null));
      return Promise.resolve();
    },
  };

  // ---------------------------------------------------------------- require / process / env
  var packages = {
    axios: axios,
    cheerio: { load: cheerioLoad, default: cheerioLoad },
    "crypto-js": CryptoJS,
    dayjs: dayjs,
    qs: qs,
    he: he,
    "big-integer": bigInt,
    "musicfree/storage": storage,
    "@react-native-cookies/cookies": {
      get: function () { return Promise.resolve({}); },
      set: function () { return Promise.resolve(); },
      clearAll: function () { return Promise.resolve(); },
    },
    webdav: {
      createClient: function () {
        return {
          getDirectoryContents: function () { return Promise.resolve([]); },
        };
      },
    },
  };

  global.require = function (name) {
    var pkg = packages[name];
    if (pkg) {
      if (typeof pkg === "object" && pkg.default === undefined) pkg.default = pkg;
      return pkg;
    }
    throw new Error("Cannot find module '" + name + "'");
  };
  global.__musicfree_require = global.require;

  global.process = {
    platform: __platform,
    version: __appVersion,
    env: {},
    nextTick: function (fn) { fn(); },
  };
  global.env = {
    os: __platform,
    appVersion: __appVersion,
    lang: "zh-CN",
    getUserVariables: function () { return {}; },
  };

  // 宿主用这两个变量拿插件返回值
  global.__invokeDone = false;
  global.__invokeResult = null;
  global.__invokeError = null;
})(globalThis);
