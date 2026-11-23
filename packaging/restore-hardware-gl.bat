@echo off
chcp 65001 >nul
cd /d "%~dp0"
if exist "opengl32.dll" del /q "opengl32.dll"
if exist "libgallium_wgl.dll" del /q "libgallium_wgl.dll"
echo [ok] 已恢复为系统自带的 OpenGL（硬件渲染）。
pause
