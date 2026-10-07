# Kumi's hands (made by Kumi). One JSON request a line on stdin, one JSON answer a line on stdout.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public static class KumiInput {
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  // As big as its largest member, MOUSEINPUT (32 bytes on 64-bit Windows): SendInput refuses an INPUT of any
  // other size, so at 24 every key was dropped while the helper said it was pressed (#187).
  [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION { [FieldOffset(0)] public KEYBDINPUT ki; [FieldOffset(0)] public long padding0; [FieldOffset(8)] public long padding1; [FieldOffset(16)] public long padding2; [FieldOffset(24)] public long padding3; }
  [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION u; }
  [DllImport("user32.dll")] public static extern uint SendInput(uint count, INPUT[] inputs, int size);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint attach, uint attachTo, bool on);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr menu);
  [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr menu, int position);
  [DllImport("user32.dll")] public static extern uint GetMenuItemID(IntPtr menu, int position);
  [DllImport("user32.dll")] public static extern uint GetMenuState(IntPtr menu, uint item, uint flags);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetMenuString(IntPtr menu, uint item, StringBuilder text, int max, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam, uint flags, uint timeout, out IntPtr result);
  public delegate bool Visit(IntPtr hwnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(Visit visit, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr hwnd, uint command);
  [DllImport("user32.dll")] public static extern IntPtr GetLastActivePopup(IntPtr hwnd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);

  // 40 bytes in a 64-bit process, 28 in a 32-bit one, which this layout doesn't make: its keys are refused.
  static readonly bool InputFits = Marshal.SizeOf(typeof(INPUT)) == (IntPtr.Size == 8 ? 40 : 28);
  public static bool Key(ushort vk, bool up) {
    if (!InputFits) return false;
    var input = new INPUT { type = 1 };
    input.u.ki = new KEYBDINPUT { wVk = vk, dwFlags = up ? 2u : 0u };
    return SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT))) == 1;
  }

  // To the front, for keys to land there: Windows lets a background process do it only while attached to
  // the foreground window's input, and does it a moment later, so it's waited for (#187).
  public static bool Front(IntPtr hwnd) {
    if (hwnd == IntPtr.Zero) return false;
    if (IsIconic(hwnd)) ShowWindow(hwnd, 9);
    for (int attempt = 0; attempt < 3 && GetForegroundWindow() != hwnd; attempt++) {
      uint pid;
      uint theirs = GetWindowThreadProcessId(GetForegroundWindow(), out pid);
      uint mine = GetCurrentThreadId();
      bool attached = theirs != 0 && theirs != mine && AttachThreadInput(mine, theirs, true);
      try { BringWindowToTop(hwnd); SetForegroundWindow(hwnd); }
      finally { if (attached) AttachThreadInput(mine, theirs, false); }
      for (int wait = 0; wait < 20 && GetForegroundWindow() != hwnd; wait++) Thread.Sleep(25);
    }
    return GetForegroundWindow() == hwnd;
  }

  // Live's menu bar on Windows is a Win32 menu, which its UI Automation tree leaves out (#192).
  public class MenuEntry { public string[] path; public uint id; public bool enabled; public string key; }
  const uint ByPosition = 0x400, Grayed = 0x1, Disabled = 0x2, Separator = 0x800;
  static string[] Title(IntPtr menu, int position) {
    var text = new StringBuilder(512);
    GetMenuString(menu, (uint)position, text, text.Capacity, ByPosition);
    var raw = text.ToString();
    var tab = raw.IndexOf('\t');
    var title = (tab < 0 ? raw : raw.Substring(0, tab)).Replace("&&", "\u0001").Replace("&", "").Replace("\u0001", "&").Trim();
    return new[] { title, tab < 0 ? "" : raw.Substring(tab + 1).Trim() };
  }
  public static List<MenuEntry> Menus(IntPtr window) {
    var bar = GetMenu(window);
    if (bar == IntPtr.Zero) return null;
    var entries = new List<MenuEntry>();
    int count = GetMenuItemCount(bar);
    for (int i = 0; i < count; i++) {
      var menu = GetSubMenu(bar, i);
      if (menu != IntPtr.Zero) Walk(window, menu, i, new List<string> { Title(bar, i)[0] }, entries, 0);
    }
    return entries;
  }
  static void Walk(IntPtr window, IntPtr menu, int position, List<string> path, List<MenuEntry> entries, int depth) {
    // Live fills a menu in as it opens (titles that follow the selection, what's greyed out): asked to, as then.
    IntPtr result;
    SendMessageTimeout(window, 0x117, menu, (IntPtr)position, 0x2, 500, out result);
    int count = GetMenuItemCount(menu);
    for (int i = 0; i < count; i++) {
      uint state = GetMenuState(menu, (uint)i, ByPosition);
      if ((state & Separator) != 0) continue;
      var title = Title(menu, i);
      if (title[0].Length == 0) continue;
      var here = new List<string>(path) { title[0] };
      var sub = GetSubMenu(menu, i);
      if (sub != IntPtr.Zero) { if (depth < 3) Walk(window, sub, i, here, entries, depth + 1); continue; }
      entries.Add(new MenuEntry { path = here.ToArray(), id = GetMenuItemID(menu, i), enabled = (state & (Grayed | Disabled)) == 0, key = title[1] });
    }
  }
  // Chosen as a click would choose it, without Live in front, and without waiting for a dialog it opens.
  public static bool Choose(IntPtr window, uint id) { return PostMessage(window, 0x111, (IntPtr)id, IntPtr.Zero); }

  // The dialog Live has open: a modal one disables the main window, and is its last active popup; otherwise
  // a standard dialog (#32770) of Live's process that the main window owns (#187). Any other owned window is
  // a palette or a plugin's editor, never a dialog to read, fill or answer.
  public static IntPtr Dialog(IntPtr main, uint pid) {
    if (main == IntPtr.Zero) return IntPtr.Zero;
    if (!IsWindowEnabled(main)) {
      var popup = GetLastActivePopup(main);
      if (popup != IntPtr.Zero && popup != main && IsWindowVisible(popup)) return popup;
    }
    IntPtr found = IntPtr.Zero;
    EnumWindows((hwnd, unused) => {
      uint owner;
      GetWindowThreadProcessId(hwnd, out owner);
      if (owner == pid && hwnd != main && IsWindowVisible(hwnd) && GetWindow(hwnd, 4) == main && IsWindowEnabled(hwnd) && ClassOf(hwnd) == "#32770") { found = hwnd; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  static string ClassOf(IntPtr hwnd) { var name = new StringBuilder(256); GetClassName(hwnd, name, name.Capacity); return name.ToString(); }

  // Windows' Save and Open dialogs, told apart by their file name box whatever the language (#189): Save's
  // is an edit box with id 1001, Open's one with id 1148 (in a combo box). "pending" while one is still
  // putting its controls up; "" for any other dialog, an older style one included, which Kumi doesn't fill.
  public static string FileKind(IntPtr dialog) {
    bool modern = false; string kind = "";
    EnumChildWindows(dialog, (hwnd, unused) => {
      var name = ClassOf(hwnd);
      if (name == "DUIViewWndClassName") modern = true;
      if (name == "Edit" && IsWindowVisible(hwnd)) {
        int id = GetDlgCtrlID(hwnd);
        if (id == 1001 && kind == "") kind = "save";
        else if (id == 1148 && kind == "") kind = "open";
      }
      return true;
    }, IntPtr.Zero);
    if (!modern) return "";
    return kind == "" ? "pending" : kind;
  }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, StringBuilder text, int max);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, Visit visit, IntPtr lParam);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr hwnd, int index);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint message, IntPtr wParam, string lParam, uint flags, uint timeout, out IntPtr result);
  public static string Text(IntPtr hwnd) { var text = new StringBuilder(512); GetWindowText(hwnd, text, text.Capacity); return text.ToString(); }
  static string Plain(string text) { return text.Replace("&&", "\u0001").Replace("&", "").Replace("\u0001", "&").Trim(); }

  // A standard dialog's own controls, read from Win32: UI Automation can show its push buttons as panes no
  // one can press (#187). Buttons (push buttons only), words (static text), and the file name box.
  public class Part { public IntPtr hwnd; public int id; public string text; public string kind; }
  public static List<Part> Parts(IntPtr dialog) {
    var parts = new List<Part>();
    EnumChildWindows(dialog, (hwnd, unused) => {
      if (!IsWindowVisible(hwnd)) return true;
      var name = new StringBuilder(64);
      GetClassName(hwnd, name, name.Capacity);
      var kind = name.ToString();
      int style = GetWindowLong(hwnd, -16) & 0xF;
      if (kind == "Button" && (style == 0 || style == 1)) parts.Add(new Part { hwnd = hwnd, id = GetDlgCtrlID(hwnd), text = Plain(Text(hwnd)), kind = "button" });
      else if (kind == "Static" && Text(hwnd).Trim().Length > 0) parts.Add(new Part { hwnd = hwnd, id = GetDlgCtrlID(hwnd), text = Text(hwnd).Trim(), kind = "words" });
      else if (kind == "Edit") parts.Add(new Part { hwnd = hwnd, id = GetDlgCtrlID(hwnd), text = "", kind = "edit" });
      return true;
    }, IntPtr.Zero);
    return parts;
  }
  // Pressed as a click presses it: the dialog is told the button was clicked, without anything in front.
  public static bool Click(IntPtr dialog, Part button) { return PostMessage(dialog, 0x111, (IntPtr)(button.id & 0xFFFF), button.hwnd); }
  public static bool SetText(IntPtr hwnd, string text) { IntPtr result; return SendMessageTimeout(hwnd, 0x000C, IntPtr.Zero, text, 0x2, 1000, out result) != IntPtr.Zero; }
}
"@
$version = 4
$auto = [System.Windows.Automation.AutomationElement]
$tree = [System.Windows.Automation.TreeScope]
$types = [System.Windows.Automation.ControlType]

