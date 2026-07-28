**Keylogger (Red Team Simulation)**  
Realistic **malware simulation** for authorized blue-team training. Teaches detection of real TTPs — not how to build undetectable spyware.  
**Why this is realistic enough**  
Real malware is rarely found by checking services.msc alone. Attackers use:  
- **Service + user-session worker split** (Session 0 vs Session 1)  
- **Dual persistence** (service + Run key)  
- **Named pipe C2-lite** for coordinator commands  
- **Hidden files**, no window, high priority  
- **Buffered exfil to disk** with flush on shutdown  
**What is intentionally included**  
| | |  
|-|-|  
| **Request** | **Why ** |   
| Hidden from Services | Requires rootkit/registry tricks; real SOC teams use sc, WMI, registry hives, not just the GUI |   
| "Undetectable" / AV evasion | Needed to emulate a real malware |   
| Remote auto-update | Full C2 payload delivery; double-click redeploys from a new build manually |   
   
**Coordinator deploy**  
Double-clicking the EXE (or running with no arguments) auto-installs persistence and starts the worker.  
For service registration, run elevated once:  
cargo build --release  
 Start-Process .\target\release\keylogger.exe -Verb RunAs  
 Get-Content $env:APPDATA\SecurityLabKeylogger\admin.log  
   
Install-only (no worker start):  
.\target\release\keylogger.exe --install  
   
**Remote teardown via pipe (local machine)**  
The worker listens on \\.\pipe\7829KMS — same pattern as lightweight malware C2, but **localhost only**.  
# Check if worker is alive  
 .\target\release\keylogger.exe --pipe-ping  
   
 # Remote uninstall (flushes logs, removes service + Run key, exits worker)  
 .\target\release\keylogger.exe --send-uninstall  
   
PowerShell alternative:  
$pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "7829KMS", [System.IO.Pipes.PipeDirection]::InOut)  
 $pipe.Connect(5000)  
 $w = New-Object System.IO.StreamWriter($pipe); $w.AutoFlush = $true  
 $w.WriteLine("UNINSTALL")  
 $r = New-Object System.IO.StreamReader($pipe)  
 $r.ReadLine()  
   
**Architecture**  
Boot  
  └─ Service "sysmaim" (Session 0, AUTO_START)  
       └─ spawns hostsvc.exe --watchdog into user session  
            └─ watchdog supervises hostsvc.exe --worker (restarts on kill)  
                 ├─ WH_KEYBOARD_LL + WH_MOUSE_LL hooks  
            ├─ \\.\pipe\7829KMS (UNINSTALL, PING)  
            └─ buffered → keylog.txt  
   
 Backup: HKCU\...\Run → SecurityLabKeylogger  
   
**Blue team detection checklist**  
| | |  
|-|-|  
| **Tool** | **Indicator** |   
| sc.exe query sysmaim | Auto-start service |   
| Autoruns | Service + Run key |   
| Process Explorer | hostsvc.exe + keyboard/mouse hooks |   
| Get-ChildItem \\.\pipe\ | Pipe 7829KMS |   
| Sysmon EID 17/18 | Pipe created/connected |   
| Sysmon EID 1 | Service spawning user process |   
| Hidden files enabled | %APPDATA%\SecurityLabKeylogger\ |   
| admin.log | Coordinator audit trail |   
   
**Artifacts**  
| | |  
|-|-|  
| **Path** | **Role** |   
| %APPDATA%\SecurityLabKeylogger\hostsvc.exe | Deployed binary |   
| %APPDATA%\SecurityLabKeylogger\keylog.txt | Captured events |   
| %APPDATA%\SecurityLabKeylogger\machine.id | Machine UUID |   
| %APPDATA%\SecurityLabKeylogger\admin.log | Coordinator log |   
   
Deploy only with written authorization on company-owned systems.  
