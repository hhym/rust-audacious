rust-audacious 5.4.0 · Windows x64 免安装版
=========================================

Audacious 内核的 Rust 重写，单文件可执行，无需安装运行库。

一、怎么跑
----------
1. 双击 audacious-rs.exe 打开播放器。
2. 添加音乐：把文件或文件夹拖进窗口，或点工具栏「添加文件」「添加目录」。
3. 双击列表里的歌开始播放；空格暂停/继续，← → 切上一首/下一首。

二、在线插件（MusicFree 标准）
-----------------------------
1. 把 MusicFree 插件 .js 放进  %APPDATA%\audacious-rs\plugins\
   （例如仓库里的 examples/plugins/itunes.js）
2. 菜单「插件」→ 底部「在线插件（MusicFree 标准）」→ 点「重新扫描」。
3. 点「在线搜索…」：选插件、输入歌名回车，结果里 ▶ 直接播放、＋ 加入列表。
   在线音源会先下载到缓存再播放，进度/频谱/切歌与本地文件一致。

三、audtool 命令行
-----------------
在文件所在目录打开 cmd / PowerShell：

  audtool                         查看当前状态
  audtool playlist-add D:\Music   把整个目录加进播放列表
  audtool playlist-play 2         播放列表第 2 首
  audtool playback-status         输出 playing / paused / stopped
  audtool current-info            输出 JSON（标题、艺人、时长、进度）
  audtool volume 60               音量 60
  audtool eq-gain 4 6             第 4 段 EQ +6dB
  audtool playlist-save list.m3u   保存播放列表
  audtool playlist-add list.m3u    载入 M3U / PLS
  audtool quit                    退出播放器

四、数据位置
------------
  %APPDATA%\audacious-rs\config.json          音量、循环、随机、EQ、主题、迷你模式
  %APPDATA%\audacious-rs\control.port         audtool 连接用的端口
  %APPDATA%\audacious-rs\plugins\             MusicFree 插件目录
  %APPDATA%\audacious-rs\plugin-storage.json  插件持久化键值
  %LOCALAPPDATA%\audacious-rs\streams\        在线音源缓存

五、说明
--------
- 需要 Windows 10 或更高版本（x64）。
- 解码：symphonia（MP3 / FLAC / Ogg Vorbis / Opus / WAV / AAC-M4A / MKV 等）
  输出：WASAPI。若系统没有可用音频设备，会自动进入静音解码模式，
  进度与频谱照常，插上耳机后重启程序即可出声。
- 首次运行 Windows Defender 可能提示确认，属正常（未签名程序）。
- 没做 Winamp 皮肤、MPRIS，详见仓库 README 与 PLUGINS.md。

六、许可证
----------
BSD-2-Clause，与原版 Audacious 一致。界面布局参考 Audacious 的 GTK 版本。
插件接口标准参考 MusicFreePlugins（https://github.com/hhym/MusicFreePlugins）。

仓库：https://github.com/hhym/rust-audacious
