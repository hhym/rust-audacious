# 在线插件（MusicFree 标准 / MyGo 标准）

`rust-audacious` 的「在线插件」同时支持两套插件标准：

- **MusicFree**：[MusicFreePlugins](https://github.com/hhym/MusicFreePlugins) —— 单个 `.js` 文件；
- **MyGo**：[mygo-music](https://github.com/hhym/mygo-music) —— 一个目录 + `plugin.json` + ES 模块。

两者在「插件」窗口和「在线搜索」窗口里统一展示，名字后面带 `· MusicFree` / `· MyGo`。

## MusicFree 标准

`rust-audacious` 的「在线插件」直接兼容 [MusicFreePlugins](https://github.com/hhym/MusicFreePlugins)
的插件标准：一个 `.js` 文件，`module.exports = { platform, version, search, getMediaSource, getLyric, ... }`。
接口定义见该仓库的 `types/plugin.d.ts`。

宿主实现在 `crates/audplugins`：用纯 Rust 的 [boa_engine](https://github.com/boa-dev/boa) 跑 JS，
重活交给 Rust 原生函数（HTTP / 哈希 / 存储），库依靠 `src/polyfill.js` 提供等价实现。

## 安装

把插件 `.js` 放到：

```
Linux/macOS： ~/.config/audacious-rs/plugins/
Windows：     %APPDATA%\audacious-rs\plugins\
```

然后在「插件」窗口底部的「在线插件（MusicFree 标准）」里点「重新扫描」，
或者「打开目录」直接跳到该文件夹。插件列表会显示 `platform`、版本号和它实现了哪些方法。

仓库自带一个可直接用的示例：[`examples/plugins/itunes.js`](examples/plugins/itunes.js)（iTunes 公开搜索接口，无需 key）。

## 支持的接口

| 方法 | 签名 | 说明 |
|---|---|---|
| `search` | `(query, page, type) => { isEnd?, data: MusicItem[] }` | 当前搜索窗口固定传 `type="music"` |
| `getMediaSource` | `(item, quality) => { url, headers?, userAgent? }` | 取播放地址，`quality` 传 `"standard"` |
| `getLyric` | `(item) => { rawLrc, lrc? }` | 宿主已封装，界面暂未接歌词面板 |
| 其他 | `getMusicInfo` / `getAlbumInfo` / `getArtistWorks` / `getTopLists` / `getTopListDetail` / `importMusicSheet` / `importMusicItem` | 可以写在插件里，宿主会识别但不能用界面触发（路线图） |

歌曲对象按标准字段：`id`（必需）、`title`、`artist`、`album`、`artwork`、`duration`（秒）、`url`、`lrc`。

## 内置库（`require` 可直接用）

| 名字 | 覆盖范围 |
|---|---|
| `axios` | `axios(config)` / `axios.get/post/put/patch/delete/head/options`，`params`、`headers`、`data`、`timeout`、`responseType:"text"`；返回 Promise，`data` 自动按 JSON 解析 |
| `cheerio` | `load(html)` + 选择器 `tag` `*` `.class` `#id` `[attr]` `[attr=v]` 后代 `>` 逗号分组；集合支持 `each/eq/first/last/find/parent/children/filter/text/html/attr/data/val/hasClass/is/remove` |
| `crypto-js` | `MD5` `SHA1` `SHA256`、`HmacSHA1` `HmacSHA256` `HmacMD5`；`enc.Hex` `enc.Base64` `enc.Utf8` `enc.Latin1`；`lib.WordArray`；`AES.encrypt/decrypt`（CBC / ECB，PKCS7 或无填充，密钥 128/192/256 位）；`mode` / `pad` 常量 |
| `dayjs` | `dayjs()` / `dayjs.unix()`，`format` `valueOf` `unix` `add` `subtract` `toDate` 等常用子集 |
| `qs` | `stringify` / `parse` |
| `he` | `decode` / `encode` / `decodeHTML` / `encodeHTML` |
| `big-integer` | 基于 JS `BigInt` 的完整实现：`bigInt(value, radix)`、`add/subtract/multiply/divide/mod`、`modPow`（网易 RSA 加密用）、`pow/gcd`、`compare`、位运算、`toString(radix)` |
| `musicfree/storage` | `getItem` / `setItem` / `removeItem`，落到 `<config>/audacious-rs/plugin-storage.json` |

`crypto-js` 的 AES 由 Rust 侧 `aes` crate 完成（`__aes`），WordArray 的 UTF-8 / Hex / Base64 编解码在 JS 里手写，
`CipherParams.toString()` 默认输出 Base64 —— 和真 CryptoJS 的用法保持一致（网易系插件的 `AES(text, key)` 直接返回可用密文）。

`require()` 返回的模块同时挂了一个指向自身的 `default`（`axios.default === axios` 等），
兼容 TS 编译产物里的 `axios_1.default(...)` 调用方式。

另外注入了 `env`（`os` / `appVersion` / `lang` / `getUserVariables`）、`process`（`platform` / `version` / `nextTick`）、
`console`、`module`/`exports`、`require`/`__musicfree_require`。

## 实现要点

- **隔离**：每次调用方法都新建一个 `boa_engine::Context` 并重新执行插件代码，
  插件之间的全局变量不会串；同一个插件连续搜索也不会带上一次的状态。
- **同步 HTTP + Promise**：`axios` 返回真正的 Promise，内部用阻塞的 `__http` 拿结果，
  调用方在处理完微任务队列后取回 `__invokeResult`（最多 64 轮，通常一两轮就够）。
- **TLS**：`reqwest` 用系统证书库（`native-tls`），这样企业代理 / 自签根证书环境也能用。
- **在线播放**：`getMediaSource` 拿到的 `url` 会先下载到 `<cache>/audacious-rs/streams/`
  （按 URL 的 MD5 命名），再交给解码器；播放列表里显示为「在线」条目，进度/频谱/切歌与本地文件一致。
- **解码器**：不再使用 `rodio::Decoder`（它把输入包成 `byte_len()` 未知的适配器，
  M4A 在探测阶段就会触发 symphonia `SeekError` 崩溃），改用 `crates/audacious/src/engine.rs`
  里的 `SymSource` —— 直接以 `File` 构造 `MediaSourceStream`，自己实现 `Source` 与 `try_seek`。

## MyGo 标准

[mygo-music](https://github.com/hhym/mygo-music) 的插件是**一个目录**，放在插件目录下：

```
<插件目录>/my-plugin/
  plugin.json
  index.js
  locales/zh-CN.json   # 可选，翻译插件自己显示的文字
```

`plugin.json`：

```json
{
  "id": "example",
  "name": "Example",
  "version": "1.0.0",
  "minAppVersion": "0.1.8",
  "main": "index.js",
  "playback": "stream",
  "permissions": { "network": ["api.example.com", "*.example.com"] },
  "settings": [{ "key": "apiKey", "label": "API Key", "help": "在你的账号页面获取" }],
  "navigation": [{ "builtin": "home" }, { "title": "Charts", "page": "charts" }]
}
```

`index.js` 是 ES 模块，按名字导出方法，每个方法接收**一个对象参数**：

```js
export async function search({ query, kind, offset }) {
  const res = await fetch(`https://api.example.com/search?q=${encodeURIComponent(query)}`, {
    headers: { authorization: `Bearer ${host.setting("apiKey")}` },
  });
  return { tracks: (await res.json()).items.map(toTrack), more: false };
}

export function stream({ id, quality }) {
  return { url: `https://api.example.com/audio/${id}.mp3`, type: "mp3" };
}

export function lyrics({ id }) {
  return { lrc: "..." };
}
```

宿主把 `search` / `stream` / `lyrics` 映射成界面上的「搜索 / 取播放地址 / 歌词」，
`search` 只走 `kind: "tracks"`；`stream` 返回的 `url` 直接进播放器（`type` 目前只用于记录）。

### 数据结构（和 mygo 一致）

- `Track`：`{ id, name, artists: [{ id, name, imageUrl? }], album: { id, name, imageUrl? }, duration }`，
  `duration` 单位是**毫秒**；宿主展示时换算成秒。
- `stream` 返回 `{ url, type, headers?, size?, gain?, peak? }`，`type` 为 `mp3` / `flac` / `ogg` / `wav` / `m4a` / `aac`。
- `lyrics` 返回 `{ lrc, translation? }` 或 `{ lines }`。

### 宿主提供的全局

| | |
|---|---|
| `fetch(url, { method, headers, body, timeout })` | 受 `permissions.network` 白名单限制，域名不在名单里直接抛错 |
| `host.platform` / `host.arch` | 如 `linux` / `windows`、`x86_64` |
| `host.dir` / `host.dataDir` | 插件目录 / `<config>/audacious-rs/mygo-data/<id>` |
| `host.storage.get/set/remove(key)` | JSON 值，跨重启保存（存在 `plugin-storage.json`） |
| `host.setting(key)` | 在「插件」窗口里填的设置项（`plugin.json` 的 `settings`） |
| `host.t(text)` / `host.language()` | 读 `locales/<语言>.json`，当前语言 `zh-CN` / `en` |
| `host.crypto.md5(text)` | 十六进制摘要 |
| `host.crypto.aesEcbEncrypt(key, text)` | AES-ECB + PKCS#7，输出十六进制 |
| `host.openURL(url)` | 用系统浏览器打开 |
| `host.signInPrompt({ qr, text })` | 把二维码内容推给宿主的扫码登录窗（宿主本地编码成二维码并显示），立即返回 |
| `host.spawn(command, args)` | 拉起插件自带的 helper，返回 `{ write, readLine, kill, closeStdin, wait }`；命令必须在 `permissions.process` 白名单里（支持 `bin/qishui-helper-*` 这种通配） |
| `URL` / `URLSearchParams` / `setTimeout` / `setInterval` / `console` | 常规 JS 环境 |
| `AbortController` / `AbortSignal` | 插件做请求超时/取消时用；宿主是同步 HTTP，`abort()` 只能拦住「还没发起」的请求，但 `signal.aborted` / `reason` / `addEventListener` / `AbortSignal.abort()` / `AbortSignal.timeout()` 都可用 |

`plugin.json` 的 `settings` 会直接显示在「插件」窗口每个插件下面，失焦即保存，
可以声明 `"type": "password"` 以掩码输入。

### ES 模块的处理方式

宿主不是真正的 Node/ESM 运行时：插件代码会被改写后执行 ——
`import x from "m"` → `require("m")`，去掉 `export` 关键字，再按导出的名字收集函数。
`require()` 能拿到 MusicFree 那一套库（axios / crypto-js / big-integer / cheerio …），
所以 MyGo 插件也能复用它们。

### 示例

仓库里有一个开箱可用的例子：[`examples/mygo-plugins/itunes`](examples/mygo-plugins/itunes)（iTunes 公开搜索，无需 key）。
把它整个目录复制到插件目录，点「重新扫描」即可。

## 扫描与缓存

「重新扫描」不会再卡住界面，做法是三条：

1. **扫描不执行插件代码**。只对源码做语法检查（boa 的解析器，毫秒级），
   `platform` / `version` / 方法名直接从 `module.exports` 附近的字面量里读。
   混淆过的插件把字符串藏在解码后的数组里，读不到就先拿文件名当名字、
   默认按「支持 search / getMediaSource / getLyric」处理 —— 真正执行推迟到第一次搜索。
2. **扫描在后台线程 + 每插件 4 秒兜底**。真的需要执行时（静态读不出来）会放进独立线程，
   超时记一条「插件在加载阶段卡住」并跳过，主界面始终保持可点。
3. **结果写进缓存** `<config>/audacious-rs/plugin-cache.json`，键是
   「路径 + 修改时间 + 大小」。文件不改就直接复用，76 个插件第二次扫描 24 ms 左右。
   改了文件缓存自然失效，点「重新扫描」即可。

语法错误的插件会显示成中文说明，例如：

```
酷我音乐Thomas · MusicFree
插件源码有语法错误（第 137 行）：同名变量在同一作用域里重复声明，该插件在原客户端也会报错
```

这类插件是真的编译不过（`function R(x, z) { let t, z = z || ... }` —— 参数和 `let` 同名），
不是宿主的问题。

## 已知边界

- 没有真 Node 语义：`setTimeout` / 事件循环 / `Buffer` / 流式响应 / `dayjs.extend` 未实现；
  `webdav`、`@react-native-cookies/cookies` 只给了 stub。
- 需要登录态或加密签名的插件可能跑不通（缺 cookie 容器与自定义 UA 配置）。
- 网易系插件已实测：`search` 能拿到真实结果；部分第三方中转 API 会限流（返回 429），
  这类错误来自插件自己的接口，不是宿主问题。
- 插件异常会在「在线搜索」窗口里显示错误文本（含调用栈），便于定位。

## 主面板怎么用这两套插件

主面板五个标签对两套插件是同一套界面，插件在右上角「来源」里切换：

| 标签 | 数据来源 | 说明 |
|---|---|---|
| 搜索 | `search`（MusicFree）/ `search`（MyGo） | 结果统一成 `{title, artist, album, duration}`，▶ 取播放地址、＋ 加入列表、☆ 加入喜欢 |
| 首页 | `home`（MyGo）/ `getTopLists`（MusicFree） | 插件没有对应导出时显示「没有首页接口」，不再退回本地介绍页 |
| 歌曲 | 本地 `Playlist` | 顶部「全选 / 取消选择 / 删除所选」，Ctrl 点行多选、Shift 点行连选 |
| 喜欢 | `liked.json`，每条记 `{plugin, item}` | 两套插件的条目都能回放；页面底部同样显示当前插件的主页 |
| 播放列表 | 本地 `playlists/*.m3u` | 与插件无关 |

### 首页数据约定

MyGo 的 `home()` 返回一个 Browse：

```js
export async function home() {
  return {
    title: "首页标题",
    subtitle: "一行说明",
    sections: [
      {
        title: "编辑推荐",                 // 段落标题
        tracks: [                          // 渲染成可播放的行
          { id, name, artists: [{ name }], album: { name }, duration },
        ],
        playlists: [{ id, name, imageUrl }],  // 渲染成卡片
        albums: [{ id, name, imageUrl }],
      },
    ],
  };
}
```

MusicFree 的 `getTopLists()` 返回 `[{ title, data: [{ id, title, coverImg }] }]`，
宿主把每个 list 当一段、`data` 里的条目渲染成卡片。

卡片目前只展示标题，点开进歌单（`getTopListDetail` / `show`）还没接，属于下一步。

### 扫码登录（MyGo 扩展，可选导出）

除了标准的 `search` / `stream` / `lyrics`，插件还可以导出两个方法，宿主会显示「扫码登录」按钮：

```js
// 生成一张新二维码
export async function loginQr() {
  // 用插件自己的接口拿到二维码图片 / 二维码里的链接
  return {
    key: "会话标识",                        // 必填（轮询时原样传回）
    qrImage: "data:image/png;base64,...",  // 图片 URL 或 data URL，宿主会下载并显示
    qrUrl: "https://...",                  // 二维码里编的链接（没有图片时也会显示出来）
    tip: "用 B 站 App 扫码",
  };
}

// 轮询状态，宿主每 2 秒调一次
export async function loginPoll({ key }) {
  return { status: "waiting" }; // waiting | scanned | done | expired | failed
}
```

约定：

- 登录态由插件自己保存（`host.storage.set("session", ...)` 或 `settings`），宿主不接触账号密码，也不存 cookie 容器；
- 宿主只负责：拉二维码 → 解码 PNG/JPEG（也接受 http 图片地址，会走宿主网络）→ 显示 → 每 2 秒轮询 → 成功时提示；
- `status` 为 `done` 后窗口出现「完成」按钮，`expired` 提示刷新二维码；
- 如果插件自己就能跑完整个登录（例如 QQ Music 0.3.0 的 `signIn()`：自己拉二维码 → 轮询 ptqrlogin → 换 cookie），
  直接 `export async function signIn()` 即可，登录过程中调用 `host.signInPrompt({ qr, text })` 把二维码交给宿主显示，
  宿主会开一个「登录流程由插件自己驱动」的窗口，方法返回后窗口显示「登录成功」。

`examples/mygo-plugins/qr-demo/` 是一个不联网的演示插件（点「刷新二维码」会重新生成，4 次轮询后进入 `done`）。

### MyGo 标准的限制

- 没有真正的 ESM 语义：`import` 被改写成 `require`，动态 `import()`、顶层 `await` 不支持。
- `host.waitForRedirect` / `host.webSignIn`（浏览器回跳登录）还没实现，调用会抛错。
- `host.spawn` 只允许 `permissions.process` 里声明过的程序：相对路径会被限制在插件目录内，
  没声明的命令直接拒绝；子进程的 stdin/stdout 走 JSON 行协议，helper 由插件包自己带（汽水音乐 1.0.12 的
  `bin/qishui-helper-windows-amd64.exe` 属于这种情况，Linux 包缺该二进制时会得到「请允许插件启动程序」提示）。
- `host.signInPrompt` 只是把二维码显示出来并立即返回，轮询仍由插件自己做；宿主不碰账号密码。
  二维码是本地编码的（`qrcode` crate），不依赖外部图片服务。
- `fetch` 的响应支持 `text()` / `json()` / `arrayBuffer()` / `bytes()` / `blob()`：
  `arrayBuffer()` 走宿主给的 base64 原始字节，图片 / 音频这类二进制不会被当成文本截坏；
  但**没有流式响应**，整个响应体会一次性读完（超过 8MB 时不再附 base64，`arrayBuffer()` 会是空的）。
- `stream` / `getMediaSource` 返回的 `headers`（Referer、User-Agent、Cookie…）会随下载请求一起发；
  缓存文件按「url + headers」取指纹，同一地址换了 Referer 不会错误命中旧文件。
- 首页接了 `home`（MyGo）与 `getTopLists`（MusicFree）；`browse`、`playlists`、`show`、`comments`
  这些更深的页面还没接到界面上，卡片点开也还没做。
