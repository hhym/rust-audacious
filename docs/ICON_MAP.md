# Tabler 图标映射表（主界面 / 迷你窗 / 多主题）

对照仓库 [`hhym/tabler-icons`](https://github.com/hhym/tabler-icons)（Tabler Icons，MIT，6500+ SVG，24×24 网格，默认 `stroke-width: 1.75`）逐按钮出的替换表。

- 图标路径：`icons/outline/<name>.svg`（线框）、`icons/filled/<name>.svg`（实心）
- 坐标基准：**主窗口 1180×900**（默认尺寸），左上角为原点；窗口缩放时按 `egui` 面板比例走，x 为常见值（±4px）
- 尺寸约定：`18` 侧栏/小按钮 · `20` 菜单栏/播放条 · `24` 主操作 · `28` 迷你窗 · `34` 圆底播放键内图标
- 颜色 token：`p.text` 常态 / `p.dim` 次级 / `p.accent` 激活 / `p.accent_soft` 悬停底（9 套主题自动换，见第 8 节）

---

## 1. 主窗口栅格

| 区域 | y 范围 | 高度 | 说明 |
|---|---|---|---|
| 菜单栏 `menubar` | 0 – 34 | 34 | 标题 + 8 个全局功能图标 |
| 工具栏 `toolbar` | 34 – 86 | 52 | 播放控制 + 随机/循环 + 进度 + 音量 + 主题 |
| 搜索条 `topsearch` | 86 – 136 | 50 | 输入框 + 搜索 + 右侧 3 个小按钮 |
| 内容区 | 136 – 776 | — | 侧栏 240px + 主区 |
| 播放条 `nowband` | 776 – 876 | 100 | 封面 + 曲目 + 6 个控制 + 频谱 + 词/待播 + 通栏进度 |
| 状态栏 `statusbar` | 876 – 900 | 24 | 纯文字（格式/队列/audtool 端口/版本） |

---

## 2. 菜单栏（y = 17，30×30 命中区，图标 20px）

| # | x ≈ | 当前控件 | 现在的画法 | Tabler 图标 | 状态差异 |
|---|---|---|---|---|---|
| 1 | 168 | 添加文件 | 自绘 `icon_file_add`（文档 + 加号） | `outline/file-plus` | — |
| 2 | 202 | 添加目录 | 自绘 `icon_folder_add` | `outline/folder-plus` | — |
| 3 | 236 | 保存列表 | 自绘 `icon_save_list`（三横线 + 下箭头） | `outline/device-floppy`（或 `outline/playlist-add`，看语义取向） | — |
| 4 | 270 | 均衡器 | 自绘 `icon_eq`（三竖杆 + 滑点） | `outline/adjustments-horizontal` | 打开 EQ 时 `filled/adjustments-horizontal` |
| 5 | 304 | 插件 | 自绘 `icon_plugins`（2×2 方块） | `outline/puzzle` | 有插件更新时叠 `outline/bell` / 角标 |
| 6 | 338 | 设置 | 自绘 `icon_settings`（齿轮） | `outline/settings` | — |
| 7 | 388 | 迷你 | 自绘 `icon_mini`（小窗 + 进度） | `outline/picture-in-picture` | 进迷你模式后右键菜单用 `filled/picture-in-picture` |
| 8 | 418 | 关于 | 自绘 `icon_info`（圆 + i） | `outline/info-circle` | — |

> 左侧标题 `AUDACIOUS · RUST` 保持纯文字，`p.accent`，13.5px，不加图标。
> 右侧 toast 保持纯文字，不放图标。

---

## 3. 工具栏（y = 60，图标 20px / 播放键 38px 圆）

| # | x ≈ | 当前控件 | 现在的画法 | Tabler 图标 | 说明 |
|---|---|---|---|---|---|
| 1 | 10–40 | 上一首 | `mini_prev_icon` | `filled/player-skip-back`（激活时）/ `outline/player-skip-back`（常态） | 30×30 |
| 2 | 52–90 | 播放 / 暂停 | 强调色实心圆 + 三角 / 双竖条 | `filled/player-play` / `filled/player-pause`，白色，24px | 圆 32px，`p.accent` 底 |
| 3 | 96–126 | 停止 | 自绘 `icon_stop` 小方块 | `filled/player-stop` | 16px，`p.dim` |
| 4 | 130–160 | 下一首 | `mini_next_icon` | `filled/player-skip-forward` / `outline/player-skip-forward` | 30×30 |
| 5 | 175–215 | 随机 | 文字胶囊「随机」 | 左侧 `outline/arrows-shuffle` 14px + 文字 | 开启时图标与文字都变 `p.accent`，底 `p.accent_soft` |
| 6 | 228–285 | 循环 | 文字胶囊「循环: 关/全部/单曲」 | `outline/repeat`（关）/ `outline/repeat`（全部，accent）/ `outline/repeat-once`（单曲） | 保留文字，图标只做状态提示 |
| 7 | 310–560 | 进度滑条 | `egui::Slider` | —（无图标） | 拖动中在滑柄上方浮 `outline/player-play`? 不需要 |
| 8 | 640–748 | 时间 `00:02 / 00:12` | 等宽文字 | — | 纯文字 |
| 9 | 760–850 | 音量滑条 | `egui::Slider` | 左端可放 `outline/volume` 12px | 可选 |
| 10 | 868–908 | 音量（静音切换） | 文字「音量」 | `outline/volume-2`（有声）/ `outline/volume-off`（静音） | 18px |
| 11 | 1105–1135 | 主题按钮 | 自绘 `paint_shirt`（T 形） | `outline/palette` | 20px，`p.accent`；右侧跟主题名小字 |
| 12 | 1070–1170 | 频谱 | `paint_spectrum` 自绘 | 无音频时占位 `outline/waveform` | 20px，`p.dim` |

---

## 4. 搜索条（y = 111，高 50）

| # | x ≈ | 当前控件 | 现在的画法 | Tabler 图标 | 说明 |
|---|---|---|---|---|---|
| 1 | 12–940 | 搜索输入框 | `TextEdit` + hint「歌名 / 歌手」 | 左内嵌 `outline/search` 16px（`p.dim`） | 聚焦时图标变 `p.accent` |
| 2 | 948–990 | 搜索按钮 | 文字「搜索」 | `outline/search` 16px + 文字 | 忙碌时换成 `outline/loader-2` 旋转 |
| 3 | 1040 | 音源切换「音」 | `top_icon_button` 文字方钮 | `outline/plug`（或 `outline/devices`） | 打开选择器时 `p.accent` + `p.accent_soft` |
| 4 | 1076 | 待播清单「≡」 | 文字方钮 | `outline/playlist` | 当前在播放列表页时 accent |
| 5 | 1112 | 随机「随」 | 文字方钮 | `outline/arrows-shuffle` | 开启时 accent |

> 建议把「音 / ≡ / 随」三个字彻底换图标，它们现在靠汉字表意，英文界面下会显得别扭。

---

## 5. 主播放条 `nowband`（y = 776–876）

上排 y ≈ 806；下排通栏进度 y ≈ 849；进度时间 y ≈ 866。

| # | x ≈ | 当前控件 | 现在的画法 | Tabler 图标 | 尺寸 / 颜色 |
|---|---|---|---|---|---|
| 1 | 12–60 | 封面（52×52 圆角 6） | `current_cover` 贴图 | 无封面时 `outline/photo` | 28px，`p.dim` |
| 2 | 68–200 | 曲名 + 艺人·专辑 | 两行文字 | 播放中可加 `outline/music` 12px 角标 | 可选 |
| 3 | 214–238 | ♥ 喜欢 | 自绘 `heart_icon`（两圆 + 尖角） | `filled/heart`（已喜欢，`#f0606c`）/ `outline/heart`（未喜欢，`p.dim`） | 20px / 30×30 |
| 4 | 250–280 | 上一首 | `mini_prev_icon` | `outline/player-skip-back` | 20px，悬停 `p.accent` |
| 5 | 290–330 | 播放 / 暂停 | 40px 强调色圆 + 三角/双竖条 | `filled/player-play` / `filled/player-pause`，白色 | 22px，圆底 `p.accent` |
| 6 | 339–369 | 下一首 | `mini_next_icon` | `outline/player-skip-forward` | 20px |
| 7 | 383–409 | 停止 | 自绘小方块 | `filled/player-stop` | 14px，`p.dim` |
| 8 | 860–980 | 频谱（可选显示） | `paint_spectrum` | 静默时 `outline/waveform` | 20px，`p.dim` |
| 9 | 940–1050 | 音量滑条 | `egui::Slider` | — | — |
| 10 | 950–985 | 音量文字（此版为标签） | 文字「音量」 | 可直接换 `outline/volume-2` 图标，点击静音 | 18px |
| 11 | 1098–1126 | 「词」歌词页 | 文字「词」 | `outline/music`（或 `outline/file-music`） | 18px，当前页 accent |
| 12 | 1142–1170 | 「≡」待播清单 | `mini_list_icon` | `outline/playlist` | 18px，当前页 accent |
| 13 | 12–1168 | 通栏进度 | 自绘 3px 线 + 已播段 | —（纯线，不加图标） | 拖拽时滑柄画 8px 圆点 |

---

## 6. 迷你窗（固定 420×140，深色卡片 `#181a1e`，圆角 18）

卡片：left = 64，right = 410，top = 16，bottom = 124（`card.center().y = 70`）；封面直径 128，圆心 `x = 66`。

| # | 位置（相对窗口） | 当前控件 | 现在的画法 | Tabler 图标 | 尺寸 / 颜色 |
|---|---|---|---|---|---|
| 1 | 中心 (66, 70)，直径 128 | 圆形封面 | `round_texture()` 圆角遮罩 | 无封面时 `outline/disc` | 48px，`accent` |
| 2 | 中心 (144, 70) | ♥ 喜欢 | `heart_icon` | `filled/heart`（已喜欢 `#f0606c`）/ `outline/heart`（灰 `#8c9198`） | 18px / 24×24 |
| 3 | 中心 (189, 70) | 上一首 | `mini_prev_icon` | `filled/player-skip-back` / `outline/player-skip-back` | 20px / 26×26 |
| 4 | 中心 (243, 70)，直径 46 | 播放 / 暂停 | 强调色圆 + 三角/双竖条 | `filled/player-play` / `filled/player-pause`，白色 | 26px，圆底 `config_accent()` |
| 5 | 中心 (278, 70) | 下一首 | `mini_next_icon` | `filled/player-skip-forward` / `outline/player-skip-forward` | 20px / 26×26 |
| 6 | 中心 (322, 70) | 待播清单 | `mini_list_icon` | `outline/playlist` | 18px / 26×26；点击后回主界面的播放列表页 |
| 7 | 中心 (390, 36)，28×28 | 关闭 × | `mini_close_icon` | `outline/x` | 14px，`#8c9198`；悬停 `#c8cdd4` |
| 8 | x 74–400，y 120–122 | 底部细进度 | 自绘 2px 线 + 已播段 | — | `#34383e` 底 / `accent` 已播 |

> 迷你窗**没有**歌曲名、艺人、时间文字（按设计稿），也**没有**音量、随机、循环、词按钮——这些都只在主窗口。

---

## 7. 侧栏与内容区

| 位置 | 当前控件 | 现在的画法 | Tabler 图标 | 说明 |
|---|---|---|---|---|
| 侧栏顶部 (x 14, y 168) | 当前音源 logo 30px | `load_logo_texture` | 无 logo 时 `outline/plug` | — |
| 侧栏 x 200, y 161 | 「切换」音源 | 文字小按钮 | `outline/chevron-down` 12px + 文字 | 打开选择器时转 `outline/chevron-up` |
| 侧栏 52×52 | 大封面 | 贴图 | `outline/photo` | — |
| 侧栏 52×52 | 「＋」添加文件 | 文字 `+` | `outline/plus` 20px（或 `outline/file-plus`） | 与菜单栏第 1 项区分：这里是「加进当前队列」 |
| 导航（6 行 34px） | 首页 | 文字 | `outline/home` | 18px，选中 accent |
| 导航 | 搜索 | 文字 | `outline/search` | — |
| 导航 | 喜欢 + 数量 | 文字 | `outline/heart` | — |
| 导航 | 待播清单 + 数量 | 文字 | `outline/playlist` | — |
| 导航 | 歌曲 + 数量 | 文字 | `outline/music` | — |
| 导航 | 歌词 | 文字 | `outline/file-music` | — |
| 我的歌单 `+` | 新建歌单 | 文字 `+` | `outline/playlist-add` | — |
| 按专辑 | 专辑分组 | 封面缩略图 + 文字 | 缩略图兜底 `outline/disc` | — |
| 内容区操作行 | 全选 / 取消选择 / 删除所选 | 文字按钮 | `outline/checkbox` / `outline/x` / `outline/trash` | 12–16px 前缀 |
| 曲目右键菜单 | 播放这一首 / 从列表删除 / 加入喜欢 / 上移 / 下移 | 文字 | `outline/player-play` / `outline/trash` / `outline/heart` / `outline/arrow-up` / `outline/arrow-down` | 16px |
| 插件页 | 重新扫描 / 在线搜索 / 管理插件 | 文字 | `outline/refresh` / `outline/search` / `outline/puzzle` | — |

---

## 8. 多主题：图标着色规则 + 主题菜单

图标只吃三种颜色，不写死：

| Token | 用途 | 取值 |
|---|---|---|
| `p.accent` | 激活态、选中的图标、圆形播放底 | 主题强调色 |
| `p.text` | 常态图标 | 主题正文色 |
| `p.dim` | 次级图标（停止、关闭、无封面占位） | 主题次要色 |
| `p.accent_soft` | 悬停底 | 强调色 52% tint |

### 9 套主题（图标自动跟随，不需要另做图标集）

| 序 | 主题 | 底色 | 强调色 | 图标观感提示 |
|---|---|---|---|---|
| 1 | 护眼绿 | `#c7edcc` | `#267a4c` | 浅底，常态图标用深绿灰 `#18462e`；不要用白 |
| 2 | 橘黄 | `#fdf2e3` | `#bc7a2a` | 默认主题，图标偏暖棕 |
| 3 | 深青 | `#0d1b1d` | `#2dbdb0` | 深底，常态图标 `#deeeec`，青色只在激活态 |
| 4 | 暗夜金 | `#141311` | `#e8c05a` | 黑金，常态图标 `#eee8dd`，金色点缀 |
| 5 | 樱粉 | `#fdf0f4` | `#ce527a` | 浅粉底，图标用 `#3a1e28` |
| 6 | 深色 | `#17181b` | 配置色 | 深色标准，图标 `#e2e4e8` |
| 7 | 浅色 | `#f5f6f8` | 强调色压暗 0.78 | 白底，图标 `#202328` |
| 8 | 天空蓝 | `#e2f0fd` | `#1c74cd` | 浅蓝底，图标 `#14263a` |
| 9 | 绚丽紫 | `#1a1226` | `#b26cff` | 深紫底，图标 `#ece4f8` |

> 跟随封面取色（`cover_accent`）开启时，`p.accent` 由封面主色经 `vividify()` 提饱和后得到，所有激活态图标（播放圆底、选中项、悬停底）同步变色；`p.text/p.dim` 不变，保证可读性。

### 主题菜单（贴工具栏主题按钮弹出，宽 152，x ≈ 945–1097）

| 行 | 元素 | 现在的画法 | Tabler 图标建议 |
|---|---|---|---|
| 1–9 | 每套主题一行，14×14 色块（底色 + 6px 强调色点 + 选中描边） | 自绘 | 色块保留；选中行右侧补 `outline/check` 16px（`p.accent`），未选留空，比只描边更好扫 |
| 分隔线 | — | `ui.separator()` | — |
| 第 11 行 | 「跟随封面取色」复选框 | `ui.checkbox` | 勾选态用 `filled/color-picker`? 用 `outline/palette` 16px 前缀，勾选框保持系统样式 |
| 设置页「外观」 | 主题按钮 + 强调色 | `horizontal_wrapped` 文字按钮 | 每个主题按钮左侧加同款 12px 色点，右侧当前项 `outline/check` |

---

## 9. 移动端（android 分支，手机布局）

| 位置 | 控件 | Tabler 图标 |
|---|---|---|
| 顶部导航（40px） | 首页 / 搜索 / 喜欢 / 歌曲 / 歌词 | `outline/home` / `search` / `heart` / `music` / `file-music` |
| 命令条 chip | ＋文件 / ＋目录 / 保存列表 / 设置 / 插件 / 主题 | `outline/file-plus` / `folder-plus` / `device-floppy` / `settings` / `puzzle` / `palette` |
| 手机播放条（100px） | 上一首 / 播放 / 下一首 | 同第 3 节 filled/outline `player-*` |
| 手机底栏进度 | 点拖 seek | 无图标 |

---

## 10. 落地方式（Rust / egui）

1. 只取用到的 SVG（约 45 个），放进 `crates/audacious/assets/tabler/`，文件名保持 Tabler 原名，附 `LICENSE`（MIT）。
2. 构建时用 `resvg`/`usvg` 栅格化成 2× 位图，缓存成 `egui::TextureHandle`；或直接把 SVG path 转成 `egui::Shape`（无外部依赖，颜色可运行时替换，推荐后者）。
3. 统一封装：

```rust
fn tabler(painter: &Painter, rect: Rect, icon: TablerIcon, color: Color32, px: f32);
```

4. 状态规则写死在一处：`active ? p.accent : (hover ? p.text : p.dim)`，保证满屏图标行为一致。
5. 需要注意 Tabler 的 24×24 网格留白较大，直接塞进 20px 命中区会显小，建议图标盒 24、命中盒 30，视觉重量才和现在的手绘图标接近。

---

## 11. 现有自绘图标替换清单（一一对应）

| 现有函数 | 行号（约） | 替换成 |
|---|---|---|
| `heart_icon` | 5388 | `outline/heart` / `filled/heart` |
| `mini_prev_icon` | 5400 | `outline/player-skip-back` |
| `mini_next_icon` | 5410 | `outline/player-skip-forward` |
| `mini_play_icon` | 5420 | `filled/player-play` |
| `mini_pause_icon` | 5430 | `filled/player-pause` |
| `mini_list_icon` | 5440 | `outline/playlist` |
| `mini_close_icon` | 5450 | `outline/x` |
| `icon_file_add` | 5350 | `outline/file-plus` |
| `icon_folder_add` | 5360 | `outline/folder-plus` |
| `icon_save_list` | 5370 | `outline/device-floppy` |
| `icon_eq` | 5380 | `outline/adjustments-horizontal` |
| `icon_plugins` | 5390 | `outline/puzzle` |
| `icon_settings` | 5400 | `outline/settings` |
| `icon_mini` | 5410 | `outline/picture-in-picture` |
| `icon_info` | 5420 | `outline/info-circle` |
| `icon_stop` | 5430 | `filled/player-stop` |
| `paint_shirt` | 2890 | `outline/palette` |

> 行号会漂移，以 `grep -n "fn heart_icon\|fn mini_"` 为准。

---

## 12. 覆盖不到的（继续自绘）

| 控件 | 原因 |
|---|---|
| 频谱 `paint_spectrum` | 实时数据驱动，Tabler 无对应 |
| 通栏/底部进度条 | 纯几何线，不需要图标 |
| 主题菜单色块 | 颜色样本，不是图标 |
| 封面圆角遮罩 `round_texture` | 图像处理，不是图标 |
| 标题 `AUDACIOUS · RUST` | 文字标识 |
