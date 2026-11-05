// Kumi's hands for Logic Pro (made by Kumi). One JSON request a line on stdin, one JSON reply a line on stdout,
// events in between. The helper knows macOS, not Logic: Accessibility reads and writes, menus, key events and
// virtual MIDI ports, each exactly as asked. See the crate's README for its build and protocol test commands.
import Cocoa
import ApplicationServices
import CoreMIDI

// MARK: - Lines out

let outputLock = NSLock()

/** One JSON line on stdout, written whole and flushed; stdout carries nothing else. */
func emit(_ object: [String: Any]) {
  var data: Data
  if JSONSerialization.isValidJSONObject(object), let encoded = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]) {
    data = encoded
  } else {
    let fallback: [String: Any] = ["id": object["id"] ?? NSNull(), "ok": false, "error": "unserialisable"]
    guard let encoded = try? JSONSerialization.data(withJSONObject: fallback, options: [.sortedKeys]) else { return }
    data = encoded
  }
  data.append(0x0A)
  outputLock.lock(); defer { outputLock.unlock() }
  do { try FileHandle.standardOutput.write(contentsOf: data) } catch { exit(0) }  // the bridge has gone
}

/** Stderr only: the bridge reads stdout as protocol. */
func log(_ message: String) {
  if let data = ("kumi-logic-hands: " + message + "\n").data(using: .utf8) { try? FileHandle.standardError.write(contentsOf: data) }
}

/** A refused request: its error and whatever else the reply should carry. */
struct Failure: Error { let fields: [String: Any] }
func fail(_ error: String, _ extra: [String: Any] = [:]) -> Failure {
  var fields = extra
  fields["error"] = error
  return Failure(fields: fields)
}

// MARK: - Requests

struct Request {
  let fields: [String: Any]
  let op: String
  /** When the work should stop so the reply still fits the 2 s the contract gives. */
  let deadline: Date

  func string(_ name: String) -> String? { fields[name] as? String }
  func int(_ name: String) -> Int? { fields[name] as? Int }
  func bool(_ name: String, _ fallback: Bool) -> Bool { (fields[name] as? Bool) ?? fallback }
  /** A required non-empty string. */
  func text(_ name: String) throws -> String {
    guard let value = string(name), !value.isEmpty else { throw fail("bad-request", ["detail": "\(name) must be a non-empty string"]) }
    return value
  }
  func pid() throws -> pid_t {
    guard let value = int("pid"), value > 0, value <= Int(Int32.max) else { throw fail("bad-request", ["detail": "pid must be a positive number"]) }
    return pid_t(value)
  }
  /** A wait after a change before reading back, in milliseconds (50 unless asked, never above 1000). */
  func settle() -> UInt32 { UInt32(min(max(int("settleMs") ?? 50, 0), 1000)) * 1000 }
}

func processExists(_ pid: pid_t) -> Bool { kill(pid, 0) == 0 || errno == EPERM }

/** Work AppKit wants on the main thread, whose run loop is free while a request runs. */
@discardableResult
func onMain<T>(_ work: () throws -> T) rethrows -> T { try DispatchQueue.main.sync(execute: work) }

// MARK: - Accessibility

func attribute(_ element: AXUIElement, _ name: String) -> AnyObject? {
  var value: CFTypeRef?
  return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}
func string(_ element: AXUIElement, _ name: String) -> String { (attribute(element, name) as? String) ?? "" }
func flag(_ element: AXUIElement, _ name: String) -> Bool { (attribute(element, name) as? Bool) ?? false }
func role(_ element: AXUIElement) -> String { string(element, kAXRoleAttribute as String) }
func children(_ element: AXUIElement) -> [AXUIElement] { (attribute(element, kAXChildrenAttribute as String) as? [AXUIElement]) ?? [] }
func element(_ element: AXUIElement, _ name: String) -> AXUIElement? {
  guard let value = attribute(element, name), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
  return (value as! AXUIElement)
}
func same(_ a: AXUIElement, _ b: AXUIElement) -> Bool { CFEqual(a, b) }
/** How a reply names another element: its role and its title, else its description. */
func summary(_ element: AXUIElement) -> String {
  let kind = role(element)
  var name = string(element, kAXTitleAttribute as String)
  if name.isEmpty { name = string(element, kAXDescriptionAttribute as String) }
  if name.isEmpty { return kind.isEmpty ? "?" : kind }
  return (kind.isEmpty ? "?" : kind) + " " + name
}

func name(_ error: AXError) -> String {
  switch error {
  case .success: return "success"
  case .failure: return "failure"
  case .illegalArgument: return "illegalArgument"
  case .invalidUIElement: return "invalidUIElement"
  case .invalidUIElementObserver: return "invalidUIElementObserver"
  case .cannotComplete: return "cannotComplete"
  case .attributeUnsupported: return "attributeUnsupported"
  case .actionUnsupported: return "actionUnsupported"
  case .notificationUnsupported: return "notificationUnsupported"
  case .notImplemented: return "notImplemented"
  case .notificationAlreadyRegistered: return "notificationAlreadyRegistered"
  case .notificationNotRegistered: return "notificationNotRegistered"
  case .apiDisabled: return "apiDisabled"
  case .noValue: return "noValue"
  case .parameterizedAttributeUnsupported: return "parameterizedAttributeUnsupported"
  case .notEnoughPrecision: return "notEnoughPrecision"
  @unknown default: return "error\(error.rawValue)"
  }
}

