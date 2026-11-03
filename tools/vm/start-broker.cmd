@echo off
rem Start the elevated VM-test broker (asks for UAC once). Stop it any time: create C:\vms\broker\stop
powershell -NoProfile -Command "Start-Process powershell -Verb RunAs -WindowStyle Minimized -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','C:\vms\broker\broker.ps1','-Hours','16'"