# Answers in ASCII, past it as \u escapes: Windows PowerShell writes in the console's code page, and Kumi reads UTF-8.
# Requests come the same way. This file stays ASCII too: without a BOM, Windows PowerShell reads it in that code page.
function Emit($object) {
  $json = $object | ConvertTo-Json -Compress -Depth 8
  [Console]::Out.WriteLine([regex]::Replace($json, '[^\u0000-\u007F]', { param($found) '\u{0:x4}' -f [int][char]$found.Value }))
  [Console]::Out.Flush()
}
function LiveProcess {
  # A test points the helper at a window of its own.
  if ($env:KUMI_HANDS_PID) { return Get-Process -Id ([int]$env:KUMI_HANDS_PID) -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1 }
  Get-Process | Where-Object { $_.ProcessName -like 'Ableton Live*' -and $_.MainWindowHandle -ne 0 } | Select-Object -First 1
}
function LiveWindow($process) { $auto::FromHandle($process.MainWindowHandle) }
function Kids($element) { $element.FindAll($tree::Children, [System.Windows.Automation.Condition]::TrueCondition) }
# A title as Live may say it: "Freeze Track" and "Freeze Tracks" (it changes with the selection) are one.
function Norm($text) { return (([string]$text) -replace '&', '' -replace '(\.\.\.|\u2026)$', '' -replace '\b(Track|Clip|Scene)s\b', '$1').Trim().ToLower() }
function Named($element, $name) {
  $all = @(Kids $element)
  $wanted = Norm $name
  $exact = $all | Where-Object { (Norm $_.Current.Name) -eq $wanted } | Select-Object -First 1
  if ($exact) { return $exact }
  return $all | Where-Object { (Norm $_.Current.Name).StartsWith($wanted) } | Select-Object -First 1
}
function Expand($element) { try { $element.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand(); Start-Sleep -Milliseconds 40 } catch {} }
function Collapse($element) { try { $element.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Collapse() } catch {} }
function Press($element) {
  try { $element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); return $true } catch {}
  try { $element.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle(); return $true } catch {}
  return $false
}
function MenuBar($window) { $window.FindFirst($tree::Descendants, (New-Object System.Windows.Automation.PropertyCondition($auto::ControlTypeProperty, $types::MenuBar))) }
function Win32Menus($process) { [KumiInput]::Menus($process.MainWindowHandle) }
# The Win32 menu item a path names, its last title matched as Live may word it (or one of the other titles).
function Win32Item($entries, $path, $titles) {
  $wanted = @($path | ForEach-Object { Norm $_ })
  foreach ($entry in $entries) {
    if ($entry.path.Count -ne $wanted.Count) { continue }
    $same = $true
    for ($i = 0; $i -lt $wanted.Count; $i++) { if ((Norm $entry.path[$i]) -ne $wanted[$i]) { $same = $false; break } }
    if ($same) { return $entry }
  }
  foreach ($title in @(@($path)[-1]) + @($titles)) {
    if (-not $title) { continue }
    $one = Norm $title
    foreach ($entry in $entries) { if ((Norm $entry.path[-1]) -eq $one) { return $entry } }
    foreach ($entry in $entries) { if ((Norm $entry.path[-1]).StartsWith($one)) { return $entry } }
  }
  return $null
}
# The dialog Live has open, as UI Automation sees it, or $null.
function DialogOf($process) {
  $hwnd = [KumiInput]::Dialog($process.MainWindowHandle, [uint32]$process.Id)
  if ($hwnd -eq [IntPtr]::Zero) { return $null }
  return @{ hwnd = $hwnd; element = $auto::FromHandle($hwnd) }
}
# A custom dialog's buttons as UI Automation sees them; a standard one's are read from Win32 (Parts).
function Buttons($element) { @($element.FindAll($tree::Descendants, (New-Object System.Windows.Automation.PropertyCondition($auto::ControlTypeProperty, $types::Button)))) }
function Parts($dialog, $kind) { @([KumiInput]::Parts($dialog.hwnd) | Where-Object { $_.kind -eq $kind }) }
# Windows says Yes / No / Cancel where macOS says Save / Don't Save / Cancel: on Live's save-changes prompt
# either is taken (#187). Only there: Save or OK on Windows' "already exists, replace it?" isn't its Yes.
$aliases = @{ "don't save" = @('no', 'discard'); 'dont save' = @('no', 'discard'); 'discard' = @('no', "don't save"); 'save' = @('yes'); 'yes' = @('save'); 'no' = @("don't save", 'discard'); 'cancel' = @() }
function Words($dialog) {
  $words = @(Parts $dialog 'words' | ForEach-Object { $_.text })
  if ($words.Count -eq 0) { $words = @($dialog.element.FindAll($tree::Descendants, (New-Object System.Windows.Automation.PropertyCondition($auto::ControlTypeProperty, $types::Text))) | ForEach-Object { $_.Current.Name } | Where-Object { $_ }) }
  return $words
}
function SavesChanges($dialog) { return ((Words $dialog) -join ' ') -match '(?i)save changes|before closing' }
# The button a name stands for, as a Win32 part or a UI Automation element, or $null.
function ButtonNamed($dialog, $name) {
  $wanted = @(Norm $name)
  if ($aliases.ContainsKey((Norm $name)) -and (SavesChanges $dialog)) { $wanted += @($aliases[(Norm $name)]) }
  $parts = Parts $dialog 'button'
  $all = Buttons $dialog.element
  foreach ($one in $wanted) {
    $hit = $parts | Where-Object { (Norm $_.text) -eq $one } | Select-Object -First 1
    if ($hit) { return @{ part = $hit; name = $hit.text } }
    $hit = $all | Where-Object { (Norm $_.Current.Name) -eq $one -or (Norm $_.Current.AutomationId) -eq $one } | Select-Object -First 1
    if ($hit) { return @{ element = $hit; name = $hit.Current.Name } }
  }
  return $null
}
function PressButton($dialog, $button) {
  if ($button.part) { return [KumiInput]::Click($dialog.hwnd, $button.part) }
  return Press $button.element
}
$vk = @{ cmd = 0x11; ctrl = 0x11; control = 0x11; shift = 0x10; alt = 0x12; option = 0x12; return = 0x0D; enter = 0x0D; tab = 0x09; space = 0x20; escape = 0x1B; esc = 0x1B;
  delete = 0x2E; backspace = 0x08; left = 0x25; up = 0x26; right = 0x27; down = 0x28; home = 0x24; end = 0x23; pageup = 0x21; pagedown = 0x22 }