/** Arrays longer than this are cut; a reply that cut one says `truncated`. */
let arrayLimit = 256
final class Conversion { var truncated = false }

func finite(_ value: CGFloat) -> Any { value.isFinite ? Double(value) : NSNull() }

/** An attribute's value as JSON: numbers, strings and booleans as they are, points and sizes as objects, elements as refs. */
func json(_ value: AnyObject?, _ conversion: Conversion, depth: Int = 0) -> Any? {
  guard let value = value else { return nil }
  let type = CFGetTypeID(value)
  if type == CFBooleanGetTypeID() { return value as? Bool }
  if let number = value as? NSNumber { return number.doubleValue.isFinite ? number : nil }
  if let text = value as? String { return text }
  if type == AXUIElementGetTypeID() { return ["ref": summary(value as! AXUIElement)] }
  if type == AXValueGetTypeID() {
    let boxed = value as! AXValue
    switch AXValueGetType(boxed) {
    case .cgPoint: var point = CGPoint.zero; AXValueGetValue(boxed, .cgPoint, &point); return ["x": finite(point.x), "y": finite(point.y)]
    case .cgSize: var size = CGSize.zero; AXValueGetValue(boxed, .cgSize, &size); return ["w": finite(size.width), "h": finite(size.height)]
    case .cgRect: var rect = CGRect.zero; AXValueGetValue(boxed, .cgRect, &rect); return ["x": finite(rect.origin.x), "y": finite(rect.origin.y), "w": finite(rect.width), "h": finite(rect.height)]
    case .cfRange: var range = CFRange(); AXValueGetValue(boxed, .cfRange, &range); return ["loc": range.location, "len": range.length]
    case .axError: var error = AXError.success; AXValueGetValue(boxed, .axError, &error); return ["error": name(error)]
    default: return nil
    }
  }
  if let url = value as? URL { return url.absoluteString }
  if let text = value as? NSAttributedString { return text.string }
  if let array = value as? [AnyObject] {
    if depth > 2 { return nil }
    if array.count > arrayLimit { conversion.truncated = true }
    return array.prefix(arrayLimit).map { json($0, conversion, depth: depth + 1) ?? NSNull() }
  }
  if let dictionary = value as? [String: AnyObject] {
    if depth > 2 { return nil }
    var object: [String: Any] = [:]
    for (key, entry) in dictionary { object[key] = json(entry, conversion, depth: depth + 1) ?? NSNull() }
    return object
  }
  return String(describing: value)
}

/** A JSON value as something an attribute can be set to. */
func axValue(_ raw: Any) throws -> CFTypeRef {
  let object = raw as AnyObject
  if CFGetTypeID(object) == CFBooleanGetTypeID() { return object }
  if let number = raw as? NSNumber { return number }
  if let text = raw as? String { return text as CFString }
  if let fields = raw as? [String: Any] {
    func number(_ key: String) -> CGFloat? { (fields[key] as? NSNumber).map { CGFloat($0.doubleValue) } }
    if let x = number("x"), let y = number("y"), let w = number("w"), let h = number("h") {
      var rect = CGRect(x: x, y: y, width: w, height: h)
      if let boxed = AXValueCreate(.cgRect, &rect) { return boxed }
    } else if let x = number("x"), let y = number("y") {
      var point = CGPoint(x: x, y: y)
      if let boxed = AXValueCreate(.cgPoint, &point) { return boxed }
    } else if let w = number("w"), let h = number("h") {
      var size = CGSize(width: w, height: h)
      if let boxed = AXValueCreate(.cgSize, &size) { return boxed }
    } else if let location = fields["loc"] as? Int, let length = fields["len"] as? Int {
      var range = CFRange(location: location, length: length)
      if let boxed = AXValueCreate(.cfRange, &range) { return boxed }
    }
  }
  throw fail("bad-request", ["detail": "value must be a number, a string, a boolean, {x, y}, {w, h}, {x, y, w, h} or {loc, len}"])
}

/** Reading these opens a pop-up button's menu, so they are read only when `popups` is true. */
let popupRoles: Set<String> = [kAXPopUpButtonRole as String, kAXMenuButtonRole as String, kAXComboBoxRole as String]
let childAttributes: Set<String> = [kAXChildrenAttribute as String, kAXVisibleChildrenAttribute as String, "AXChildrenInNavigationOrder"]

/** The app the request names, once Accessibility is granted and the process exists. */
func application(_ request: Request) throws -> (pid: pid_t, app: AXUIElement) {
  let pid = try request.pid()
  guard AXIsProcessTrusted() else { throw fail("untrusted") }
  guard processExists(pid) else { throw fail("no-process", ["pid": Int(pid)]) }
  let app = AXUIElementCreateApplication(pid)
  // A hung app answers each call late, not never: the request's own deadline does the rest.
  AXUIElementSetMessagingTimeout(app, 1.5)
  return (pid, app)
}

