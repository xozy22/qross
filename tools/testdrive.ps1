# Simuliert am Server Mausbewegungen, damit Qross ohne Handarbeit getestet werden kann:
# Cursor an die rechte Kante → Wechsel zum Client → Bewegungen + Shift antippen → zurück.
# Es werden bewusst keine Klicks und keine Zeichen gesendet, damit auf dem Client nichts ausgelöst wird.
param(
    [int]$Steps = 20,
    [int]$StepPx = 15,
    [int]$ReturnSteps = 80
)

Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class QrossDrive {
    [StructLayout(LayoutKind.Sequential)]
    struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Sequential)]
    struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Explicit)]
    struct INPUT {
        [FieldOffset(0)] public uint type;
        [FieldOffset(8)] public MOUSEINPUT mi;
        [FieldOffset(8)] public KEYBDINPUT ki;
    }

    [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);

    static void Send(INPUT input) { SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT))); }

    public static void Move(int dx, int dy) {
        var i = new INPUT { type = 0 };
        i.mi.dx = dx; i.mi.dy = dy; i.mi.dwFlags = 0x0001; // MOUSEEVENTF_MOVE (relativ)
        Send(i);
    }

    public static void Key(ushort scan, bool up) {
        var i = new INPUT { type = 1 };
        i.ki.wScan = scan; i.ki.dwFlags = 0x0008u | (up ? 0x0002u : 0u); // SCANCODE | KEYUP
        Send(i);
    }
}
"@

# Per-Monitor-v2 wie qross.exe, damit Koordinaten physische Pixel sind.
[void][QrossDrive]::SetThreadDpiAwarenessContext([IntPtr]-4)
$right = [QrossDrive]::GetSystemMetrics(76) + [QrossDrive]::GetSystemMetrics(78) - 1

Write-Output "Cursor an die rechte Kante (x=$right)"
[void][QrossDrive]::SetCursorPos($right - 3, 400)
Start-Sleep -Milliseconds 200
[QrossDrive]::Move(20, 0)
Start-Sleep -Milliseconds 500

Write-Output "$Steps Bewegungen à $StepPx px nach rechts unten"
for ($i = 0; $i -lt $Steps; $i++) { [QrossDrive]::Move($StepPx, 5); Start-Sleep -Milliseconds 16 }
Start-Sleep -Milliseconds 300

Write-Output "Shift antippen"
[QrossDrive]::Key(0x2A, $false); Start-Sleep -Milliseconds 50; [QrossDrive]::Key(0x2A, $true)
Start-Sleep -Milliseconds 300

Write-Output "zurück nach links"
for ($i = 0; $i -lt $ReturnSteps; $i++) { [QrossDrive]::Move(-60, 0); Start-Sleep -Milliseconds 16 }
Start-Sleep -Milliseconds 300
Write-Output "fertig"
