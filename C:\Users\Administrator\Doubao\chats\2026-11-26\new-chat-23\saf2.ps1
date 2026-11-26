$dir = 'C:\Users\Administrator\Doubao\chats\2026-11-26\new-chat-23'
$apk = Join-Path $dir 'audacious-saf2-armv7.apk'
Invoke-WebRequest -Uri 'https://aka.doubaocdn.com/s/0oUKVx4EkP' -OutFile $apk -UseBasicParsing
adb install -r $apk
adb logcat -c
adb shell am force-stop io.github.hhym.audaciousrs
adb shell am start -n io.github.hhym.audaciousrs/.MainActivity
Start-Sleep -Seconds 9
adb shell input tap 70 90
Start-Sleep -Seconds 5
adb shell screencap -p /sdcard/s2.png
adb pull /sdcard/s2.png (Join-Path $dir 'saf2.png') | Out-Null
adb shell dumpsys window windows | Select-String -Pattern 'mCurrentFocus'
adb logcat -d -t 80 | Select-String -Pattern 'audacious|AudaciousRS|OPEN_DOCUMENT'
Write-Output 'done'