/** Every window the app admits to: AXWindows, then the main and the focused one (a Space away, only those). */
func windows(_ app: AXUIElement) -> [AXUIElement] {
  var found = (attribute(app, kAXWindowsAttribute as String) as? [AXUIElement]) ?? []
  for name in [kAXMainWindowAttribute as String, kAXFocusedWindowAttribute as String] {
    if let window = element(app, name), !found.contains(where: { same($0, window) }) { found.append(window) }
  }
  return found
}

enum Root { case app, main, focused, menubar, title(String) }

func parseRoot(_ raw: Any?) throws -> Root {
  guard let raw = raw else { return .app }
  if let text = raw as? String {
    switch text {
    case "app": return .app
    case "main": return .main
    case "focused": return .focused
    case "menubar": return .menubar
    default: break
    }
  } else if let fields = raw as? [String: Any], let title = fields["title"] as? String, !title.isEmpty {
    return .title(title)
  }
  throw fail("bad-request", ["detail": "root is app, main, focused, menubar or {\"title\": substring}"])
}

func resolve(_ app: AXUIElement, _ root: Root) throws -> AXUIElement {
  switch root {
  case .app: return app
  case .main: if let window = element(app, kAXMainWindowAttribute as String) { return window }; throw fail("no-root", ["root": "main"])
  case .focused: if let window = element(app, kAXFocusedWindowAttribute as String) { return window }; throw fail("no-root", ["root": "focused"])
  case .menubar: if let bar = element(app, kAXMenuBarAttribute as String) { return bar }; throw fail("no-root", ["root": "menubar"])
  case .title(let part):
    let all = windows(app)
    if let window = all.first(where: { string($0, kAXTitleAttribute as String).contains(part) })
      ?? all.first(where: { string($0, kAXTitleAttribute as String).range(of: part, options: .caseInsensitive) != nil }) { return window }
    throw fail("no-root", ["root": part])
  }
}

/** The element at a path of child indexes ("0.3.2") below a root. */
func follow(_ root: AXUIElement, _ path: String, popups: Bool) throws -> AXUIElement {
  var current = root
  var walked: [String] = []
  for segment in path.split(separator: ".") {
    guard let index = Int(segment), index >= 0 else { throw fail("bad-request", ["detail": "path is child indexes joined by dots"]) }
    if !popups && popupRoles.contains(role(current)) { throw fail("popup", ["path": walked.joined(separator: ".")]) }
    let kids = children(current)
    walked.append(String(segment))
    guard index < kids.count else { throw fail("no-element", ["path": walked.joined(separator: "."), "children": kids.count]) }
    current = kids[index]
  }
  return current
}

/** The element a request addresses: its root, then its path. */
func target(_ request: Request, _ app: AXUIElement) throws -> AXUIElement {
  let root = try resolve(app, try parseRoot(request.fields["root"]))
  return try follow(root, request.string("path") ?? "", popups: request.bool("popups", false))
}

// MARK: - Trees

enum Field: String, CaseIterable { case role, subrole, title, desc, value, valueDesc, help, id, enabled, selected, pos, size, set, act }
let fieldAttributes: [Field: String] = [
  .role: kAXRoleAttribute as String, .subrole: kAXSubroleAttribute as String, .title: kAXTitleAttribute as String,
  .desc: kAXDescriptionAttribute as String, .value: kAXValueAttribute as String, .valueDesc: kAXValueDescriptionAttribute as String,
  .help: kAXHelpAttribute as String, .id: "AXIdentifier", .enabled: kAXEnabledAttribute as String,
  .selected: kAXSelectedAttribute as String, .pos: kAXPositionAttribute as String, .size: kAXSizeAttribute as String,
]
let fieldsByAttribute: [String: Field] = Dictionary(uniqueKeysWithValues: fieldAttributes.map { ($1, $0) })
/** What a tree reads unless asked otherwise: everything but the settable attributes and the actions, which cost extra calls. */
let cheapFields: Set<Field> = Set(Field.allCases).subtracting([.set, .act])

/** The node fields a request wants, by field name or by attribute name; "all" is every one. */
func parseFields(_ raw: Any?) throws -> Set<Field> {
  guard let raw = raw else { return cheapFields }
  if let text = raw as? String, text == "all" { return Set(Field.allCases) }
  guard let names = raw as? [String] else { throw fail("bad-request", ["detail": "attrs is \"all\" or a list of names"]) }
  var fields: Set<Field> = [.role]
  for name in names {
    guard let field = Field(rawValue: name) ?? fieldsByAttribute[name] else { throw fail("unknown-attr", ["attr": name]) }
    fields.insert(field)
  }
  return fields
}

func settable(_ element: AXUIElement) -> [String] {
  var names: CFArray?
  guard AXUIElementCopyAttributeNames(element, &names) == .success, let all = names as? [String] else { return [] }
  return all.filter { name in
    var yes: DarwinBoolean = false
    return AXUIElementIsAttributeSettable(element, name as CFString, &yes) == .success && yes.boolValue
  }
}
func actions(_ element: AXUIElement) -> [String] {
  var names: CFArray?
  guard AXUIElementCopyActionNames(element, &names) == .success, let all = names as? [String] else { return [] }
  return all
}

struct TreeLimits { let depth: Int; let children: Int; let fields: Set<Field>; let popups: Bool; let deadline: Date }
final class Walk { var timedOut = false; let conversion = Conversion() }

