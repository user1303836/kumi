// Kumi's hands (made by Kumi). One JSON request a line on stdin, one JSON answer a line on stdout.
import Cocoa
import ApplicationServices

let version = 3
let liveBundles = ["com.ableton.live"]

func emit(_ object: [String: Any]) {
  if let data = try? JSONSerialization.data(withJSONObject: object, options: []) {
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
  }
}

func live() -> NSRunningApplication? {
  for bundle in liveBundles { if let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first { return app } }
  return NSWorkspace.shared.runningApplications.first { ($0.bundleIdentifier ?? "").hasPrefix("com.ableton.live") }
}

func value(_ element: AXUIElement, _ name: String) -> AnyObject? {
  var result: AnyObject?
  return AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success ? result : nil
}
func children(_ element: AXUIElement) -> [AXUIElement] { (value(element, kAXChildrenAttribute as String) as? [AXUIElement]) ?? [] }
func title(_ element: AXUIElement) -> String { (value(element, kAXTitleAttribute as String) as? String) ?? "" }
/** What a screen reader says for it: its title, else its description. */
func label(_ element: AXUIElement) -> String { let named = title(element); return named.isEmpty ? ((value(element, kAXDescriptionAttribute as String) as? String) ?? "") : named }
func role(_ element: AXUIElement) -> String { (value(element, kAXRoleAttribute as String) as? String) ?? "" }
func enabled(_ element: AXUIElement) -> Bool { (value(element, kAXEnabledAttribute as String) as? Bool) ?? true }

/** Live's menu items, with their place: [["Edit", "Group Tracks"], …], each with whether it's enabled and its key. */
func walk(_ element: AXUIElement, _ path: [String], _ into: inout [[String: Any]], _ depth: Int) {
  if depth > 6 { return }
  for child in children(element) {
    let kind = role(child)
    if kind == (kAXMenuItemRole as String) || kind == (kAXMenuBarItemRole as String) {
      let name = title(child)
      if name.isEmpty { continue }
      let here = path + [name]
      let submenu = children(child).first { role($0) == (kAXMenuRole as String) }
      if let submenu = submenu { walk(submenu, here, &into, depth + 1) }
      else if kind == (kAXMenuItemRole as String) {
        var item: [String: Any] = ["path": here, "enabled": enabled(child)]
        if let key = value(child, kAXMenuItemCmdCharAttribute as String) as? String, !key.isEmpty {
          item["key"] = key
          item["modifiers"] = (value(child, kAXMenuItemCmdModifiersAttribute as String) as? Int) ?? 0
        }
        into.append(item)
      }
    } else if kind == (kAXMenuRole as String) { walk(child, path, &into, depth + 1) }
  }
}

func menuBar(_ app: NSRunningApplication) -> AXUIElement? {
  let element = AXUIElementCreateApplication(app.processIdentifier)
  guard let bar = value(element, kAXMenuBarAttribute as String) else { return nil }
  return (bar as! AXUIElement)
}

/** A title as Live may say it: "Freeze Track" and "Freeze Tracks" (it changes with the selection) are one. */
func norm(_ text: String) -> String {
  var words = text.replacingOccurrences(of: "…", with: "").replacingOccurrences(of: "...", with: "").lowercased().split(separator: " ").map(String.init)
  words = words.map { ["tracks", "clips", "scenes"].contains($0) ? String($0.dropLast()) : $0 }
  return words.joined(separator: " ").trimmingCharacters(in: .whitespaces)
}

/** The menu item at a path of titles ("Edit", "Group Tracks"); a title can be its start ("Freeze"). */
func item(_ bar: AXUIElement, _ path: [String]) -> AXUIElement? {
  var current: AXUIElement = bar
  for (index, name) in path.enumerated() {
    var holder = current
    if index > 0, let submenu = children(current).first(where: { role($0) == (kAXMenuRole as String) }) { holder = submenu }
    let wanted = norm(name)
    let found = children(holder).first { norm(title($0)) == wanted } ?? children(holder).first { norm(title($0)).hasPrefix(wanted) }
    guard let next = found else { return nil }
    current = next
  }
  return current
}

let keyCodes: [String: CGKeyCode] = [
  "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17,
  "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33,
  "i": 34, "p": 35, "return": 36, "enter": 36, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
  "tab": 48, "space": 49, "\u{60}": 50, "delete": 51, "backspace": 51, "escape": 53, "esc": 53, "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
  "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111, "home": 115, "pageup": 116, "forwarddelete": 117, "end": 119, "pagedown": 121,
  "left": 123, "right": 124, "down": 125, "up": 126,
]

