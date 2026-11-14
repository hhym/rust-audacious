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
  // TS 编译出来的插件常写 axios_1.default(...)，这里做个自引用兼容
  axios.default = axios;
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
  function strToBytes(text) {
    // 手写 UTF-8 编码，避免依赖 TextEncoder
    var bytes = [];
    for (var i = 0; i < text.length; i++) {
      var code = text.charCodeAt(i);
      if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length) {
        var next = text.charCodeAt(i + 1);
        if (next >= 0xdc00 && next <= 0xdfff) {
          code = ((code - 0xd800) << 10) + (next - 0xdc00) + 0x10000;
          i++;
        }
      }
      if (code < 0x80) bytes.push(code);
      else if (code < 0x800) bytes.push(0xc0 | (code >> 6), 0x80 | (code & 63));
      else if (code < 0x10000) bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 63), 0x80 | (code & 63));
      else bytes.push(0xf0 | (code >> 18), 0x80 | ((code >> 12) & 63), 0x80 | ((code >> 6) & 63), 0x80 | (code & 63));
    }
    return bytes;
  }

  function bytesToStr(bytes) {
    var out = "";
    for (var i = 0; i < bytes.length; i++) {
      var b = bytes[i];
      if (b < 0x80) out += String.fromCharCode(b);
      else if (b < 0xe0) out += String.fromCharCode(((b & 31) << 6) | (bytes[++i] & 63));
      else if (b < 0xf0) out += String.fromCharCode(((b & 15) << 12) | ((bytes[++i] & 63) << 6) | (bytes[++i] & 63));
      else {
        var cp = ((b & 7) << 18) | ((bytes[++i] & 63) << 12) | ((bytes[++i] & 63) << 6) | (bytes[++i] & 63);
        cp -= 0x10000;
        out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 1023));
      }
    }
    return out;
  }

  function bytesToHex(bytes) {
    var out = "";
    for (var i = 0; i < bytes.length; i++) {
      var b = bytes[i];
      out += (b < 16 ? "0" : "") + b.toString(16);
    }
    return out;
  }

  function hexToBytes(text) {
    var out = [];
    for (var i = 0; i < text.length; i += 2) out.push(parseInt(text.substr(i, 2), 16));
    return out;
  }

  var B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  function bytesToBase64(bytes) {
    var out = "";
    for (var i = 0; i < bytes.length; i += 3) {
      var b0 = bytes[i];
      var b1 = i + 1 < bytes.length ? bytes[i + 1] : undefined;
      var b2 = i + 2 < bytes.length ? bytes[i + 2] : undefined;
      out += B64.charAt(b0 >> 2);
      out += B64.charAt(((b0 & 3) << 4) | (b1 === undefined ? 0 : b1 >> 4));
      out += b1 === undefined ? "=" : B64.charAt(((b1 & 15) << 2) | (b2 === undefined ? 0 : b2 >> 6));
      out += b2 === undefined ? "=" : B64.charAt(b2 & 63);
    }
    return out;
  }

  function base64ToBytes(text) {
    var clean = String(text).replace(/[^A-Za-z0-9+/=]/g, "");
    var out = [];
    for (var i = 0; i < clean.length; i += 4) {
      var c0 = B64.indexOf(clean.charAt(i));
      var c1 = B64.indexOf(clean.charAt(i + 1));
      var c2 = B64.indexOf(clean.charAt(i + 2));
      var c3 = B64.indexOf(clean.charAt(i + 3));
      out.push((c0 << 2) | (c1 >> 4));
      if (c2 >= 0) out.push(((c1 & 15) << 4) | (c2 >> 2));
      if (c3 >= 0) out.push(((c2 & 3) << 6) | c3);
    }
    return out;
  }

  function WordArray(words, sigBytes) {
    this.words = words || [];
    this.sigBytes = sigBytes === undefined ? this.words.length * 4 : sigBytes;
  }

  function wordArrayBytes(wa) {
    var bytes = [];
    for (var i = 0; i < wa.sigBytes; i++) {
      var word = wa.words[Math.floor(i / 4)] || 0;
      bytes.push((word >>> (24 - (i % 4) * 8)) & 0xff);
    }
    return bytes;
  }

  function bytesToWordArray(bytes, sigBytes) {
    var words = [];
    for (var i = 0; i < bytes.length; i++) {
      words[i >>> 2] = (words[i >>> 2] || 0) | (bytes[i] << (24 - (i % 4) * 8));
    }
    return new WordArray(words, sigBytes === undefined ? bytes.length : sigBytes);
  }

  WordArray.prototype.toString = function (encoder) {
    var bytes = wordArrayBytes(this);
    var kind = encoder && encoder.__enc ? encoder.__enc : "hex";
    if (kind === "base64") return bytesToBase64(bytes);
    if (kind === "utf8") return bytesToStr(bytes);
    return bytesToHex(bytes);
  };
  WordArray.prototype.concat = function (other) {
    var bytes = wordArrayBytes(this).concat(wordArrayBytes(other));
    return bytesToWordArray(bytes);
  };
  WordArray.prototype.clone = function () {
    return bytesToWordArray(wordArrayBytes(this));
  };
  WordArray.prototype.__valueOf = function () {
    return this.toString();
  };

  var encHex = { __enc: "hex", parse: function (text) { return bytesToWordArray(hexToBytes(String(text))); }, stringify: function (wa) { return bytesToHex(wordArrayBytes(wa)); } };
  var encBase64 = { __enc: "base64", parse: function (text) { return bytesToWordArray(base64ToBytes(String(text))); }, stringify: function (wa) { return bytesToBase64(wordArrayBytes(wa)); } };
  var encUtf8 = { __enc: "utf8", parse: function (text) { return bytesToWordArray(strToBytes(String(text))); }, stringify: function (wa) { return bytesToStr(wordArrayBytes(wa)); } };
  var encLatin1 = { __enc: "latin1", parse: function (text) { return bytesToWordArray(String(text).split("").map(function (c) { return c.charCodeAt(0) & 0xff; })); }, stringify: function (wa) { return wordArrayBytes(wa).map(function (b) { return String.fromCharCode(b); }).join(""); } };

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

  function toBytes(value, asString) {
    if (value instanceof WordArray) return wordArrayBytes(value);
    if (typeof value === "string") return asString ? strToBytes(value) : strToBytes(value);
    if (typeof value === "number") return [value & 0xff];
    return strToBytes(String(value));
  }

  function toWordArray(value) {
    return value instanceof WordArray ? value : bytesToWordArray(toBytes(value, true));
  }

  function CipherParams(base64, bytes) {
    this.ciphertext = bytesToWordArray(bytes);
    this.__b64 = base64;
    this.__bytes = bytes;
  }
  CipherParams.prototype.toString = function (encoder) {
    if (encoder && encoder.__enc) return this.ciphertext.toString(encoder);
    return this.__b64;
  };
  CipherParams.prototype.__valueOf = function () {
    return this.__b64;
  };

  function aesProcess(op, input, key, cfg) {
    cfg = cfg || {};
    var mode = cfg.mode && cfg.mode.__mode ? cfg.mode.__mode : "cbc";
    var pad = cfg.padding && cfg.padding.__pad ? cfg.padding.__pad : "pkcs7";
    var keyBytes = toBytes(key, true);
    var ivValue = cfg.iv;
    var ivBytes = ivValue === undefined || ivValue === null ? [] : toBytes(ivValue, true);
    if (mode === "ecb") ivBytes = [];

    var dataBytes;
    if (op === "encrypt") {
      dataBytes = toBytes(input, true);
      if (pad === "none" && dataBytes.length % 16 !== 0) {
        throw new Error("AES 无填充模式下数据长度必须是 16 的倍数");
      }
    } else {
      if (input instanceof CipherParams) dataBytes = input.__bytes;
      else if (input && input.ciphertext instanceof WordArray) dataBytes = wordArrayBytes(input.ciphertext);
      else if (input instanceof WordArray) dataBytes = wordArrayBytes(input);
      else dataBytes = base64ToBytes(String(input));
    }

    var raw = __aes(op, bytesToBase64(dataBytes), bytesToBase64(keyBytes), bytesToBase64(ivBytes), pad);
    var resp = JSON.parse(raw);
    if (resp.error) throw new Error("AES 失败: " + resp.error);
    var out = base64ToBytes(resp.data);
    if (op === "encrypt") return new CipherParams(resp.data, out);
    return bytesToWordArray(out);
  }

  var CryptoJS = {
    lib: { WordArray: { create: function (words, sigBytes) { return new WordArray(words, sigBytes); } } },
    enc: { Hex: encHex, Base64: encBase64, Utf8: encUtf8, Latin1: encLatin1 },
    mode: { CBC: { __mode: "cbc" }, ECB: { __mode: "ecb" }, CFB: { __mode: "cbc" }, OFB: { __mode: "cbc" }, CTR: { __mode: "cbc" } },
    pad: { Pkcs7: { __pad: "pkcs7" }, NoPadding: { __pad: "none" }, ZeroPadding: { __pad: "pkcs7" }, AnsiX923: { __pad: "pkcs7" }, Iso10126: { __pad: "pkcs7" }, Iso97971: { __pad: "pkcs7" } },
    AES: {
      encrypt: function (message, key, cfg) { return aesProcess("encrypt", message, key, cfg); },
      decrypt: function (cipher, key, cfg) { return aesProcess("decrypt", cipher, key, cfg); },
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
  dayjs.default = dayjs;

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
  function toBigInt(value, radix) {
    if (value instanceof BigInteger) return value.value;
    if (typeof value === "bigint") return value;
    if (typeof value === "number") return BigInt(Math.floor(value));
    var text = String(value).trim();
    var base = 10;
    if (typeof radix === "number" && radix > 0) base = radix;
    else if (typeof radix === "string") {
      var named = { hex: 16, hexadecimal: 16, binary: 2, bin: 2, octal: 8, decimal: 10 };
      base = named[radix] || (parseInt(radix, 10) > 0 ? parseInt(radix, 10) : 10);
    }
    if (base === 10 && text.toLowerCase().indexOf("0x") === 0) {
      base = 16;
      text = text.slice(2);
    }
    if (base === 16) return BigInt("0x" + (text === "" ? "0" : text));
    if (base === 2) return BigInt("0b" + (text === "" ? "0" : text));
    if (base === 8) return BigInt("0o" + (text === "" ? "0" : text));
    if (text === "" || text === "0") return BigInt(0);
    if (base === 10) return BigInt(text);
    // 其它进制手写转换
    var result = BigInt(0);
    var big = BigInt(base);
    for (var i = 0; i < text.length; i++) {
      result = result * big + BigInt(parseInt(text.charAt(i), base));
    }
    return result;
  }

  function BigInteger(value, radix) {
    this.value = toBigInt(value, radix);
  }
  BigInteger.prototype.toString = function (radix) {
    var base = typeof radix === "number" && radix > 1 ? radix : 10;
    return this.value.toString(base);
  };
  BigInteger.prototype.valueOf = function () {
    return Number(this.value);
  };
  BigInteger.prototype.toJSNumber = function () {
    return Number(this.value);
  };
  BigInteger.prototype.add = function (other) {
    return new BigInteger(this.value + toBigInt(other));
  };
  BigInteger.prototype.plus = BigInteger.prototype.add;
  BigInteger.prototype.subtract = function (other) {
    return new BigInteger(this.value - toBigInt(other));
  };
  BigInteger.prototype.minus = BigInteger.prototype.subtract;
  BigInteger.prototype.multiply = function (other) {
    return new BigInteger(this.value * toBigInt(other));
  };
  BigInteger.prototype.times = BigInteger.prototype.multiply;
  BigInteger.prototype.divide = function (other) {
    return new BigInteger(this.value / toBigInt(other));
  };
  BigInteger.prototype.mod = function (other) {
    return new BigInteger(this.value % toBigInt(other));
  };
  BigInteger.prototype.pow = function (exp) {
    return new BigInteger(this.value ** toBigInt(exp));
  };
  /** 模幂：先算平方再乘，指数大也不至于爆炸 */
  BigInteger.prototype.modPow = function (exp, mod) {
    var e = toBigInt(exp);
    var m = toBigInt(mod);
    if (m === BigInt(0)) throw new Error("modPow: 模数不能为 0");
    var base = ((this.value % m) + m) % m;
    var result = BigInt(1);
    while (e > BigInt(0)) {
      if (e & BigInt(1)) result = (result * base) % m;
      e = e >> BigInt(1);
      base = (base * base) % m;
    }
    return new BigInteger(result);
  };
  BigInteger.prototype.compare = function (other) {
    var a = this.value;
    var b = toBigInt(other);
    return a === b ? 0 : a > b ? 1 : -1;
  };
  BigInteger.prototype.compareTo = BigInteger.prototype.compare;
  BigInteger.prototype.equals = function (other) {
    return this.value === toBigInt(other);
  };
  BigInteger.prototype.eq = BigInteger.prototype.equals;
  BigInteger.prototype.greater = function (other) {
    return this.value > toBigInt(other);
  };
  BigInteger.prototype.gt = BigInteger.prototype.greater;
  BigInteger.prototype.lesser = function (other) {
    return this.value < toBigInt(other);
  };
  BigInteger.prototype.lt = BigInteger.prototype.lesser;
  BigInteger.prototype.isZero = function () {
    return this.value === BigInt(0);
  };
  BigInteger.prototype.isNegative = function () {
    return this.value < BigInt(0);
  };
  BigInteger.prototype.negate = function () {
    return new BigInteger(-this.value);
  };
  BigInteger.prototype.abs = function () {
    return new BigInteger(this.value < BigInt(0) ? -this.value : this.value);
  };
  BigInteger.prototype.and = function (other) {
    return new BigInteger(this.value & toBigInt(other));
  };
  BigInteger.prototype.or = function (other) {
    return new BigInteger(this.value | toBigInt(other));
  };
  BigInteger.prototype.xor = function (other) {
    return new BigInteger(this.value ^ toBigInt(other));
  };
  BigInteger.prototype.not = function () {
    return new BigInteger(~this.value);
  };
  BigInteger.prototype.shiftLeft = function (n) {
    return new BigInteger(this.value << toBigInt(n));
  };
  BigInteger.prototype.shiftRight = function (n) {
    return new BigInteger(this.value >> toBigInt(n));
  };
  BigInteger.prototype.gcd = function (other) {
    var a = this.value < BigInt(0) ? -this.value : this.value;
    var b = toBigInt(other);
    b = b < BigInt(0) ? -b : b;
    while (b !== BigInt(0)) {
      var t = b;
      b = a % b;
      a = t;
    }
    return new BigInteger(a);
  };

  function bigInt(value, radix) {
    return new BigInteger(value, radix);
  }
  bigInt.zero = new BigInteger(0);
  bigInt.one = new BigInteger(1);
  bigInt.minusOne = new BigInteger(-1);

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