func isEmpty(_ value: Any) -> Bool {
  if let text = value as? String { return text.isEmpty }
  if let array = value as? [Any] { return array.isEmpty }
  if let object = value as? [String: Any] { return object.isEmpty }
  return value is NSNull
}

/** One element and, down to the depth limit, its first `children` children; empty fields left out. */
func node(_ element: AXUIElement, _ index: String, _ depth: Int, _ limits: TreeLimits, _ walk: Walk) -> [String: Any] {
  var out: [String: Any] = [:]
  if !index.isEmpty { out["i"] = index }
  if Date() >= limits.deadline { walk.timedOut = true; return out }
  // The role is always read: the pop-up rule and the refs need it.
  let kind = role(element)
  if !kind.isEmpty { out["role"] = kind }
  for field in limits.fields where field != .role {
    switch field {
    case .set: let names = settable(element); if !names.isEmpty { out["set"] = names }
    case .act: let names = actions(element); if !names.isEmpty { out["act"] = names }
    default:
      guard let name = fieldAttributes[field], let converted = json(attribute(element, name), walk.conversion), !isEmpty(converted) else { continue }
      out[field.rawValue] = converted
    }
  }
  if popupRoles.contains(kind) && !limits.popups { return out }
  if depth < limits.depth {
    let kids = children(element)
    guard !kids.isEmpty else { return out }
    var listed: [[String: Any]] = []
    for (k, kid) in kids.prefix(limits.children).enumerated() {
      if walk.timedOut { break }
      listed.append(node(kid, index.isEmpty ? "\(k)" : "\(index).\(k)", depth + 1, limits, walk))
    }
    out["c"] = listed
    if kids.count > limits.children { out["more"] = kids.count - limits.children }
  } else {
    var count: CFIndex = 0
    if AXUIElementGetAttributeValueCount(element, kAXChildrenAttribute as CFString, &count) == .success, count > 0 { out["more"] = count }
  }
  return out
}

// MARK: - Windows and menus

func windowRecord(_ window: AXUIElement) -> [String: Any] {
  var out: [String: Any] = ["title": string(window, kAXTitleAttribute as String), "role": role(window)]
  let subrole = string(window, kAXSubroleAttribute as String)
  if !subrole.isEmpty { out["subrole"] = subrole }
  out["main"] = flag(window, kAXMainAttribute as String)
  out["focused"] = flag(window, kAXFocusedAttribute as String)
  out["minimized"] = flag(window, kAXMinimizedAttribute as String)
  out["fullscreen"] = flag(window, "AXFullScreen")
  out["modal"] = flag(window, kAXModalAttribute as String)
  if let document = attribute(window, kAXDocumentAttribute as String) {
    if let text = document as? String, !text.isEmpty { out["document"] = text } else if let url = document as? URL { out["document"] = url.absoluteString }
  }
  return out
}

func isSeparator(_ item: AXUIElement) -> Bool {
  string(item, kAXTitleAttribute as String).isEmpty || string(item, kAXSubroleAttribute as String).contains("Separator")
}

/** The menu below a menu bar item or a menu item (nil when it has none). */
func submenu(_ item: AXUIElement) -> AXUIElement? { children(item).first { role($0) == (kAXMenuRole as String) } }

/** The window titles the app's Window menu lists: the items after its last separator. */
func listedWindows(_ app: AXUIElement, menuTitle: String) -> [String] {
  guard let bar = element(app, kAXMenuBarAttribute as String),
        let item = children(bar).first(where: { string($0, kAXTitleAttribute as String) == menuTitle }),
        let menu = submenu(item) else { return [] }
  var titles: [String] = []
  for entry in children(menu) {
    if isSeparator(entry) { titles.removeAll() } else { titles.append(string(entry, kAXTitleAttribute as String)) }
  }
  return titles
}

/** A title as a menu shows it and as a request may say it: without a trailing ellipsis, in any case. */
func plain(_ title: String) -> String {
  var text = title.trimmingCharacters(in: .whitespaces)
  while text.hasSuffix("…") || text.hasSuffix("...") { text = String(text.dropLast(text.hasSuffix("…") ? 1 : 3)).trimmingCharacters(in: .whitespaces) }
  return text.lowercased()
}

/** The menu item at a path of titles from the menu bar, or the first title that is not there. */
func menuItem(_ bar: AXUIElement, _ titles: [String]) throws -> AXUIElement {
  var current = bar
  for (depth, title) in titles.enumerated() {
    let holder: AXUIElement
    if depth == 0 { holder = bar } else {
      guard let menu = submenu(current) else { throw fail("no-menu-item", ["title": title, "detail": "\(titles[depth - 1]) has no submenu"]) }
      holder = menu
    }
    let items = children(holder)
    let wanted = plain(title)
    guard let found = items.first(where: { string($0, kAXTitleAttribute as String) == title }) ?? items.first(where: { plain(string($0, kAXTitleAttribute as String)) == wanted }) else {
      throw fail("no-menu-item", ["title": title])
    }
    current = found
  }
  return current
}

// MARK: - Keys