/** "cmd+shift+r": the key and its modifiers, pressed and released in the app (no focus needed for most). */
func press(_ combo: String, _ app: NSRunningApplication) -> String? {
  var flags = CGEventFlags()
  var key: CGKeyCode?
  for part in combo.lowercased().split(separator: "+").map(String.init) {
    switch part {
    case "cmd", "command": flags.insert(.maskCommand)
    case "shift": flags.insert(.maskShift)
    case "alt", "option", "opt": flags.insert(.maskAlternate)
    case "ctrl", "control": flags.insert(.maskControl)
    default: key = keyCodes[part]
    }
  }
  guard let code = key else { return "Kumi doesn't know the key in \(combo)." }
  let source = CGEventSource(stateID: .hidSystemState)
  guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true), let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false) else { return "The keys couldn't be made." }
  down.flags = flags; up.flags = flags
  down.post(tap: .cghidEventTap); up.post(tap: .cghidEventTap)
  return nil
}

/** Bring an app to the front: macOS 14 dropped "ignoring other apps", which 13 still needs. */
func activate(_ app: NSRunningApplication) {
  if #available(macOS 14.0, *) { app.activate() } else { app.activate(options: [.activateIgnoringOtherApps]) }
}

/** The app in front now, from the window server (NSWorkspace's answer goes stale in a program with no run loop). */
func frontmost() -> NSRunningApplication? {
  let windows = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? []
  for window in windows where (window[kCGWindowLayer as String] as? Int) == 0 {
    if let pid = window[kCGWindowOwnerPID as String] as? Int32 { return NSRunningApplication(processIdentifier: pid) }
  }
  return nil
}

/** Bring Live to the front (and say what was in front, to give it back). */
func front(_ app: NSRunningApplication) -> NSRunningApplication? {
  let before = frontmost()
  if before?.processIdentifier != app.processIdentifier {
    activate(app)
    let deadline = Date().addingTimeInterval(1.0)
    while frontmost()?.processIdentifier != app.processIdentifier && Date() < deadline { usleep(5_000) }
  }
  return before?.processIdentifier == app.processIdentifier ? nil : before
}

/** Live's track headers in the view it shows (Arrangement or Session): one row a track, return and Main. */
func trackHeaders(_ app: NSRunningApplication) -> AXUIElement? {
  func find(_ element: AXUIElement, _ depth: Int) -> AXUIElement? {
    if role(element) == (kAXOutlineRole as String) && label(element) == "Track Headers" { return element }
    if depth > 6 { return nil }
    for child in children(element) { if let found = find(child, depth + 1) { return found } }
    return nil
  }
  let element = AXUIElementCreateApplication(app.processIdentifier)
  let windows = (value(element, kAXWindowsAttribute as String) as? [AXUIElement]) ?? []
  for window in windows { if let found = find(window, 0) { return found } }
  return nil
}

/**
 * The rows for these tracks: a row reads the track's name, with its state after a comma ("Bass, Frozen").
 * Each name takes its nth row of that name (several tracks can share one); a whole name first, then one
 * followed by a state.
 */
func rows(_ all: [AXUIElement], _ wanted: [[String: Any]]) -> (picked: [AXUIElement], missing: [String]) {
  var picked: [AXUIElement] = []; var missing: [String] = []
  let labels = all.map { label($0) }
  for entry in wanted {
    let name = (entry["name"] as? String) ?? ""; let nth = (entry["nth"] as? Int) ?? 0
    let whole = labels.indices.filter { labels[$0] == name }
    let stated = labels.indices.filter { labels[$0].hasPrefix(name + ", ") && !whole.contains($0) }
    let matches = whole.count > nth ? whole : (whole + stated).sorted()
    if matches.count > nth { picked.append(all[matches[nth]]) } else { missing.append(name) }
  }
  return (picked, missing)
}

