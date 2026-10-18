# Schreibt die aktuelle Cursorposition (physische Pixel) in eine Datei.
# Muss in der interaktiven Sitzung laufen (z. B. als geplante Aufgabe mit /IT), nicht direkt per SSH.
param([Parameter(Mandatory)] [string]$OutFile)

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class GlideProbe {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
}
"@

# Per-Monitor-v2, wie glide.exe – sonst wären die Koordinaten skaliert.
[void][GlideProbe]::SetThreadDpiAwarenessContext([IntPtr]-4)
$p = New-Object GlideProbe+POINT
[void][GlideProbe]::GetCursorPos([ref]$p)
"$($p.X),$($p.Y)" | Set-Content -Encoding ascii $OutFile