let keyCodes: [String: CGKeyCode] = [
  "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17,
  "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33,
  "i": 34, "p": 35, "return": 36, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
  "tab": 48, "space": 49, "`": 50, "delete": 51, "backspace": 51, "escape": 53, "esc": 53,
  "keypad.": 65, "keypad*": 67, "keypad+": 69, "clear": 71, "keypad/": 75, "enter": 76, "keypadenter": 76, "keypad-": 78, "keypad=": 81,
  "keypad0": 82, "keypad1": 83, "keypad2": 84, "keypad3": 85, "keypad4": 86, "keypad5": 87, "keypad6": 88, "keypad7": 89, "keypad8": 91, "keypad9": 92,
  "f5": 96, "f6": 97, "f7": 98, "f3": 99, "f8": 100, "f9": 101, "f11": 103, "f13": 105, "f16": 106, "f14": 107, "f10": 109, "f12": 111, "f15": 113,
  "help": 114, "home": 115, "pageup": 116, "forwarddelete": 117, "f4": 118, "end": 119, "f2": 120, "pagedown": 121, "f1": 122,
  "left": 123, "right": 124, "down": 125, "up": 126, "f17": 64, "f18": 79, "f19": 80, "f20": 90,
]
let modifierKeys: [(names: [String], flag: CGEventFlags, code: CGKeyCode)] = [
  (["cmd", "command"], .maskCommand, 55), (["shift"], .maskShift, 56), (["opt", "option", "alt"], .maskAlternate, 58), (["ctrl", "control"], .maskControl, 59),
]

/** A key as a request names it: a name or a character from the US layout, or a virtual key code. */
func keyCode(_ raw: Any?) throws -> CGKeyCode {
  if let code = raw as? Int {
    guard (0...127).contains(code) else { throw fail("unknown-key", ["key": code]) }
    return CGKeyCode(code)
  }
  guard let text = raw as? String, !text.isEmpty else { throw fail("bad-request", ["detail": "key is a key name, a character or a virtual key code"]) }
  guard let code = keyCodes[text.lowercased()] else { throw fail("unknown-key", ["key": text]) }
  return code
}

func modifierFlags(_ raw: Any?) throws -> CGEventFlags {
  guard let names = raw as? [String] else {
    if raw == nil { return [] }
    throw fail("bad-request", ["detail": "mods is a list of cmd, shift, opt and ctrl"])
  }
  var flags = CGEventFlags()
  for name in names {
    guard let modifier = modifierKeys.first(where: { $0.names.contains(name.lowercased()) }) else { throw fail("unknown-mod", ["mod": name]) }
    flags.insert(modifier.flag)
  }
  return flags
}

/** The key pressed and released in the process, each modifier pressed before it and released after it as its own event. */
func postKey(_ code: CGKeyCode, _ flags: CGEventFlags, to pid: pid_t) throws {
  let source = CGEventSource(stateID: .hidSystemState)
  func event(_ key: CGKeyCode, down: Bool, _ held: CGEventFlags) throws -> CGEvent {
    guard let made = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: down) else { throw fail("event-failed") }
    made.flags = held
    return made
  }
  var held = CGEventFlags()
  for modifier in modifierKeys where flags.contains(modifier.flag) {
    held.insert(modifier.flag)
    try event(modifier.code, down: true, held).postToPid(pid)
    usleep(15_000)
  }
  try event(code, down: true, held).postToPid(pid)
  usleep(30_000)
  try event(code, down: false, held).postToPid(pid)
  usleep(15_000)
  for modifier in modifierKeys.reversed() where flags.contains(modifier.flag) {
    held.remove(modifier.flag)
    try event(modifier.code, down: false, held).postToPid(pid)
    usleep(15_000)
  }
}

/** Text typed into the process, one key event a character, carrying the character itself. */
func postText(_ text: String, to pid: pid_t) throws {
  let source = CGEventSource(stateID: .hidSystemState)
  for scalar in text.unicodeScalars {
    var units = Array(String(scalar).utf16)
    guard let down = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true), let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false) else { throw fail("event-failed") }
    down.keyboardSetUnicodeString(stringLength: units.count, unicodeString: &units)
    up.keyboardSetUnicodeString(stringLength: units.count, unicodeString: &units)
    down.postToPid(pid)
    usleep(4_000)
    up.postToPid(pid)
    usleep(4_000)
  }
}

// MARK: - Processes and apps

/** The pids whose executable path contains the text (libproc sees XPC services that NSWorkspace does not list). */
func processes(containing part: String) -> [Int] {
  let estimate = Int(proc_listallpids(nil, 0))
  var pids = [pid_t](repeating: 0, count: max(estimate, 1) + 64)
  let filled = Int(proc_listallpids(&pids, Int32(pids.count * MemoryLayout<pid_t>.size)))
  var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
  var found: [Int] = []
  for pid in pids.prefix(max(filled, 0)) where pid > 0 {
    let length = proc_pidpath(pid, &buffer, UInt32(buffer.count))
    guard length > 0 else { continue }
    buffer[Int(length)] = 0
    if let path = String(validatingUTF8: buffer), path.contains(part) { found.append(Int(pid)) }
  }
  return found.sorted()
}

func appRecord(_ app: NSRunningApplication) -> [String: Any] {
  ["bundle": app.bundleIdentifier ?? "", "pid": Int(app.processIdentifier), "name": app.localizedName ?? "", "active": app.isActive, "hidden": app.isHidden]
}

