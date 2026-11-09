@echo off
rem Installs the Tether code-signing certificate so Windows trusts Tether. Double-click it.
net session >nul 2>&1 || (powershell -NoProfile -Command "Start-Process -Verb RunAs -FilePath '%~f0'" & exit /b)
set "C=%TEMP%\tether-codesign.cer"
>"%C%" echo -----BEGIN CERTIFICATE-----
>>"%C%" echo MIIEDDCCAnSgAwIBAgIQQ3gQ/SzKfq5IRFvadJ6TzjANBgkqhkiG9w0BAQsFADAe
>>"%C%" echo MRwwGgYDVQQDDBNUZXRoZXIgQ29kZSBTaWduaW5nMB4XDTI2MTEwOTE1MDEwMVoX
>>"%C%" echo DTM2MTEwOTE1MTEwMFowHjEcMBoGA1UEAwwTVGV0aGVyIENvZGUgU2lnbmluZzCC
>>"%C%" echo AaIwDQYJKoZIhvcNAQEBBQADggGPADCCAYoCggGBAL8y7XLh2ENlS0JMUSCNL2Uz
>>"%C%" echo RLfu3uYktzrFgD52ZQ3fdrqQsFn/MmdoeFeFZCmGEYXbKJeHUKCG+iHpCERFUrDO
>>"%C%" echo ERLGyc631uDniNQ+TmoaEyKfv8okj2jozW8CIS63h7nd9bdtDYdceDuFmQzpNqY9
>>"%C%" echo FtDuJ6KLeSf8PodlJxMG5MDfs7qD12hml3vueCvEQFWvyvExoP5MaB+le1E/pW93
>>"%C%" echo VZzpizZfzWbvZtHn5f8VZWSlXphEuVILMPgyYNdyaPi1TG8cR0SeR8jawH0fwNus
>>"%C%" echo 4W5ZZZGdQGPv5Rxn5vBL6yjTQC/mcTbJw+MkjwLg3+SlliD0yso4mwd1x/F1XwCV
>>"%C%" echo BhPws970INi4oY7WIlxT8oCsoRaguo2Gt0739FYC0ai7UOQrjULFjecpmgVgwQcH
>>"%C%" echo UdCtwAiiHxTuH/IBY/F4tGE0CiOZX+4B3sVGpQ72iObgvYoLC2eFdauItcHtJxJY
>>"%C%" echo eAAF6btQd755NxzbnJxGGwn0FB9H/JfhFuaavbUSVQIDAQABo0YwRDAOBgNVHQ8B
>>"%C%" echo Af8EBAMCB4AwEwYDVR0lBAwwCgYIKwYBBQUHAwMwHQYDVR0OBBYEFCmuvsR4z1oU
>>"%C%" echo JnRqo+Oa0IiS3XLoMA0GCSqGSIb3DQEBCwUAA4IBgQA5shzLYnczclDq+xKHeE+c
>>"%C%" echo Sww7Jcp5lkmWsaesktzIwGiT2CNQCb9NSshQLdSNYadWCSPiKOIpw3NgynnBBxHK
>>"%C%" echo 37lhSh2AP+J5bGCfy0cMWtFp0Kk8i0zXpq6r65Yp4n4a1ZWzD6AXon6Pd9rDYqsa
>>"%C%" echo TUv5vOTSmXeyj1StE9Z/OveTaeb/E6DtsqSWawwVtnSGDml57/l0gbvdotFDIqAP
>>"%C%" echo nlI2APwOQZQ4FmvVfeNWXqdauaXjSLycDamMYdHt+p3sJE8+hNRMhH75EPe7ENZs
>>"%C%" echo P761CbsqIiQd8w1c9apHmZcx2aoYzfv8Qb4gV/0YwDuqmFDIB8yCHYfs5YPgeesb
>>"%C%" echo aO88hK7zXIvQ0qZaBs7+Sj0LgT9zk7YTplwkG4D9OeEOY98lFj5lNedopMNFmbfH
>>"%C%" echo CAg5PDn1vTg6dCXIC4cbFe4AGyA2XAC9TZr4m10Nh5KPXVGOGw8xid8UzxcAHB+k
>>"%C%" echo yTzL1aOY5tU3vqQikzxPXrHi36ZvlGF1kNlKLveZk9c=
>>"%C%" echo -----END CERTIFICATE-----
certutil -addstore -f Root "%C%" >nul && certutil -addstore -f TrustedPublisher "%C%" >nul && (echo Tether certificate installed.) || (echo Install FAILED.)
del "%C%" >nul 2>&1
echo.
pause
