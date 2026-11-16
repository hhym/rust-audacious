# Bootstrap Icons 图标映射表（主界面 / 迷你窗 / 多主题）

先说清楚仓库：你给的 [`hhym/bootstrap`](https://github.com/hhym/bootstrap) 是 Bootstrap **框架**仓库（CSS/JS 组件），里面只有 `bootstrap-logo.svg` 这类品牌图，**不含一套界面图标**。图标集是独立的 `bootstrap-icons` 包，本表按官方 **bootstrap-icons 1.11.3** 校验：每个名字都用 `https://unpkg.com/bootstrap-icons@1.11.3/icons/<name>.svg` 请求过，返回 200 才算可用。

- 网格：Bootstrap Icons 是 **16×16** viewBox（Tabler 是 24×24），同一像素尺寸下线条更细、留白更小，塞进 18–20px 命中区比 Tabler 更“满”，一般不用再放大
- 命名：`<语义>` 为线框，`<语义>-fill` 为实心；没有 Tabler 的 `filled/`、`outline/` 目录之分
- 许可：MIT（Bootstrap 团队），保留 `LICENSE` 即可
- 坐标基准与 Tabler 表一致：**主窗口 1180×900**，左上角原点，x 为常见值（±4px）

> 本表只做映射，不改代码。当前代码仍用 Tabler（`crates/audacious/assets/tabler/`），要切换见第 10 节。

---

## 1. 主窗口栅格（同 Tabler 表，未变）

| 区域 | y 范围 | 高度 |
|---|---|---|
| 菜单栏 `menubar` | 0 – 34 | 34 |
| 工具栏 `toolbar` | 34 – 86 | 52 |
| 搜索条 `topsearch` | 86 – 136 | 50 |
| 内容区 | 136 – 776 | — |
| 播放条 `nowband` | 776 – 876 | 100 |
| 状态栏 `statusbar` | 876 – 900 | 24 |

---

## 2. 菜单栏（y = 17，30×30 命中区，图标 20px）

| # | x ≈ | 控件 | 现在的画法 | Bootstrap 图标 | 与 Tabler 的差异 |
|---|---|---|---|---|---|
| 1 | 168 | 添加文件 | `icon_file_add` | `file-earmark-plus` | 同名可用 |
| 2 | 202 | 添加目录 | `icon_folder_add` | `folder-plus` | 同名可用 |
| 3 | 236 | 保存列表 | `icon_save_list` | `floppy` | Tabler 叫 `device-floppy` |
| 4 | 270 | 均衡器 | `icon_eq` | `sliders` | Tabler 叫 `adjustments-horizontal` |
| 5 | 304 | 插件 | `icon_plugins` | `puzzle` | 同名可用 |
| 6 | 338 | 设置 | `icon_settings` | `gear` | Tabler 叫 `settings`；打开设置窗时用 `gear-fill` |
| 7 | 388 | 迷你 | `icon_mini` | `pip` | Tabler 叫 `picture-in-picture`（bootstrap 无此名） |
| 8 | 418 | 关于 | `icon_info` | `info-circle` | 同名可用 |

标题 `AUDACIOUS · RUST` 保持纯文字，`p.accent`。

---

## 3. 工具栏（y = 60，图标 20px / 播放键 38px 圆）

| # | x ≈ | 控件 | 现在的画法 | Bootstrap 图标 | 说明 |
|---|---|---|---|---|---|
| 1 | 10–40 | 上一首 | `mini_prev_icon` | `skip-backward-fill`（激活）/ `skip-backward`（常态） | 30×30 |
| 2 | 52–90 | 播放 / 暂停 | 强调圆 + 三角 / 双竖条 | `play-fill` / `pause-fill`（白色 24px） | 圆 32px，`p.accent` 底 |
| 3 | 96–126 | 停止 | `icon_stop` | `stop-fill` | 16px，`p.dim` |
| 4 | 130–160 | 下一首 | `mini_next_icon` | `skip-forward-fill` / `skip-forward` | 30×30 |
| 5 | 175–215 | 随机 | 文字胶囊「随机」 | `shuffle` 14px + 文字 | 开启时图标与文字变 `p.accent` |
| 6 | 228–285 | 循环 | 文字胶囊 | `repeat`（关/全部）/ `repeat-1`（单曲） | 保留文字，图标只做状态 |
| 7 | 310–560 | 进度滑条 | `egui::Slider` | — | 无图标 |
| 8 | 640–748 | 时间文字 | 等宽文字 | — | — |
| 9 | 760–850 | 音量滑条 | `egui::Slider` | 左端可选 `volume-up` 12px | — |
| 10 | 868–908 | 静音切换 | 文字「音量」 | `volume-up`（有声）/ `volume-mute`（静音） | 18px；Tabler 的 `volume-2` 在 bootstrap 叫 `volume-up` |
| 11 | 1105–1135 | 主题按钮 | `paint_shirt` | `palette` | 20px，`p.accent` |
| 12 | 1070–1170 | 频谱 | `paint_spectrum` | 无音频时 `activity` | 20px，`p.dim`（Tabler 用 `waveform`） |

---

## 4. 搜索条（y = 111，高 50）

| # | x ≈ | 控件 | Bootstrap 图标 | 说明 |
|---|---|---|---|---|
| 1 | 12–940 | 搜索输入框 | 内嵌 `search` 16px（`p.dim`） | 聚焦变 `p.accent` |
| 2 | 948–990 | 搜索按钮 | `search` 16px + 文字 | 忙碌时 `arrow-repeat` 旋转 |
| 3 | 1040 | 音源切换「音」 | `music-note-list`（或 `collection`） | 打开选择器时 accent |
| 4 | 1076 | 待播清单「≡」 | `music-note-list` / `list` | 在播放列表页时 accent |
| 5 | 1112 | 随机「随」 | `shuffle` | 开启时 accent |

> 「音 / ≡ / 随」三个汉字按钮建议彻底换图标，英文界面下更统一。

---

## 5. 主播放条 `nowband`（y = 776–876，上排 y ≈ 806）

| # | x ≈ | 控件 | Bootstrap 图标 | 尺寸 / 颜色 |
|---|---|---|---|---|
| 1 | 12–60 | 封面 | 无封面时 `disc` | 28px，`p.dim` |
| 2 | 68–200 | 曲名 + 艺人 | 播放中可加 `music-note` 12px 角标 | 可选 |
| 3 | 214–238 | ♥ 喜欢 | `heart-fill`（已喜欢 `#f0606c`）/ `heart`（未喜欢 `p.dim`） | 20px / 30×30 |
| 4 | 250–280 | 上一首 | `skip-backward` | 20px |
| 5 | 290–330 | 播放 / 暂停 | `play-fill` / `pause-fill` 白色 | 22px，40px 圆底 `p.accent` |
| 6 | 339–369 | 下一首 | `skip-forward` | 20px |
| 7 | 383–409 | 停止 | `stop-fill` | 14px，`p.dim` |
| 8 | 860–980 | 频谱 | 静默时 `activity` | 20px，`p.dim` |
| 9 | 940–1050 | 音量滑条 | — | — |
| 10 | 950–985 | 音量 | `volume-up` / `volume-mute`，点击静音 | 18px |
| 11 | 1098–1126 | 「词」 | `file-music`（或 `music-note`） | 18px，当前页 accent |
| 12 | 1142–1170 | 「≡」 | `list` | 18px |
| 13 | 12–1168 | 通栏进度 | — | 纯线 |

---

## 6. 迷你窗（420×140，深色卡片，圆角 18）

卡片 left 64 / right 410 / top 16 / bottom 124；封面直径 128，圆心 x = 66。

| # | 位置（相对窗口） | 控件 | Bootstrap 图标 | 尺寸 / 颜色 |
|---|---|---|---|---|
| 1 | 中心 (66, 70) 直径 128 | 圆形封面 | 无封面 `disc` | 48px，`accent` |
| 2 | 中心 (144, 70) | ♥ 喜欢 | `heart-fill` `#f0606c` / `heart` `#8c9198` | 18px / 24×24 |
| 3 | 中心 (189, 70) | 上一首 | `skip-backward-fill` / `skip-backward` | 20px / 26×26 |
| 4 | 中心 (243, 70) 直径 46 | 播放 / 暂停 | `play-fill` / `pause-fill` 白色 | 26px，圆底 accent |
| 5 | 中心 (278, 70) | 下一首 | `skip-forward-fill` / `skip-forward` | 20px / 26×26 |
| 6 | 中心 (322, 70) | 待播清单 | `list` | 18px / 26×26 |
| 7 | 中心 (390, 36) 28×28 | 关闭 × | `x-lg` | 14px，`#8c9198`，悬停 `#c8cdd4` |
| 8 | x 74–400，y 120–122 | 底部细进度 | — | 2px 线，底 `#34383e` / 已播 accent |

---

## 7. 侧栏与内容区

| 位置 | 控件 | Bootstrap 图标 | 说明 |
|---|---|---|---|
| 侧栏顶部 logo 30px | 当前音源 | 无 logo 时 `music-note-beamed` | — |
| 「切换」 | 切换音源 | `chevron-down` 12px + 文字 | 打开时 `chevron-up` |
| 侧栏大封面 | 封面 | `disc` | 无图兜底 |
| 「＋」 | 加文件进队列 | `plus-lg`（或 `file-plus`） | 与菜单栏第 1 项区分 |
| 导航 | 首页 | `house` | Tabler 叫 `home`；18px，选中 accent |
| 导航 | 搜索 | `search` | — |
| 导航 | 喜欢 | `heart` | — |
| 导航 | 待播清单 | `list` | — |
| 导航 | 歌曲 | `music-note-list` | — |
| 导航 | 歌词 | `file-music` | — |
| 我的歌单 `+` | 新建歌单 | `music-note-list`（或 `plus-lg`） | — |
| 按专辑 | 专辑分组 | 缩略图兜底 `disc` | — |
| 内容区操作行 | 全选 / 取消 / 删除 | `check-square`（或 `check2-square`）/ `x-lg` / `trash` | 12–16px 前缀 |
| 曲目右键菜单 | 播放 / 删除 / 喜欢 / 上移 / 下移 | `play-fill` / `trash` / `heart` / `arrow-up` / `arrow-down` | 16px |
| 插件页 | 重新扫描 / 在线搜索 / 管理插件 | `arrow-clockwise` / `search` / `puzzle` | Tabler 的 `refresh` 在这里叫 `arrow-clockwise` |

---

## 8. 多主题：着色规则 + 主题菜单

图标只吃四个 token，不写死颜色：

| Token | 用途 |
|---|---|
| `p.accent` | 激活态、选中项、圆底播放键 |
| `p.text` | 常态图标 |
| `p.dim` | 次级（停止、关闭、无封面） |
| `p.accent_soft` | 悬停底 |

9 套主题（护眼绿 / 橘黄 / 深青 / 暗夜金 / 樱粉 / 深色 / 浅色 / 天空蓝 / 绚丽紫）下图标自动跟随，无需另做图标集；`cover_accent` 开启时 `p.accent` 由封面主色 `vividify()` 得到，`p.text/p.dim` 不变。

### 主题菜单（宽 152，x ≈ 945–1097）

| 行 | 元素 | Bootstrap 图标建议 |
|---|---|---|
| 1–9 | 主题色块（底色 + 6px 强调点 + 选中描边） | 色块保留，选中行右侧补 `check-lg` 16px（`p.accent`） |
| 分隔线 | — | `ui.separator()` |
| 第 11 行 | 跟随封面取色 | `palette` 16px 前缀，勾选框保持系统样式 |
| 设置页「外观」 | 主题按钮 | 每个按钮左侧 12px 色点，当前项右侧 `check-lg` |

---

## 9. 移动端（android 分支）

| 位置 | Bootstrap 图标 |
|---|---|
| 顶部导航 | `house` / `search` / `heart` / `music-note-list` / `file-music` |
| 命令条 chip | `file-earmark-plus` / `folder-plus` / `floppy` / `gear` / `puzzle` / `palette` |
| 手机播放条 | `skip-backward-fill` / `play-fill` / `pause-fill` / `skip-forward-fill` |
| 底栏进度 | 无图标 |

---

## 10. 落地方式（Rust / egui）

1. 取约 45 个 SVG，放 `crates/audacious/assets/bootstrap-icons/`，附 MIT `LICENSE`；文件名保持原名（无 `outline/filled` 目录）
2. 栅格化：`resvg`/`usvg` → 2× 位图 → `egui::TextureHandle`，或把 path 转 `egui::Shape`（推荐，可运行时上色）
3. 统一封装，替换现有 `tabler()`：

```rust
fn bi(painter: &Painter, rect: Rect, icon: BiIcon, color: Color32, px: f32);
```

4. 16×16 网格线条细，`px` 相比 Tabler 建议 **不放大**（Tabler 需 ×1.12）；圆底播放键里的 `play-fill` 仍要 `rect.shrink(w * 0.18)` 左右才不顶边
5. 状态规则集中一处：`active ? p.accent : (hover ? p.text : p.dim)`

---

## 11. 现有自绘函数 → Bootstrap 图标

| 现有函数 | 替换成 |
|---|---|
| `heart_icon` | `heart` / `heart-fill` |
| `mini_prev_icon` | `skip-backward` / `skip-backward-fill` |
| `mini_next_icon` | `skip-forward` / `skip-forward-fill` |
| `mini_play_icon` | `play-fill` |
| `mini_pause_icon` | `pause-fill` |
| `mini_list_icon` | `list` |
| `mini_close_icon` | `x-lg` |
| `icon_file_add` | `file-earmark-plus` |
| `icon_folder_add` | `folder-plus` |
| `icon_save_list` | `floppy` |
| `icon_eq` | `sliders` |
| `icon_plugins` | `puzzle` |
| `icon_settings` | `gear` / `gear-fill` |
| `icon_mini` | `pip` |
| `icon_info` | `info-circle` |
| `icon_stop` | `stop-fill` |
| `paint_shirt` | `palette` |

> 行号漂移，以 `grep -n "fn heart_icon\|fn mini_"` 为准。

---

## 12. 覆盖不到的（继续自绘）

| 控件 | 原因 |
|---|---|
| 频谱 `paint_spectrum` | 实时数据驱动，图标无法承载 |
| 通栏 / 底部进度条 | 纯几何线 |
| 主题菜单色块 | 颜色样本 |
| 封面圆角遮罩 `round_texture` | 图像处理 |
| 标题 `AUDACIOUS · RUST` | 文字标识 |

---

## 13. 与 Tabler 的取舍（要换之前先看）

| 维度 | Tabler Icons | Bootstrap Icons |
|---|---|---|
| 网格 | 24×24，`stroke-width 1.75` | 16×16，描边约 1px |
| 数量 | 6500+ | 2000+ |
| 实心变体 | `filled/` 整目录 | 仅部分 `-fill`（播放/暂停/停止/心/齿轮等够用） |
| 现有代码 | 已接入，43 个 PNG + `tabler()` | 需重新栅格化 + 换函数，约半天工作量 |
| 视觉重量 | 偏轻，20px 盒子偏小需放大 | 偏满，18–20px 刚好 |
| 缺名替换 | — | `settings→gear`、`device-floppy→floppy`、`picture-in-picture→pip`、`volume-2→volume-up`、`refresh→arrow-clockwise`、`home→house`、`playlist→list/music-note-list` |

结论：**如果只是想要更“实”的图标，不必换库**——把现有 Tabler 的 `px` 再放大 0.1 倍即可；真要换，按第 10 节一次性替换，两套映射表（本表 + `docs/ICON_MAP.md`）一一对得上，改起来不会漏按钮。