/** The bundle ids the last `apps` request named, whose launches, exits, activations and deactivations become events. */
let watchLock = NSLock()
var watched: Set<String> = []
var observers: [NSObjectProtocol] = []

func watch(_ bundles: [String]) {
  watchLock.lock(); watched = Set(bundles); watchLock.unlock()
  guard observers.isEmpty else { return }
  let states: [(NSNotification.Name, String)] = [
    (NSWorkspace.didLaunchApplicationNotification, "launched"), (NSWorkspace.didTerminateApplicationNotification, "terminated"),
    (NSWorkspace.didActivateApplicationNotification, "activated"), (NSWorkspace.didDeactivateApplicationNotification, "deactivated"),
  ]
  for (notification, state) in states {
    observers.append(NSWorkspace.shared.notificationCenter.addObserver(forName: notification, object: nil, queue: nil) { note in
      guard let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication, let bundle = app.bundleIdentifier else { return }
      watchLock.lock(); let wanted = watched.contains(bundle); watchLock.unlock()
      if wanted { emit(["event": "app", "state": state, "pid": Int(app.processIdentifier), "bundle": bundle]) }
    })
  }
}

/** The app in front, from the window server (NSWorkspace can lag a moment behind it). */
func frontmostPid() -> pid_t? {
  let list = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? []
  for window in list where (window[kCGWindowLayer as String] as? Int) == 0 {
    if let pid = window[kCGWindowOwnerPID as String] as? Int32 { return pid }
  }
  return nil
}
func isFront(_ app: NSRunningApplication) -> Bool { app.isActive || frontmostPid() == app.processIdentifier }

// MARK: - MIDI

func hex(_ bytes: [UInt8]) -> String { bytes.map { String(format: "%02X", $0) }.joined(separator: " ") }

func bytes(fromHex text: String) throws -> [UInt8] {
  var digits = ""
  for character in text where !character.isWhitespace && character != "," && character != ":" { digits.append(character) }
  guard !digits.isEmpty, digits.count % 2 == 0 else { throw fail("bad-hex", ["detail": "hex is pairs of hex digits, spaces allowed"]) }
  var out: [UInt8] = []
  var index = digits.startIndex
  while index < digits.endIndex {
    let next = digits.index(index, offsetBy: 2)
    guard let byte = UInt8(digits[index..<next], radix: 16) else { throw fail("bad-hex", ["detail": "hex is pairs of hex digits, spaces allowed"]) }
    out.append(byte)
    index = next
  }
  return out
}

/** A unique id for a virtual port: the number given, or a 32-bit FNV-1a hash of the text given, kept positive. */
func uniqueID(_ raw: Any?) throws -> Int32 {
  if let number = raw as? Int, number > 0, number < Int(Int32.max) { return Int32(number) }
  if let text = raw as? String, !text.isEmpty {
    var hash: UInt32 = 2_166_136_261
    for byte in text.utf8 { hash = (hash ^ UInt32(byte)) &* 16_777_619 }
    return Int32(max(hash & 0x7FFF_FFFE, 1))
  }
  throw fail("bad-request", ["detail": "uid is a positive number or a name to hash"])
}

final class Midi {
  var client = MIDIClientRef()
  var source = MIDIEndpointRef()
  var destination = MIDIEndpointRef()
  var open = false

  func openPorts(name: String, uid: Int32) throws -> [String: Any] {
    guard !open else { throw fail("midi-open", ["detail": "close the ports before opening others"]) }
    if client == 0 {
      // The client is made on the main thread: CoreMIDI notifies on the run loop that made it.
      let status = onMain { MIDIClientCreateWithBlock("Kumi Logic Hands" as CFString, &client, nil) }
      guard status == noErr else { throw fail("midi-failed", ["status": Int(status), "detail": "client"]) }
    }
    var status = MIDISourceCreate(client, name as CFString, &source)
    guard status == noErr else { throw fail("midi-failed", ["status": Int(status), "detail": "source"]) }
    MIDIObjectSetIntegerProperty(source, kMIDIPropertyUniqueID, uid)
    status = MIDIDestinationCreateWithBlock(client, name as CFString, &destination) { list, _ in
      let stamp = Int64(Date().timeIntervalSince1970 * 1000)
      for packet in list.unsafeSequence() {
        let data = UnsafeRawPointer(packet).advanced(by: MemoryLayout<MIDIPacket>.offset(of: \MIDIPacket.data)!)
        emit(["event": "midi", "hex": hex(Array(UnsafeRawBufferPointer(start: data, count: Int(packet.pointee.length)))), "ms": stamp])
      }
    }
    guard status == noErr else {
      MIDIEndpointDispose(source); source = 0
      throw fail("midi-failed", ["status": Int(status), "detail": "destination"])
    }
    MIDIObjectSetIntegerProperty(destination, kMIDIPropertyUniqueID, uid &+ 1)
    open = true
    var sourceID: Int32 = 0, destinationID: Int32 = 0
    MIDIObjectGetIntegerProperty(source, kMIDIPropertyUniqueID, &sourceID)
    MIDIObjectGetIntegerProperty(destination, kMIDIPropertyUniqueID, &destinationID)
    return ["opened": true, "source": Int(sourceID), "destination": Int(destinationID)]
  }