/** Live's dialog, if one is up: its words and buttons. */
func dialog(_ app: NSRunningApplication) -> AXUIElement? {
  let element = AXUIElementCreateApplication(app.processIdentifier)
  let windows = (value(element, kAXWindowsAttribute as String) as? [AXUIElement]) ?? []
  return windows.first { window in
    let subrole = (value(window, kAXSubroleAttribute as String) as? String) ?? ""
    let modal = (value(window, kAXModalAttribute as String) as? Bool) ?? false
    return modal || subrole == (kAXDialogSubrole as String) || subrole == (kAXSystemDialogSubrole as String)
  }
}
func texts(_ element: AXUIElement, _ depth: Int = 0) -> (words: [String], buttons: [String]) {
  var words: [String] = []; var buttons: [String] = []
  if depth > 8 { return (words, buttons) }
  for child in children(element) {
    let kind = role(child)
    if kind == (kAXButtonRole as String) { let name = title(child); if !name.isEmpty { buttons.append(name) } }
    else if kind == (kAXStaticTextRole as String) { if let text = value(child, kAXValueAttribute as String) as? String, !text.isEmpty { words.append(text) } }
    let deeper = texts(child, depth + 1); words += deeper.words; buttons += deeper.buttons
  }
  return (words, buttons)
}
func button(_ element: AXUIElement, _ name: String, _ depth: Int = 0) -> AXUIElement? {
  if depth > 8 { return nil }
  for child in children(element) {
    if role(child) == (kAXButtonRole as String) && (title(child).lowercased() == name.lowercased() || identifier(child) == name) { return child }
    if let found = button(child, name, depth + 1) { return found }
  }
  return nil
}
/** Its accessibility identifier: Live's are the same in every language (VocalsCheckControl). */
func identifier(_ element: AXUIElement) -> String { (value(element, "AXIdentifier") as? String) ?? "" }
/** Whether a check box is on (its value is 1; 2 is mixed). */
func isOn(_ element: AXUIElement) -> Bool { ((value(element, kAXValueAttribute as String) as? NSNumber)?.intValue ?? 0) == 1 }
/** A dialog's toggles (check boxes): each with its name, whether it's on, whether it can be changed now, and its identifier. */
func toggles(_ element: AXUIElement, _ depth: Int = 0) -> [[String: Any]] {
  var found: [[String: Any]] = []
  if depth > 8 { return found }
  for child in children(element) {
    if role(child) == (kAXCheckBoxRole as String) {
      let name = label(child).trimmingCharacters(in: .whitespaces)
      if !name.isEmpty {
        var toggle: [String: Any] = ["name": name, "on": isOn(child), "enabled": enabled(child)]
        if !identifier(child).isEmpty { toggle["id"] = identifier(child) }
        found.append(toggle)
      }
    }
    found += toggles(child, depth + 1)
  }
  return found
}
/** The first check box under an element that matches. */
func checkBox(_ element: AXUIElement, _ matches: (AXUIElement) -> Bool, _ depth: Int = 0) -> AXUIElement? {
  if depth > 8 { return nil }
  for child in children(element) {
    if role(child) == (kAXCheckBoxRole as String) && matches(child) { return child }
    if let found = checkBox(child, matches, depth + 1) { return found }
  }
  return nil
}
/** The check box a request names: by its identifier when it gives one, else by its whole name. */
func checkBox(_ window: AXUIElement, id: String, name: String) -> AXUIElement? {
  let named = name.trimmingCharacters(in: .whitespaces)
  if !id.isEmpty, let found = checkBox(window, { identifier($0) == id }) { return found }
  return checkBox(window, { label($0).trimmingCharacters(in: .whitespaces) == named })
}

