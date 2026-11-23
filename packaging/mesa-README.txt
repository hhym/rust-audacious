Mesa 软件渲染（可选，仅供老显卡 / 无 OpenGL 驱动的机器使用）
================================================================

背景
  程序界面用 egui + glutin 绘制，正常走显卡的 OpenGL。
  很老的核显（如 Intel HD Graphics 2011 版驱动 9.17.x）不提供
  WGL 建 ES 上下文的扩展，会直接报：
      Error: Glutin(NotSupported("extension to create ES context with wgl is not present"))
  这时用本目录的 Mesa（llvmpipe，CPU 软件光栅）就能跑起来。

怎么用
  1. 双击同级的 use-software-gl.bat
     它会把 mesa\opengl32.dll 和 mesa\libgallium_wgl.dll 复制到 exe 旁边，
     然后启动程序。
  2. 想换回硬件渲染：双击 restore-hardware-gl.bat，删掉这两个 dll。

说明
  - opengl32.dll 必须和 audacious-rs.exe 在同一目录才会被加载，脚本已自动处理。
  - 软件渲染吃 CPU，播放本身不受影响，界面动画会稍慢一点。
  - DLL 来源：Mesa3D for Windows 26.3.1（pal1000/mesa-dist-win，MSVC release 构建），
    opengl32.dll sha256 2ae2472a335b400bbc0d16b99981feacb3a7653eb4add6f694ac0e6d6351bbb3
    libgallium_wgl.dll sha256 8f6a0670aaf73604ea0ad8241af583a4ebc21d8a0c112102fafe2bf7695ef6c4
