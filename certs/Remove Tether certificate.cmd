@echo off
rem Removes the Tether code-signing certificate that "Install Tether certificate.cmd" installed. Double-click it.
net session >nul 2>&1 || (powershell -NoProfile -Command "Start-Process -Verb RunAs -FilePath '%~f0'" & exit /b)
certutil -delstore Root 9BDFE83E548CEF50DF2E96B8737ABCCD39BDC346 >nul
certutil -delstore TrustedPublisher 9BDFE83E548CEF50DF2E96B8737ABCCD39BDC346 >nul
echo Tether certificate removed.
echo.
pause
