$dir = 'C:\Users\Administrator\Doubao\chats\2026-11-26\new-chat-22'
$apk = Join-Path $dir 'audacious-wrap-armv7.apk'
Invoke-WebRequest -Uri 'https://aka.doubaocdn.com/s/HqtnhpdLH3' -OutFile $apk -UseBasicParsing
adb install -r $apk
adb shell am force-stop io.github.hhym.audaciousrs
adb shell am start -n io.github.hhym.audaciousrs/.MainActivity
Start-Sleep -Seconds 9
adb shell input tap 660 60
Start-Sleep -Seconds 3
adb shell screencap -p /sdcard/w1.png
adb pull /sdcard/w1.png (Join-Path $dir 'plugins-wrap.png') | Out-Null
Write-Output 'done'
