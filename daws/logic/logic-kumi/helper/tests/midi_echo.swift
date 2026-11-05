// The tests' MIDI counterpart: connects to the virtual source the helper named, sends bytes to the helper's
// destination of the same name, then prints every packet the source delivers for a while, one hex line each.
// usage: midi_echo <port name> <hex bytes to send> [seconds to listen]
import CoreMIDI
import Foundation

let arguments = CommandLine.arguments
guard arguments.count >= 3 else { FileHandle.standardError.write("usage: midi_echo <port name> <hex> [seconds]\n".data(using: .utf8)!); exit(2) }
let wanted = arguments[1]
let outgoing = arguments[2].split(separator: " ").compactMap { UInt8($0, radix: 16) }
let seconds = arguments.count > 3 ? (Double(arguments[3]) ?? 2) : 2

func named(_ object: MIDIObjectRef) -> String {
  var name: Unmanaged<CFString>?
  return MIDIObjectGetStringProperty(object, kMIDIPropertyName, &name) == noErr ? (name?.takeRetainedValue() as String? ?? "") : ""
}
func find() -> (source: MIDIEndpointRef, destination: MIDIEndpointRef)? {
  var source = MIDIEndpointRef(), destination = MIDIEndpointRef()
  for index in 0..<MIDIGetNumberOfSources() where named(MIDIGetSource(index)) == wanted { source = MIDIGetSource(index) }
  for index in 0..<MIDIGetNumberOfDestinations() where named(MIDIGetDestination(index)) == wanted { destination = MIDIGetDestination(index) }
  return source != 0 && destination != 0 ? (source, destination) : nil
}

var client = MIDIClientRef()
guard MIDIClientCreateWithBlock("Kumi midi_echo" as CFString, &client, nil) == noErr else { print("client failed"); exit(1) }
var ports: (source: MIDIEndpointRef, destination: MIDIEndpointRef)?
let until = Date().addingTimeInterval(3)
while ports == nil && Date() < until { ports = find(); if ports == nil { usleep(50_000) } }
guard let found = ports else { print("no ports named \(wanted)"); exit(3) }

var input = MIDIPortRef()
let status = MIDIInputPortCreateWithBlock(client, "in" as CFString, &input) { list, _ in
  for packet in list.unsafeSequence() {
    let data = UnsafeRawPointer(packet).advanced(by: MemoryLayout<MIDIPacket>.offset(of: \MIDIPacket.data)!)
    let bytes = Array(UnsafeRawBufferPointer(start: data, count: Int(packet.pointee.length)))
    print(bytes.map { String(format: "%02X", $0) }.joined(separator: " "))
    fflush(stdout)
  }
}
guard status == noErr, MIDIPortConnectSource(input, found.source, nil) == noErr else { print("input failed"); exit(1) }
var output = MIDIPortRef()
guard MIDIOutputPortCreate(client, "out" as CFString, &output) == noErr else { print("output failed"); exit(1) }
if !outgoing.isEmpty {
  var list = MIDIPacketList()
  let packet = MIDIPacketListInit(&list)
  _ = MIDIPacketListAdd(&list, MemoryLayout<MIDIPacketList>.size, packet, 0, outgoing.count, outgoing)
  if MIDISend(output, found.destination, &list) != noErr { print("send failed") }
}
print("ready"); fflush(stdout)
CFRunLoopRunInMode(.defaultMode, seconds, false)
