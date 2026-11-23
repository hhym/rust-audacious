@echo off
chcp 65001 >nul
cd /d "%~dp0"
if not exist "mesa\opengl32.dll" (
  echo [x] 没找到 mesa 目录，软件渲染不可用。
  pause
  exit /b 1
)
copy /y "mesa\opengl32.dll" ".\opengl32.dll" >nul
copy /y "mesa\libgallium_wgl.dll" ".\libgallium_wgl.dll" >nul
echo [ok] 已启用 Mesa 软件渲染（老显卡专用），正在启动…
start "" "audacious-rs.exe"
