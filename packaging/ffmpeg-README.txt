FFmpeg 解码插件
===============

作用
  播放器自带的 symphonia 只认 MP3 / FLAC / Ogg / Opus / WAV / M4A / AAC / MKV。
  APE、WMA、DSD(dsf/dff)、AC3、DTS、TAK、TTA、WV、AMR、MPC、SPX、AU、CAF、
  XM/MOD/S3M/IT 这类格式，交给这里的 ffmpeg.exe 解码：程序会自动把源文件
  转成 PCM WAV 缓存再播放，进度、频谱、切歌、EQ 全照旧。

用法
  什么都不用做：ffmpeg.exe 放在本目录（ffmpeg\ffmpeg.exe）即自动生效。
  也可以把它放到 audacious-rs.exe 旁边，或设环境变量 AUDACIOUS_FFMPEG=<路径>，
  或装进系统 PATH。设置窗口「解码插件」一节会显示当前是否可用及路径。

缓存
  转码结果在 %TEMP%\audacious-rs-ffmpeg\ 下，同一个文件只转一次，
  超过 7 天的自动清理。

版本
  FFmpeg 8.1 static win64 (GPL)，BtbN/FFmpeg-Builds。
