$dir = 'C:\Users\Administrator\Doubao\chats\2026-11-26\new-chat-22'
$apk = Join-Path $dir 'audacious-saf-armv7.apk'
Invoke-WebRequest -Uri 'https://aka.doubaocdn.com/s/7MSg4XWkIm' -OutFile $apk -UseBasicParsing
adb install -r $apk
adb shell am force-stop io.github.hhym.audaciousrs
adb shell am start -n io.github.hhym.audaciousrs/.MainActivity
Start-Sleep -Seconds 9
adb shell input tap 70 90
Start-Sleep -Seconds 4
adb shell screencap -p /sdcard/s1.png
adb pull /sdcard/s1.png (Join-Path $dir 'saf-picker.png') | Out-Null
adb shell dumpsys window windows | Select-String -Pattern 'mCurrentFocus|mFocusedApp'
Write-Output 'done'