  func send(_ bytes: [UInt8]) throws {
    guard open else { throw fail("midi-closed") }
    let size = bytes.count + 512
    let raw = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: 4)
    defer { raw.deallocate() }
    let list = raw.bindMemory(to: MIDIPacketList.self, capacity: 1)
    let packet = MIDIPacketListInit(list)
    _ = MIDIPacketListAdd(list, size, packet, 0, bytes.count, bytes)
    let status = MIDIReceived(source, list)
    guard status == noErr else { throw fail("midi-failed", ["status": Int(status), "detail": "send"]) }
  }

  func close() {
    guard open else { return }
    MIDIEndpointDispose(destination); destination = 0
    MIDIEndpointDispose(source); source = 0
    open = false
  }
}
let midi = Midi()

// MARK: - The ops

func handle(_ request: Request) throws -> [String: Any] {
  switch request.op {
  case "trusted":
    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: request.bool("prompt", false)] as CFDictionary
    return ["trusted": AXIsProcessTrustedWithOptions(options)]

  case "apps":
    guard let bundles = request.fields["bundles"] as? [String] else { throw fail("bad-request", ["detail": "bundles is a list of bundle identifiers"]) }
    let apps: [[String: Any]] = onMain {
      watch(bundles)
      return bundles.flatMap { NSRunningApplication.runningApplications(withBundleIdentifier: $0).map(appRecord) }
    }
    return ["apps": apps]

  case "processes":
    return ["pids": processes(containing: try request.text("contains"))]

  case "activate":
    let pid = try request.pid()
    guard let app = NSRunningApplication(processIdentifier: pid) else { throw fail("no-app", ["pid": Int(pid)]) }
    if !isFront(app) {
      onMain { if #available(macOS 14.0, *) { app.activate() } else { app.activate(options: [.activateIgnoringOtherApps]) } }
      let until = Date().addingTimeInterval(1.0)
      while !isFront(app) && Date() < until { usleep(20_000) }
    }
    return ["active": isFront(app)]

  case "windows":
    let (_, app) = try application(request)
    return ["windows": windows(app).map(windowRecord), "listed": listedWindows(app, menuTitle: request.string("menu") ?? "Window")]

  case "tree":
    let (_, app) = try application(request)
    let limits = TreeLimits(depth: max(request.int("depth") ?? 1, 0), children: max(request.int("children") ?? 100, 0), fields: try parseFields(request.fields["attrs"]),
                            popups: request.bool("popups", false), deadline: request.deadline)
    let walk = Walk()
    let path = request.string("path") ?? ""
    let tree = node(try target(request, app), path, 0, limits, walk)
    if walk.timedOut { throw fail("timeout", ["detail": "the tree is larger than 2 s allow; lower depth or children"]) }
    var reply: [String: Any] = ["node": tree]
    if walk.conversion.truncated { reply["truncated"] = true }
    return reply

  case "get":
    let (_, app) = try application(request)
    let element = try target(request, app)
    let names: [String]
    if let all = request.fields["attrs"] as? String, all == "all" {
      var array: CFArray?
      guard AXUIElementCopyAttributeNames(element, &array) == .success, let list = array as? [String] else { throw fail("no-attributes") }
      names = list
    } else if let list = request.fields["attrs"] as? [String] {
      names = list.map { Field(rawValue: $0).flatMap { fieldAttributes[$0] } ?? $0 }
    } else { throw fail("bad-request", ["detail": "attrs is a list of attribute names, or \"all\""]) }
    let popup = popupRoles.contains(role(element)) && !request.bool("popups", false)
    let conversion = Conversion()
    var attrs: [String: Any] = [:]
    for name in names where !(popup && childAttributes.contains(name)) {
      if let converted = json(attribute(element, name), conversion) { attrs[name] = converted }
    }
    var reply: [String: Any] = ["attrs": attrs]
    if conversion.truncated { reply["truncated"] = true }
    return reply

  case "set":
    let (_, app) = try application(request)
    let element = try target(request, app)
    let asked = try request.text("attr")
    let attr = Field(rawValue: asked).flatMap { fieldAttributes[$0] } ?? asked
    guard let raw = request.fields["value"] else { throw fail("bad-request", ["detail": "value is what to set"]) }
    let value = try axValue(raw)
    let conversion = Conversion()
    var reply: [String: Any] = [:]
    if let before = json(attribute(element, attr), conversion) { reply["before"] = before }
    let result = AXUIElementSetAttributeValue(element, attr as CFString, value)
    usleep(request.settle())
    if let after = json(attribute(element, attr), conversion) { reply["after"] = after }
    reply["result"] = name(result)
    guard result == .success else { throw Failure(fields: reply.merging(["error": name(result)]) { $1 }) }
    return reply

  case "action":
    let (_, app) = try application(request)
    let element = try target(request, app)
    let action = try request.text("name")
    let conversion = Conversion()
    var reply: [String: Any] = [:]
    if let before = json(attribute(element, kAXValueAttribute as String), conversion) { reply["before"] = before }
    let result = AXUIElementPerformAction(element, action as CFString)
    usleep(request.settle())
    if let after = json(attribute(element, kAXValueAttribute as String), conversion) { reply["after"] = after }
    reply["result"] = name(result)
    guard result == .success else { throw Failure(fields: reply.merging(["error": name(result)]) { $1 }) }
    return reply

  case "menu":
    let (pid, app) = try application(request)
    guard let titles = request.fields["titles"] as? [String], !titles.isEmpty, titles.allSatisfy({ !$0.isEmpty }) else {
      throw fail("bad-request", ["detail": "titles is the menu bar item's title, then each item's down to the one to press"])
    }
    guard let bar = element(app, kAXMenuBarAttribute as String) else { throw fail("no-menubar") }
    let item = try menuItem(bar, titles)
    let title = string(item, kAXTitleAttribute as String)
    let active = NSRunningApplication(processIdentifier: pid).map(isFront) ?? false
    guard flag(item, kAXEnabledAttribute as String) else { throw fail("disabled", ["title": title, "active": active]) }
    let result = AXUIElementPerformAction(item, kAXPressAction as CFString)
    guard result == .success else { throw fail(name(result), ["result": name(result), "title": title, "active": active]) }
    return ["result": name(result), "title": title, "active": active]

  case "keys":
    let pid = try request.pid()
    let code = try keyCode(request.fields["key"])
    let flags = try modifierFlags(request.fields["mods"])
    guard processExists(pid) else { throw fail("no-process", ["pid": Int(pid)]) }
    guard AXIsProcessTrusted() else { throw fail("untrusted") }
    try postKey(code, flags, to: pid)
    return ["posted": true, "active": NSRunningApplication(processIdentifier: pid).map(isFront) ?? false]

  case "type":
    let pid = try request.pid()
    let text = try request.text("text")
    guard text.unicodeScalars.count <= 200 else { throw fail("too-long", ["detail": "type takes up to 200 characters a request"]) }
    guard processExists(pid) else { throw fail("no-process", ["pid": Int(pid)]) }
    guard AXIsProcessTrusted() else { throw fail("untrusted") }
    try postText(text, to: pid)
    return ["posted": true, "active": NSRunningApplication(processIdentifier: pid).map(isFront) ?? false]

  case "midi.open":
    return try midi.openPorts(name: try request.text("name"), uid: try uniqueID(request.fields["uid"]))

  case "midi.send":
    let bytes = try bytes(fromHex: try request.text("hex"))
    guard bytes.count <= 65_535 else { throw fail("too-long", ["detail": "midi.send takes up to 65535 bytes a request"]) }
    try midi.send(bytes)
    return ["sent": true]

  case "midi.close":
    midi.close()
    return ["closed": true]

  case "tap.start", "tap.stop":
    throw fail("reserved", ["detail": "the process tap comes with M5"])

  default:
    throw fail("unknown-op", ["op": request.op])
  }
}