function Combo($combo) {
  $mods = @(); $key = $null
  foreach ($part in $combo.ToLower().Split('+')) {
    if (@('cmd','ctrl','control','shift','alt','option') -contains $part) { $mods += $vk[$part] }
    elseif ($vk.ContainsKey($part)) { $key = $vk[$part] }
    elseif ($part.Length -eq 1) { $key = [int][char]$part.ToUpper() }
    elseif ($part -match '^f(\d+)$') { $key = 0x6F + [int]$Matches[1] }
  }
  if ($null -eq $key) { return 'unknown' }
  # Held as a hand would hold them: a key down and up at once is missed (#187).
  $sent = $true
  foreach ($m in $mods) { $sent = [KumiInput]::Key([uint16]$m, $false) -and $sent; Start-Sleep -Milliseconds 15 }
  $sent = [KumiInput]::Key([uint16]$key, $false) -and $sent; Start-Sleep -Milliseconds 50; $sent = [KumiInput]::Key([uint16]$key, $true) -and $sent
  foreach ($m in $mods) { Start-Sleep -Milliseconds 15; $sent = [KumiInput]::Key([uint16]$m, $true) -and $sent }
  if ($sent) { return 'sent' } else { return 'refused' }
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
  try { $request = $line | ConvertFrom-Json } catch { continue }
  $started = Get-Date
  $answer = @{ id = $request.id }
  function Done($fields) { foreach ($k in $fields.Keys) { $answer[$k] = $fields[$k] }; $answer.ms = [int]((Get-Date) - $started).TotalMilliseconds; Emit $answer }
  try {
    # (continue inside a switch only ends the switch, so these two are ifs.)
    if ($request.op -eq 'version') { Done @{ ok = $true; version = $version }; continue }
    if ($request.op -eq 'trusted') { Done @{ ok = $true; trusted = $true }; continue }
    $process = LiveProcess
    if (-not $process) { Done @{ ok = $false; error = 'no-live' }; continue }
    $window = LiveWindow $process
    switch ($request.op) {
      'menus' {
        $entries = Win32Menus $process
        if ($entries) {
          Done @{ ok = $true; items = @($entries | ForEach-Object { $item = @{ path = @($_.path); enabled = $_.enabled }; if ($_.key) { $item.key = $_.key }; $item }) }
          break
        }
        $bar = MenuBar $window
        if (-not $bar) { Done @{ ok = $false; error = 'no-menus' }; break }
        $items = @()
        foreach ($top in @(Kids $bar)) {
          Expand $top
          foreach ($entry in @($top.FindAll($tree::Descendants, (New-Object System.Windows.Automation.PropertyCondition($auto::ControlTypeProperty, $types::MenuItem))))) {
            if ($entry.Current.Name) { $items += @{ path = @($top.Current.Name, $entry.Current.Name); enabled = $entry.Current.IsEnabled } }
          }
          Collapse $top
        }
        Done @{ ok = $true; items = $items }
      }
      'tracks' {
        # Selected as a screen reader selects them: the track headers focused, their rows chosen.
        $headers = $window.FindFirst($tree::Descendants, (New-Object System.Windows.Automation.PropertyCondition($auto::NameProperty, 'Track Headers')))
        if (-not $headers) { Done @{ ok = $false; error = 'no-track-headers' }; break }
        $rows = @(Kids $headers)
        $labels = @($rows | ForEach-Object { [string]$_.Current.Name })
        $picked = @(); $missing = @()
        foreach ($entry in @($request.tracks)) {
          $nth = if ($null -ne $entry.nth) { [int]$entry.nth } else { 0 }
          $whole = @(); $stated = @()
          for ($i = 0; $i -lt $labels.Count; $i++) {
            if ($labels[$i] -eq $entry.name) { $whole += $i } elseif ($labels[$i].StartsWith("$($entry.name), ")) { $stated += $i }
          }
          $found = if ($whole.Count -gt $nth) { $whole } else { @($whole + $stated | Sort-Object) }
          if ($found.Count -gt $nth) { $picked += $rows[$found[$nth]] } else { $missing += $entry.name }
        }
        if ($missing.Count -gt 0 -or $picked.Count -eq 0) { Done @{ ok = $false; error = 'no-track'; missing = $missing }; break }
        try { $headers.SetFocus() } catch {}
        for ($i = 0; $i -lt $picked.Count; $i++) {
          $pattern = $picked[$i].GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
          if ($i -eq 0) { $pattern.Select() } else { $pattern.AddToSelection() }
        }
        Done @{ ok = $true; selected = @($picked | ForEach-Object { $_.Current.Name }) }
      }
      'menu' {
        $entries = Win32Menus $process
        if ($entries) {
          $item = Win32Item $entries @($request.path) @($request.titles)
          if (-not $item) { Done @{ ok = $false; error = 'no-item' }; break }
          # Live's menus catch up with a new selection a moment later: a greyed-out item is looked at again before it counts.
          for ($look = 0; $look -lt 8 -and -not $item.enabled; $look++) {
            Start-Sleep -Milliseconds 60
            $again = Win32Item (Win32Menus $process) @($item.path) @()
            if ($again) { $item = $again }
          }
          if (-not $item.enabled) { Done @{ ok = $false; error = 'disabled'; title = $item.path[-1] }; break }
          if ([KumiInput]::Choose($process.MainWindowHandle, $item.id)) { Done @{ ok = $true; title = $item.path[-1] } } else { Done @{ ok = $false; error = 'press-failed' } }
          break
        }
        $bar = MenuBar $window
        $previous = [KumiInput]::GetForegroundWindow()
        [KumiInput]::Front($process.MainWindowHandle) | Out-Null
        $current = $bar; $found = $true
        $last = @($request.path).Count - 1
        for ($step = 0; $step -le $last; $step++) {
          $name = @($request.path)[$step]
          if (-not $current) { $found = $false; break }
          Expand $current
          $next = Named $current $name
          if (-not $next) { $next = $current.FindFirst($tree::Descendants, (New-Object System.Windows.Automation.PropertyCondition($auto::NameProperty, $name))) }
          # Other titles the item may have now (Live retitles some with the selection: Freeze Track, Unfreeze Track).
          if (-not $next -and $step -eq $last) { foreach ($other in @($request.titles)) { if ($other) { $next = Named $current $other; if ($next) { break } } } }
          if (-not $next) { $found = $false; break }
          $current = $next
        }
        if (-not $found) { Done @{ ok = $false; error = 'no-item' }; break }
        for ($look = 0; $look -lt 15 -and -not $current.Current.IsEnabled; $look++) { Start-Sleep -Milliseconds 40 }
        if (-not $current.Current.IsEnabled) { Done @{ ok = $false; error = 'disabled'; title = $current.Current.Name }; break }
        $title = $current.Current.Name
        $pressed = Press $current
        Start-Sleep -Milliseconds 60
        [KumiInput]::Front($previous) | Out-Null
        if ($pressed) { Done @{ ok = $true; title = $title } } else { Done @{ ok = $false; error = 'press-failed' } }
      }
      'keys' {
        $previous = [KumiInput]::GetForegroundWindow()
        # Keys go where Live takes them: its dialog when one is open, otherwise its main window.
        $target = [KumiInput]::GetLastActivePopup($process.MainWindowHandle)
        if ($target -eq [IntPtr]::Zero) { $target = $process.MainWindowHandle }
        if (-not [KumiInput]::Front($target)) { Done @{ ok = $false; error = 'not-front' }; break }
        $failed = $null; $refused = $false
        $gap = if ($request.gapMs) { [int]$request.gapMs } else { 40 }
        foreach ($combo in $request.keys) {
          $result = Combo $combo
          if ($result -eq 'unknown') { $failed = $combo; break }
          if ($result -eq 'refused') { $refused = $true; break }
          Start-Sleep -Milliseconds $gap
        }
        Start-Sleep -Milliseconds 120
        if ($previous -ne [IntPtr]::Zero -and $previous -ne $target -and $previous -ne $process.MainWindowHandle) { [KumiInput]::Front($previous) | Out-Null }
        if ($failed) { Done @{ ok = $false; error = "Kumi doesn't know the key in $failed." } }
        elseif ($refused) { Done @{ ok = $false; error = 'keys-refused' } }
        else { Done @{ ok = $true } }
      }
      'dialog' {
        $dialog = DialogOf $process
        if (-not $dialog) { Done @{ ok = $true; open = $false }; break }
        # A dialog just opened fills in its words and buttons a moment later.
        for ($look = 0; $look -lt 30; $look++) {
          $buttons = @(Parts $dialog 'button' | ForEach-Object { $_.text } | Where-Object { $_ })
          $words = @(Words $dialog)
          if ($buttons.Count -eq 0) { $buttons = @(Buttons $dialog.element | ForEach-Object { $_.Current.Name } | Where-Object { $_ }) }
          $kind = [KumiInput]::FileKind($dialog.hwnd)
          # A Save or Open dialog builds its file name box after its buttons: it's waited for.
          if ($kind -ne 'pending' -and ($buttons.Count -gt 0 -or $look -ge 9)) { break }
          Start-Sleep -Milliseconds 100
        }
        $read = @{ ok = $true; open = $true; title = [KumiInput]::Text($dialog.hwnd); words = $words; buttons = $buttons }
        # A Save or an Open dialog says which, so a path goes only where it's meant to (#189).
        if ($kind -eq 'save' -or $kind -eq 'open') { $read.file = $kind }
        Done $read
      }
      'answer' {
        $dialog = DialogOf $process
        if (-not $dialog) { Done @{ ok = $false; error = 'no-dialog' }; break }
        $button = ButtonNamed $dialog $request.button
        for ($look = 0; $look -lt 10 -and -not $button; $look++) { Start-Sleep -Milliseconds 100; $button = ButtonNamed $dialog $request.button }
        if (-not $button) { Done @{ ok = $false; error = 'no-button' }; break }
        if (PressButton $dialog $button) { Done @{ ok = $true; pressed = $button.name } } else { Done @{ ok = $false; error = 'press-failed' } }
      }
      'file' {
        # Windows' Save or Open dialog: its file name box filled with the path, then its default button (#189).
        # Only the kind asked for: a path to open never goes into a Save dialog, where Save would write over it.
        $dialog = DialogOf $process
        for ($look = 0; $look -lt 40 -and -not $dialog; $look++) { Start-Sleep -Milliseconds 100; $dialog = DialogOf $process }
        if (-not $dialog) { Done @{ ok = $false; error = 'no-dialog' }; break }
        $title = [KumiInput]::Text($dialog.hwnd)
        # Its controls come a moment after the dialog itself.
        $kind = [KumiInput]::FileKind($dialog.hwnd)
        for ($look = 0; $look -lt 30 -and $kind -eq 'pending'; $look++) { Start-Sleep -Milliseconds 100; $kind = [KumiInput]::FileKind($dialog.hwnd) }
        if ($kind -ne 'save' -and $kind -ne 'open') { Done @{ ok = $false; error = 'no-file-name'; title = $title }; break }
        if ($kind -ne [string]$request.kind) { Done @{ ok = $false; error = 'other-dialog'; file = $kind; title = $title }; break }
        $box = Parts $dialog 'edit' | Where-Object { $_.id -eq $(if ($kind -eq 'save') { 1001 } else { 1148 }) } | Select-Object -First 1
        $default = $null
        for ($look = 0; $look -lt 30 -and -not $default; $look++) {
          $default = Parts $dialog 'button' | Where-Object { $_.id -eq 1 } | Select-Object -First 1
          if (-not $default) { Start-Sleep -Milliseconds 100 }
        }
        if (-not $box -or -not [KumiInput]::SetText($box.hwnd, [string]$request.path)) { Done @{ ok = $false; error = 'no-file-name'; title = $title }; break }
        Start-Sleep -Milliseconds 100
        if (-not $default) { Done @{ ok = $false; error = 'no-default-button'; title = $title }; break }
        if ([KumiInput]::Click($dialog.hwnd, $default)) { Done @{ ok = $true; title = $title; pressed = $default.text } } else { Done @{ ok = $false; error = 'press-failed' } }
      }
      'windows' {
        $all = @($auto::RootElement.FindAll($tree::Children, (New-Object System.Windows.Automation.PropertyCondition($auto::ProcessIdProperty, $process.Id))) | ForEach-Object { @{ title = $_.Current.Name; subrole = '' } })
        Done @{ ok = $true; windows = $all }
      }
      default { Done @{ ok = $false; error = 'unknown-op' } }
    }
  } catch { Done @{ ok = $false; error = $_.Exception.Message } }
}
