# Changelog

All notable changes to Qross. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning](https://semver.org/).

This file is the single source of the release notes: the app shows it under “What's new”, and each GitHub release
uses the section for its version.

Sections: **New**, **Improved**, **Fixed**, **Important** (e.g. breaking changes).

## [Unreleased]

### New

- **Log file:** Qross now also writes its log to `qross.log` in the data folder (Settings → About → Data folder),
  in English and with every start, including the one at login. Above 1 MB it starts a new file and keeps the
  previous one as `qross.old.log`.

### Fixed

- **Start with the system didn't start Qross on Windows:** Qross wrote its path as `\\?\C:\…` into the startup
  entry. Windows showed the entry as enabled but didn't run it at login. Qross now writes the normal path and
  corrects an existing entry the next time it starts, so after this update the next login starts Qross again.

## [0.15.0] - 2026-10-21

### New

- **Hint when a window running as administrator blocks Qross (Windows):** While such a window, e.g. Task Manager,
  is in front or under the cursor, Windows doesn't let Qross see the mouse or keyboard, so the cursor can't move
  to the other computer. Qross now says so in the window and the tray tooltip, and with a notification when you
  push against the edge. Until you click another window, Qross doesn't switch at all, so keys can't end up in the
  admin window.

### Improved

- **Much less memory in the tray:** When the window is closed, Qross ends its interface after 30 s and starts it
  again when you open the window, in about half a second, at the same position and size. Started at login, it
  doesn't start the interface at all. In the tray, Qross now needs about 5 MB instead of about 100 MB.
- **CPU and memory per part:** The display shows Qross itself and its interface separately; the tooltip lists the
  interface processes (page, graphics, browser, network, other). Clients report this from this version on.

### Fixed

- **Memory display too high:** On Windows, the display also counted programs that Qross had merely opened, such as
  a browser for a link, which could add several hundred MB. It now counts only Qross itself and its interface.

## [0.14.0] - 2026-10-20

### New

- **English interface:** Qross now speaks German and English. You choose the language under Settings → General
  (“Automatic” follows the system language); the tray menu, notifications and status messages switch along with
  it. New installations start in the system language, existing ones stay German until you switch.
- **CPU and memory of Qross per computer:** The details show how much processing time (percent of one core over the
  last 2 s) and memory Qross uses on a computer, including its web interface and all helper processes: for the
  server itself (“Qross here”), on the “Connection” tab of a client (“Qross there”) and in the client view. Qross
  only measures while such a display is visible. Clients report their values from Qross 0.14 on.

### Improved

- **Logs, error messages and the command line are in English** throughout, so they're easier to share and search.
  The release notes are in English now too, also in the German interface.

## [0.13.0] - 2026-10-20

### New

- **On Windows, copied files only arrive when pasted:** If you copy files on a Mac (or another computer) and paste
  them on the Windows computer, they're on the clipboard right away, but they're only transferred when
  File Explorer pastes them. This removes the 1 GB limit: even a movie of several gigabytes can simply be pasted,
  and whatever is never pasted never goes over the wire. Explorer shows its own progress, and Qross shows its own
  as usual; canceling works in Explorer and in Qross on both sides. Both computers need this version; otherwise,
  copied files arrive upfront as before. Programs that only accept real files get nothing this way; in that case,
  `QROSS_PASTE_ON_DEMAND=0` turns it off. On macOS and Linux, files are still transferred upfront for now.

### Improved

- **Input during large transfers:** When many files were being sent over, a key press could wait up to 2.4 s until
  there was room in the connection again. Qross now sends files one after another and leaves at most three
  unacknowledged: in the measurement, keys and mouse movements during a running transfer wait at most 50 ms, 99% of
  them under 30 ms, instead of up to 2.2 s.
- **Clipboard when switching:** Images, copied files and other large content only go over once the mouse briefly
  rests after the switch (after 1 s at the latest), instead of right when you get going on the other side. Text
  still goes along immediately.
- **No cursor replay after a Wi-Fi hiccup:** After a brief loss of signal, the cursor on the other side retraced
  its old path (measured: around 1700 stale positions after 2 s). Now it's right where the mouse is. Movements that
  arrive shortly before the switch are no longer lost either.
- **Client view grows with the window:** Previously it was at most 560 px wide. Now it widens with the window,
  switches to two columns from about 1180 px when connected (the connection with its history on the left, the
  server and server selection on the right), and in very large windows everything gets a little bigger.
- **More robust against untrusted devices on the network:** Before verification, the first message of a new
  connection may be at most 16 KiB and must arrive within 3 s. Previously, any device on the LAN could tie up
  64 MiB of memory per connection.

## [0.12.2] - 2026-10-20

### Fixed

- **Text copied from Chrome stayed on the computer:** On Windows, Qross also withheld content that's only meant to
  be kept out of the clipboard history (Win+V) and the cloud. Chrome, for example, marks copies from an Incognito
  window this way, even a simple URL; the log said “confidential content (marked with
  ‘CanIncludeInClipboardHistory’)”. Qross now only withholds what password managers like KeePass, KeePassXC and
  1Password clearly mark as confidential.
- **Jump to the center of the screen when switching:** When switching to a client, Qross parks the Windows cursor
  in the center. In the millisecond around that, a mouse movement could count as a jump of half a screen width: the
  cursor came right back (in the log: “largest step 1909 px, back on the server after 1.3 ms”) and landed in the
  center of the server's screen instead of at the edge. Qross now calculates movements around the parking from the
  point Windows calculated them from.

## [0.12.1] - 2026-10-20

### Fixed

- **Shortcuts made of a single modifier key:** If, for example, “Left Ctrl” was assigned as a shortcut, it
  triggered as soon as it was pressed. Ctrl+C then immediately switched to the other computer, into the middle of
  the screen, and the C ended up there instead of in the program; nothing was copied. A modifier key on its own now
  only takes effect when released, and only if no other key was pressed in between: Ctrl+C stays Ctrl+C. This
  applies to switching shortcuts and the cursor lock key. On Windows, a Win or Alt key tapped this way doesn't open
  the Start menu or the menu bar.

## [0.12.0] - 2026-10-20

### New

- **Progress in the tray and the taskbar:** While files are being sent or received, the tray icon shows a bar at
  the bottom instead of the status dot. The tooltip names the file, computer, percentage and time remaining, e.g.
  “Film.mkv to ‘laptop’: 42% · 21 s left”, and for multiple transfers the overall progress. In addition, the
  taskbar button (Windows, while the window is open or minimized) or the Dock icon (macOS) fills up. The bar only
  appears after one second, so it doesn't flash for small files. On Linux, the tray icon shows it; there are no
  tooltips there.

## [0.11.0] - 2026-10-20

### New

- **Custom title bar:** Minimize, Maximize and Close now sit on the right of the Qross header, in the same style as
  the rest of the interface, instead of in the system's bar above it. The window can be moved by dragging the
  header, and a double-click maximizes it. On macOS, the familiar traffic light buttons remain, embedded in the
  header.

### Improved

- **Windows: quiet in the background:** When Qross is in the tray or the window is minimized, the interface now
  rests completely. Previously, its animations kept running invisibly while the server or client was running,
  constantly using CPU time: around 10% of a CPU core in the measurement, now under 0.5%.
- **Quiet behind dialogs:** The interface's animations pause behind open settings, “What's new” and prompts.
  Without a graphics card, for example in a VM or over Remote Desktop, the blurred background otherwise took almost
  a whole CPU core.
- **Less work on every switch:** The tray icon now only updates what has actually changed, without holding up the
  connection. The interface redraws at most once per frame, and not at all while the window is hidden; when opened,
  it shows the latest state right away.
- **macOS: App Nap off only while running:** Qross now only turns off App Nap while the server or client is
  running. When Qross is stopped, macOS may save power again, which is easier on the battery.

### Fixed

- **Restless header without a server:** If the server couldn't be reached, the client's header kept jumping between
  “Connecting …” and “no connection”, rearranging itself each time. Now it calmly shows “Server unreachable”, with
  the reason in the tooltip, until the connection is established; the tray and the client view say the same.

## [0.10.0] - 2026-10-19

### New

- **Drag preview at the cursor:** When you drag files across the edge to another computer, a small card hangs from
  the cursor there, just like when dragging on your own computer: icon, file name (“3 files” for several) and total
  size. It follows the cursor until you release, go back without dropping, or the transfer is canceled; as before,
  files are dropped into the folder under the cursor. Works in both directions on Windows, macOS (in the app) and
  X11 with a compositor, but not over full-screen programs. On Wayland, no windows can be placed over other
  programs, so there's no preview there.

## [0.9.0] - 2026-10-19

### Improved

- **Tidier server view:** The layout fills the whole area. The sidebar now only shows the computers and the details
  of the selected one, in three tabs: Connection (latency, history, input delay), Permissions, and Info with Rename
  and Remove. You select a computer by clicking it in the list or in the layout; for the server itself, the details
  show its area, port and fingerprint. With multiple monitors, the sketch sits small below the label instead of
  behind it.
- **One place for connection requests:** A request appears as a narrow row above the computers. “Review” opens the
  dialog with the fingerprint, and the × ignores it.
- **Settings sorted by scope:** At the top, the “Server” and “Client” sections for the active profile, with
  everything that used to be in the main view: name, port, discoverability, corner dead zone, delay, permissions
  for new computers, fingerprints, test mode and the configuration as a file. Below that, what applies to all
  profiles.
- **Calmer keyboard page:** The shortcuts are shown as a list, each with a menu for Change and Remove, and
  unassigned ones with “Assign”. Reset to default and Turn off for the cursor lock key are in the same menu.
- **Compact client view:** At the top, the status with latency; below it, the connection quality as on the server,
  and the configured server. The search for other servers is collapsed while connected; name and test mode are
  under Settings → Client.
- **Small touches:** The settings gear sits to the right of Start and Stop. Long explanations are behind a “?”.

## [0.8.3] - 2026-10-19

### Fixed

- **Hitch at the edge of the Windows server when switching quickly:** If you moved the cursor swiftly from the
  Windows server to a client, it got caught at the server's edge, and the faster you moved, the longer it stayed
  there. Moving slowly or on the way back, nothing was noticeable. During fast movement, Windows doesn't report
  where the cursor is, but where it would be without the screen edge, i.e. a point beyond the edge. Since Qross
  became aware of multiple monitors (0.5.0), such a touch didn't count as the edge, and the switch only happened
  once your hand slowed down. Now the point on the monitor counts. The changes made for this in 0.8.1 and 0.8.2
  didn't address this cause.

### Improved

- **A log entry for every switch:** After every switch to a client, the server and the client each write a line to
  the log: how often the cursor ran against the edge, whether the movements afterward arrived and were injected
  without gaps, and whether any were lost before the cursor entered. It only appears at info level when something
  stands out.

## [0.8.2] - 2026-10-19

### Fixed

- **Keyboard with a Remote Desktop window open:** If a Remote Desktop client was in the foreground on the Windows
  server, keys kept going to the RDP session after switching to a client, while the cursor did move over. The RDP
  client has its own keyboard hook, and Windows asked it before the one from Qross. Qross now reinstalls its hooks
  on every switch, which puts it first in line again. This also helps with virtual machines and keyboard shortcut
  tools that have their own hook.
- **Windows server: switching without waiting for the mouse pointer:** Hiding and showing the mouse pointer briefly
  held up the forwarding of input. This now happens in parallel.

### Improved

- **Delays in the log:** If an input takes longer than 50 ms to reach the client, the server writes it to the log
  with its parts (server, network, client), at most once per second. If a step on the server holds up forwarding,
  or one on the client holds up injection, for more than 20 ms, that's logged too. This way, a hitch when switching
  can be traced to its cause, such as Wi-Fi power saving.

## [0.8.1] - 2026-10-19

### Fixed

- **Hang when switching quickly to a Mac:** If you moved the cursor swiftly from the server to a Mac, it briefly
  stopped there; moving slowly or on the way back, nothing was noticeable. Since 0.8.0, the Mac takes over the
  Caps Lock state on entry, and for that Qross set up a new connection to the input system every time. All the
  mouse movements that followed had to wait that long. The connection now stays open, and the query only takes a
  moment.
- **macOS: no more App Nap:** When Qross was running on a Mac only in the tray or as `qross client`, macOS also
  throttled it. Qross now turns off App Nap for as long as it runs, like Synergy. The Mac's screen saver, display
  sleep and system sleep remain allowed.

## [0.8.0] - 2026-10-19

### New

- **macOS: notice for “Secure Keyboard Entry”:** If an app on the Mac acting as the server turns on
  Secure Keyboard Entry – an active password field, Terminal with “Secure Keyboard Entry”, a password manager –
  Qross no longer receives any keystrokes. Previously, whatever you typed for another computer then ended up
  unnoticed in the frontmost app on the Mac. Now Qross brings the cursor back to the Mac at that moment and keeps
  it there until Secure Keyboard Entry is off again. The top of the window and the tray show which app is holding
  it and what helps (leave the password field, turn it off in Terminal, quit the app, lock and unlock if the login
  window is stuck). If the cursor is brought back for this reason or stops at the edge, a system notification
  appears once. Without a connected client, everything stays quiet. `qross server` holds the cursor the same way
  and writes it to the log.
- **Caps Lock, Num Lock and Scroll Lock in sync on all computers:** When the cursor enters a client, the client
  takes over the server's state. If you press Caps Lock while the cursor is on a client, the server toggles along:
  the keyboard lights are right, and after switching back, your typing doesn't suddenly come out in capitals. Like
  Synergy, with no setting. On Windows all three, on X11 Caps Lock and Num Lock. On a Mac, Qross toggles Caps Lock
  through the system, because a synthetic key press doesn't do that there; Macs have no Num Lock or Scroll Lock,
  and those stay as they are on the client. On Wayland only as a client, by key press and only as well as the
  compositor reports its state (see `docs/wayland.md`). Compatible: with clients and servers up to 0.7, the key
  only goes to the computer with the cursor, as before.
- **Cursor finder:** When switching, a ring flashes around the cursor for about a second on the computer where the
  cursor arrives, contracting around it, with the computer's name below. The ring and the name can be turned on
  separately (Settings → General or `[finder]` in `qross.toml`); the default is off. It's set on the server, which
  passes it on to all clients without reconnecting, and it also appears back on the server. The display follows the
  cursor, adapts to the monitor and scaling, lets clicks through, doesn't take focus and stays off when a program
  is running in full screen there. “Try it” shows it right away. Clients from Qross 0.8 on; older ones show
  nothing. On Wayland, nothing can be placed over other windows, so it isn't available there; on macOS only in the
  app, not with `qross` on the command line.
- **Screen saver sync, like Synergy:** When the screen saver starts on the server, the connected clients start
  theirs too; when you move the mouse or press a key on the server, they all end, even if the cursor is on a client
  at the time. The cursor stays where it is. Clients without a screen saver set up or with a video playing stay as
  they are. On by default, configurable under Settings → Screen saver or with `[screensaver]` in `qross.toml`.
- **Lock the clients too** (off by default): When the server's screen saver starts, the clients lock themselves
  instead of just starting their own.
- **Measuring input delay:** The connection quality now also shows how long an input really takes, from the server
  until it's injected on the client – separately for mouse movements and keys, as the median and 95th percentile of
  the last minute, split into server, network and client. A bar shows where the time goes, and the tooltip explains
  the parts. Measurements use samples (every 32nd mouse movement, every key press and click), without a single
  extra byte on the input path; the client reports its times back every 200 ms. Qross estimates the difference
  between the two computers' clocks like NTP; how accurate this makes the network share is shown alongside. With an
  older client, the server only shows its own share. Description in `docs/protokoll.md`.
- **Drag and drop from Mac and Linux servers:** Files can now also be dragged across the edge to a client from a
  Mac or a Linux computer with X11, not just from Windows: drag them with the button held down from Finder or the
  file manager to the edge and release them on the other side. Progress, canceling, going back without releasing
  and the permissions per computer work as usual. On X11, the reverse direction works too, from the Linux client to
  the server. On Wayland, drag and drop stays off: there, no program can see another program's drag-and-drop
  operations.
- **Dropping under the cursor on a Mac and on Linux too:** On a Mac, dragged files land in the folder of the Finder
  window under the cursor or on the desktop, on Linux (X11) on the desktop, otherwise in `Downloads/Qross` as
  before. Qross still doesn't drop directly into a program, e.g. as an attachment into an email.

### Improved

- **Settings in sections:** General, Profiles, Keyboard, Game mode, Screen saver, Updates, Clipboard and About are
  now each a page of their own instead of one long list to scroll through. In a narrow window, the sections appear
  as a row at the top; arrow keys switch between them, and the next time you open the settings, the last one you
  chose comes up.
- **Autostart reworked on all systems:** Qross now manages the entry itself, in the same places as before (Windows:
  startup apps, macOS: Login Items, Linux: `~/.config/autostart`). If you move Qross or the AppImage, the entry
  follows at the next start. On Windows, the path is in quotation marks (paths with spaces), and on macOS,
  Login Items show “Qross”. If it's turned off in the system (Task Manager, GNOME Tweaks), Qross shows it as off.
  Below the toggle, it says where to find the entry. Everything about it, including running `qross` without an
  interface as a service, is in `docs/autostart.md`.
- **Confirmation when changing roles:** If the server is running and you switch to client – via the toggle, a
  profile or the tray menu – Qross asks first, because the server shuts down in the process and connected computers
  lose the mouse and keyboard. The same applies when a connected client is about to become the server.
- **Linux server on X11 without fixed polling:** Capture no longer asks for the pointer every 4 ms but waits for
  the X server; it learns about mouse movements via XInput 2. As a result, edges and shortcuts take effect up to
  4 ms sooner, and when idle it sleeps instead of waking up around 500 times per second (cursor on a client: 940
  times). As on Windows and macOS, only pushing against the edge now counts toward the switch delay, not merely
  stopping there. Without XInput 2.1, it keeps polling.
- **Back faster after sleep and network changes:** Qross now watches for sleep, wake-up and network changes (Wi-Fi
  network changed, cable unplugged or plugged in, VPN) and no longer waits for a timeout. When the laptop wakes up,
  the client checks the connection immediately and, if necessary, re-establishes it without waiting – usually
  within a fraction of a second once the network is back. With a new address, the connection simply moves along,
  without being re-established. When a computer goes to sleep, it signs off first: the server brings the cursor
  back immediately instead of sending it into the void for 5 s, and a server going to sleep tells its clients. When
  the server wakes up, it disconnects clients that no longer respond. If the server can't be reached, the client
  retries about every second for the first minute, then less often. If name resolution hangs after waking up, it
  uses the address it last reached.

### Fixed

- **Windows: capture survives load spikes:** If the server PC briefly took too long to process an input, Windows
  removed the input hooks without any notice. Qross then received no more input and stopped switching until it was
  restarted. A watchdog now notices this after about two seconds, reinstalls the hooks and writes it to the log
  with a counter. If the cursor is on a client at the time, Qross releases all keys and mouse buttons there so
  nothing gets stuck. The lock screen, UAC, windows with administrator rights and sleep don't trigger false alarms.
  As a precaution, the hook itself never waits anymore: events go to the server core through a lock-free queue, and
  the hook thread runs at the highest priority, even in the background.
- **“A client named … is already connected”:** If a computer reconnected while the server was still holding on to
  the old connection (after waking up, a network change or a restart of Qross), the server rejected it until the
  old one expired after a few seconds. Now the new connection replaces the old one immediately, provided it comes
  from the same computer (same key). If two clients with the same name and key are accidentally running, the newer
  one wins, and the older one stops with a message.
- **macOS: autostart from the DMG:** If Qross was first started directly from the DMG or from the quarantined copy,
  the autostart entry afterward pointed nowhere. Now it only adds itself once it's running from the “Applications”
  folder, and explains why in the settings.

### Important

- If a client asks for a password after its screen saver, or “Lock the clients too” is on, it is locked afterward.
  You have to unlock it at its own keyboard: Qross can't type into the lock screen. On KDE, the screen saver is
  always the lock screen.
- Clients follow the screen saver from this version on. Older clients stay connected and unchanged.

## [0.7.0] - 2026-10-18

### New

- **Profiles per location:** multiple named profiles, e.g. “Office” and “Home”, each with its own role, screen
  layout, name and port, switching settings and server address. Switch profiles in the header or the tray menu;
  create, rename, duplicate and delete them under Settings → Profiles. Keyboard, game mode, clipboard, updates and
  autostart still apply to all profiles.
- **Automatic profile by network:** Each profile can be tied to one or more networks (“Add current network”). Qross
  recognizes the network by its router, from the gateway's address and MAC address, without extra permissions. It
  switches at startup and whenever the network changes, restarts the server or client with the new profile and
  reports the switch. If no profile matches, a selectable profile for other networks applies. Can be turned off per
  profile and globally, also in the tray menu.
- **Permissions per computer:** The server decides for each client what is shared: clipboard (text, HTML, image),
  copied files, and drag and drop, each as send and receive, receive only, send only or not at all. A default
  applies to newly added computers. Configurable in the server view under “Permissions per computer” or with
  `[sharing]` in `qross.toml`. Changes take effect immediately without disconnecting; running transfers that are no
  longer allowed are canceled.
- Restricted computers carry a badge in the layout. Clients show what the server restricts for them and don't even
  try: where drag and drop isn't allowed, the cursor doesn't switch with the button held down either.
- **Characters instead of key positions, per computer:** If a computer is set to “Characters” under
  Settings → Keyboard, what you type on the server arrives there, even if it has a different keyboard layout (e.g.
  server German, Mac US): Y and Z, umlauts, @, €, brackets, dead keys like ^ and ´, and all AltGr characters.
  Shortcuts like Ctrl+Z follow the character and remain “Undo” there; arrows, F keys and the like stay keys. The
  default remains “Key position”, which games need. In `qross.toml`: `[keys.mode]`.
- **Passwords stay where they were copied:** Qross doesn't share content that a password manager like KeePassXC
  marks as confidential with other computers, whether as text, image or file list, in either direction. The other
  side keeps its previous content, and the next normal content is shared again. Qross recognizes the markers of
  Windows (history, cloud clipboard, clipboard programs), macOS (`org.nspasteboard.ConcealedType` and
  `TransientType`) and Linux (`x-kde-passwordManagerHint`). A short notice in the window and in the log says when
  something was withheld, without showing the content. Can be turned off under Settings → Clipboard or with
  `[clipboard] share_concealed = true`.

### Fixed

- In the settings, jumping to a section pushed the window's header out of view.

### Important

- On first start, the existing settings become the “Default” profile. The profiles are stored in `profiles.json`
  next to `gui.json`; an older version keeps loading `gui.json` and leaves the profiles alone.
- Qross shows the Wi-Fi name on Windows and Linux (NetworkManager). Since version 14, macOS only reveals it with
  Location Services; there, Qross shows the subnet and still recognizes the network.
- The server also enforces the permissions for clients running Qross 0.5 and 0.6: whatever they send anyway, it
  rejects without leaving anything behind. Only clients from this version on can display the permissions.
- Only computers with the new Qross on Windows, macOS or X11 receive characters. Older clients and clients on
  Wayland still receive key positions; the interface shows this for the computer. A server on Wayland always sends
  key positions.

## [0.6.0] - 2026-10-18

### New

- **Game mode:** When a game, video or presentation runs in full screen on the computer with the cursor, the cursor
  stays there until it ends. The cursor lock key releases it immediately. The interface shows why the cursor is
  being held. Can be turned off in the settings or with `[game] enabled = false`.
- Games on a client get relative mouse movements as if from a real mouse, including games that capture the pointer.
  This way, the camera keeps turning even at the edge of the screen.
- **Keyboard shortcuts for switching:** freely assignable shortcuts jump directly to a computer (the cursor lands
  in its center) or to the neighbor in a direction, e.g. Ctrl+Alt+1 or Ctrl+Alt+Left Arrow. Configurable under
  Settings → Keyboard or `[keys.switch]` in `qross.toml`.
- **Progress for file transfers:** Drag and drop and copied files show percentage, rate and time remaining, and can
  be canceled on both sides without leaving anything behind.
- **Linux with Wayland** (GNOME 45 or later, KDE Plasma 6.1 or later): mouse and keyboard go through the desktop
  portals. At first start, the system asks for permission once; where the desktop supports it, Qross remembers it.
  As a server, Qross captures input via InputCapture (also Hyprland 0.56 or later). Without a suitable portal, the
  client injects via XWayland. The interface explains when the portal is missing or permission was denied.
- **macOS: notice for missing permissions:** If Qross lacks “Accessibility” or “Input Monitoring”, it shows this at
  the top of the window, with a button that adds Qross to the list in System Settings and opens the right page.
  Previously, mouse and keyboard input simply didn't arrive.

### Improved

- After the emergency exit, the interface shows the cursor lock key as released again.

### Fixed

- **macOS: permissions survive updates:** Qross is now signed with its own permanent certificate instead of just
  ad hoc. Previously, macOS treated every new version as a different app: Qross stayed switched on under
  “Accessibility” and “Input Monitoring”, but the permissions no longer took effect.

### Important

- Mac users have to grant the permissions again one last time after this update: remove Qross under
  System Settings → Privacy & Security → Accessibility (and, as a server, Input Monitoring) with “−” and add it
  again via the notice in Qross. From then on, they're kept across updates.
- Computers with Qross 0.5 stay connected. With them, game mode only applies to full screen on the server itself.
- On Wayland, the cursor lock key only works while the cursor is on a client, drag and drop doesn't work there yet,
  and on GNOME the clipboard goes through XWayland.

## [0.5.0] - 2026-10-18

### New

- **Multiple monitors per computer:** Qross now knows each monitor of a computer individually. The layout editor
  shows them to scale in the computer's box, with the resolutions in the tooltip.
- **Connection quality:** For each connection, Qross shows latency, jitter, packet loss and throughput, plus a
  rating from “Very good” to “Poor” and a history of the last minute. On the server, this appears per client under
  “Connected computers”; on the client, in the status. It's measured without extra network traffic.

### Improved

- With monitors of different sizes or offset from each other, the cursor stays on actual screen areas instead of
  getting stuck in dead corners of the bounding rectangle. Switching only happens where a monitor actually lies at
  the edge, and the cursor arrives on the nearest monitor.

### Important

- A client only reports its individual monitors to a server running 0.5.0 or later. With older servers, it stays
  with the bounding rectangle, and the connection keeps working.

## [0.4.0] - 2026-10-18

### New

- **Automatic updates:** Qross checks for a new version at startup and daily after that. It downloads it in the
  background, verifies its signature and installs it at a quiet moment, never in the middle of a remote-control
  session. Qross then restarts and resumes the server or client.
- **Start with the system:** Qross starts unobtrusively in the tray when you sign in and resumes whatever was
  running last.
- **Settings:** a dedicated section for autostart, updates, clipboard and keyboard that grows with future options.
- **Drag and drop in both directions:** Files can now also be dragged from the client to the server. Grab them in
  File Explorer or Finder, drag them across the edge and release them on the server. They land in the folder under
  the cursor (Windows) or in “Downloads/Qross”.
- **Freely selectable cursor lock key:** Instead of the Scroll Lock key, e.g. Ctrl+Alt+L, also on the Mac, which
  has no Scroll Lock key.
- **Media keys:** Play/pause, next, previous and stop are passed along. On the Mac, volume and playback now
  actually work. Optionally, media keys always stay on the server.
- **Remap keys per computer**, e.g. swap Alt and the Windows key for a Mac.

### Fixed

- If you dragged something on the client with the mouse button held down to the edge toward the server, Qross
  switched back immediately, and the client released the button. Dragged files then ended up at the edge of the
  screen unintentionally. With the button held down, Qross now only switches when files are being dragged.

### Important

- Automatic updates are available from this version on. If you're using an older version, install 0.4.0 manually
  once.
- Drag and drop to the server requires Qross 0.4.0 on both computers. “Media keys always on the server” works when
  the server runs on Windows.

## [0.3.1] - 2026-10-18

### Fixed

- macOS: After downloading, macOS reported “Qross is damaged and can't be opened”. The app bundle is now fully
  signed, and macOS instead asks whether Qross should be opened.

### Improved

- The first-launch instructions describe the way via “Open Anyway” in System Settings, as current macOS versions
  require.

## [0.3.0] - 2026-10-18

### New

- **Files via the clipboard:** Copy files and folders in File Explorer, Finder or a file manager, move the cursor
  to the other computer and paste them there. This works in both directions and, via the server, also from one
  client to another.
- The transfer runs in the background, and mouse and keyboard stay smooth. Every file is verified with SHA-256, and
  pasting is only possible once everything has arrived completely.
- Up to 1 GB this happens automatically; the limit is configurable (0 = off). Use drag and drop for larger files,
  as before.
- The app tells you as soon as the files are ready to paste, or if they're too large. If the window is in the
  background, a system notification appears.

### Important

- Files on the clipboard require Qross 0.3.0 on both computers. Computers with 0.2 stay connected; text, formatted
  text and images keep working with them.

## [0.2.1] - 2026-10-18

### New

- The app shows what changed in each version under “What's new” – after an update, with a notice about the new
  version.

### Improved

- The release notes on GitHub now come directly from this changelog.

## [0.2.0] - 2026-10-18

### New

- The clipboard now transfers **images** (e.g. screenshots with Win+Shift+S) and **formatted text** (HTML from a
  browser or Office) – in addition to plain text, and in both directions.

### Improved

- Large images are converted in the background and transferred at low priority. Mouse and keyboard stay smooth
  throughout.
- On Windows, an image is also placed on the clipboard as a bitmap so that older programs like Paint can paste it
  too.

### Important

- New protocol version 3: all computers need Qross 0.2.0 or later. Older versions are rejected with a clear
  message.

## [0.1.2] - 2026-10-18

### Improved

- The logo in the header is larger, and the two screens in the artwork are now easy to make out.

## [0.1.1] - 2026-10-18

### Improved

- In the logo, the pointer tip now sits exactly at the end of the trail.
- The app's header shows the full logo.

### Fixed

- Linux without a tray icon (e.g. GNOME without the AppIndicator extension): closing now quits the app cleanly
  instead of leaving it running invisibly.

## [0.1.0] - 2026-10-18

Initial release.

### New

- Share mouse and keyboard across multiple computers – over **QUIC** instead of TCP, so the cursor stays smooth
  even with packet loss on Wi-Fi.
- **Drag and drop** of large files and entire folders across the screen edge, with SHA-256 verification.
- Text clipboard in both directions.
- **Server discovery on the network**: clients find the server without entering an IP address.
- **Corner dead zone** and optional **switch delay** against accidental switching.
- Ctrl and Cmd are swapped automatically between PC and Mac.
- **Scroll Lock** keeps the cursor on the current screen, **Ctrl+Alt+Shift+Esc** brings mouse and keyboard back
  immediately.
- Secure connection with mutual certificates: new computers are only approved after comparing fingerprints.
- Graphical interface with layout editor, approval dialog and tray icon.
- Runs on Windows, Linux (X11) and macOS (Apple Silicon).

[0.15.0]: https://github.com/xozy22/qross/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/xozy22/qross/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/xozy22/qross/compare/v0.12.2...v0.13.0
[0.12.2]: https://github.com/xozy22/qross/compare/v0.12.1...v0.12.2
[0.12.1]: https://github.com/xozy22/qross/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/xozy22/qross/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/xozy22/qross/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/xozy22/qross/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/xozy22/qross/compare/v0.8.3...v0.9.0
[0.8.3]: https://github.com/xozy22/qross/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/xozy22/qross/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/xozy22/qross/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/xozy22/qross/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/xozy22/qross/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/xozy22/qross/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/xozy22/qross/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/xozy22/qross/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/xozy22/qross/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/xozy22/qross/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/xozy22/qross/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/xozy22/qross/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/xozy22/qross/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/xozy22/qross/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/xozy22/qross/releases/tag/v0.1.0