// MARK: - One request at a time, a reply within 2 s

final class Ticket {
  private let lock = NSLock()
  private var answered = false
  private var started = false
  func start() { lock.lock(); started = true; lock.unlock() }
  /** Whether the worker got to the request at all (a request behind a stuck one never does). */
  var hasStarted: Bool { lock.lock(); defer { lock.unlock() }; return started }
  /** Whether the caller is the one to answer: the first of the worker and the clock. */
  func claim() -> Bool { lock.lock(); defer { lock.unlock() }; if answered { return false }; answered = true; return true }
}

let worker = DispatchQueue(label: "kumi-logic-hands.worker")

func reply(_ id: Any, _ fields: [String: Any], _ started: Date) -> [String: Any] {
  var out = fields
  out["id"] = id
  out["ms"] = Int(Date().timeIntervalSince(started) * 1000)
  return out
}

func serve(_ fields: [String: Any]) {
  let id = fields["id"] ?? NSNull()
  let started = Date()
  let request = Request(fields: fields, op: (fields["op"] as? String) ?? "", deadline: started.addingTimeInterval(1.8))
  let ticket = Ticket()
  let done = DispatchSemaphore(value: 0)
  worker.async {
    ticket.start()
    var answer: [String: Any]
    do { answer = try handle(request); answer["ok"] = true } catch let failure as Failure { answer = failure.fields; answer["ok"] = false } catch { answer = ["ok": false, "error": "failed", "detail": "\(error)"] }
    if ticket.claim() { emit(reply(id, answer, started)) }
    done.signal()
  }
  if done.wait(timeout: .now() + 2.0) == .timedOut, ticket.claim() {
    let error = ticket.hasStarted ? "timeout" : "busy"
    log("\(request.op) \(id): \(error)")
    emit(reply(id, ["ok": false, "error": error], started))
  }
}

func read() {
  while let line = readLine(strippingNewline: true) {
    let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty { continue }
    guard let data = trimmed.data(using: .utf8), let fields = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
      emit(["id": NSNull(), "ok": false, "error": "malformed"])
      continue
    }
    if (fields["op"] as? String) == "quit" {
      emit(["id": fields["id"] ?? NSNull(), "ok": true])
      midi.close()
      exit(0)
    }
    serve(fields)
  }
  midi.close()
  exit(0)
}

signal(SIGPIPE, SIG_IGN)
let reader = Thread { read() }
reader.name = "kumi-logic-hands.stdin"
reader.start()
// The main thread serves AppKit and CoreMIDI: the workspace notifications and the main queue need its run loop.
let keepAlive = Timer(timeInterval: 3600, repeats: true) { _ in }
RunLoop.main.add(keepAlive, forMode: .default)
RunLoop.main.run()
