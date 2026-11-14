# 在线插件（MusicFree 标准）

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

## 已知边界

- 没有真 Node 语义：`setTimeout` / 事件循环 / `Buffer` / 流式响应 / `dayjs.extend` 未实现；
  `webdav`、`@react-native-cookies/cookies` 只给了 stub。
- 需要登录态或加密签名的插件可能跑不通（缺 cookie 容器与自定义 UA 配置）。
- 网易系插件已实测：`search` 能拿到真实结果；部分第三方中转 API 会限流（返回 429），
  这类错误来自插件自己的接口，不是宿主问题。
- 插件异常会在「在线搜索」窗口里显示错误文本（含调用栈），便于定位。
