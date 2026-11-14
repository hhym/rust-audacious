// MyGo 插件的宿主环境（由 mygo.rs 在插件代码之前执行）
//
// 对应 https://github.com/hhym/mygo-music 的插件约定：插件目录里一个
// plugin.json + 一个 JS/TS 模块，模块按名字导出 search / stream / lyrics 等方法，
// 每个方法接收一个对象参数。这里把 mygo 的 `host`、`fetch`、定时器和 URL 补齐。

(function () {
  var hostInfo = JSON.parse(__mygoInfo || "{}");

  // ------------------------------------------------------------ 工具
  function nowMs() {
    return Date.now();
  }

  // ------------------------------------------------------------ URL / URLSearchParams
  function UrlSearchParams(init) {
    this.__entries = [];
    if (typeof init === "string") {
      var s = init.charAt(0) === "?" ? init.slice(1) : init;
      if (s) {
        s.split("&").forEach(
          function (pair) {
            if (!pair) return;
            var i = pair.indexOf("=");
            var k = i < 0 ? pair : pair.slice(0, i);
            var v = i < 0 ? "" : pair.slice(i + 1);
            this.__entries.push([decodeURIComponent(k.replace(/\+/g, " ")), decodeURIComponent(v.replace(/\+/g, " "))]);
          }.bind(this)
        );
      }
    } else if (init && typeof init === "object") {
      if (init instanceof UrlSearchParams) this.__entries = init.__entries.slice();
      else Object.keys(init).forEach(function (k) { this.__entries.push([k, String(init[k])]); }, this);
    }
  }
  UrlSearchParams.prototype.append = function (k, v) { this.__entries.push([String(k), String(v)]); };
  UrlSearchParams.prototype.set = function (k, v) {
    this["delete"](k);
    this.append(k, v);
  };
  UrlSearchParams.prototype.get = function (k) {
    var found = this.__entries.filter(function (e) { return e[0] === String(k); })[0];
    return found ? found[1] : null;
  };
  UrlSearchParams.prototype.getAll = function (k) {
    return this.__entries.filter(function (e) { return e[0] === String(k); }).map(function (e) { return e[1]; });
  };
  UrlSearchParams.prototype.has = function (k) {
    return this.__entries.some(function (e) { return e[0] === String(k); });
  };
  UrlSearchParams.prototype["delete"] = function (k) {
    this.__entries = this.__entries.filter(function (e) { return e[0] !== String(k); });
  };
  UrlSearchParams.prototype.toString = function () {
    return this.__entries
      .map(function (e) { return encodeURIComponent(e[0]) + "=" + encodeURIComponent(e[1]); })
      .join("&");
  };
  UrlSearchParams.prototype.forEach = function (fn) {
    this.__entries.forEach(function (e) { fn(e[1], e[0], this); }, this);
  };
  UrlSearchParams.prototype[Symbol.iterator] = function () {
    return this.__entries[Symbol.iterator]();
  };

  function URL(url, base) {
    var text = String(url);
    if (base && !/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(text)) {
      var baseText = String(base);
      var origin = baseText.replace(/^([a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^/?#]*).*$/, "$1");
      if (text.charAt(0) === "/") text = origin + text;
      else text = origin + baseText.slice(origin.length).replace(/[^/]*$/, "") + text;
    }
    var m = /^([a-zA-Z][a-zA-Z0-9+.-]*:)(\/\/)?([^/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/.exec(text);
    if (!m) throw new Error("Invalid URL: " + text);
    this.protocol = m[1];
    this.host = m[3] || "";
    this.hostname = this.host.replace(/:\d+$/, "");
    this.port = (/:(\d+)$/.exec(this.host) || ["", ""])[1];
    this.pathname = m[4] || "/";
    this.search = m[5] || "";
    this.hash = m[6] || "";
    this.origin = m[3] ? m[1] + "//" + this.host : "null";
    this.searchParams = new UrlSearchParams(this.search);
  }
  URL.prototype.toString = function () {
    var search = this.searchParams.toString();
    return this.protocol + "//" + this.host + this.pathname + (search ? "?" + search : "") + this.hash;
  };
  URL.prototype.toJSON = URL.prototype.toString;

  // ------------------------------------------------------------ 定时器
  // 没有事件循环，定时器交给宿主每次 pump 时执行；只支持"到点就跑一次"。
  var timers = [];
  var timerSeq = 1;
  function setTimeout(fn, delay) {
    var args = Array.prototype.slice.call(arguments, 2);
    var id = timerSeq++;
    timers.push({ id: id, at: nowMs() + (Number(delay) || 0), fn: fn, args: args, repeat: 0 });
    return id;
  }
  function setInterval(fn, delay) {
    var args = Array.prototype.slice.call(arguments, 2);
    var id = timerSeq++;
    timers.push({ id: id, at: nowMs() + (Number(delay) || 1), fn: fn, args: args, repeat: Math.max(1, Number(delay) || 1) });
    return id;
  }
  function clearTimeout(id) {
    timers = timers.filter(function (t) { return t.id !== id; });
  }
  function runTimers() {
    var due = timers.filter(function (t) { return t.at <= nowMs(); });
    timers = timers.filter(function (t) { return t.at > nowMs(); });
    due.forEach(function (t) {
      try {
        if (typeof t.fn === "function") t.fn.apply(null, t.args);
      } catch (e) {
        __log("定时器回调出错: " + (e && e.message ? e.message : String(e)));
      }
      if (t.repeat) timers.push({ id: t.id, at: nowMs() + t.repeat, fn: t.fn, args: t.args, repeat: t.repeat });
    });
  }
  var pendingTimers = function () { return timers.length > 0; };

  // ------------------------------------------------------------ fetch（带域名白名单）
  var allowed = (hostInfo.network || []).slice();
  function hostAllowed(hostName) {
    if (allowed.indexOf("*") >= 0) return true;
    var h = String(hostName || "").toLowerCase();
    return allowed.some(function (pattern) {
      var p = String(pattern).toLowerCase();
      if (p.indexOf("*.") === 0) {
        var suffix = p.slice(1);
        return h === p.slice(2) || h.slice(-suffix.length) === suffix;
      }
      return h === p;
    });
  }

  function Headers(init) {
    this.__map = {};
    var self = this;
    if (init) {
      if (typeof init.forEach === "function" && !(init instanceof Array)) {
        init.forEach(function (v, k) { self.__map[String(k).toLowerCase()] = String(v); });
      } else {
        Object.keys(init).forEach(function (k) { self.__map[k.toLowerCase()] = String(init[k]); });
      }
    }
  }
  Headers.prototype.get = function (k) {
    var v = this.__map[String(k).toLowerCase()];
    return v === undefined ? null : v;
  };
  Headers.prototype.set = function (k, v) { this.__map[String(k).toLowerCase()] = String(v); };
  Headers.prototype.has = function (k) { return this.__map[String(k).toLowerCase()] !== undefined; };
  Headers.prototype.forEach = function (fn) {
    Object.keys(this.__map).forEach(function (k) { fn(this.__map[k], k, this); }, this);
  };

  function ResponseBody(resp) {
    this.status = resp.status || 0;
    this.ok = this.status >= 200 && this.status < 300;
    this.statusText = "";
    this.url = resp.url || "";
    this.headers = new Headers(resp.headers || {});
    this.__body = resp.body === undefined || resp.body === null ? "" : String(resp.body);
    this.bodyUsed = false;
  }
  ResponseBody.prototype.text = function () {
    this.bodyUsed = true;
    return Promise.resolve(this.__body);
  };
  ResponseBody.prototype.json = function () {
    this.bodyUsed = true;
    try {
      return Promise.resolve(JSON.parse(this.__body));
    } catch (e) {
      return Promise.reject(e);
    }
  };
  ResponseBody.prototype.arrayBuffer = function () {
    return Promise.reject(new Error("这版宿主不支持 arrayBuffer，请用 text()/json()"));
  };
  ResponseBody.prototype.clone = function () {
    var copy = new ResponseBody({ status: this.status, headers: {}, body: this.__body, url: this.url });
    copy.headers = this.headers;
    return copy;
  };

  function fetch(input, init) {
    init = init || {};
    var urlText = typeof input === "string" ? input : input && input.url ? String(input.url) : String(input);
    var parsed;
    try {
      parsed = new URL(urlText);
    } catch (e) {
      return Promise.reject(new TypeError("fetch: 无效的 URL " + urlText));
    }
    if (!hostAllowed(parsed.hostname)) {
      return Promise.reject(
        new TypeError(
          "fetch: 网络权限没有允许 " + parsed.hostname + "（在 plugin.json 的 permissions.network 里加上它）"
        )
      );
    }

    var headers = init.headers || {};
    var merged = {};
    if (headers instanceof Headers) headers.forEach(function (v, k) { merged[k] = v; });
    else if (typeof headers.forEach === "function" && !(headers instanceof Array)) headers.forEach(function (v, k) { merged[k] = v; });
    else Object.keys(headers).forEach(function (k) { merged[k] = String(headers[k]); });

    var payload = { method: init.method || "GET", url: urlText, headers: merged };
    if (init.body !== undefined && init.body !== null) {
      payload.body = typeof init.body === "string" ? init.body : JSON.stringify(init.body);
    }
    if (init.timeout) payload.timeout = init.timeout;

    var raw = __http(JSON.stringify(payload));
    var resp = JSON.parse(raw);
    if (resp.error) return Promise.reject(new TypeError("fetch: " + resp.error));
    return Promise.resolve(new ResponseBody(resp));
  }

  // ------------------------------------------------------------ host
  var dataDir = hostInfo.dataDir || hostInfo.dir || ".";
  var host = {
    platform: hostInfo.platform || "linux",
    arch: hostInfo.arch || "x86_64",
    dir: hostInfo.dir || ".",
    dataDir: dataDir,
    language: function () {
      return hostInfo.language || "zh-CN";
    },
    t: function (text) {
      var table = hostInfo.locale || {};
      return Object.prototype.hasOwnProperty.call(table, text) ? String(table[text]) : String(text);
    },
    setting: function (key) {
      var v = __storage("mygo/" + hostInfo.id + "/setting/" + key, null);
      if (v === null || v === undefined) return undefined;
      try {
        return JSON.parse(v);
      } catch (e) {
        return String(v);
      }
    },
    storage: {
      get: function (key) {
        var v = __storage("mygo/" + hostInfo.id + "/" + key, null);
        if (v === null || v === undefined) return null;
        try {
          return JSON.parse(v);
        } catch (e) {
          return String(v);
        }
      },
      set: function (key, value) {
        __storage("mygo/" + hostInfo.id + "/" + key, JSON.stringify(value === undefined ? null : value));
      },
      remove: function (key) {
        __storage("mygo/" + hostInfo.id + "/" + key, JSON.stringify(null));
      },
    },
    crypto: {
      md5: function (text) {
        return JSON.parse(__hash("md5", String(text), null)).hex;
      },
      aesEcbEncrypt: function (key, text) {
        // 和 mygo 一样：AES-ECB + PKCS#7，输出十六进制
        var data = CryptoJS.enc.Base64.stringify(CryptoJS.enc.Utf8.parse(String(text)));
        var keyB64 = CryptoJS.enc.Base64.stringify(CryptoJS.enc.Utf8.parse(String(key)));
        var resp = JSON.parse(__aes("encrypt", data, keyB64, "", "pkcs7"));
        if (resp.error) throw new Error("AES 失败: " + resp.error);
        return CryptoJS.enc.Hex.stringify(CryptoJS.enc.Base64.parse(resp.data));
      },
    },
    openURL: function (url) {
      __open(String(url));
    },
    removeDataDir: function (name) {
      __log("host.removeDataDir 这版未实现: " + name);
    },
    signInPrompt: function () {
      throw new Error("这版宿主不支持 host.signInPrompt（登录窗口）");
    },
    waitForRedirect: function () {
      throw new Error("这版宿主不支持 host.waitForRedirect（浏览器回跳）");
    },
    webSignIn: function () {
      throw new Error("这版宿主不支持 host.webSignIn（网页登录）");
    },
    spawn: function () {
      throw new Error("这版宿主不支持 host.spawn（启动外部程序）");
    },
  };

  globalThis.host = host;
  globalThis.fetch = fetch;
  globalThis.Headers = Headers;
  globalThis.URL = URL;
  globalThis.URLSearchParams = UrlSearchParams;
  globalThis.setTimeout = setTimeout;
  globalThis.setInterval = setInterval;
  globalThis.clearTimeout = clearTimeout;
  globalThis.clearInterval = clearTimeout;
  globalThis.__runTimers = runTimers;
  globalThis.__hasTimers = pendingTimers;
  if (typeof globalThis.crypto === "undefined") {
    globalThis.crypto = { getRandomValues: function (arr) { for (var i = 0; i < arr.length; i++) arr[i] = Math.floor(Math.random() * 256); return arr; } };
  }
  if (typeof globalThis.structuredClone === "undefined") {
    globalThis.structuredClone = function (value) { return JSON.parse(JSON.stringify(value)); };
  }
})();