while let line = readLine() {
  guard let data = line.data(using: .utf8), let request = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { continue }
  let id = request["id"] ?? 0
  let op = (request["op"] as? String) ?? ""
  let started = Date()
  var answer: [String: Any] = ["id": id]
  func done(_ fields: [String: Any]) { for (key, value) in fields { answer[key] = value }; answer["ms"] = Int(Date().timeIntervalSince(started) * 1000); emit(answer) }
  if op == "version" { done(["ok": true, "version": version]); continue }
  if op == "trusted" {
    let prompt = (request["prompt"] as? Bool) ?? false
    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: prompt] as CFDictionary
    done(["ok": true, "trusted": AXIsProcessTrustedWithOptions(options)]); continue
  }
  guard AXIsProcessTrusted() else { done(["ok": false, "error": "untrusted"]); continue }
  guard let app = live() else { done(["ok": false, "error": "no-live"]); continue }
  switch op {
  case "menus":
    guard let bar = menuBar(app) else { done(["ok": false, "error": "no-menus"]); break }
    var items: [[String: Any]] = []
    walk(bar, [], &items, 0)
    done(["ok": true, "items": items])
  case "tracks":
    // Selected as a screen reader selects them: the track headers focused, their rows chosen. Live needn't be in front.
    guard let outline = trackHeaders(app) else { done(["ok": false, "error": "no-track-headers"]); break }
    let all = (value(outline, kAXRowsAttribute as String) as? [AXUIElement]) ?? []
    let found = rows(all, (request["tracks"] as? [[String: Any]]) ?? [])
    if !found.missing.isEmpty || found.picked.isEmpty { done(["ok": false, "error": "no-track", "missing": found.missing]); break }
    AXUIElementSetAttributeValue(outline, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    let result = AXUIElementSetAttributeValue(outline, kAXSelectedRowsAttribute as CFString, found.picked as CFArray)
    let now = ((value(outline, kAXSelectedRowsAttribute as String) as? [AXUIElement]) ?? []).map { label($0) }
    done(result == .success ? ["ok": true, "selected": now] : ["ok": false, "error": "select-failed", "code": result.rawValue])
  case "menu":
    let path = (request["path"] as? [String]) ?? []
    // Other titles the item may have now (Live retitles some with the selection: Freeze Track, Unfreeze Track).
    let others = ((request["titles"] as? [String]) ?? []).map { Array(path.dropLast()) + [$0] }
    guard let bar = menuBar(app), var target = ([path] + others).lazy.compactMap({ item(bar, $0) }).first else { done(["ok": false, "error": "no-item"]); break }
    // Live's menus catch up with a new selection a moment later: a greyed-out item is looked at again before it counts.
    for _ in 0..<15 where !enabled(target) { usleep(40_000); if let again = item(bar, path) { target = again } }
    if !enabled(target) { done(["ok": false, "error": "disabled", "title": title(target)]); break }
    let pressedTitle = title(target)
    let back = (request["front"] as? Bool) == true ? front(app) : nil
    let result = AXUIElementPerformAction(target, kAXPressAction as CFString)
    if let back = back, (request["giveBack"] as? Bool) != false { usleep(UInt32(((request["settleMs"] as? Int) ?? 60) * 1000)); activate(back) }
    done(result == .success ? ["ok": true, "title": pressedTitle] : ["ok": false, "error": "press-failed", "code": result.rawValue])
  case "keys":
    let combos = (request["keys"] as? [String]) ?? []
    let back = front(app)
    var failure: String?
    for combo in combos { if let problem = press(combo, app) { failure = problem; break }; usleep(UInt32(((request["gapMs"] as? Int) ?? 25) * 1000)) }
    if let back = back, (request["giveBack"] as? Bool) != false { usleep(UInt32(((request["settleMs"] as? Int) ?? 60) * 1000)); activate(back) }
    done(failure == nil ? ["ok": true] : ["ok": false, "error": failure!])
  case "dialog":
    guard let window = dialog(app) else { done(["ok": true, "open": false]); break }
    let read = texts(window)
    var reply: [String: Any] = ["ok": true, "open": true, "title": title(window), "words": read.words, "buttons": read.buttons]
    let boxes = toggles(window)
    if !boxes.isEmpty { reply["toggles"] = boxes }
    done(reply)
  case "answer":
    let name = (request["button"] as? String) ?? ""
    guard let window = dialog(app), let target = button(window, name) else { done(["ok": false, "error": "no-button"]); break }
    // A button greyed out isn't pressed (Separate Stems' Separate with no stem on).
    if !enabled(target) { done(["ok": false, "error": "disabled", "title": title(target)]); break }
    let result = AXUIElementPerformAction(target, kAXPressAction as CFString)
    done(result == .success ? ["ok": true] : ["ok": false, "error": "press-failed"])
  case "toggles":
    // Each toggle asked for, in order, set on or off as a click would. One Live won't change yet (Separate
    // Stems' Merge Stems until two or three stems are on) is tried again after the rest.
    guard let window = dialog(app) else { done(["ok": false, "error": "no-dialog"]); break }
    var missing: [String] = []
    var later = (request["set"] as? [[String: Any]]) ?? []
    for _ in 0..<2 {
      let wanted = later
      later = []
      for one in wanted {
        let name = (one["name"] as? String) ?? ""
        let want = (one["on"] as? Bool) ?? false
        guard let box = checkBox(window, id: (one["id"] as? String) ?? "", name: name) else { missing.append(name); continue }
        if isOn(box) != want && enabled(box) { AXUIElementPerformAction(box, kAXPressAction as CFString); usleep(50_000) }
        if isOn(box) != want { later.append(one) }
      }
      if later.isEmpty { break }
    }
    done(["ok": true, "toggles": toggles(window), "missing": missing, "refused": later.map { ($0["name"] as? String) ?? "" }])
  case "windows":
    let element = AXUIElementCreateApplication(app.processIdentifier)
    let windows = (value(element, kAXWindowsAttribute as String) as? [AXUIElement]) ?? []
    done(["ok": true, "windows": windows.map { ["title": title($0), "subrole": (value($0, kAXSubroleAttribute as String) as? String) ?? ""] }])
  case "front":
    let back = front(app)
    done(["ok": true, "previous": back?.bundleIdentifier ?? ""])
  default:
    done(["ok": false, "error": "unknown-op"])
  }
}
