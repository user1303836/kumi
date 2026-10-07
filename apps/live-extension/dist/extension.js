"use strict";
var __create = Object.create;
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __getProtoOf = Object.getPrototypeOf;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __commonJS = (cb, mod) => function __require() {
  try {
    return mod || (0, cb[__getOwnPropNames(cb)[0]])((mod = { exports: {} }).exports, mod), mod.exports;
  } catch (e) {
    throw mod = 0, e;
  }
};
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toESM = (mod, isNodeMode, target) => (target = mod != null ? __create(__getProtoOf(mod)) : {}, __copyProps(
  // If the importer is in node compatibility mode or this is not an ESM
  // file that has been converted to a CommonJS file using a Babel-
  // compatible transform (i.e. "__esModule" has not been set), then set
  // "default" to the CommonJS "module.exports" for node compatibility.
  isNodeMode || !mod || !mod.__esModule ? __defProp(target, "default", { value: mod, enumerable: true }) : target,
  mod
));
var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

// vendor/ableton-extensions-sdk-1.0.0-beta.1/package 3/dist/index.cjs
var require_dist = __commonJS({
  "vendor/ableton-extensions-sdk-1.0.0-beta.1/package 3/dist/index.cjs"(exports2) {
    Object.defineProperty(exports2, Symbol.toStringTag, { value: "Module" });
    var DataModelObject3 = class DataModelObject4 {
      /** @internal */
      constructor(handle, dataModel, objectRegistry) {
        this.handle = handle;
        this.dataModel = dataModel;
        this.objectRegistry = objectRegistry;
      }
      /** The canonical parent of this object in Live's object hierarchy, or `null` if it has none. */
      get parent() {
        const handle = this.dataModel.getObjectCanonicalParent(this.handle);
        return handle ? this.objectRegistry.getObjectFromHandle(handle, DataModelObject4) : null;
      }
    };
    var invokeAsync = (dataModel, fn, ...args) => new Promise((resolve, reject) => {
      dataModel.withinTransaction(() => fn(...args, resolve, reject));
    });
    var createAsync = (dataModel, registry2, type, fn, ...args) => new Promise((resolve, reject) => {
      dataModel.withinTransaction(() => fn(...args, (handle) => resolve(registry2.getObjectFromHandle(handle, type)), reject));
    });
    var Clip2 = class extends DataModelObject3 {
      static className = "Clip";
      get name() {
        return this.dataModel.clipGetName(this.handle);
      }
      set name(name) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.clipSetName(this.handle, name);
        });
      }
      get startTime() {
        return this.dataModel.clipGetStartTime(this.handle);
      }
      get endTime() {
        return this.dataModel.clipGetEndTime(this.handle);
      }
      get duration() {
        return this.dataModel.clipGetEndTime(this.handle) - this.dataModel.clipGetStartTime(this.handle);
      }
      get startMarker() {
        return this.dataModel.clipGetStartMarker(this.handle);
      }
      get endMarker() {
        return this.dataModel.clipGetEndMarker(this.handle);
      }
      /**
      * Whether the clip is looped. Enabling looping on an unwarped audio clip
      * automatically enables warping.
      */
      get looping() {
        return this.dataModel.clipGetLooping(this.handle);
      }
      set looping(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.clipSetLooping(this.handle, value);
        });
      }
      get loopStart() {
        return this.dataModel.clipGetLoopStart(this.handle);
      }
      get loopEnd() {
        return this.dataModel.clipGetLoopEnd(this.handle);
      }
      get color() {
        return Number(this.dataModel.clipGetColor(this.handle));
      }
      set color(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.clipSetColor(this.handle, BigInt(value));
        });
      }
      get muted() {
        return this.dataModel.clipGetMuted(this.handle);
      }
      set muted(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.clipSetMuted(this.handle, value);
        });
      }
    };
    var AudioClip2 = class extends Clip2 {
      static className = "AudioClip";
      get filePath() {
        return this.dataModel.audioclipGetFilePath(this.handle);
      }
      get warping() {
        return this.dataModel.audioclipGetWarping(this.handle);
      }
      set warping(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.audioclipSetWarping(this.handle, value);
        });
      }
      get warpMode() {
        return this.dataModel.audioclipGetWarpMode(this.handle);
      }
      set warpMode(warpMode) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.audioclipSetWarpMode(this.handle, warpMode);
        });
      }
      get warpMarkers() {
        return this.dataModel.audioclipGetWarpMarkers(this.handle);
      }
    };
    var MidiClip = class extends Clip2 {
      static className = "MidiClip";
      get notes() {
        return this.dataModel.midiclipGetNotes(this.handle);
      }
      set notes(notes) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.midiclipSetNotes(this.handle, notes);
        });
      }
    };
    var ClipSlot2 = class extends DataModelObject3 {
      static className = "ClipSlot";
      get clip() {
        const handle = this.dataModel.clipslotGetClip(this.handle);
        return handle ? this.objectRegistry.getObjectFromHandle(handle, Clip2) : null;
      }
      /**
      * Deletes the clip in this slot. Await the returned promise to ensure the
      * deletion has been fully processed.
      */
      deleteClip() {
        return invokeAsync(this.dataModel, this.dataModel.clipslotDeleteClip, this.handle);
      }
      /** @param length - Length of the clip in beats. */
      createMidiClip(length) {
        return createAsync(this.dataModel, this.objectRegistry, MidiClip, this.dataModel.clipslotCreateMidiClip, this.handle, length);
      }
      /**
      * Creates an audio clip in this session slot.
      *
      * @param args.filePath - Absolute path to the audio file.
      * @param args.isWarped - See {@link AudioTrack.createAudioClip}.
      * @param args.loopSettings - See {@link AudioTrack.createAudioClip}.
      */
      createAudioClip(args) {
        return createAsync(this.dataModel, this.objectRegistry, AudioClip2, this.dataModel.clipslotCreateAudioClip, this.handle, {
          filePath: args.filePath,
          isWarped: args.isWarped,
          loopSettings: args.loopSettings
        });
      }
    };
    var DeviceParameter = class extends DataModelObject3 {
      static className = "DeviceParameter";
      get name() {
        return this.dataModel.deviceParameterGetName(this.handle);
      }
      get min() {
        return this.dataModel.deviceParameterGetInternalMin(this.handle);
      }
      get max() {
        return this.dataModel.deviceParameterGetInternalMax(this.handle);
      }
      get isQuantized() {
        return this.dataModel.deviceParameterGetIsQuantized(this.handle);
      }
      get defaultValue() {
        return this.dataModel.deviceParameterGetDefaultValue(this.handle);
      }
      get valueItems() {
        return this.dataModel.deviceParameterGetValueItems(this.handle);
      }
      getValue() {
        return new Promise((resolve) => {
          this.dataModel.deviceParameterGetInternalValue(this.handle, resolve);
        });
      }
      setValue(value) {
        return new Promise((resolve, reject) => {
          this.dataModel.withinTransaction(() => {
            this.dataModel.deviceParameterSetInternalValue(this.handle, value, resolve, (error) => reject(new Error(error)));
          });
        });
      }
    };
    var Device2 = class extends DataModelObject3 {
      static className = "Device";
      get name() {
        return this.dataModel.deviceGetName(this.handle);
      }
      get parameters() {
        return this.dataModel.deviceGetParameters(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, DeviceParameter));
      }
    };
    var TakeLane3 = class extends DataModelObject3 {
      static className = "TakeLane";
      get clips() {
        return this.dataModel.takelaneGetClips(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Clip2));
      }
      get name() {
        return this.dataModel.takelaneGetName(this.handle);
      }
      set name(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.takelaneSetName(this.handle, value);
        });
      }
      /**
      * @param startTime - Position in the arrangement in beats.
      * @param duration - Length of the clip in beats.
      */
      createMidiClip(startTime, duration) {
        return createAsync(this.dataModel, this.objectRegistry, MidiClip, this.dataModel.takelaneCreateMidiClip, this.handle, startTime, duration);
      }
      /**
      * Creates an audio clip on this take lane. See {@link AudioTrack.createAudioClip}
      * for argument semantics.
      */
      createAudioClip(args) {
        return createAsync(this.dataModel, this.objectRegistry, AudioClip2, this.dataModel.takelaneCreateAudioClip, this.handle, {
          duration: args.duration,
          filePath: args.filePath,
          isWarped: args.isWarped,
          loopSettings: args.loopSettings,
          startTime: args.startTime
        });
      }
    };
    var TrackMixer = class extends DataModelObject3 {
      static className = "MixerDevice";
      get volume() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.mixerdeviceGetVolume(this.handle), DeviceParameter);
      }
      get panning() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.mixerdeviceGetPanning(this.handle), DeviceParameter);
      }
      get sends() {
        return this.dataModel.mixerdeviceGetSends(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, DeviceParameter));
      }
    };
    var Track3 = class Track4 extends DataModelObject3 {
      static className = "Track";
      get name() {
        return this.dataModel.trackGetName(this.handle);
      }
      set name(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.trackSetName(this.handle, value);
        });
      }
      get mute() {
        return this.dataModel.trackGetMute(this.handle);
      }
      set mute(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.trackSetMute(this.handle, value);
        });
      }
      get solo() {
        return this.dataModel.trackGetSolo(this.handle);
      }
      set solo(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.trackSetSolo(this.handle, value);
        });
      }
      get mutedViaSolo() {
        return this.dataModel.trackGetMutedViaSolo(this.handle);
      }
      get arm() {
        return this.dataModel.trackGetArm(this.handle);
      }
      set arm(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.trackSetArm(this.handle, value);
        });
      }
      get clipSlots() {
        return this.dataModel.trackGetClipSlots(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, ClipSlot2));
      }
      get takeLanes() {
        return this.dataModel.trackGetTakeLanes(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, TakeLane3));
      }
      get arrangementClips() {
        return this.dataModel.trackGetArrangementClips(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Clip2));
      }
      get groupTrack() {
        const handle = this.dataModel.trackGetGroupTrack(this.handle);
        return handle ? this.objectRegistry.getObjectFromHandle(handle, Track4) : null;
      }
      get devices() {
        return this.dataModel.trackGetDevices(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Device2));
      }
      get mixer() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.trackGetMixerDevice(this.handle), TrackMixer);
      }
      /** Appended to the end of {@link takeLanes}. */
      createTakeLane() {
        return createAsync(this.dataModel, this.objectRegistry, TakeLane3, this.dataModel.trackCreateTakeLane, this.handle);
      }
      /**
      * Inserts a built-in Live device with its default preset into the track's device chain.
      * Only devices native to Live are supported – third-party plug-ins cannot be loaded this way.
      *
      * @param deviceName - The name of the built-in Live device (e.g. `"Reverb"`, `"Auto Filter"`).
      * @param index - Zero-based position in the device chain at which to insert.
      */
      insertDevice(deviceName, index) {
        return createAsync(this.dataModel, this.objectRegistry, Device2, this.dataModel.trackInsertDevice, this.handle, deviceName, BigInt(index));
      }
      /**
      * Deletes a device from this track's device chain. Await the returned
      * promise to ensure the deletion has been fully processed.
      */
      deleteDevice(device) {
        return invokeAsync(this.dataModel, this.dataModel.trackDeleteDevice, this.handle, device.handle);
      }
      /** The duplicate is inserted directly after the original in the device chain. */
      duplicateDevice(device) {
        return createAsync(this.dataModel, this.objectRegistry, Device2, this.dataModel.trackDuplicateDevice, this.handle, device.handle);
      }
      /**
      * Deletes an arrangement clip. For session clips, use {@link ClipSlot.deleteClip}.
      * Await the returned promise to ensure the deletion has been fully processed.
      */
      deleteClip(clip) {
        return invokeAsync(this.dataModel, this.dataModel.trackDeleteClip, this.handle, clip.handle);
      }
      /**
      * Deletes clips within the range. Clips that overlap a boundary are truncated
      * to the range edge rather than fully deleted.
      *
      * @param startTime - Start of the range in beats.
      * @param endTime - End of the range in beats.
      */
      clearClipsInRange(startTime, endTime) {
        return invokeAsync(this.dataModel, this.dataModel.trackClearClipsInRange, this.handle, startTime, endTime);
      }
    };
    var AudioTrack2 = class extends Track3 {
      static className = "AudioTrack";
      /**
      * Creates an audio clip from a file in the track's arrangement timeline.
      *
      * @param args.filePath - Absolute path to the audio file.
      * @param args.startTime - Position in the arrangement timeline in beats.
      * @param args.duration - Length of the clip on the arrangement timeline,
      *   in beats. Capped at the sample's natural length for non-looping clips;
      *   looping clips repeat to fill the full length. Defaults to the sample's
      *   natural length at the current tempo when omitted.
      * @param args.isWarped - Whether warping is enabled. Defaults to the clip's
      *   saved `.asd` settings if present, otherwise Live's "Auto-Warp" preference.
      *   Must be provided when `loopSettings` is provided.
      * @param args.loopSettings - Initial loop settings. Requires `isWarped` to be
      *   defined. If `isWarped` is `false`, `loopSettings.looping` must be `false`.
      *
      * @example
      * const clip = await track.createAudioClip({ filePath: '/samples/kick.wav', startTime: 0 });
      *
      * @example
      * const clip = await track.createAudioClip({
      *   filePath: '/samples/ambient.wav',
      *   startTime: 16,
      *   isWarped: false,
      * });
      *
      * @example
      * // Clip view: Start=beat 0, End=beat 2, Loop position=beat 0, Loop length=1 beat.
      * const clip = await track.createAudioClip({
      *   filePath: '/samples/loop.wav',
      *   startTime: 0,
      *   isWarped: true,
      *   loopSettings: { looping: true, startMarker: 0, endMarker: 2, loopStart: 0, loopEnd: 1 },
      * });
      *
      * @example
      * const clip = await track.createAudioClip({
      *   filePath: '/samples/loop.wav',
      *   startTime: 0,
      *   isWarped: true,
      *   duration: 8,
      *   loopSettings: { looping: true, startMarker: 0, endMarker: 2, loopStart: 0, loopEnd: 2 },
      * });
      */
      createAudioClip(args) {
        return createAsync(this.dataModel, this.objectRegistry, AudioClip2, this.dataModel.trackCreateAudioClip, this.handle, {
          duration: args.duration,
          filePath: args.filePath,
          isWarped: args.isWarped,
          loopSettings: args.loopSettings,
          startTime: args.startTime
        });
      }
    };
    var CuePoint = class extends DataModelObject3 {
      static className = "CuePoint";
      get time() {
        return this.dataModel.cuePointGetTime(this.handle);
      }
      get name() {
        return this.dataModel.cuePointGetName(this.handle);
      }
      set name(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.cuePointSetName(this.handle, value);
        });
      }
    };
    var MidiTrack2 = class extends Track3 {
      static className = "MidiTrack";
      /**
      * @param startTime - Position in the arrangement in beats.
      * @param duration - Length of the clip in beats.
      */
      createMidiClip(startTime, duration) {
        return createAsync(this.dataModel, this.objectRegistry, MidiClip, this.dataModel.trackCreateMidiClip, this.handle, startTime, duration);
      }
    };
    var Scene2 = class extends DataModelObject3 {
      static className = "Scene";
      get name() {
        return this.dataModel.sceneGetName(this.handle);
      }
      set name(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.sceneSetName(this.handle, value);
        });
      }
      get tempo() {
        return this.dataModel.sceneGetTempo(this.handle);
      }
      get signatureNumerator() {
        return this.dataModel.sceneGetSignatureNumerator(this.handle);
      }
      get signatureDenominator() {
        return this.dataModel.sceneGetSignatureDenominator(this.handle);
      }
    };
    var Song = class extends DataModelObject3 {
      static className = "Song";
      /** Regular tracks only – excludes return tracks and the main track. */
      get tracks() {
        return this.dataModel.songGetTracks(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Track3));
      }
      get returnTracks() {
        return this.dataModel.songGetReturnTracks(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Track3));
      }
      get mainTrack() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.songGetMainTrack(this.handle), Track3);
      }
      get scenes() {
        return this.dataModel.songGetScenes(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Scene2));
      }
      get cuePoints() {
        return this.dataModel.songGetCuePoints(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, CuePoint));
      }
      get tempo() {
        return this.dataModel.songGetTempo(this.handle);
      }
      set tempo(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.songSetTempo(this.handle, value);
        });
      }
      /**
      * The current arrangement grid quantization. Use with {@link gridIsTriplet} to
      * determine the full grid setting.
      */
      get gridQuantization() {
        return this.dataModel.songGetGridQuantization(this.handle);
      }
      /**
      * Whether the arrangement grid uses triplet subdivisions of the current
      * {@link gridQuantization} value.
      */
      get gridIsTriplet() {
        return this.dataModel.songGetGridIsTriplet(this.handle);
      }
      /**
      * The root note of the scale currently selected in Live, as a MIDI note number
      * from 0 (C) to 11 (B).
      */
      get rootNote() {
        return Number(this.dataModel.songGetRootNote(this.handle));
      }
      /** The name of the scale selected in Live, as shown in the Current Scale Name chooser. */
      get scaleName() {
        return this.dataModel.songGetScaleName(this.handle);
      }
      /** Whether Live's Scale Mode is enabled. */
      get scaleMode() {
        return this.dataModel.songGetScaleMode(this.handle);
      }
      /** The intervals of the current scale as semitone offsets from the root note. */
      get scaleIntervals() {
        return this.dataModel.songGetScaleIntervals(this.handle).map(Number);
      }
      /** Inserted after the last selected track, or appended if no track is selected. */
      createAudioTrack() {
        return createAsync(this.dataModel, this.objectRegistry, AudioTrack2, this.dataModel.songCreateAudioTrack, this.handle);
      }
      /** Inserted after the last selected track, or appended if no track is selected. */
      createMidiTrack() {
        return createAsync(this.dataModel, this.objectRegistry, MidiTrack2, this.dataModel.songCreateMidiTrack, this.handle);
      }
      /**
      * @param index - 0-based insert position in the range `[0, song.scenes.length]`.
      * Pass `-1` to append at the end.
      */
      createScene(index) {
        return createAsync(this.dataModel, this.objectRegistry, Scene2, this.dataModel.songCreateScene, this.handle, BigInt(index));
      }
      /**
      * Deletes a track from the song. Await the returned promise to ensure the
      * deletion has been fully processed.
      */
      deleteTrack(track) {
        return invokeAsync(this.dataModel, this.dataModel.songDeleteTrack, this.handle, track.handle);
      }
      /**
      * Deletes a scene from the song. Await the returned promise to ensure the
      * deletion has been fully processed.
      */
      deleteScene(scene) {
        return invokeAsync(this.dataModel, this.dataModel.songDeleteScene, this.handle, scene.handle);
      }
      /** Duplicates the track. The duplicate is inserted immediately after the original. */
      duplicateTrack(track) {
        return createAsync(this.dataModel, this.objectRegistry, Track3, this.dataModel.songDuplicateTrack, this.handle, track.handle);
      }
      /** Duplicates the scene. The duplicate is inserted immediately after the original. */
      duplicateScene(scene) {
        return createAsync(this.dataModel, this.objectRegistry, Scene2, this.dataModel.songDuplicateScene, this.handle, scene.handle);
      }
      /** @param time - Position in the arrangement in beats. */
      createCuePoint(time) {
        return createAsync(this.dataModel, this.objectRegistry, CuePoint, this.dataModel.songCreateCuePoint, this.handle, time);
      }
      /**
      * Deletes a cue point from the song. Await the returned promise to ensure
      * the deletion has been fully processed.
      */
      deleteCuePoint(cuePoint) {
        return invokeAsync(this.dataModel, this.dataModel.songDeleteCuePoint, this.handle, cuePoint.handle);
      }
    };
    var Application = class extends DataModelObject3 {
      static className = "Application";
      get song() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.rootGetSong(this.handle), Song);
      }
    };
    var Commands = class {
      module;
      /** @internal */
      constructor(module3) {
        this.module = module3;
      }
      /**
      * Registers a command that can be invoked by Live or via {@link Commands.executeCommand}.
      *
      * @param commandId - A unique string identifier for this command.
      * @param callback - Called when the command is invoked. May receive arguments passed by the invoker.
      */
      registerCommand(commandId, callback) {
        this.module.registerCommand(commandId, callback);
      }
      /**
      * Programmatically invokes a registered command.
      *
      * @param commandId - The ID of the command to invoke.
      * @param args - Arguments to pass to the command's callback.
      */
      executeCommand(commandId, ...args) {
        this.module.executeCommand(commandId, ...args);
      }
    };
    var ChainMixer = class extends DataModelObject3 {
      static className = "ChainMixerDevice";
      get volume() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.chainmixerdeviceGetVolume(this.handle), DeviceParameter);
      }
      get panning() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.chainmixerdeviceGetPanning(this.handle), DeviceParameter);
      }
      get sends() {
        return this.dataModel.chainmixerdeviceGetSends(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, DeviceParameter));
      }
    };
    var Chain2 = class extends DataModelObject3 {
      static className = "Chain";
      get devices() {
        return this.dataModel.chainGetDevices(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Device2));
      }
      get mixer() {
        return this.objectRegistry.getObjectFromHandle(this.dataModel.chainGetMixerDevice(this.handle), ChainMixer);
      }
      /**
      * Inserts a built-in Live device with its default preset into the chain.
      * Only devices native to Live are supported – third-party plug-ins cannot be loaded this way.
      *
      * @param deviceName - The name of the built-in Live device (e.g. `"Reverb"`, `"Auto Filter"`).
      * @param index - Zero-based position in the device chain at which to insert.
      */
      insertDevice(deviceName, index) {
        return createAsync(this.dataModel, this.objectRegistry, Device2, this.dataModel.chainInsertDevice, this.handle, deviceName, BigInt(index));
      }
      /**
      * Deletes a device from this chain. Await the returned promise to ensure
      * the deletion has been fully processed.
      */
      deleteDevice(device) {
        return invokeAsync(this.dataModel, this.dataModel.chainDeleteDevice, this.handle, device.handle);
      }
      /** The duplicate is inserted directly after the original in the device chain. */
      duplicateDevice(device) {
        return createAsync(this.dataModel, this.objectRegistry, Device2, this.dataModel.chainDuplicateDevice, this.handle, device.handle);
      }
    };
    var DrumChain2 = class extends Chain2 {
      static className = "DrumChain";
      get receivingNote() {
        return Number(this.dataModel.drumchainGetReceivingNote(this.handle));
      }
      set receivingNote(value) {
        this.dataModel.withinTransaction(() => {
          this.dataModel.drumchainSetReceivingNote(this.handle, BigInt(value));
        });
      }
    };
    var RackDevice2 = class extends Device2 {
      static className = "RackDevice";
      get chains() {
        return this.dataModel.rackdeviceGetChains(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, Chain2));
      }
      /** @param index - 0-based insert position in the range `[0, rack.chains.length]`. */
      insertChain(index) {
        return createAsync(this.dataModel, this.objectRegistry, Chain2, this.dataModel.rackdeviceInsertChain, this.handle, BigInt(index));
      }
    };
    var DrumRack2 = class extends RackDevice2 {
      static className = "DrumRackDevice";
      get chains() {
        return this.dataModel.rackdeviceGetChains(this.handle).map((handle) => this.objectRegistry.getObjectFromHandle(handle, DrumChain2));
      }
    };
    var Sample = class extends DataModelObject3 {
      static className = "Sample";
      get filePath() {
        return this.dataModel.sampleGetFilePath(this.handle);
      }
    };
    var Simpler = class extends Device2 {
      static className = "Simpler";
      get sample() {
        const handle = this.dataModel.simplerGetSample(this.handle);
        return handle ? this.objectRegistry.getObjectFromHandle(handle, Sample) : null;
      }
      /** Replaces the loaded sample with the audio file at the given absolute path. */
      replaceSample(filePath) {
        return createAsync(this.dataModel, this.objectRegistry, Sample, this.dataModel.simplerReplaceSample, this.handle, filePath);
      }
    };
    var dataModelClasses = [
      Application,
      Song,
      AudioTrack2,
      MidiTrack2,
      Track3,
      AudioClip2,
      MidiClip,
      Clip2,
      ClipSlot2,
      TakeLane3,
      Simpler,
      DrumRack2,
      RackDevice2,
      Device2,
      Sample,
      DrumChain2,
      Chain2,
      Scene2,
      CuePoint,
      DeviceParameter,
      TrackMixer,
      ChainMixer
    ];
    var DataModelObjectRegistry = class {
      cache = /* @__PURE__ */ new Map();
      dataModel;
      /** @internal */
      constructor(dataModel) {
        this.dataModel = dataModel;
      }
      getOrCreateObjectFromHandle(handle) {
        const cached = this.cache.get(handle.id);
        if (cached) return cached;
        const ModelClass = dataModelClasses.find((cls) => this.dataModel.getObjectIsOfClass(handle, cls.className));
        if (!ModelClass) throw new Error("Unknown object type");
        const obj = new ModelClass(handle, this.dataModel, this);
        this.cache.set(handle.id, obj);
        return obj;
      }
      /**
      * Resolves a {@link Handle} into a typed SDK object.
      *
      * Pass {@link DataModelObject} as `type` when the exact type of the handle is not known
      * in advance, then use `instanceof` to branch on the actual type:
      *
      * ```ts
      * const obj = objects.getObjectFromHandle(handle, DataModelObject);
      * if (obj instanceof ClipSlot) {
      *   // ...
      * }
      * ```
      *
      * Throws if the underlying object has been deleted, if it is of a different
      * type than `type`, or if its type is not recognised.
      *
      * @param handle - The handle to resolve.
      * @param type - The expected SDK class (e.g. `ClipSlot`).
      */
      getObjectFromHandle(handle, type) {
        const obj = this.getOrCreateObjectFromHandle(handle);
        if (!(obj instanceof type)) throw new Error("Object of incorrect type");
        return obj;
      }
    };
    var Environment = class {
      module;
      /** @internal */
      constructor(module3) {
        this.module = module3;
      }
      /**
      * Per-extension directory for persistent storage. Use it for configuration, credentials,
      * and cached state – anything that should survive across Live sessions.
      */
      get storageDirectory() {
        return this.module.storageDirectory;
      }
      /**
      * Per-extension directory for temporary files, such as intermediate audio or analysis
      * results. May be cleaned up between sessions.
      */
      get tempDirectory() {
        return this.module.tempDirectory;
      }
      /** Live's current UI language as an uppercase ISO 639-1 code (e.g. `"EN"`, `"DE"`, `"JA"`). */
      get language() {
        return this.module.language;
      }
    };
    var Resources = class {
      module;
      /** @internal */
      constructor(module3) {
        this.module = module3;
      }
      /**
      * Renders the pre-effects audio of a track in the arrangement between two beat
      * positions. Returns a path to an audio file written to the extension's temp
      * directory. The file format (e.g. WAV or AIFF) follows Live's Record File Type
      * setting.
      */
      renderPreFxAudio(track, startTime, endTime) {
        return new Promise((resolve, reject) => {
          this.module.renderPreFxAudio(track.handle, {
            endTime,
            startTime
          }, resolve, reject);
        });
      }
      /**
      * Copies a file into the Live project folder so that Live manages it.
      * Returns the path to the imported copy. Use the returned path in subsequent API
      * calls, not the original.
      */
      importIntoProject(filePath) {
        return new Promise((resolve, reject) => {
          this.module.importIntoProject(filePath, resolve, reject);
        });
      }
    };
    var toProgressOptions = (text, progress) => typeof progress === "number" ? {
      progress,
      text
    } : { text };
    var Ui = class {
      module;
      /** @internal */
      constructor(module3) {
        this.module = module3;
      }
      /**
      * Registers a context menu action in the given {@link ContextMenuScope}.
      *
      * When the user triggers the action, Live invokes the command identified by
      * `commandId`. Depending on the scope, the command receives either the triggered
      * object's {@link Handle}, an {@link ArrangementSelection}, or a
      * {@link ClipSlotSelection} as its first argument.
      *
      * Returns a function that unregisters the action when called.
      */
      registerContextMenuAction(scope, title, commandId) {
        return new Promise((resolve) => {
          this.module.registerContextMenuAction(scope, title, commandId, (unregister) => {
            resolve(() => new Promise((done) => {
              unregister(done);
            }));
          });
        });
      }
      /**
      * Opens a modal dialog that loads the given URL. Supported URL schemes are
      * `file:`, `data:`, `https:`, and `http://localhost`.
      *
      * To return a result and close the dialog, the dialog's HTML must post the message
      * `{ method: "close_and_send", params: [resultString] }` to the host's message
      * handler – `window.webkit.messageHandlers.live.postMessage` on macOS or
      * `window.chrome.webview.postMessage` on Windows. The returned promise resolves
      * with that string.
      *
      * Rejects if `url` is malformed or an unexpected error occurred.
      */
      showModalDialog(url, width, height) {
        return new Promise((resolve, reject) => {
          this.module.showModalDialog(url, width, height, resolve, reject);
        });
      }
      /**
      * Shows a progress dialog while `callback` runs.
      * The callback receives an `update` function to change the text/progress
      * (progress is a percentage, 0–100), and an `AbortSignal` that fires if
      * the user cancels the dialog.
      * The dialog closes automatically when the callback resolves or rejects.
      *
      * @example
      * ```ts
      * const audioPath = await ui.withinProgressDialog(
      *   "Rendering audio…",
      *   { progress: 0 },
      *   async (update, signal) => {
      *     await update("Analysing…", 30);
      *     if (signal.aborted) return;
      *     await update("Rendering…", 70);
      *     return await resources.renderPreFxAudio(track, startBeat, endBeat);
      *   },
      * );
      * ```
      */
      withinProgressDialog(text, options, callback) {
        const ac = new AbortController();
        return new Promise((resolve, reject) => {
          this.module.showProgressDialog(toProgressOptions(text, options.progress), ({ update, close }) => {
            const asyncUpdate = (updateText, progress) => new Promise((resolveUpdate) => {
              update(toProgressOptions(updateText, progress), resolveUpdate);
            });
            const asyncClose = () => new Promise((done) => {
              close(done);
            });
            callback(asyncUpdate, ac.signal).finally(asyncClose).then(resolve).catch(reject);
          }, () => {
            ac.abort();
          });
        });
      }
    };
    var initialize2 = (context, apiVersion) => {
      const { commands, dataModel, environment, resources, ui } = context.initializeExtensionHost({ apiVersion });
      const objectRegistry = new DataModelObjectRegistry(dataModel);
      return {
        application: objectRegistry.getObjectFromHandle(dataModel.getRoot(), Application),
        commands: new Commands(commands),
        environment: new Environment(environment),
        getObjectFromHandle: objectRegistry.getObjectFromHandle.bind(objectRegistry),
        resources: new Resources(resources),
        ui: new Ui(ui),
        withinTransaction: dataModel.withinTransaction.bind(dataModel)
      };
    };
    var GridQuantization = /* @__PURE__ */ (function(GridQuantization2) {
      GridQuantization2[GridQuantization2["NoGrid"] = 0] = "NoGrid";
      GridQuantization2[GridQuantization2["EightBars"] = 1] = "EightBars";
      GridQuantization2[GridQuantization2["FourBars"] = 2] = "FourBars";
      GridQuantization2[GridQuantization2["TwoBars"] = 3] = "TwoBars";
      GridQuantization2[GridQuantization2["Bar"] = 4] = "Bar";
      GridQuantization2[GridQuantization2["Half"] = 5] = "Half";
      GridQuantization2[GridQuantization2["Quarter"] = 6] = "Quarter";
      GridQuantization2[GridQuantization2["Eighth"] = 7] = "Eighth";
      GridQuantization2[GridQuantization2["Sixteenth"] = 8] = "Sixteenth";
      GridQuantization2[GridQuantization2["ThirtySecond"] = 9] = "ThirtySecond";
      return GridQuantization2;
    })({});
    var WarpMode = /* @__PURE__ */ (function(WarpMode2) {
      WarpMode2[WarpMode2["Beats"] = 0] = "Beats";
      WarpMode2[WarpMode2["Tones"] = 1] = "Tones";
      WarpMode2[WarpMode2["Texture"] = 2] = "Texture";
      WarpMode2[WarpMode2["Repitch"] = 3] = "Repitch";
      WarpMode2[WarpMode2["Complex"] = 4] = "Complex";
      WarpMode2[WarpMode2["ComplexPro"] = 6] = "ComplexPro";
      return WarpMode2;
    })({});
    var EXTENSIONS_API_VERSIONS = ["1.0.0"];
    exports2.Application = Application;
    exports2.AudioClip = AudioClip2;
    exports2.AudioTrack = AudioTrack2;
    exports2.Chain = Chain2;
    exports2.ChainMixer = ChainMixer;
    exports2.Clip = Clip2;
    exports2.ClipSlot = ClipSlot2;
    exports2.Commands = Commands;
    exports2.CuePoint = CuePoint;
    exports2.DataModelObject = DataModelObject3;
    exports2.Device = Device2;
    exports2.DeviceParameter = DeviceParameter;
    exports2.DrumChain = DrumChain2;
    exports2.DrumRack = DrumRack2;
    exports2.EXTENSIONS_API_VERSIONS = EXTENSIONS_API_VERSIONS;
    exports2.Environment = Environment;
    exports2.GridQuantization = GridQuantization;
    exports2.MidiClip = MidiClip;
    exports2.MidiTrack = MidiTrack2;
    exports2.RackDevice = RackDevice2;
    exports2.Resources = Resources;
    exports2.Sample = Sample;
    exports2.Scene = Scene2;
    exports2.Simpler = Simpler;
    exports2.Song = Song;
    exports2.TakeLane = TakeLane3;
    exports2.Track = Track3;
    exports2.TrackMixer = TrackMixer;
    exports2.Ui = Ui;
    exports2.WarpMode = WarpMode;
    exports2.initialize = initialize2;
  }
});

// apps/live-extension/src/extension.ts
var extension_exports = {};
__export(extension_exports, {
  activate: () => activate,
  deactivate: () => deactivate
});
module.exports = __toCommonJS(extension_exports);
var import_node_fs3 = require("node:fs");
var import_node_os = require("node:os");
var import_node_path2 = require("node:path");
var import_sdk4 = __toESM(require_dist());

// apps/live-extension/src/operations.ts
var import_node_fs2 = require("node:fs");
var import_node_path = require("node:path");
var import_sdk2 = __toESM(require_dist());

// apps/live-extension/src/audio-info.ts
var import_node_fs = require("node:fs");
function head(path, length) {
  const fd = (0, import_node_fs.openSync)(path, "r");
  try {
    const buffer = Buffer.alloc(length);
    const read = (0, import_node_fs.readSync)(fd, buffer, 0, length, 0);
    return buffer.subarray(0, read);
  } finally {
    (0, import_node_fs.closeSync)(fd);
  }
}
function extended(buffer, offset) {
  const exponent = buffer.readUInt16BE(offset) & 32767;
  const mantissa = buffer.readUInt32BE(offset + 2) * 2 ** 32 + buffer.readUInt32BE(offset + 6);
  if (exponent === 0 && mantissa === 0) return 0;
  return mantissa * 2 ** (exponent - 16383 - 63);
}
function audioInfo(path) {
  const bytes = (0, import_node_fs.statSync)(path).size;
  const buffer = head(path, Math.min(bytes, 1 << 16));
  const tag = buffer.toString("ascii", 0, 4);
  const kind = buffer.toString("ascii", 8, 12);
  if ((tag === "RIFF" || tag === "RF64") && kind === "WAVE") {
    let channels = 0;
    let sampleRate = 0;
    let bitDepth = null;
    let dataBytes = 0;
    for (let offset = 12; offset + 8 <= buffer.length; ) {
      const id = buffer.toString("ascii", offset, offset + 4);
      const size = buffer.readUInt32LE(offset + 4);
      if (id === "fmt ") {
        channels = buffer.readUInt16LE(offset + 10);
        sampleRate = buffer.readUInt32LE(offset + 12);
        bitDepth = buffer.readUInt16LE(offset + 22);
      }
      if (id === "data") {
        dataBytes = size === 4294967295 ? bytes - offset - 8 : size;
        break;
      }
      offset += 8 + size + (size & 1);
    }
    if (!channels || !sampleRate || !bitDepth) throw new Error("the render isn't a WAV file Kumi can read");
    return { format: "wav", channels, sampleRate, bitDepth, seconds: dataBytes / (channels * (bitDepth / 8) * sampleRate), bytes };
  }
  if (tag === "FORM" && (kind === "AIFF" || kind === "AIFC")) {
    for (let offset = 12; offset + 8 <= buffer.length; ) {
      const id = buffer.toString("ascii", offset, offset + 4);
      const size = buffer.readUInt32BE(offset + 4);
      if (id === "COMM") {
        const channels = buffer.readUInt16BE(offset + 8);
        const frames = buffer.readUInt32BE(offset + 10);
        const bitDepth = buffer.readUInt16BE(offset + 14);
        const sampleRate = Math.round(extended(buffer, offset + 16));
        return { format: "aiff", channels, sampleRate, bitDepth, seconds: sampleRate ? frames / sampleRate : 0, bytes };
      }
      offset += 8 + size + (size & 1);
    }
  }
  throw new Error("the render is neither WAV nor AIFF");
}

// apps/live-extension/src/refs.ts
var import_sdk = __toESM(require_dist());
function parseRef(reference) {
  const parts = reference.split(":");
  if (parts.length < 3) throw new Error(`not a Live reference: ${reference}`);
  const [epoch, kind, ...rest] = parts;
  const path = rest.map((part) => {
    const index = Number(part);
    if (!Number.isSafeInteger(index) || index < 0) throw new Error(`a reference this channel can't follow: ${reference}`);
    return index;
  });
  return { epoch, kind, path };
}
function makeRef(epoch, kind, path) {
  return `${epoch}:${kind}:${path.join(":")}`;
}
function allTracks(context) {
  const song = context.application.song;
  return [...song.tracks, ...song.returnTracks, song.mainTrack];
}
function at(items, index, what) {
  if (index === void 0 || index >= items.length) throw new Error(`${what} ${index ?? "?"} isn't in the Set any more`);
  return items[index];
}
function trackAt(context, index) {
  return at(allTracks(context), index, "track");
}
function deviceAt(context, path) {
  if (path.length < 2 || path.length % 2 !== 0) throw new Error(`not a device path: ${path.join(":")}`);
  const track = trackAt(context, path[0]);
  let owner = track;
  let device = at(track.devices, path[1], "device");
  let index = path[1];
  for (let step = 2; step < path.length; step += 2) {
    if (!(device instanceof import_sdk.RackDevice)) throw new Error(`${device.name} has no chains`);
    owner = at(device.chains, path[step], "chain");
    index = path[step + 1];
    device = at(owner.devices, index, "device");
  }
  return { device, owner, index, track };
}
function takeLaneAt(context, path) {
  return at(trackAt(context, path[0]).takeLanes, path[1], "take lane");
}
function checkName(object, expected2, what) {
  if (expected2 !== void 0 && object.name !== expected2) throw new Error(`the ${what} at that position is "${object.name}" now, not "${expected2}"`);
}
var same = (a, b) => a.handle.id === b.handle.id;
function locateDevice(devices, target, path, trail) {
  for (const [index, device] of devices.entries()) {
    if (same(device, target)) return { kind: "device", path: [...path, index], name: device.name, trail: [...trail, device.name] };
    if (device instanceof import_sdk.RackDevice) {
      for (const [chainIndex, chain] of device.chains.entries()) {
        const found = locateDevice(chain.devices, target, [...path, index, chainIndex], [...trail, device.name]);
        if (found) return found;
      }
    }
  }
  return void 0;
}
function locate(context, target) {
  const song = context.application.song;
  for (const [index, scene] of song.scenes.entries()) if (same(scene, target)) return { kind: "scene", path: [index], name: scene.name, trail: [scene.name] };
  for (const [t, track] of allTracks(context).entries()) {
    if (same(track, target)) return { kind: "track", path: [t], name: track.name, trail: [track.name] };
    for (const [s, slot] of track.clipSlots.entries()) {
      if (same(slot, target)) return { kind: "clip_slot", path: [t, s], name: slot.clip?.name ?? "", trail: [track.name] };
      const clip = slot.clip;
      if (clip && same(clip, target)) return { kind: "clip", path: [t, s], name: clip.name, trail: [track.name, clip.name] };
    }
    for (const [c, clip] of track.arrangementClips.entries()) if (same(clip, target)) return { kind: "arrangement_clip", path: [t, c], name: clip.name, trail: [track.name, clip.name] };
    for (const [l, lane] of track.takeLanes.entries()) {
      if (same(lane, target)) return { kind: "take_lane", path: [t, l], name: lane.name, trail: [track.name, lane.name] };
      for (const [c, clip] of lane.clips.entries()) if (same(clip, target)) return { kind: "take_lane_clip", path: [t, l, c], name: clip.name, trail: [track.name, lane.name, clip.name] };
    }
    const device = locateDevice(track.devices, target, [t], [track.name]);
    if (device) return device;
  }
  return void 0;
}

// apps/live-extension/src/wire.ts
var import_node_crypto = require("node:crypto");
var LOOPBACK_PROTOCOL = "ableton-loopback/v1";
var LIVE_PROTOCOL = "ableton-live/v1";
var MAX_FRAME_BYTES = 256 * 1048576;
var MAX_DEPTH = 256;
var MAX_STRING = 1048576;
var MAX_ARRAY = 1e7;
var MAX_KEYS = 1e6;
function canonical(value, depth = 0) {
  if (depth > MAX_DEPTH) throw new Error("wire payload is too deeply nested");
  if (value === null || typeof value === "boolean") return JSON.stringify(value);
  if (typeof value === "string") {
    if (value.length > MAX_STRING) throw new Error("wire string is too large");
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new Error("wire number is not finite");
    return JSON.stringify(Object.is(value, -0) ? 0 : value);
  }
  if (Array.isArray(value)) {
    if (value.length > MAX_ARRAY) throw new Error("wire array is too large");
    return `[${value.map((item) => canonical(item, depth + 1)).join(",")}]`;
  }
  if (typeof value === "object") {
    const object = value;
    const keys = Object.keys(object);
    if (keys.length > MAX_KEYS) throw new Error("wire object is too large");
    return `{${keys.sort().map((key) => `${JSON.stringify(key)}:${canonical(object[key], depth + 1)}`).join(",")}}`;
  }
  throw new Error("unsupported wire value");
}
function sign(secret, payload) {
  const text = canonical(payload);
  if (Buffer.byteLength(text) > MAX_FRAME_BYTES) throw new Error("wire payload is too large");
  return (0, import_node_crypto.createHmac)("sha256", secret).update(text).digest("base64url");
}
function signed(secret, payload) {
  return { ...payload, mac: sign(secret, payload) };
}
function verify(secret, frame) {
  const { mac, ...unsigned } = frame;
  if (typeof mac !== "string") return false;
  let expected2;
  try {
    expected2 = Buffer.from(sign(secret, unsigned));
  } catch {
    return false;
  }
  const received = Buffer.from(mac);
  return expected2.length === received.length && (0, import_node_crypto.timingSafeEqual)(expected2, received);
}
function token(bytes = 18) {
  return (0, import_node_crypto.randomBytes)(bytes).toString("base64url");
}

// apps/live-extension/src/operations.ts
var str = (value) => typeof value === "string" ? value : void 0;
var expected = (args) => {
  const name = str(args.expectedName);
  if (name === void 0) throw new Error("expectedName is required on the Extensions channel");
  return name;
};
var num = (value) => {
  if (typeof value !== "number" || !Number.isFinite(value)) throw new Error("a number is missing");
  return value;
};
var RENDER_KEEP_MS = 6 * 60 * 60 * 1e3;
function pruneRenders(dir) {
  const now = Date.now();
  for (const name of (0, import_node_fs2.existsSync)(dir) ? (0, import_node_fs2.readdirSync)(dir) : []) {
    const path = (0, import_node_path.join)(dir, name);
    try {
      if (now - (0, import_node_fs2.statSync)(path).mtimeMs > RENDER_KEEP_MS) (0, import_node_fs2.unlinkSync)(path);
    } catch {
    }
  }
}
var renderOffline = async (context, args, environment) => {
  const { path } = parseRef(String(args.trackRef));
  const track = trackAt(context, path[0]);
  checkName(track, expected(args), "track");
  if (!(track instanceof import_sdk2.AudioTrack)) throw new Error(`"${track.name}" isn't an audio track: offline renders are of an audio track's own clips, before its devices`);
  if (allTracks(context).some((other) => other.groupTrack?.handle.id === track.handle.id)) throw new Error(`"${track.name}" is a group: render its tracks`);
  const from = num(args.fromBeat);
  const to = num(args.toBeat);
  if (!(to > from)) throw new Error("the range to render is empty");
  const started = performance.now();
  const rendered = await context.resources.renderPreFxAudio(track, from, to);
  const renderMs = performance.now() - started;
  (0, import_node_fs2.mkdirSync)(environment.rendersDir, { recursive: true, mode: 448 });
  pruneRenders(environment.rendersDir);
  const target = (0, import_node_path.join)(environment.rendersDir, `${Date.now()}-${token(6)}${(0, import_node_path.extname)(rendered).toLowerCase() || ".wav"}`);
  (0, import_node_fs2.copyFileSync)(rendered, target);
  try {
    (0, import_node_fs2.unlinkSync)(rendered);
  } catch {
  }
  const info = audioInfo(target);
  return { path: target, format: info.format, channels: info.channels, sampleRate: info.sampleRate, bitDepth: info.bitDepth, seconds: info.seconds, bytes: info.bytes, renderMs };
};
function noteDescriptions(value) {
  if (!Array.isArray(value)) throw new Error("notes must be a list");
  return value.map((raw) => {
    const note = raw;
    const description = { pitch: num(note.pitch), startTime: num(note.start), duration: num(note.duration) };
    if (typeof note.velocity === "number") description.velocity = note.velocity;
    if (typeof note.mute === "boolean") description.muted = note.mute;
    if (typeof note.probability === "number") description.probability = note.probability;
    if (typeof note.velocityDeviation === "number") description.velocityDeviation = note.velocityDeviation;
    if (typeof note.releaseVelocity === "number") description.releaseVelocity = note.releaseVelocity;
    return description;
  });
}
async function makeArrangementMidiClip(context, args) {
  const reference = String(args.trackRef);
  const { epoch, path } = parseRef(reference);
  const track = trackAt(context, path[0]);
  checkName(track, expected(args), "track");
  if (!(track instanceof import_sdk2.MidiTrack)) throw new Error(`"${track.name}" isn't a MIDI track`);
  const notes = noteDescriptions(args.notes);
  let lane = track;
  let laneIndex;
  if (typeof args.takeLaneRef === "string") {
    const lanePath = parseRef(args.takeLaneRef).path;
    if (lanePath[0] !== path[0]) throw new Error("that take lane belongs to another track");
    lane = takeLaneAt(context, lanePath);
    laneIndex = lanePath[1];
  }
  const clip = await lane.createMidiClip(num(args.start), num(args.length));
  return {
    finish: () => {
      clip.notes = notes;
      if (typeof args.name === "string") clip.name = args.name;
      if (typeof args.looping === "boolean") clip.looping = args.looping;
    },
    result: () => {
      const clips = lane instanceof import_sdk2.TakeLane ? lane.clips : track.arrangementClips;
      const index = clips.findIndex((candidate) => candidate.handle.id === clip.handle.id);
      if (index < 0) throw new Error("Live made the clip, but it isn't among the lane's clips");
      const ref = laneIndex === void 0 ? makeRef(epoch, "arrangement_clip", [path[0], index]) : makeRef(epoch, "take_lane_clip", [path[0], laneIndex, index]);
      return { ref, trackRef: reference, name: clip.name, start: clip.startTime, end: clip.endTime, notes: clip.notes.length };
    }
  };
}
var arrangementMidiClip = async (context, args) => {
  const made = await makeArrangementMidiClip(context, args);
  context.withinTransaction(made.finish);
  return made.result();
};
var clearRange = async (context, args) => {
  const reference = String(args.trackRef);
  const { path } = parseRef(reference);
  const track = trackAt(context, path[0]);
  checkName(track, expected(args), "track");
  if (args.takeLaneRef !== void 0) throw new Error("a range is cleared on the track's own lane");
  const from = num(args.fromBeat);
  const to = num(args.toBeat);
  if (!(to > from)) throw new Error("the range to clear is empty");
  const before = track.arrangementClips.map((clip) => ({ name: clip.name, start: clip.startTime, end: clip.endTime, isAudio: clip instanceof import_sdk2.AudioClip }));
  await track.clearClipsInRange(from, to);
  const clipsAfter = track.arrangementClips.length;
  return { trackRef: reference, clipsBefore: before.length, clipsAfter, removed: before.filter((clip) => clip.start >= from && clip.end <= to) };
};
var duplicateDevice = async (context, args) => {
  const reference = String(args.ref);
  const { epoch, path } = parseRef(reference);
  const { device, owner, index } = deviceAt(context, path);
  checkName(device, expected(args), "device");
  const copy = await owner.duplicateDevice(device);
  const copyIndex = owner.devices.findIndex((candidate) => candidate.handle.id === copy.handle.id);
  if (copyIndex < 0) throw new Error("Live made the copy, but not in this device's chain");
  return { ref: makeRef(epoch, "device", [...path.slice(0, -1), copyIndex]), name: copy.name, index: copyIndex };
};
var padSampleChain = async (context, args) => {
  const reference = String(args.rackRef);
  const { epoch, path } = parseRef(reference);
  const { device: rack } = deviceAt(context, path);
  checkName(rack, expected(args), "Drum Rack");
  if (!(rack instanceof import_sdk2.DrumRack)) throw new Error(`"${rack.name}" isn't a Drum Rack`);
  const samplePath = String(args.samplePath);
  if (!(0, import_node_fs2.existsSync)(samplePath)) throw new Error("the sample isn't there any more");
  const note = num(args.note);
  if (rack.chains.some((existing) => existing.receivingNote === note)) throw new Error(`pad ${note} of "${rack.name}" already plays something; clear it first`);
  const chain = await rack.insertChain(rack.chains.length);
  if (!(chain instanceof import_sdk2.DrumChain)) throw new Error("Live didn't add a pad chain to the Drum Rack");
  chain.receivingNote = note;
  const simpler = await chain.insertDevice("Simpler", 0);
  if (!("replaceSample" in simpler)) throw new Error("Live didn't put a Simpler in the new chain");
  await simpler.replaceSample(samplePath);
  const chainIndex = rack.chains.findIndex((candidate) => candidate.handle.id === chain.handle.id);
  if (chainIndex < 0) throw new Error("Live added the chain, but it isn't in the rack's chains");
  return { chainRef: makeRef(epoch, "chain", [...path, chainIndex]), deviceRef: makeRef(epoch, "device", [...path, chainIndex, 0]), note, samplePath };
};
var projectImport = async (context, args) => ({ path: await context.resources.importIntoProject(String(args.filePath)) });
var OPERATIONS = {
  "render.offline": renderOffline,
  "arrangement.midi-clip.create": arrangementMidiClip,
  "clip.clear-range": clearRange,
  "device.duplicate": duplicateDevice,
  "drum-pad.sample-chain": padSampleChain,
  "project.import": projectImport
};
function transactionGroup(validate2) {
  return async (context, args, environment) => {
    const steps = Array.isArray(args.ops) ? args.ops : [];
    for (const [index, step] of steps.entries()) {
      if (!OPERATIONS[step.operation]) throw new Error(`step ${index + 1}: a group can't hold ${step.operation}`);
      validate2(step.operation, step.args);
    }
    const start = (step) => step.operation === "arrangement.midi-clip.create" ? makeArrangementMidiClip(context, step.args) : OPERATIONS[step.operation](context, step.args, environment);
    const settled = await context.withinTransaction(() => Promise.allSettled(steps.map(start)));
    const made = settled.flatMap((outcome) => outcome.status === "fulfilled" && typeof outcome.value.finish === "function" ? [outcome.value] : []);
    if (made.length) context.withinTransaction(() => {
      for (const clip of made) clip.finish();
    });
    const failed = settled.findIndex((outcome) => outcome.status === "rejected");
    if (failed >= 0) {
      const reason = settled[failed].reason;
      const done = settled.filter((outcome) => outcome.status === "fulfilled").length;
      throw new Error(`step ${failed + 1} failed (${reason instanceof Error ? reason.message : String(reason ?? "Live refused it without a reason")}); ${done} of ${steps.length} steps were made`);
    }
    return { results: settled.map((outcome) => {
      const value = outcome.value;
      return typeof value.finish === "function" ? value.result() : value;
    }) };
  };
}

// apps/live-extension/src/registry.ts
var import_node_crypto2 = require("node:crypto");

// protocol/ableton-live-v1.operations.json
var ableton_live_v1_operations_default = {
  version: 1,
  protocol: "ableton-live/v1",
  operations: [
    {
      id: "application.dialog",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          action: {
            type: "string",
            enum: [
              "read",
              "press"
            ]
          },
          button: {
            type: "integer",
            minimum: 0,
            maximum: 16
          },
          expectedMessage: {
            type: [
              "string",
              "null"
            ],
            maxLength: 1024
          },
          expectedButtonCount: {
            type: "integer",
            minimum: 0,
            maximum: 64
          },
          expectedOpenDialogCount: {
            type: "integer",
            minimum: 0,
            maximum: 64
          }
        },
        required: [
          "action"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          buttonCount: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 64
          },
          message: {
            type: [
              "string",
              "null"
            ],
            maxLength: 1024
          },
          openDialogCount: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 64
          },
          done: {
            type: "boolean"
          }
        },
        required: [
          "buttonCount",
          "message",
          "openDialogCount",
          "done"
        ],
        additionalProperties: false
      }
    },
    {
      id: "application.message",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          text: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          modal: {
            type: "boolean"
          }
        },
        required: [
          "text"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          shown: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "shown"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.audio-clip.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "trackRef",
          "filePath",
          "position",
          "expectedTrackIdentity",
          "expectedCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "start",
          "length",
          "filePath",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.automation.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          created: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "created"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.automation.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.automation.point.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          from: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          to: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "from",
          "to",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.automation.point.insert",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          points: {
            type: "array",
            items: {
              type: "object",
              properties: {
                time: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                value: {
                  type: "number",
                  minimum: -1e6,
                  maximum: 1e6
                }
              },
              required: [
                "time",
                "value"
              ],
              additionalProperties: false
            },
            minItems: 1
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "points",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          inserted: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "inserted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.automation.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "clipRef",
          "parameterRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          available: {
            type: "boolean"
          },
          exists: {
            type: "boolean"
          },
          points: {
            type: "array",
            items: {
              type: "object",
              properties: {
                time: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                value: {
                  type: "number",
                  minimum: -1e6,
                  maximum: 1e6
                }
              },
              required: [
                "time",
                "value"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "available",
          "exists",
          "points"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.clip.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e5
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "trackRef",
          "position",
          "length",
          "name",
          "expectedTrackIdentity",
          "expectedCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "start",
          "length",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.clip.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          explicitDeletion: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.clip.move",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedContentFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          keepSource: {
            type: "boolean"
          },
          targetTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTargetTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "position",
          "expectedObjectIdentity",
          "expectedAuthorityRevision",
          "expectedContentFingerprint"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          cleared: {
            type: "array",
            maxItems: 4096,
            items: {
              type: "object",
              properties: {
                name: {
                  type: "string",
                  maxLength: 256
                },
                start: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                end: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                }
              },
              required: [
                "name",
                "start",
                "end"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "start",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "arrangement.midi-clip.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          takeLaneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 1e-3,
            maximum: 1e9
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          notes: {
            type: "array",
            items: {
              type: "object",
              properties: {
                pitch: {
                  type: "integer",
                  minimum: 0,
                  maximum: 127
                },
                start: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                duration: {
                  type: "number",
                  minimum: 1e-3,
                  maximum: 1e9
                },
                velocity: {
                  type: "number",
                  minimum: 1,
                  maximum: 127
                },
                mute: {
                  type: "boolean"
                },
                probability: {
                  type: "number",
                  minimum: 0,
                  maximum: 1
                },
                velocityDeviation: {
                  type: "number",
                  minimum: -127,
                  maximum: 127
                },
                releaseVelocity: {
                  type: "number",
                  minimum: 0,
                  maximum: 127
                }
              },
              required: [
                "pitch",
                "start",
                "duration"
              ],
              additionalProperties: false
            }
          },
          expectedName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          looping: {
            type: "boolean"
          }
        },
        additionalProperties: false,
        required: [
          "trackRef",
          "start",
          "length",
          "notes"
        ]
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          end: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          notes: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "ref",
          "trackRef",
          "name",
          "start",
          "end",
          "notes"
        ],
        additionalProperties: false
      }
    },
    {
      id: "audio.capture.cleanup",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          captureId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          token: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          expectedClipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "captureId",
          "token",
          "expectedClipRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          cleaned: {
            type: "boolean",
            const: true
          },
          filePath: {
            type: [
              "string",
              "null"
            ],
            maxLength: 4096
          }
        },
        required: [
          "cleaned",
          "filePath"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "audio.capture.emergency-stop",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          captureId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          sourceSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          destinationSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "captureId",
          "sourceSlotRef",
          "destinationSlotRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          stopped: {
            type: "boolean",
            const: true
          },
          state: {
            type: "string",
            enum: [
              "stopped",
              "captured",
              "cleaned",
              "failed"
            ]
          }
        },
        required: [
          "stopped",
          "state"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "audio.capture.inspect",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sourceSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          destinationSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "setName",
          "sourceSlotRef",
          "destinationSlotRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          supported: {
            type: "boolean",
            const: true
          },
          fence: {
            type: "string",
            pattern: "^[a-f0-9]{64}$"
          },
          sourceSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          destinationSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          destinationTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "supported",
          "fence",
          "sourceSlotRef",
          "destinationSlotRef",
          "destinationTrackRef"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "audio.capture.start",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          captureId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          setName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sourceSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          destinationSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          fence: {
            type: "string",
            pattern: "^[a-f0-9]{64}$"
          },
          maxDurationMs: {
            type: "integer",
            minimum: 1e3,
            maximum: 1e4
          },
          outputSafety: {
            type: "object",
            properties: {
              safe: {
                type: "boolean",
                const: true
              },
              provenance: {
                type: "string",
                minLength: 1,
                maxLength: 512
              },
              observedAt: {
                type: "string",
                minLength: 1,
                maxLength: 64
              },
              scope: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "safe",
              "provenance"
            ],
            additionalProperties: false
          }
        },
        required: [
          "captureId",
          "setName",
          "sourceSlotRef",
          "destinationSlotRef",
          "fence",
          "maxDurationMs",
          "outputSafety"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          captureId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          token: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          expiresAt: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          state: {
            type: "string",
            const: "active"
          }
        },
        required: [
          "captureId",
          "token",
          "expiresAt",
          "state"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "audio.capture.status",
      method: "invoke",
      request: {
        type: "object",
        properties: {},
        required: [],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          active: {
            type: "boolean"
          },
          state: {
            type: "string",
            enum: [
              "idle",
              "active",
              "stopped",
              "captured",
              "cleaned",
              "failed"
            ]
          }
        },
        required: [
          "active",
          "state"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "audio.capture.stop",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          captureId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          token: {
            type: "string",
            minLength: 16,
            maxLength: 128
          }
        },
        required: [
          "captureId",
          "token"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          stopped: {
            type: "boolean",
            const: true
          },
          state: {
            type: "string",
            enum: [
              "stopped",
              "captured",
              "failed"
            ]
          }
        },
        required: [
          "stopped",
          "state"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "audio.clip.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          gain: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          pitchCoarse: {
            type: "number",
            minimum: -48,
            maximum: 48
          },
          pitchFine: {
            type: "number",
            minimum: -50,
            maximum: 50
          },
          loopStart: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          loopEnd: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          warpMode: {
            type: "integer",
            minimum: 0,
            maximum: 16
          },
          warping: {
            type: "boolean"
          },
          fadeInLength: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          fadeOutLength: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedAuthorityRevision",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "audio.comp.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "clipRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          segments: {
            type: "array",
            items: {
              type: "object",
              properties: {
                laneRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                from: {
                  type: "number",
                  minimum: 0
                },
                to: {
                  type: "number",
                  minimum: 0
                }
              },
              required: [
                "laneRef",
                "from",
                "to"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "segments"
        ],
        additionalProperties: false
      }
    },
    {
      id: "audio.take-lane.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "trackRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          lanes: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                name: {
                  type: "string",
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "name"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "lanes"
        ],
        additionalProperties: false
      }
    },
    {
      id: "audio.warp-marker.add",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          beatTime: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          expectedClipAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedMarkerCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "beatTime",
          "expectedClipAuthorityDigest",
          "expectedMarkerCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "audio.warp-marker.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          beatTime: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          expectedClipAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedMarkerCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "beatTime",
          "expectedClipAuthorityDigest",
          "expectedMarkerCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "integer",
            minimum: 0
          },
          revision: {
            type: "integer",
            minimum: 1
          }
        },
        required: [
          "deleted",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "audio.warp-marker.move",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          beatTime: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          distance: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          expectedClipAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedMarkerCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "beatTime",
          "distance",
          "expectedClipAuthorityDigest",
          "expectedMarkerCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "audio.warp-marker.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          markers: {
            type: "array",
            items: {
              type: "object",
              properties: {
                beatTime: {
                  type: "number",
                  minimum: -1e6,
                  maximum: 1e9
                },
                sampleTime: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                }
              },
              required: [
                "beatTime",
                "sampleTime"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "revision",
          "markers"
        ],
        additionalProperties: false
      }
    },
    {
      id: "authority.digest",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          operation: {
            type: "string",
            minLength: 3,
            maxLength: 128,
            pattern: "^[a-z0-9]+(?:[.-][a-z0-9]+)+$"
          },
          args: {
            type: "object",
            additionalProperties: true,
            maxProperties: 64
          }
        },
        additionalProperties: false,
        required: [
          "operation",
          "args"
        ]
      },
      result: {
        type: "object",
        properties: {
          stateDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          epoch: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "stateDigest",
          "epoch"
        ],
        additionalProperties: false
      }
    },
    {
      id: "authority.preflight",
      method: "preflight",
      request: {
        type: "object",
        properties: {
          operation: {
            type: "string",
            minLength: 3,
            maxLength: 128,
            pattern: "^[a-z0-9]+(?:[.-][a-z0-9]+)+$"
          },
          argsDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          transactionId: {
            type: "string",
            minLength: 8,
            maxLength: 128
          }
        },
        required: [
          "operation",
          "argsDigest",
          "transactionId"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          preflightToken: {
            type: "string",
            minLength: 24,
            maxLength: 128
          },
          confirmation: {
            type: "string",
            minLength: 24,
            maxLength: 128
          },
          operation: {
            type: "string",
            minLength: 3,
            maxLength: 128
          },
          argsDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64
          },
          stateDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64
          },
          impact: {
            type: "string",
            const: "mutates-live"
          },
          expiresAt: {
            type: "integer",
            minimum: 0
          }
        },
        required: [
          "preflightToken",
          "confirmation",
          "operation",
          "argsDigest",
          "stateDigest",
          "impact",
          "expiresAt"
        ],
        additionalProperties: false
      }
    },
    {
      id: "authority.prepare",
      method: "prepare",
      request: {
        type: "object",
        properties: {
          operation: {
            type: "string",
            minLength: 3,
            maxLength: 128,
            pattern: "^[a-z0-9]+(?:[.-][a-z0-9]+)+$"
          },
          argsDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          preflightToken: {
            type: "string",
            minLength: 24,
            maxLength: 128
          },
          confirmation: {
            type: "string",
            minLength: 24,
            maxLength: 128
          },
          idempotencyKey: {
            type: "string",
            minLength: 8,
            maxLength: 128
          },
          transactionId: {
            type: "string",
            minLength: 8,
            maxLength: 128
          }
        },
        required: [
          "operation",
          "argsDigest",
          "preflightToken",
          "confirmation",
          "idempotencyKey",
          "transactionId"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          authorityToken: {
            type: "string",
            minLength: 24,
            maxLength: 128
          },
          operation: {
            type: "string",
            minLength: 3,
            maxLength: 128
          },
          argsDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64
          },
          stateDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64
          },
          expiresAt: {
            type: "integer",
            minimum: 0
          }
        },
        required: [
          "authorityToken",
          "operation",
          "argsDigest",
          "stateDigest",
          "expiresAt"
        ],
        additionalProperties: false
      }
    },
    {
      id: "authority.retire",
      method: "retire",
      request: {
        type: "object",
        properties: {
          transactionId: {
            type: "string",
            minLength: 8,
            maxLength: 128
          },
          terminal: {
            type: "boolean"
          }
        },
        required: [
          "transactionId"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          retired: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "retired"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.envelope.clear",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedEnvelopesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "clipRef",
          "expectedAuthorityDigest",
          "expectedEnvelopesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          cleared: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          },
          envelopesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "cleared",
          "envelopesRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.envelope.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          created: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "created"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.envelope.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.envelope.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "clipRef",
          "parameterRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          available: {
            type: "boolean"
          },
          exists: {
            type: "boolean"
          },
          points: {
            type: "array",
            items: {
              type: "object",
              properties: {
                time: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                value: {
                  type: "number",
                  minimum: -1e6,
                  maximum: 1e6
                }
              },
              required: [
                "time",
                "value"
              ],
              additionalProperties: false
            }
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "available",
          "exists",
          "points",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.point.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          from: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          to: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "from",
          "to",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.point.insert",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          points: {
            type: "array",
            items: {
              type: "object",
              properties: {
                time: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                value: {
                  type: "number",
                  minimum: -1e6,
                  maximum: 1e6
                }
              },
              required: [
                "time",
                "value"
              ],
              additionalProperties: false
            }
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "points",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          inserted: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "inserted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.step.insert",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 1e-3,
            maximum: 1e9
          },
          value: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          expectedAuthorityDigest: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          expectedEnvelopeRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "start",
          "length",
          "value",
          "expectedAuthorityDigest",
          "expectedEnvelopeRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          inserted: {
            type: "integer",
            minimum: 0,
            maximum: 1
          }
        },
        required: [
          "inserted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "automation.value-at",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          time: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          }
        },
        required: [
          "clipRef",
          "parameterRef",
          "time"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          value: {
            type: [
              "number",
              "null"
            ],
            minimum: -1e6,
            maximum: 1e6
          }
        },
        required: [
          "value"
        ],
        additionalProperties: false
      }
    },
    {
      id: "browser.inspect",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          itemId: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "itemId"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          id: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          category: {
            type: "string",
            maxLength: 64
          },
          path: {
            type: "string",
            maxLength: 512
          },
          isDevice: {
            type: "boolean"
          }
        },
        required: [
          "id",
          "objectIdentity",
          "name",
          "category",
          "path",
          "isDevice"
        ],
        additionalProperties: false
      }
    },
    {
      id: "browser.load",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          itemId: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          chainRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedChainIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            maxLength: 256
          },
          expectedItemIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "itemId",
          "trackRef",
          "expectedName",
          "expectedItemIdentity",
          "expectedTrackIdentity",
          "expectedSiblings"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          loaded: {
            type: "boolean",
            const: true
          },
          deviceRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          deviceObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "loaded",
          "deviceRef",
          "deviceObjectIdentity",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "browser.preview.start",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          itemId: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedItemIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "itemId",
          "expectedName",
          "expectedItemIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          previewId: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          started: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "previewId",
          "started"
        ],
        additionalProperties: false
      }
    },
    {
      id: "browser.preview.stop",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          previewId: {
            type: "string",
            minLength: 32,
            maxLength: 256
          }
        },
        required: [
          "previewId"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          stopped: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "stopped"
        ],
        additionalProperties: false
      }
    },
    {
      id: "browser.roots",
      method: "invoke",
      request: {
        type: "object",
        properties: {},
        required: [],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          roots: {
            type: "array",
            items: {
              type: "object",
              properties: {
                name: {
                  type: "string",
                  minLength: 1,
                  maxLength: 128
                },
                binding: {
                  type: "string",
                  enum: [
                    "unofficial-internal"
                  ]
                },
                searchable: {
                  type: "boolean"
                }
              },
              required: [
                "name",
                "binding",
                "searchable"
              ],
              additionalProperties: false
            }
          },
          previewAvailable: {
            type: "boolean"
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          bindingEvidence: {
            type: "string",
            maxLength: 256
          }
        },
        required: [
          "roots",
          "previewAvailable",
          "bindingEvidence",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "browser.search",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          category: {
            type: "string",
            enum: [
              "instruments",
              "audio_effects",
              "midi_effects",
              "modulators",
              "drums",
              "plugins",
              "packs",
              "max_for_live",
              "clips"
            ]
          },
          query: {
            type: "string",
            maxLength: 256
          },
          limit: {
            type: "integer",
            minimum: 1,
            maximum: 1e4
          }
        },
        required: [],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          items: {
            type: "array",
            items: {
              type: "object",
              properties: {
                id: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                name: {
                  type: "string",
                  maxLength: 256
                },
                category: {
                  type: "string",
                  maxLength: 64
                },
                path: {
                  type: "string",
                  maxLength: 512
                },
                isDevice: {
                  type: "boolean"
                }
              },
              required: [
                "id",
                "objectIdentity",
                "name",
                "category",
                "path",
                "isDevice"
              ],
              additionalProperties: false
            },
            maxItems: 1e4
          }
        },
        required: [
          "items"
        ],
        additionalProperties: false
      }
    },
    {
      id: "chain-mixer.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          volume: {
            type: "number",
            minimum: 0,
            maximum: 1
          },
          pan: {
            type: "number",
            minimum: -1,
            maximum: 1
          },
          sends: {
            type: "array",
            items: {
              type: "number",
              minimum: 0,
              maximum: 1
            }
          },
          chainActivator: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedMixerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedMixerIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "chain.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          colorIndex: {
            type: "integer",
            minimum: 0,
            maximum: 69
          },
          autoColor: {
            type: "boolean"
          },
          mute: {
            type: "boolean"
          },
          solo: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.action",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          action: {
            type: "string",
            enum: [
              "crop",
              "duplicate-loop",
              "duplicate-region",
              "scrub-start",
              "scrub-stop",
              "move-playing-position"
            ]
          },
          regionStart: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          regionEnd: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          destination: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          offset: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedContentFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "action",
          "expectedObjectIdentity",
          "expectedAuthorityRevision",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.clear-range",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          fromBeat: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          toBeat: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          expectedName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          takeLaneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        additionalProperties: false,
        required: [
          "trackRef",
          "fromBeat",
          "toBeat"
        ]
      },
      result: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          clipsBefore: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          },
          clipsAfter: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          },
          removed: {
            type: "array",
            items: {
              type: "object",
              properties: {
                name: {
                  type: "string",
                  maxLength: 256
                },
                start: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                end: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e9
                },
                isAudio: {
                  type: "boolean"
                }
              },
              required: [
                "name",
                "start",
                "end",
                "isAudio"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "trackRef",
          "clipsBefore",
          "clipsAfter",
          "removed"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          kind: {
            type: "string",
            enum: [
              "midi",
              "audio"
            ]
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          length: {
            type: "number",
            minimum: 1e-3,
            maximum: 1e5
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "trackRef",
          "sceneIndex",
          "kind",
          "name",
          "length",
          "expectedTrackIdentity",
          "expectedSlotRef",
          "expectedSlotIdentity",
          "expectedSceneRef",
          "expectedSceneIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "length",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          explicitDeletion: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedTrackRef",
          "expectedTrackIdentity",
          "expectedSlotRef",
          "expectedSlotIdentity",
          "expectedSceneRef",
          "expectedSceneIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.duplicate",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          targetTrackRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          targetSceneIndex: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 1e5
          },
          arrangementPosition: {
            type: [
              "number",
              "null"
            ],
            minimum: 0,
            maximum: 1e9
          },
          expectedContentFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTargetTrackIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSlotRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSlotIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSceneRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSceneIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetCollectionRevision: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "targetTrackRef",
          "targetSceneIndex",
          "arrangementPosition",
          "expectedContentFingerprint",
          "expectedObjectIdentity",
          "expectedTrackRef",
          "expectedTrackIdentity",
          "expectedSlotRef",
          "expectedSlotIdentity",
          "expectedSceneRef",
          "expectedSceneIdentity",
          "expectedTargetTrackIdentity",
          "expectedTargetSlotRef",
          "expectedTargetSlotIdentity",
          "expectedTargetSceneRef",
          "expectedTargetSceneIdentity",
          "expectedTargetCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.follow-actions.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          followActionEnabled: {
            type: "boolean"
          },
          followActionLinked: {
            type: "boolean"
          },
          followActionA: {
            type: "integer",
            minimum: 0,
            maximum: 9
          },
          followActionB: {
            type: "integer",
            minimum: 0,
            maximum: 9
          },
          followActionChanceA: {
            type: "integer",
            minimum: 0,
            maximum: 100
          },
          followActionChanceB: {
            type: "integer",
            minimum: 0,
            maximum: 100
          },
          followActionLoopCount: {
            type: "integer",
            minimum: 1,
            maximum: 1073741823
          },
          followActionTime: {
            type: "number",
            minimum: 0.25,
            maximum: 1e9
          },
          followActionJumpA: {
            type: "integer",
            minimum: 1,
            maximum: 8388608
          },
          followActionJumpB: {
            type: "integer",
            minimum: 1,
            maximum: 8388608
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedAuthorityRevision",
          "expectedStateRevision",
          "followActionEnabled",
          "followActionLinked",
          "followActionA",
          "followActionB",
          "followActionChanceA",
          "followActionChanceB",
          "followActionLoopCount",
          "followActionTime",
          "followActionJumpA",
          "followActionJumpB"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.move",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          targetTrackRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          targetSceneIndex: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 1e5
          },
          arrangementPosition: {
            type: [
              "number",
              "null"
            ],
            minimum: 0,
            maximum: 1e9
          },
          expectedContentFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTargetTrackIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSlotRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSlotIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSceneRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetSceneIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetCollectionRevision: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "targetTrackRef",
          "targetSceneIndex",
          "arrangementPosition",
          "expectedContentFingerprint",
          "expectedObjectIdentity",
          "expectedTrackRef",
          "expectedTrackIdentity",
          "expectedSlotRef",
          "expectedSlotIdentity",
          "expectedSceneRef",
          "expectedSceneIdentity",
          "expectedTargetTrackIdentity",
          "expectedTargetSlotRef",
          "expectedTargetSlotIdentity",
          "expectedTargetSceneRef",
          "expectedTargetSceneIdentity",
          "expectedTargetCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.rename",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "name",
          "expectedName",
          "expectedObjectIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          renamed: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "renamed",
          "name"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          muted: {
            type: "boolean"
          },
          colorIndex: {
            type: "integer",
            minimum: 0,
            maximum: 69
          },
          looping: {
            type: "boolean"
          },
          loopStart: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          loopEnd: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          launchMode: {
            type: "integer",
            minimum: 0,
            maximum: 3
          },
          launchQuantization: {
            type: "integer",
            minimum: 0,
            maximum: 14
          },
          legato: {
            type: "boolean"
          },
          ramMode: {
            type: "boolean"
          },
          velocityAmount: {
            type: "number",
            minimum: 0,
            maximum: 1
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          grooveRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedAuthorityRevision",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.time-convert",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          from: {
            type: "string",
            enum: [
              "beats",
              "samples",
              "seconds"
            ]
          },
          value: {
            type: "number",
            minimum: -1e9,
            maximum: 1e12
          }
        },
        required: [
          "ref",
          "from",
          "value"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          beats: {
            type: [
              "number",
              "null"
            ]
          },
          samples: {
            type: [
              "number",
              "null"
            ]
          },
          seconds: {
            type: [
              "number",
              "null"
            ]
          }
        },
        required: [
          "beats",
          "samples",
          "seconds"
        ],
        additionalProperties: false
      }
    },
    {
      id: "clip.view.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          gridQuantization: {
            type: "integer",
            minimum: 0,
            maximum: 16
          },
          showEnvelope: {
            type: "boolean"
          },
          showLoop: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          gridIsTriplet: {
            type: "boolean"
          },
          envelopeParameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "compressor.sidechain.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          routingType: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "data.get",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          key: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "key"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          key: {
            type: "string",
            maxLength: 256
          },
          value: {
            type: [
              "string",
              "null"
            ],
            maxLength: 1048576
          }
        },
        required: [
          "ref",
          "key",
          "value"
        ],
        additionalProperties: false
      }
    },
    {
      id: "data.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          key: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          value: {
            type: [
              "string",
              "null"
            ],
            maxLength: 1048576
          },
          expectedValue: {
            type: [
              "string",
              "null"
            ],
            maxLength: 1048576
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          entries: {
            type: "array",
            minItems: 1,
            maxItems: 1024,
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                key: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                value: {
                  type: [
                    "string",
                    "null"
                  ],
                  maxLength: 1048576
                },
                expectedValue: {
                  type: [
                    "string",
                    "null"
                  ],
                  maxLength: 1048576
                },
                expectedObjectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "key",
                "value",
                "expectedObjectIdentity"
              ],
              additionalProperties: false
            }
          }
        },
        required: [],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          key: {
            type: "string",
            maxLength: 256
          },
          value: {
            type: [
              "string",
              "null"
            ],
            maxLength: 1048576
          },
          prior: {
            type: [
              "string",
              "null"
            ],
            maxLength: 1048576
          },
          entries: {
            type: "array",
            minItems: 1,
            maxItems: 1024,
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                key: {
                  type: "string",
                  maxLength: 256
                },
                value: {
                  type: [
                    "string",
                    "null"
                  ],
                  maxLength: 1048576
                },
                prior: {
                  type: [
                    "string",
                    "null"
                  ],
                  maxLength: 1048576
                }
              },
              required: [
                "ref",
                "key",
                "value",
                "prior"
              ],
              additionalProperties: false
            }
          }
        },
        required: [],
        additionalProperties: false
      }
    },
    {
      id: "dev.lom-audit",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          maxDepth: {
            type: "integer",
            minimum: 1,
            maximum: 16
          }
        },
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          liveVersion: {
            type: [
              "string",
              "null"
            ],
            maxLength: 128
          },
          classes: {
            type: "array",
            items: {
              type: "object",
              properties: {
                name: {
                  type: "string",
                  maxLength: 256
                },
                path: {
                  type: "string",
                  maxLength: 1024
                },
                doc: {
                  type: [
                    "string",
                    "null"
                  ],
                  maxLength: 16384
                },
                members: {
                  type: "array",
                  items: {
                    type: "object",
                    properties: {
                      name: {
                        type: "string",
                        maxLength: 256
                      },
                      kind: {
                        type: "string",
                        maxLength: 64
                      },
                      doc: {
                        type: [
                          "string",
                          "null"
                        ],
                        maxLength: 16384
                      }
                    },
                    required: [
                      "name",
                      "kind"
                    ],
                    additionalProperties: false
                  }
                }
              },
              required: [
                "name",
                "path",
                "members"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "liveVersion",
          "classes"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device-io.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          routingType: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          routingChannel: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.action",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          action: {
            type: "string",
            enum: [
              "cc-control-resend",
              "simpler-warp-as",
              "simpler-warp-double",
              "simpler-warp-half"
            ]
          },
          beats: {
            type: "number",
            minimum: 1e-3,
            maximum: 1e6
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "action",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "done",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.bank.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          bank: {
            type: "integer",
            minimum: 0,
            maximum: 32
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          scriptIndex: {
            type: "integer",
            minimum: 0,
            maximum: 16
          }
        },
        required: [
          "ref",
          "bank",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.banks.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          banks: {
            type: "array",
            items: {
              type: "object",
              properties: {
                name: {
                  type: "string",
                  maxLength: 256
                },
                parameters: {
                  type: "array",
                  items: {
                    type: "integer",
                    minimum: -1,
                    maximum: 1e7
                  }
                }
              },
              required: [
                "name",
                "parameters"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "banks"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.comparison.save-to-slot",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "done"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          explicitDeletion: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedOwnerRef",
          "expectedOwnerIdentity",
          "expectedSiblings",
          "expectedTrackRef",
          "expectedTrackIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.duplicate",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          }
        },
        additionalProperties: false,
        required: [
          "ref"
        ]
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "name",
          "index"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.enable",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          enabled: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "enabled",
          "expectedObjectIdentity",
          "expectedOwnerRef",
          "expectedOwnerIdentity",
          "expectedSiblings",
          "expectedTrackRef",
          "expectedTrackIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          enabled: {
            type: "boolean"
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "enabled",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.insert",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          deviceName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          index: {
            type: [
              "integer",
              "null"
            ],
            minimum: -1,
            maximum: 1e5
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          },
          chainRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          samplePath: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "trackRef",
          "deviceName",
          "expectedTrackIdentity",
          "expectedSiblings"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          },
          samplePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "index",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.move",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          targetTrackRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          targetChainRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedTargetIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "index",
          "expectedObjectIdentity",
          "expectedOwnerRef",
          "expectedOwnerIdentity",
          "expectedSiblings",
          "expectedTrackRef",
          "expectedTrackIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "index"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.parameter.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          value: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          expectedRevision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          },
          gesture: {
            type: "boolean"
          }
        },
        required: [
          "ref",
          "value",
          "expectedRevision",
          "expectedObjectIdentity",
          "expectedOwnerRef",
          "expectedOwnerIdentity",
          "expectedTrackRef",
          "expectedTrackIdentity",
          "expectedSiblings"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean"
          },
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          property: {
            type: "string",
            const: "value"
          },
          value: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "ref",
          "property",
          "value",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.parameters.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          expectedOwnerRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedOwnerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSiblings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "objectIdentity"
              ],
              additionalProperties: false
            }
          },
          parameters: {
            type: "array",
            minItems: 1,
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                value: {
                  type: "number",
                  minimum: -1e6,
                  maximum: 1e6
                },
                expectedRevision: {
                  type: "integer",
                  minimum: 1,
                  maximum: 9007199254740991
                },
                expectedObjectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "ref",
                "value",
                "expectedRevision",
                "expectedObjectIdentity"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "expectedOwnerRef",
          "expectedOwnerIdentity",
          "expectedTrackRef",
          "expectedTrackIdentity",
          "expectedSiblings",
          "parameters"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          parameters: {
            type: "array",
            minItems: 1,
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                value: {
                  type: "number",
                  minimum: -1e6,
                  maximum: 1e6
                },
                revision: {
                  type: "integer",
                  minimum: 1,
                  maximum: 9007199254740991
                }
              },
              required: [
                "ref",
                "value",
                "revision"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "parameters"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.property.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          property: {
            type: "string",
            enum: [
              "roar.routing_mode_index",
              "roar.env_listen",
              "shifter.pitch_mode_index",
              "spectral_resonator.frequency_dial_mode",
              "spectral_resonator.midi_gate",
              "spectral_resonator.mod_mode",
              "spectral_resonator.mono_poly",
              "spectral_resonator.pitch_mode",
              "hybrid_reverb.ir_time_shaping_on",
              "cc_control.custom_bool_target",
              "simpler.playback_mode",
              "simpler.retrigger",
              "simpler.slicing_playback_mode",
              "simpler.voices",
              "simpler.pad_slicing",
              "simpler.note_pitch_bend_range",
              "cc_control.custom_float_target_0",
              "cc_control.custom_float_target_1",
              "cc_control.custom_float_target_2",
              "cc_control.custom_float_target_3",
              "cc_control.custom_float_target_4",
              "cc_control.custom_float_target_5",
              "cc_control.custom_float_target_6",
              "cc_control.custom_float_target_7",
              "cc_control.custom_float_target_8",
              "cc_control.custom_float_target_9",
              "cc_control.custom_float_target_10",
              "cc_control.custom_float_target_11"
            ]
          },
          value: {
            type: [
              "integer",
              "number",
              "boolean"
            ]
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "property",
          "value",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          value: {
            type: [
              "integer",
              "number",
              "boolean"
            ]
          }
        },
        required: [
          "changed",
          "revision",
          "value"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.rename",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "name",
          "expectedName",
          "expectedObjectIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          renamed: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "renamed",
          "name"
        ],
        additionalProperties: false
      }
    },
    {
      id: "device.view.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          collapsed: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "collapsed",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "discover",
      method: "discover",
      request: {
        type: "object",
        properties: {
          kind: {
            type: "string",
            minLength: 1,
            maxLength: 64
          },
          parent: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          filters: {
            type: "object",
            maxProperties: 16,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean",
                "null"
              ],
              maxLength: 256,
              minimum: -9007199254740991,
              maximum: 9007199254740991
            }
          },
          requestedFields: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 64
            },
            maxItems: 256
          },
          traversalBudget: {
            type: "integer",
            minimum: 1,
            maximum: 1e7
          },
          limit: {
            type: "integer",
            minimum: 1,
            maximum: 1e5
          },
          cursor: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "kind"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          epoch: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          items: {
            type: "array",
            items: {
              type: "object",
              maxProperties: 64,
              additionalProperties: true
            }
          },
          truncated: {
            type: "boolean"
          },
          revision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          kind: {
            type: "string",
            minLength: 1,
            maxLength: 64
          },
          nextCursor: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "epoch",
          "items",
          "truncated",
          "revision",
          "kind"
        ],
        additionalProperties: false
      }
    },
    {
      id: "drift.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          pitchBendRange: {
            type: "integer",
            minimum: 1,
            maximum: 96
          },
          voiceCount: {
            type: "integer",
            minimum: 0,
            maximum: 64
          },
          voiceMode: {
            type: "integer",
            minimum: 0,
            maximum: 8
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          modFilterSource1: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modFilterSource2: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modLfoSource: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modPitchSource1: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modPitchSource2: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modShapeSource: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modSource1: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modSource2: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modSource3: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modTarget1: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modTarget2: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          modTarget3: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "drum-cell.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          gain: {
            type: "number",
            minimum: -70,
            maximum: 24
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "gain",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "drum-pad.delete-all-chains",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "drum-pad.load-sample",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          samplePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          name: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          instrument: {
            type: "string",
            enum: [
              "Simpler",
              "Drum Sampler"
            ]
          },
          presetItemId: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          chainIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          deviceIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          samplePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          route: {
            type: "string",
            enum: [
              "hotswap",
              "chain",
              "preset"
            ]
          },
          tried: {
            type: "array",
            maxItems: 8,
            items: {
              type: "string",
              maxLength: 256
            }
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "chainIdentity",
          "deviceIdentity",
          "samplePath",
          "route",
          "tried"
        ],
        additionalProperties: false
      }
    },
    {
      id: "drum-pad.load-samples",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          pads: {
            type: "array",
            minItems: 1,
            maxItems: 128,
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                expectedObjectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                samplePath: {
                  type: "string",
                  minLength: 1,
                  maxLength: 1024
                },
                name: {
                  type: [
                    "string",
                    "null"
                  ],
                  minLength: 1,
                  maxLength: 256
                },
                instrument: {
                  type: "string",
                  enum: [
                    "Simpler",
                    "Drum Sampler"
                  ]
                },
                presetItemId: {
                  type: "string",
                  minLength: 1,
                  maxLength: 1024
                }
              },
              required: [
                "ref",
                "expectedObjectIdentity"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "pads"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          pads: {
            type: "array",
            minItems: 1,
            maxItems: 128,
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                chainIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                deviceIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                samplePath: {
                  type: "string",
                  minLength: 1,
                  maxLength: 1024
                },
                route: {
                  type: "string",
                  enum: [
                    "hotswap",
                    "chain",
                    "preset"
                  ]
                },
                tried: {
                  type: "array",
                  maxItems: 8,
                  items: {
                    type: "string",
                    maxLength: 256
                  }
                }
              },
              required: [
                "ref",
                "objectIdentity",
                "chainIdentity",
                "deviceIdentity",
                "samplePath",
                "route",
                "tried"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "pads"
        ],
        additionalProperties: false
      }
    },
    {
      id: "drum-pad.sample-chain",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          rackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          note: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          samplePath: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        additionalProperties: false,
        required: [
          "rackRef",
          "note",
          "samplePath"
        ]
      },
      result: {
        type: "object",
        properties: {
          chainRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          deviceRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          note: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          samplePath: {
            type: "string",
            maxLength: 4096
          }
        },
        required: [
          "chainRef",
          "deviceRef",
          "note",
          "samplePath"
        ],
        additionalProperties: false
      }
    },
    {
      id: "drum-pad.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          note: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          solo: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "eq8.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          editMode: {
            type: "integer",
            minimum: 0,
            maximum: 4
          },
          globalMode: {
            type: "integer",
            minimum: 0,
            maximum: 4
          },
          selectedBand: {
            type: "integer",
            minimum: 0,
            maximum: 8
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          oversample: {
            type: "boolean"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "fire-button.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          pressed: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          outputSafety: {
            type: "object",
            properties: {
              safe: {
                type: "boolean",
                const: true
              },
              provenance: {
                type: "string",
                minLength: 1,
                maxLength: 512
              },
              observedAt: {
                type: "string",
                minLength: 1,
                maxLength: 64
              },
              scope: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "safe",
              "provenance"
            ],
            additionalProperties: false
          }
        },
        required: [
          "ref",
          "pressed",
          "expectedObjectIdentity",
          "outputSafety"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          pressed: {
            type: "boolean"
          }
        },
        required: [
          "ref",
          "pressed"
        ],
        additionalProperties: false
      }
    },
    {
      id: "get",
      method: "get",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref"
        ],
        additionalProperties: true,
        maxProperties: 64
      }
    },
    {
      id: "groove.edit",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          base: {
            type: "integer",
            minimum: 0,
            maximum: 16
          },
          quantizationAmount: {
            type: "number",
            minimum: 0,
            maximum: 100
          },
          randomAmount: {
            type: "number",
            minimum: 0,
            maximum: 100
          },
          timingAmount: {
            type: "number",
            minimum: 0,
            maximum: 100
          },
          velocityAmount: {
            type: "number",
            minimum: -100,
            maximum: 100
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "groove.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "setRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          grooveAmount: {
            type: [
              "number",
              "null"
            ],
            minimum: 0,
            maximum: 2
          },
          grooves: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                name: {
                  type: "string",
                  maxLength: 256
                },
                base: {
                  type: [
                    "integer",
                    "null"
                  ],
                  minimum: 0,
                  maximum: 16
                },
                quantizationAmount: {
                  type: [
                    "number",
                    "null"
                  ],
                  minimum: 0,
                  maximum: 100
                },
                randomAmount: {
                  type: [
                    "number",
                    "null"
                  ],
                  minimum: 0,
                  maximum: 100
                },
                timingAmount: {
                  type: [
                    "number",
                    "null"
                  ],
                  minimum: 0,
                  maximum: 100
                },
                velocityAmount: {
                  type: [
                    "number",
                    "null"
                  ],
                  minimum: -100,
                  maximum: 100
                }
              },
              required: [
                "ref",
                "objectIdentity",
                "name",
                "base",
                "quantizationAmount",
                "randomAmount",
                "timingAmount",
                "velocityAmount"
              ],
              additionalProperties: false
            }
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "grooveAmount",
          "grooves",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "groove.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          grooveAmount: {
            type: "number",
            minimum: 0,
            maximum: 1.3125
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "setRef",
          "grooveAmount",
          "expectedObjectIdentity",
          "expectedRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "hybrid-reverb.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          irCategory: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          irFile: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          attack: {
            type: "number",
            minimum: 0,
            maximum: 1e4
          },
          decay: {
            type: "number",
            minimum: 0,
            maximum: 1e5
          },
          size: {
            type: "number",
            minimum: 0,
            maximum: 1e4
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "locator.add",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          name: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e5
          },
          expectedCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "name",
          "position",
          "expectedCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "position",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "locator.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          explicitDeletion: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "locator.jump",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          direction: {
            type: "string",
            enum: [
              "next",
              "previous"
            ]
          }
        },
        required: [
          "direction"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          direction: {
            type: "string",
            enum: [
              "next",
              "previous"
            ]
          },
          before: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          }
        },
        required: [
          "direction",
          "before",
          "position"
        ],
        additionalProperties: false
      }
    },
    {
      id: "locator.jump-to",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          }
        },
        required: [
          "position"
        ],
        additionalProperties: false
      }
    },
    {
      id: "locator.rename",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "name",
          "expectedName",
          "expectedObjectIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          renamed: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "renamed",
          "name"
        ],
        additionalProperties: false
      }
    },
    {
      id: "looper.action",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          action: {
            type: "string",
            enum: [
              "record",
              "overdub",
              "play",
              "stop",
              "clear",
              "undo",
              "double-speed",
              "half-speed",
              "export",
              "double-length",
              "half-length"
            ]
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          slotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "action",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "done",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "looper.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          overdubAfterRecord: {
            type: "boolean"
          },
          recordLengthIndex: {
            type: "integer",
            minimum: 0,
            maximum: 8
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "meld.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          engine: {
            type: "integer",
            minimum: 0,
            maximum: 4
          },
          unison: {
            type: "integer",
            minimum: 0,
            maximum: 3
          },
          monoPoly: {
            type: "boolean"
          },
          polyphony: {
            type: "integer",
            minimum: 0,
            maximum: 6
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "mixer.extended.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          trackActivator: {
            type: "boolean"
          },
          crossfader: {
            type: "number",
            minimum: -1,
            maximum: 1
          },
          crossfadeAssign: {
            type: "integer",
            minimum: 0,
            maximum: 2
          },
          panningMode: {
            type: "integer",
            minimum: 0,
            maximum: 8
          },
          panningLeft: {
            type: "number",
            minimum: -1,
            maximum: 1
          },
          panningRight: {
            type: "number",
            minimum: -1,
            maximum: 1
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedMixerIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedMixerIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "mixer.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          volume: {
            type: "number",
            minimum: 0,
            maximum: 1
          },
          pan: {
            type: "number",
            minimum: -1,
            maximum: 1
          },
          mute: {
            type: "boolean"
          },
          solo: {
            type: "boolean"
          },
          cueVolume: {
            type: "number",
            minimum: 0,
            maximum: 1
          },
          sends: {
            type: "array",
            items: {
              type: "number",
              minimum: 0,
              maximum: 1
            }
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedVolumeIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedPanIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedCueIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedSendIdentities: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            }
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedVolumeIdentity",
          "expectedPanIdentity",
          "expectedCueIdentity",
          "expectedSendIdentities",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.add",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          note: {
            type: "object",
            properties: {
              pitch: {
                type: "integer",
                minimum: 0,
                maximum: 127
              },
              start: {
                type: "number",
                minimum: 0,
                maximum: 1e5
              },
              duration: {
                type: "number",
                minimum: 1e-3,
                maximum: 1e5
              },
              velocity: {
                type: "integer",
                minimum: 1,
                maximum: 127
              },
              channel: {
                type: "integer",
                minimum: 1,
                maximum: 16
              },
              probability: {
                type: "number",
                minimum: 0,
                maximum: 1
              },
              velocityDeviation: {
                type: "number",
                minimum: -127,
                maximum: 127
              },
              releaseVelocity: {
                type: "number",
                minimum: 0,
                maximum: 127
              },
              mute: {
                type: "boolean"
              }
            },
            required: [
              "pitch",
              "start",
              "duration",
              "velocity",
              "channel"
            ],
            additionalProperties: false
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          },
          expectedNotesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "ref",
          "note",
          "expectedClipAuthority",
          "expectedNotesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          added: {
            type: "boolean"
          },
          noteId: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 9007199254740991
          }
        },
        required: [
          "added",
          "noteId"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.add-batch",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          notes: {
            type: "array",
            items: {
              type: "object",
              properties: {
                pitch: {
                  type: "integer",
                  minimum: 0,
                  maximum: 127
                },
                start: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e5
                },
                duration: {
                  type: "number",
                  minimum: 1e-3,
                  maximum: 1e5
                },
                velocity: {
                  type: "integer",
                  minimum: 1,
                  maximum: 127
                },
                channel: {
                  type: "integer",
                  minimum: 1,
                  maximum: 16
                },
                probability: {
                  type: "number",
                  minimum: 0,
                  maximum: 1
                },
                velocityDeviation: {
                  type: "number",
                  minimum: -127,
                  maximum: 127
                },
                releaseVelocity: {
                  type: "number",
                  minimum: 0,
                  maximum: 127
                },
                mute: {
                  type: "boolean"
                }
              },
              required: [
                "pitch",
                "start",
                "duration",
                "velocity",
                "channel"
              ],
              additionalProperties: false
            },
            minItems: 1
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          },
          expectedNotesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "ref",
          "notes",
          "expectedClipAuthority",
          "expectedNotesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          added: {
            type: "integer",
            minimum: 1,
            maximum: 1e7
          },
          noteIds: {
            type: "array",
            items: {
              type: [
                "integer",
                "null"
              ],
              minimum: 0,
              maximum: 9007199254740991
            },
            minItems: 1
          },
          notesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "added",
          "noteIds",
          "notesRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          noteIds: {
            type: "array",
            items: {
              type: "integer",
              minimum: 0,
              maximum: 9007199254740991
            }
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          },
          expectedNotesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "ref",
          "noteIds",
          "expectedClipAuthority",
          "expectedNotesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.delete-range",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          fromPitch: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          pitchSpan: {
            type: "integer",
            minimum: 1,
            maximum: 128
          },
          fromTime: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          timeSpan: {
            type: "number",
            minimum: 1e-3,
            maximum: 1e9
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          },
          expectedNotesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "ref",
          "fromPitch",
          "pitchSpan",
          "fromTime",
          "timeSpan",
          "expectedClipAuthority",
          "expectedNotesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          },
          notesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "deleted",
          "notesRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.duplicate",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          noteIds: {
            type: "array",
            items: {
              type: "integer",
              minimum: 0,
              maximum: 9007199254740991
            },
            minItems: 1
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          },
          expectedNotesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "ref",
          "noteIds",
          "expectedClipAuthority",
          "expectedNotesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          duplicated: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          },
          notesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "duplicated",
          "notesRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.quantize",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          grid: {
            type: "number",
            minimum: 1e-4,
            maximum: 1e6
          },
          amount: {
            type: "number",
            minimum: 0,
            maximum: 1
          },
          pitch: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          },
          expectedNotesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "ref",
          "grid",
          "amount",
          "expectedClipAuthority",
          "expectedNotesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          notesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "changed",
          "notesRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.read-by-id",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          noteIds: {
            type: "array",
            items: {
              type: "integer",
              minimum: 0,
              maximum: 9007199254740991
            },
            minItems: 1
          }
        },
        required: [
          "ref",
          "noteIds"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          notes: {
            type: "array",
            items: {
              type: "object",
              additionalProperties: true,
              maxProperties: 16
            }
          },
          notesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "notes",
          "notesRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.read-selected",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          available: {
            type: "boolean"
          },
          notes: {
            type: "array",
            items: {
              type: "object",
              additionalProperties: true,
              maxProperties: 16
            }
          },
          notesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "available",
          "notes",
          "notesRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.select",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          noteIds: {
            type: "array",
            items: {
              type: "integer",
              minimum: 0,
              maximum: 9007199254740991
            }
          },
          all: {
            type: "boolean"
          },
          none: {
            type: "boolean"
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          }
        },
        required: [
          "ref",
          "expectedClipAuthority"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          selected: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "selected"
        ],
        additionalProperties: false
      }
    },
    {
      id: "note.update",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          notes: {
            type: "array",
            items: {
              type: "object",
              properties: {
                id: {
                  type: "integer",
                  minimum: 0,
                  maximum: 9007199254740991
                },
                pitch: {
                  type: "integer",
                  minimum: 0,
                  maximum: 127
                },
                start: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e5
                },
                duration: {
                  type: "number",
                  minimum: 0,
                  maximum: 1e5
                },
                velocity: {
                  type: "number",
                  minimum: 0,
                  maximum: 127
                },
                mute: {
                  type: "boolean"
                },
                probability: {
                  type: "number",
                  minimum: 0,
                  maximum: 1
                },
                velocityDeviation: {
                  type: "number",
                  minimum: -127,
                  maximum: 127
                },
                releaseVelocity: {
                  type: "number",
                  minimum: 0,
                  maximum: 127
                }
              },
              required: [
                "id"
              ],
              additionalProperties: false
            }
          },
          expectedClipAuthority: {
            type: "object",
            properties: {
              expectedObjectIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedTrackIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSlotIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneRef: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              expectedSceneIdentity: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "expectedObjectIdentity",
              "expectedTrackRef",
              "expectedTrackIdentity"
            ],
            additionalProperties: false
          },
          expectedNotesRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "ref",
          "notes",
          "expectedClipAuthority",
          "expectedNotesRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          updated: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "updated"
        ],
        additionalProperties: false
      }
    },
    {
      id: "observe.poll",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          subscriptionId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          }
        },
        required: [
          "subscriptionId"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          events: {
            type: "array",
            items: {
              type: "object",
              properties: {
                kind: {
                  type: "string"
                },
                ref: {
                  type: [
                    "string",
                    "null"
                  ]
                },
                revision: {
                  type: "string",
                  minLength: 64,
                  maxLength: 64,
                  pattern: "^[0-9a-f]{64}$"
                },
                changedFields: {
                  type: "array",
                  items: {
                    type: "string",
                    maxLength: 64
                  },
                  maxItems: 32
                }
              },
              required: [
                "kind",
                "ref",
                "revision",
                "changedFields"
              ],
              additionalProperties: false
            },
            maxItems: 64
          },
          overflow: {
            type: "boolean"
          },
          sequence: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "events",
          "overflow",
          "sequence"
        ],
        additionalProperties: false
      }
    },
    {
      id: "observe.subscribe",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          topics: {
            type: "array",
            items: {
              type: "object",
              properties: {
                kind: {
                  type: "string",
                  enum: [
                    "transport",
                    "selection",
                    "track",
                    "clip",
                    "device",
                    "parameter",
                    "groove",
                    "tuning",
                    "scene",
                    "meters",
                    "rack"
                  ]
                },
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "kind"
              ],
              additionalProperties: false
            },
            minItems: 1,
            maxItems: 64
          },
          minIntervalMs: {
            type: "integer",
            minimum: 100,
            maximum: 6e4
          }
        },
        required: [
          "topics"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          subscriptionId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          topics: {
            type: "array",
            items: {
              type: "object",
              properties: {
                kind: {
                  type: "string",
                  enum: [
                    "transport",
                    "selection",
                    "track",
                    "clip",
                    "device",
                    "parameter",
                    "groove",
                    "tuning",
                    "scene",
                    "meters",
                    "rack"
                  ]
                },
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "kind"
              ],
              additionalProperties: false
            },
            minItems: 1,
            maxItems: 64
          },
          minIntervalMs: {
            type: "integer",
            minimum: 100,
            maximum: 6e4
          },
          revisions: {
            type: "object",
            additionalProperties: true,
            maxProperties: 64
          }
        },
        required: [
          "subscriptionId",
          "topics",
          "minIntervalMs",
          "revisions"
        ],
        additionalProperties: false
      }
    },
    {
      id: "observe.unsubscribe",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          subscriptionId: {
            type: "string",
            minLength: 16,
            maxLength: 128
          }
        },
        required: [
          "subscriptionId"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          unsubscribed: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "unsubscribed"
        ],
        additionalProperties: false
      }
    },
    {
      id: "ownership.settle",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedFingerprint"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          settled: {
            type: "boolean",
            const: true
          },
          fingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "settled",
          "fingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "parameter.re-enable-automation",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "done"
        ],
        additionalProperties: false
      }
    },
    {
      id: "performance.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "setRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          averageProcessUsage: {
            type: [
              "number",
              "null"
            ]
          },
          peakProcessUsage: {
            type: [
              "number",
              "null"
            ]
          },
          tracks: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                performanceImpact: {
                  type: [
                    "integer",
                    "null"
                  ]
                },
                inputMeterLeft: {
                  type: [
                    "number",
                    "null"
                  ]
                },
                inputMeterRight: {
                  type: [
                    "number",
                    "null"
                  ]
                },
                inputMeterLevel: {
                  type: [
                    "number",
                    "null"
                  ]
                },
                outputMeterLeft: {
                  type: [
                    "number",
                    "null"
                  ]
                },
                outputMeterRight: {
                  type: [
                    "number",
                    "null"
                  ]
                },
                outputMeterLevel: {
                  type: [
                    "number",
                    "null"
                  ]
                },
                devices: {
                  type: "array",
                  items: {
                    type: "object",
                    properties: {
                      ref: {
                        type: "string",
                        minLength: 1,
                        maxLength: 256
                      },
                      latencySamples: {
                        type: [
                          "integer",
                          "null"
                        ]
                      },
                      latencyMs: {
                        type: [
                          "number",
                          "null"
                        ]
                      }
                    },
                    required: [
                      "ref",
                      "latencySamples",
                      "latencyMs"
                    ],
                    additionalProperties: false
                  }
                }
              },
              required: [
                "ref",
                "performanceImpact",
                "inputMeterLeft",
                "inputMeterRight",
                "inputMeterLevel",
                "outputMeterLeft",
                "outputMeterRight",
                "outputMeterLevel",
                "devices"
              ],
              additionalProperties: false
            }
          },
          sampledAt: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "averageProcessUsage",
          "peakProcessUsage",
          "tracks",
          "sampledAt",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "plugin.parameter-names",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          begin: {
            type: "integer",
            minimum: 0,
            maximum: 1e7
          },
          end: {
            type: "integer",
            minimum: -1,
            maximum: 1e7
          }
        },
        required: [
          "ref"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          names: {
            type: "array",
            items: {
              type: "string",
              maxLength: 1024
            }
          },
          total: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 1e7
          }
        },
        required: [
          "names",
          "total"
        ],
        additionalProperties: false
      }
    },
    {
      id: "plugin.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          presetIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          isEditorOpen: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.bounce",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          allowedRoot: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          expectedSha256: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "path",
          "allowedRoot",
          "expectedSha256"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          completed: {
            type: "boolean",
            const: true
          },
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          manifestSha256: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "completed",
          "path",
          "manifestSha256"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.collect",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          allowedRoot: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          expectedSha256: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "path",
          "allowedRoot",
          "expectedSha256"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          completed: {
            type: "boolean",
            const: true
          },
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          manifestSha256: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "completed",
          "path",
          "manifestSha256"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.export",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          allowedRoot: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          expectedSha256: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "path",
          "allowedRoot",
          "expectedSha256"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          completed: {
            type: "boolean",
            const: true
          },
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          manifestSha256: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "completed",
          "path",
          "manifestSha256"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.import",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          }
        },
        additionalProperties: false,
        required: [
          "filePath"
        ]
      },
      result: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          }
        },
        required: [
          "path"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.new",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          allowedRoot: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          expectedSha256: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "path",
          "allowedRoot",
          "expectedSha256"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          completed: {
            type: "boolean",
            const: true
          },
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          manifestSha256: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "completed",
          "path",
          "manifestSha256"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.open",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          allowedRoot: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          expectedSha256: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "path",
          "allowedRoot",
          "expectedSha256"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          completed: {
            type: "boolean",
            const: true
          },
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          manifestSha256: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "completed",
          "path",
          "manifestSha256"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.save",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          allowedRoot: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          expectedSha256: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "path",
          "allowedRoot",
          "expectedSha256"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          completed: {
            type: "boolean",
            const: true
          },
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          manifestSha256: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "completed",
          "path",
          "manifestSha256"
        ],
        additionalProperties: false
      }
    },
    {
      id: "project.save-as",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          allowedRoot: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          expectedSha256: {
            type: [
              "string",
              "null"
            ],
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "path",
          "allowedRoot",
          "expectedSha256"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          completed: {
            type: "boolean",
            const: true
          },
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          manifestSha256: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "completed",
          "path",
          "manifestSha256"
        ],
        additionalProperties: false
      }
    },
    {
      id: "python.run",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          code: {
            type: "string",
            minLength: 1,
            maxLength: 65536
          },
          mode: {
            type: "string",
            enum: [
              "eval",
              "exec"
            ]
          },
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          timeoutMs: {
            type: "integer",
            minimum: 1,
            maximum: 3e4
          }
        },
        required: [
          "code"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ok: {
            type: "boolean"
          },
          stdout: {
            type: "string",
            maxLength: 1048576
          },
          error: {
            type: [
              "object",
              "null"
            ],
            properties: {
              type: {
                type: "string",
                maxLength: 128
              },
              message: {
                type: "string",
                maxLength: 1048576
              },
              traceback: {
                type: "string",
                maxLength: 1048576
              }
            },
            required: [
              "type",
              "message",
              "traceback"
            ],
            additionalProperties: false
          }
        },
        required: [
          "ok",
          "result",
          "stdout",
          "error"
        ],
        additionalProperties: true,
        maxProperties: 4
      }
    },
    {
      id: "rack.action",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          action: {
            type: "string",
            enum: [
              "add-macro",
              "remove-macro",
              "randomize-macros",
              "insert-chain",
              "copy-pad",
              "store-variation",
              "recall-variation",
              "delete-variation",
              "recall-last-variation"
            ]
          },
          index: {
            type: "integer",
            minimum: -1,
            maximum: 1e5
          },
          sourceIndex: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          targetIndex: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "action",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          chainRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          chainObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "done",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "rack.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          visibleMacroCount: {
            type: "integer",
            minimum: 1,
            maximum: 16
          },
          selectedVariationIndex: {
            type: "integer",
            minimum: -1,
            maximum: 1e5
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "rack.view.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          selectedChainRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          selectedPadIndex: {
            type: "integer",
            minimum: -1,
            maximum: 127
          },
          padScrollPosition: {
            type: "integer",
            minimum: 0,
            maximum: 127
          },
          showChainDevices: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "realtime.arm",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ttlMs: {
            type: "integer",
            minimum: 1e3,
            maximum: 3e4
          },
          channels: {
            type: "array",
            items: {
              type: "string",
              enum: [
                "udp-json",
                "osc",
                "xy",
                "max"
              ]
            },
            minItems: 1,
            maxItems: 4,
            uniqueItems: true
          },
          parameterRefs: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            },
            maxItems: 32,
            uniqueItems: true
          },
          sourcePorts: {
            type: "array",
            items: {
              type: "integer",
              minimum: 1,
              maximum: 65535
            },
            maxItems: 16,
            uniqueItems: true
          },
          outputSafety: {
            type: "object",
            properties: {
              safe: {
                type: "boolean",
                const: true
              },
              provenance: {
                type: "string",
                minLength: 1,
                maxLength: 512
              },
              observedAt: {
                type: "string",
                minLength: 1,
                maxLength: 64
              },
              scope: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "safe",
              "provenance"
            ],
            additionalProperties: false
          },
          targetAuthorities: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                parameterIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                ownerRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                ownerIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                trackRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                trackIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                siblings: {
                  type: "array",
                  items: {
                    type: "object",
                    properties: {
                      ref: {
                        type: "string",
                        minLength: 1,
                        maxLength: 256
                      },
                      objectIdentity: {
                        type: "string",
                        minLength: 1,
                        maxLength: 256
                      }
                    },
                    required: [
                      "ref",
                      "objectIdentity"
                    ],
                    additionalProperties: false
                  },
                  maxItems: 256
                }
              },
              required: [
                "ref",
                "parameterIdentity",
                "ownerRef",
                "ownerIdentity",
                "trackRef",
                "trackIdentity",
                "siblings"
              ],
              additionalProperties: false
            },
            maxItems: 32
          }
        },
        required: [
          "channels",
          "parameterRefs",
          "outputSafety",
          "targetAuthorities"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          host: {
            type: "string",
            minLength: 1,
            maxLength: 64
          },
          port: {
            type: "integer",
            minimum: 1,
            maximum: 65535
          },
          token: {
            type: "string",
            minLength: 16,
            maxLength: 128
          },
          expiresAt: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          channels: {
            type: "array",
            items: {
              type: "string",
              enum: [
                "udp-json",
                "osc",
                "xy",
                "max"
              ]
            },
            minItems: 1,
            maxItems: 4,
            uniqueItems: true
          },
          parameterRefs: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            },
            maxItems: 32,
            uniqueItems: true
          },
          packetLimitBytes: {
            type: "integer",
            const: 512
          },
          ratePerSecond: {
            type: "integer",
            const: 64
          },
          burst: {
            type: "integer",
            const: 16
          }
        },
        required: [
          "host",
          "port",
          "token",
          "expiresAt",
          "channels",
          "parameterRefs",
          "packetLimitBytes",
          "ratePerSecond",
          "burst"
        ],
        additionalProperties: false
      }
    },
    {
      id: "realtime.disarm",
      method: "invoke",
      request: {
        type: "object",
        properties: {},
        required: [],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          armed: {
            type: "boolean",
            const: false
          }
        },
        required: [
          "armed"
        ],
        additionalProperties: false
      }
    },
    {
      id: "realtime.stats",
      method: "invoke",
      request: {
        type: "object",
        properties: {},
        required: [],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          armed: {
            type: "boolean"
          },
          accepted: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          applied: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          applyFailures: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          pending: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedUnarmed: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedEndpoint: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedTarget: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedInvalid: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedReplay: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedRateLimited: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedQueueFull: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          droppedBeforeDispatch: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          revokedBeforeApply: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          sequenceGaps: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          lastSequence: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          jitterMs: {
            type: "number",
            minimum: 0,
            maximum: 6e4
          },
          maxJitterMs: {
            type: "number",
            minimum: 0,
            maximum: 6e4
          }
        },
        required: [
          "armed",
          "accepted",
          "applied",
          "applyFailures",
          "pending",
          "droppedUnarmed",
          "droppedEndpoint",
          "droppedTarget",
          "droppedInvalid",
          "droppedReplay",
          "droppedRateLimited",
          "droppedQueueFull",
          "droppedBeforeDispatch",
          "revokedBeforeApply",
          "sequenceGaps",
          "lastSequence",
          "jitterMs",
          "maxJitterMs"
        ],
        additionalProperties: false
      }
    },
    {
      id: "reconnect",
      method: "reconnect",
      request: {
        type: "object",
        properties: {},
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          connected: {
            type: "boolean"
          },
          epoch: {
            type: [
              "integer",
              "null"
            ],
            minimum: 1,
            maximum: 9007199254740991
          },
          protocol: {
            type: "string",
            const: "ableton-live/v1"
          }
        },
        required: [
          "connected",
          "epoch",
          "protocol"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "recording.arrangement",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          action: {
            type: "string",
            enum: [
              "start",
              "stop"
            ]
          },
          expectedSessionRecord: {
            type: "boolean"
          },
          expectedArrangementRecord: {
            type: "boolean"
          },
          destinationTrackRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          destinationTrackIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          alsoTrackRefs: {
            type: "array",
            maxItems: 1024,
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            }
          },
          alsoTrackIdentities: {
            type: "array",
            maxItems: 1024,
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            }
          },
          outputSafety: {
            type: "object",
            properties: {
              safe: {
                type: "boolean",
                const: true
              },
              provenance: {
                type: "string",
                minLength: 1,
                maxLength: 512
              },
              observedAt: {
                type: "string",
                minLength: 1,
                maxLength: 64
              },
              scope: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "safe",
              "provenance"
            ],
            additionalProperties: false
          }
        },
        required: [
          "action",
          "expectedSessionRecord",
          "expectedArrangementRecord",
          "destinationTrackRef",
          "destinationTrackIdentity",
          "outputSafety"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          recording: {
            type: "boolean"
          }
        },
        required: [
          "recording"
        ],
        additionalProperties: false
      }
    },
    {
      id: "recording.session",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          action: {
            type: "string",
            enum: [
              "start",
              "stop"
            ]
          },
          expectedSessionRecord: {
            type: "boolean"
          },
          expectedArrangementRecord: {
            type: "boolean"
          },
          destinationTrackRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          destinationTrackIdentity: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          alsoTrackRefs: {
            type: "array",
            maxItems: 1024,
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            }
          },
          alsoTrackIdentities: {
            type: "array",
            maxItems: 1024,
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            }
          },
          outputSafety: {
            type: "object",
            properties: {
              safe: {
                type: "boolean",
                const: true
              },
              provenance: {
                type: "string",
                minLength: 1,
                maxLength: 512
              },
              observedAt: {
                type: "string",
                minLength: 1,
                maxLength: 64
              },
              scope: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "safe",
              "provenance"
            ],
            additionalProperties: false
          }
        },
        required: [
          "action",
          "expectedSessionRecord",
          "expectedArrangementRecord",
          "destinationTrackRef",
          "destinationTrackIdentity",
          "outputSafety"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          recording: {
            type: "boolean"
          }
        },
        required: [
          "recording"
        ],
        additionalProperties: false
      }
    },
    {
      id: "render.offline",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          fromBeat: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          toBeat: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          expectedName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        additionalProperties: false,
        required: [
          "trackRef",
          "fromBeat",
          "toBeat"
        ]
      },
      result: {
        type: "object",
        properties: {
          path: {
            type: "string",
            minLength: 1,
            maxLength: 4096
          },
          format: {
            type: "string",
            enum: [
              "wav",
              "aiff"
            ]
          },
          channels: {
            type: "integer",
            minimum: 1,
            maximum: 64
          },
          sampleRate: {
            type: "integer",
            minimum: 1,
            maximum: 1e6
          },
          bitDepth: {
            type: [
              "integer",
              "null"
            ],
            minimum: 1,
            maximum: 64
          },
          seconds: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          bytes: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          renderMs: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          }
        },
        required: [
          "path",
          "format",
          "channels",
          "sampleRate",
          "bitDepth",
          "seconds",
          "bytes",
          "renderMs"
        ],
        additionalProperties: false
      }
    },
    {
      id: "routing.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          inputType: {
            type: "string",
            maxLength: 256
          },
          inputSubRouting: {
            type: "string",
            maxLength: 256
          },
          outputType: {
            type: "string",
            maxLength: 256
          },
          outputSubRouting: {
            type: "string",
            maxLength: 256
          },
          arm: {
            type: "boolean"
          },
          monitoring: {
            type: "string",
            enum: [
              "in",
              "auto",
              "off"
            ]
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "sample.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          beatsGranulationResolution: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          beatsTransientEnvelope: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          beatsTransientLoopMode: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          complexProEnvelope: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          complexProFormants: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          textureFlux: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          textureGrainSize: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          tonesGrainSize: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          slicingStyle: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          slicingBeatDivision: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          slicingRegionCount: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          slicingSensitivity: {
            type: "number",
            minimum: 0,
            maximum: 1e6
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "sample.slice",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          action: {
            type: "string",
            enum: [
              "insert",
              "move",
              "remove",
              "clear",
              "reset"
            ]
          },
          time: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          toTime: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "action",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          slices: {
            type: "array",
            items: {
              type: "integer",
              minimum: 0,
              maximum: 9007199254740991
            }
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "slices",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "scene.capture",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          captured: {
            type: "boolean",
            const: true
          },
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "captured",
          "ref",
          "objectIdentity",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "scene.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          name: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "name",
          "expectedStructureRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "index",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "scene.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          explicitDeletion: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "ref",
          "expectedStructureRevision",
          "expectedObjectIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "scene.duplicate",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStructureRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "index",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "scene.fire-selected",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedAuthorityRevision",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          fired: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "fired"
        ],
        additionalProperties: false
      }
    },
    {
      id: "scene.rename",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "name",
          "expectedName",
          "expectedObjectIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          renamed: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "renamed",
          "name"
        ],
        additionalProperties: false
      }
    },
    {
      id: "scene.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          colorIndex: {
            type: "integer",
            minimum: 0,
            maximum: 69
          },
          tempo: {
            type: "number",
            minimum: 20,
            maximum: 999
          },
          tempoEnabled: {
            type: "boolean"
          },
          signatureNumerator: {
            type: "integer",
            minimum: 1,
            maximum: 99
          },
          signatureDenominator: {
            type: "integer",
            minimum: 1,
            maximum: 99
          },
          timeSignatureEnabled: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedAuthorityRevision",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "selection.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          sceneRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          slotRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          deviceRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          parameterRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          chainRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          detailClipRef: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.audio-clip.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSlotIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedSceneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "trackRef",
          "sceneIndex",
          "filePath",
          "expectedTrackIdentity",
          "expectedSlotRef",
          "expectedSlotIdentity",
          "expectedSceneRef",
          "expectedSceneIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "length",
          "filePath",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.audition-launch",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          setName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneName: {
            type: "string",
            maxLength: 256
          },
          sceneIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          playbackRevision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          eligibleTargets: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 1024
            }
          },
          outputSafety: {
            type: "object",
            properties: {
              safe: {
                type: "boolean",
                const: true
              },
              provenance: {
                type: "string",
                minLength: 1,
                maxLength: 512
              },
              observedAt: {
                type: "string",
                minLength: 1,
                maxLength: 64
              },
              scope: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "safe",
              "provenance"
            ],
            additionalProperties: false
          },
          expectedSetIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "setName",
          "sceneName",
          "sceneIndex",
          "playbackRevision",
          "eligibleTargets",
          "outputSafety",
          "expectedSetIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          launched: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          targets: {
            type: "array",
            items: {
              type: "object",
              properties: {
                trackRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                clipSlotRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneIndex: {
                  type: "integer",
                  minimum: 0,
                  maximum: 1e5
                },
                clipRef: {
                  type: [
                    "string",
                    "null"
                  ],
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "trackRef",
                "clipSlotRef",
                "sceneRef",
                "sceneIndex",
                "clipRef"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "launched",
          "targets"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.audition-stop",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          setName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          eligibleTargets: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 1024
            }
          },
          expectedSetIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "setName",
          "eligibleTargets",
          "expectedSetIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          stopped: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "stopped"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.capture-midi",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          captured: {
            type: "boolean"
          },
          clips: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            }
          },
          clipIdentities: {
            type: "array",
            items: {
              type: "object",
              properties: {
                ref: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                objectIdentity: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                createdFingerprint: {
                  type: "string",
                  minLength: 64,
                  maxLength: 64,
                  pattern: "^[0-9a-f]{64}$"
                },
                ownershipToken: {
                  type: "string",
                  minLength: 32,
                  maxLength: 128,
                  pattern: "^[A-Za-z0-9_-]{32,128}$"
                }
              },
              required: [
                "ref",
                "objectIdentity",
                "createdFingerprint"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "captured",
          "clips",
          "clipIdentities"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.clip-launch",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          slotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          trackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          slotIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          clipIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          playbackRevision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          outputSafety: {
            type: "object",
            properties: {
              safe: {
                type: "boolean",
                const: true
              },
              provenance: {
                type: "string",
                minLength: 1,
                maxLength: 512
              },
              observedAt: {
                type: "string",
                minLength: 1,
                maxLength: 64
              },
              scope: {
                type: "string",
                minLength: 1,
                maxLength: 256
              }
            },
            required: [
              "safe",
              "provenance"
            ],
            additionalProperties: false
          }
        },
        required: [
          "slotRef",
          "trackRef",
          "sceneRef",
          "sceneIndex",
          "clipRef",
          "trackIdentity",
          "sceneIdentity",
          "slotIdentity",
          "clipIdentity",
          "playbackRevision",
          "outputSafety"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          launched: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          targets: {
            type: "array",
            items: {
              type: "object",
              properties: {
                trackRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                clipSlotRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneIndex: {
                  type: "integer",
                  minimum: 0,
                  maximum: 1e5
                },
                clipRef: {
                  type: [
                    "string",
                    "null"
                  ],
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "trackRef",
                "clipSlotRef",
                "sceneRef",
                "sceneIndex",
                "clipRef"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "launched",
          "targets"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.clip-stop",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          slotRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          clipRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          trackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          sceneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          slotIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          clipIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "slotRef",
          "trackRef",
          "sceneRef",
          "sceneIndex",
          "clipRef",
          "trackIdentity",
          "sceneIdentity",
          "slotIdentity",
          "clipIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          stopped: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "stopped"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.discover",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          kind: {
            type: "string",
            minLength: 1,
            maxLength: 64
          },
          parent: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          filters: {
            type: "object",
            maxProperties: 16,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean",
                "null"
              ],
              maxLength: 256,
              minimum: -9007199254740991,
              maximum: 9007199254740991
            }
          },
          requestedFields: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 64
            },
            maxItems: 256
          },
          traversalBudget: {
            type: "integer",
            minimum: 1,
            maximum: 1e7
          },
          limit: {
            type: "integer",
            minimum: 1,
            maximum: 1e5
          },
          cursor: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "kind"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          epoch: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          items: {
            type: "array",
            items: {
              type: "object",
              maxProperties: 64,
              additionalProperties: true
            }
          },
          truncated: {
            type: "boolean"
          },
          revision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          kind: {
            type: "string",
            minLength: 1,
            maxLength: 64
          },
          nextCursor: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "epoch",
          "items",
          "truncated",
          "revision",
          "kind"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.emergency-stop",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          expectedTargets: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 1024
            }
          },
          expectedRecording: {
            type: "string",
            enum: [
              "stopped",
              "session",
              "arrangement",
              "both"
            ]
          }
        },
        required: [
          "expectedTargets",
          "expectedRecording"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          stopped: {
            type: "boolean",
            const: true
          },
          stoppedTargets: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 1024
            }
          },
          recordingStopped: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "stopped",
          "stoppedTargets",
          "recordingStopped"
        ],
        additionalProperties: false
      }
    },
    {
      id: "session.playback",
      method: "discover",
      request: {
        type: "object",
        properties: {},
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          epoch: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          revision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          transport: {
            type: "object",
            properties: {
              playing: {
                type: [
                  "boolean",
                  "null"
                ]
              },
              arrangementRecord: {
                type: [
                  "boolean",
                  "null"
                ]
              },
              sessionRecord: {
                type: [
                  "boolean",
                  "null"
                ]
              },
              position: {
                type: [
                  "number",
                  "null"
                ],
                minimum: 0,
                maximum: 1e9
              },
              launchQuantization: {
                type: "object",
                properties: {
                  raw: {
                    type: [
                      "string",
                      "number",
                      "null"
                    ],
                    maxLength: 128,
                    minimum: 0,
                    maximum: 1e5
                  },
                  normalized: {
                    type: [
                      "string",
                      "null"
                    ],
                    maxLength: 256
                  }
                },
                required: [
                  "raw",
                  "normalized"
                ],
                additionalProperties: false
              },
              loop: {
                type: "object",
                properties: {
                  enabled: {
                    type: [
                      "boolean",
                      "null"
                    ]
                  },
                  start: {
                    type: [
                      "number",
                      "null"
                    ],
                    minimum: 0,
                    maximum: 1e9
                  },
                  length: {
                    type: [
                      "number",
                      "null"
                    ],
                    minimum: 0,
                    maximum: 1e9
                  }
                },
                required: [
                  "enabled",
                  "start",
                  "length"
                ],
                additionalProperties: false
              },
              punchIn: {
                type: [
                  "boolean",
                  "null"
                ]
              },
              punchOut: {
                type: [
                  "boolean",
                  "null"
                ]
              },
              metronome: {
                type: [
                  "boolean",
                  "null"
                ]
              },
              countIn: {
                type: [
                  "number",
                  "null"
                ],
                minimum: 0,
                maximum: 1e3
              }
            },
            required: [
              "playing",
              "arrangementRecord",
              "sessionRecord",
              "position",
              "launchQuantization",
              "loop",
              "punchIn",
              "punchOut",
              "metronome",
              "countIn"
            ],
            additionalProperties: false
          },
          firedTargets: {
            type: "array",
            items: {
              type: "object",
              properties: {
                trackRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                clipSlotRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneIndex: {
                  type: "integer",
                  minimum: 0,
                  maximum: 1e5
                },
                clipRef: {
                  type: [
                    "string",
                    "null"
                  ],
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "trackRef",
                "clipSlotRef",
                "sceneRef",
                "sceneIndex",
                "clipRef"
              ],
              additionalProperties: false
            }
          },
          playingTargets: {
            type: "array",
            items: {
              type: "object",
              properties: {
                trackRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                clipSlotRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneRef: {
                  type: "string",
                  minLength: 1,
                  maxLength: 256
                },
                sceneIndex: {
                  type: "integer",
                  minimum: 0,
                  maximum: 1e5
                },
                clipRef: {
                  type: [
                    "string",
                    "null"
                  ],
                  minLength: 1,
                  maxLength: 256
                }
              },
              required: [
                "trackRef",
                "clipSlotRef",
                "sceneRef",
                "sceneIndex",
                "clipRef"
              ],
              additionalProperties: false
            }
          }
        },
        required: [
          "ref",
          "epoch",
          "revision",
          "transport",
          "firedTargets",
          "playingTargets"
        ],
        additionalProperties: false
      }
    },
    {
      id: "simpler.replace-sample",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "filePath",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          }
        },
        required: [
          "changed",
          "revision",
          "filePath"
        ],
        additionalProperties: false
      }
    },
    {
      id: "snapshot",
      method: "snapshot",
      request: {
        type: "object",
        properties: {
          tracks: {
            type: "object",
            properties: {
              from: {
                type: "integer",
                minimum: 0,
                maximum: 1e5
              },
              count: {
                type: "integer",
                minimum: 1,
                maximum: 1e5
              }
            },
            required: [
              "from",
              "count"
            ],
            additionalProperties: false
          },
          scenes: {
            type: "object",
            properties: {
              from: {
                type: "integer",
                minimum: 0,
                maximum: 1e5
              },
              count: {
                type: "integer",
                minimum: 1,
                maximum: 1e5
              }
            },
            required: [
              "from",
              "count"
            ],
            additionalProperties: false
          },
          focus: {
            type: "array",
            items: {
              type: "integer",
              minimum: 0,
              maximum: 1e5
            },
            uniqueItems: true
          },
          parts: {
            type: "array",
            items: {
              type: "string",
              enum: [
                "set",
                "tracks",
                "scenes",
                "arrangement",
                "playback",
                "selection"
              ]
            },
            maxItems: 6,
            uniqueItems: true
          }
        },
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          set: {
            type: "object",
            maxProperties: 64,
            additionalProperties: true
          },
          tracks: {
            type: "array",
            items: {
              type: "object",
              maxProperties: 64,
              additionalProperties: true
            }
          },
          scenes: {
            type: "array",
            items: {
              type: "object",
              maxProperties: 64,
              additionalProperties: true
            }
          },
          arrangement: {
            type: "object",
            maxProperties: 64,
            additionalProperties: true
          },
          playback: {
            type: "object",
            properties: {
              ref: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              epoch: {
                type: "integer",
                minimum: 1,
                maximum: 9007199254740991
              },
              revision: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              transport: {
                type: "object",
                properties: {
                  playing: {
                    type: [
                      "boolean",
                      "null"
                    ]
                  },
                  arrangementRecord: {
                    type: [
                      "boolean",
                      "null"
                    ]
                  },
                  sessionRecord: {
                    type: [
                      "boolean",
                      "null"
                    ]
                  },
                  position: {
                    type: [
                      "number",
                      "null"
                    ],
                    minimum: 0,
                    maximum: 1e9
                  },
                  launchQuantization: {
                    type: "object",
                    properties: {
                      raw: {
                        type: [
                          "string",
                          "number",
                          "null"
                        ],
                        maxLength: 128,
                        minimum: 0,
                        maximum: 1e5
                      },
                      normalized: {
                        type: [
                          "string",
                          "null"
                        ],
                        maxLength: 256
                      }
                    },
                    required: [
                      "raw",
                      "normalized"
                    ],
                    additionalProperties: false
                  },
                  loop: {
                    type: "object",
                    properties: {
                      enabled: {
                        type: [
                          "boolean",
                          "null"
                        ]
                      },
                      start: {
                        type: [
                          "number",
                          "null"
                        ],
                        minimum: 0,
                        maximum: 1e9
                      },
                      length: {
                        type: [
                          "number",
                          "null"
                        ],
                        minimum: 0,
                        maximum: 1e9
                      }
                    },
                    required: [
                      "enabled",
                      "start",
                      "length"
                    ],
                    additionalProperties: false
                  },
                  punchIn: {
                    type: [
                      "boolean",
                      "null"
                    ]
                  },
                  punchOut: {
                    type: [
                      "boolean",
                      "null"
                    ]
                  },
                  metronome: {
                    type: [
                      "boolean",
                      "null"
                    ]
                  },
                  countIn: {
                    type: [
                      "number",
                      "null"
                    ],
                    minimum: 0,
                    maximum: 1e3
                  }
                },
                required: [
                  "playing",
                  "arrangementRecord",
                  "sessionRecord",
                  "position",
                  "launchQuantization",
                  "loop",
                  "punchIn",
                  "punchOut",
                  "metronome",
                  "countIn"
                ],
                additionalProperties: false
              },
              firedTargets: {
                type: "array",
                items: {
                  type: "object",
                  properties: {
                    trackRef: {
                      type: "string",
                      minLength: 1,
                      maxLength: 256
                    },
                    clipSlotRef: {
                      type: "string",
                      minLength: 1,
                      maxLength: 256
                    },
                    sceneRef: {
                      type: "string",
                      minLength: 1,
                      maxLength: 256
                    },
                    sceneIndex: {
                      type: "integer",
                      minimum: 0,
                      maximum: 1e5
                    },
                    clipRef: {
                      type: [
                        "string",
                        "null"
                      ],
                      minLength: 1,
                      maxLength: 256
                    }
                  },
                  required: [
                    "trackRef",
                    "clipSlotRef",
                    "sceneRef",
                    "sceneIndex",
                    "clipRef"
                  ],
                  additionalProperties: false
                }
              },
              playingTargets: {
                type: "array",
                items: {
                  type: "object",
                  properties: {
                    trackRef: {
                      type: "string",
                      minLength: 1,
                      maxLength: 256
                    },
                    clipSlotRef: {
                      type: "string",
                      minLength: 1,
                      maxLength: 256
                    },
                    sceneRef: {
                      type: "string",
                      minLength: 1,
                      maxLength: 256
                    },
                    sceneIndex: {
                      type: "integer",
                      minimum: 0,
                      maximum: 1e5
                    },
                    clipRef: {
                      type: [
                        "string",
                        "null"
                      ],
                      minLength: 1,
                      maxLength: 256
                    }
                  },
                  required: [
                    "trackRef",
                    "clipSlotRef",
                    "sceneRef",
                    "sceneIndex",
                    "clipRef"
                  ],
                  additionalProperties: false
                }
              }
            },
            required: [
              "ref",
              "epoch",
              "revision",
              "transport",
              "firedTargets",
              "playingTargets"
            ],
            additionalProperties: false
          },
          browser: {
            type: "array",
            items: {
              type: "object",
              maxProperties: 64,
              additionalProperties: true
            }
          },
          epoch: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          selected: {
            type: [
              "string",
              "null"
            ],
            minLength: 1,
            maxLength: 256
          },
          selection: {
            type: "object",
            properties: {
              trackRef: {
                type: [
                  "string",
                  "null"
                ],
                minLength: 1,
                maxLength: 256
              },
              sceneRef: {
                type: [
                  "string",
                  "null"
                ],
                minLength: 1,
                maxLength: 256
              },
              slotRef: {
                type: [
                  "string",
                  "null"
                ],
                minLength: 1,
                maxLength: 256
              },
              detailClipRef: {
                type: [
                  "string",
                  "null"
                ],
                minLength: 1,
                maxLength: 256
              },
              deviceRef: {
                type: [
                  "string",
                  "null"
                ],
                minLength: 1,
                maxLength: 256
              },
              parameterRef: {
                type: [
                  "string",
                  "null"
                ],
                minLength: 1,
                maxLength: 256
              },
              chainRef: {
                type: [
                  "string",
                  "null"
                ],
                minLength: 1,
                maxLength: 256
              }
            },
            additionalProperties: false
          },
          trackCount: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          sceneCount: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          window: {
            type: "object",
            properties: {
              tracks: {
                type: "object",
                properties: {
                  from: {
                    type: "integer",
                    minimum: 0,
                    maximum: 1e5
                  },
                  count: {
                    type: "integer",
                    minimum: 1,
                    maximum: 1e5
                  }
                },
                required: [
                  "from",
                  "count"
                ],
                additionalProperties: false
              },
              scenes: {
                type: "object",
                properties: {
                  from: {
                    type: "integer",
                    minimum: 0,
                    maximum: 1e5
                  },
                  count: {
                    type: "integer",
                    minimum: 1,
                    maximum: 1e5
                  }
                },
                required: [
                  "from",
                  "count"
                ],
                additionalProperties: false
              },
              focus: {
                type: "array",
                items: {
                  type: "integer",
                  minimum: 0,
                  maximum: 1e5
                },
                uniqueItems: true
              },
              parts: {
                type: "array",
                items: {
                  type: "string",
                  enum: [
                    "set",
                    "tracks",
                    "scenes",
                    "arrangement",
                    "playback",
                    "selection"
                  ]
                },
                maxItems: 6,
                uniqueItems: true
              }
            },
            additionalProperties: false
          }
        },
        required: [],
        additionalProperties: false
      }
    },
    {
      id: "song.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "setRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          visibleTracks: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 256
            }
          },
          appointedDevice: {
            type: [
              "string",
              "null"
            ],
            maxLength: 256
          },
          songLength: {
            type: [
              "number",
              "null"
            ]
          },
          startTime: {
            type: [
              "number",
              "null"
            ]
          },
          signatureNumerator: {
            type: [
              "integer",
              "null"
            ]
          },
          signatureDenominator: {
            type: [
              "integer",
              "null"
            ]
          },
          swingAmount: {
            type: [
              "number",
              "null"
            ]
          },
          overdub: {
            type: [
              "boolean",
              "null"
            ]
          },
          arrangementOverdub: {
            type: [
              "boolean",
              "null"
            ]
          },
          backToArranger: {
            type: [
              "boolean",
              "null"
            ]
          },
          canCaptureMidi: {
            type: [
              "boolean",
              "null"
            ]
          },
          canUndo: {
            type: [
              "boolean",
              "null"
            ]
          },
          canRedo: {
            type: [
              "boolean",
              "null"
            ]
          },
          exclusiveArm: {
            type: [
              "boolean",
              "null"
            ]
          },
          exclusiveSolo: {
            type: [
              "boolean",
              "null"
            ]
          },
          isCountingIn: {
            type: [
              "boolean",
              "null"
            ]
          },
          tempoFollowerEnabled: {
            type: [
              "boolean",
              "null"
            ]
          },
          reEnableAutomationEnabled: {
            type: [
              "boolean",
              "null"
            ]
          },
          sessionRecord: {
            type: [
              "boolean",
              "null"
            ]
          },
          sessionAutomationRecord: {
            type: [
              "boolean",
              "null"
            ]
          },
          clipTriggerQuantization: {
            type: [
              "object",
              "null"
            ],
            maxProperties: 4,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean"
              ],
              maxLength: 256
            }
          },
          midiRecordingQuantization: {
            type: [
              "object",
              "null"
            ],
            maxProperties: 4,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean"
              ],
              maxLength: 256
            }
          },
          isAbletonLinkEnabled: {
            type: [
              "boolean",
              "null"
            ]
          },
          isAbletonLinkStartStopSyncEnabled: {
            type: [
              "boolean",
              "null"
            ]
          },
          tempoFollower: {
            type: [
              "boolean",
              "null"
            ]
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          lastEventTime: {
            type: [
              "number",
              "null"
            ],
            minimum: 0,
            maximum: 1e9
          },
          sessionRecordStatus: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 16
          },
          canJumpToNextCue: {
            type: [
              "boolean",
              "null"
            ]
          },
          canJumpToPrevCue: {
            type: [
              "boolean",
              "null"
            ]
          },
          isCuePointSelected: {
            type: [
              "boolean",
              "null"
            ]
          },
          selectOnLaunch: {
            type: [
              "boolean",
              "null"
            ]
          },
          beatsSongTime: {
            type: [
              "string",
              "null"
            ],
            maxLength: 64
          }
        },
        required: [
          "visibleTracks",
          "appointedDevice",
          "songLength",
          "startTime",
          "signatureNumerator",
          "signatureDenominator",
          "swingAmount",
          "overdub",
          "arrangementOverdub",
          "backToArranger",
          "canCaptureMidi",
          "canUndo",
          "canRedo",
          "exclusiveArm",
          "exclusiveSolo",
          "isCountingIn",
          "tempoFollowerEnabled",
          "reEnableAutomationEnabled",
          "sessionRecord",
          "sessionAutomationRecord",
          "clipTriggerQuantization",
          "isAbletonLinkEnabled",
          "isAbletonLinkStartStopSyncEnabled",
          "tempoFollower",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "song.redo",
      method: "invoke",
      request: {
        type: "object",
        properties: {},
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean"
          },
          canUndo: {
            type: [
              "boolean",
              "null"
            ]
          },
          canRedo: {
            type: [
              "boolean",
              "null"
            ]
          }
        },
        required: [
          "done",
          "canUndo",
          "canRedo"
        ],
        additionalProperties: false
      }
    },
    {
      id: "song.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          signatureNumerator: {
            type: "integer",
            minimum: 1,
            maximum: 99
          },
          signatureDenominator: {
            type: "integer",
            minimum: 1,
            maximum: 99
          },
          swingAmount: {
            type: "number",
            minimum: 0,
            maximum: 1
          },
          clipTriggerQuantization: {
            type: "integer",
            minimum: 0,
            maximum: 13
          },
          midiRecordingQuantization: {
            type: "integer",
            minimum: 0,
            maximum: 8
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          selectOnLaunch: {
            type: "boolean"
          }
        },
        required: [
          "setRef",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "song.time-convert",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          query: {
            type: "string",
            enum: [
              "beats-loop",
              "current-smpte"
            ]
          },
          smpteFormat: {
            type: "string",
            enum: [
              "smpte-24",
              "smpte-25",
              "smpte-29",
              "smpte-30",
              "smpte-30-drop"
            ]
          }
        },
        required: [
          "setRef",
          "query"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          available: {
            type: "boolean"
          },
          loopStart: {
            type: [
              "number",
              "null"
            ]
          },
          loopLength: {
            type: [
              "number",
              "null"
            ]
          },
          smpte: {
            type: [
              "object",
              "null"
            ],
            maxProperties: 8,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean"
              ],
              maxLength: 256
            }
          }
        },
        required: [
          "available",
          "loopStart",
          "loopLength",
          "smpte"
        ],
        additionalProperties: false
      }
    },
    {
      id: "song.undo",
      method: "invoke",
      request: {
        type: "object",
        properties: {},
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean"
          },
          canUndo: {
            type: [
              "boolean",
              "null"
            ]
          },
          canRedo: {
            type: [
              "boolean",
              "null"
            ]
          }
        },
        required: [
          "done",
          "canUndo",
          "canRedo"
        ],
        additionalProperties: false
      }
    },
    {
      id: "song.view.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          drawMode: {
            type: "boolean"
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "status",
      method: "status",
      request: {
        type: "object",
        properties: {},
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          connected: {
            type: "boolean"
          },
          adapter: {
            type: "string",
            minLength: 1,
            maxLength: 32
          },
          epoch: {
            type: [
              "integer",
              "null"
            ],
            minimum: 1,
            maximum: 9007199254740991
          },
          protocol: {
            type: "string",
            const: "ableton-live/v1"
          },
          registryHash: {
            type: "string",
            pattern: "^[a-f0-9]{64}$"
          },
          operations: {
            type: "array",
            items: {
              type: "string",
              minLength: 1,
              maxLength: 128
            }
          }
        },
        required: [
          "connected",
          "adapter",
          "epoch",
          "protocol",
          "registryHash",
          "operations"
        ],
        additionalProperties: true,
        maxProperties: 32
      }
    },
    {
      id: "subscribe",
      method: "subscribe",
      request: {
        type: "object",
        properties: {
          types: {
            type: "array",
            items: {
              type: "string",
              enum: [
                "transport",
                "object",
                "reset",
                "selection",
                "name",
                "mixer",
                "parameter",
                "structure"
              ]
            },
            maxItems: 8,
            uniqueItems: true
          }
        },
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          subscribed: {
            type: "boolean"
          },
          subscriptionId: {
            type: "string",
            minLength: 1,
            maxLength: 128
          }
        },
        required: [
          "subscribed",
          "subscriptionId"
        ],
        additionalProperties: false
      }
    },
    {
      id: "take-lane.audio-clip.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          takeLaneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTakeLaneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "takeLaneRef",
          "filePath",
          "position",
          "expectedTakeLaneIdentity",
          "expectedCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          filePath: {
            type: "string",
            minLength: 1,
            maxLength: 1024
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "start",
          "length",
          "filePath",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "take-lane.clip.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          takeLaneRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          position: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTakeLaneIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "takeLaneRef",
          "position",
          "length",
          "name",
          "expectedTakeLaneIdentity",
          "expectedCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          start: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          length: {
            type: "number",
            minimum: 0,
            maximum: 1e9
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "start",
          "length",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "take-lane.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTrackIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedTakeLaneCollectionRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "trackRef",
          "expectedTrackIdentity",
          "expectedTakeLaneCollectionRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "index",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "take-lane.rename",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "name",
          "expectedName",
          "expectedObjectIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          renamed: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "renamed",
          "name"
        ],
        additionalProperties: false
      }
    },
    {
      id: "tempo.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          value: {
            type: "number",
            minimum: 20,
            maximum: 999
          },
          expectedTempo: {
            type: "number",
            minimum: 20,
            maximum: 999
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "value",
          "expectedTempo",
          "expectedObjectIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          tempo: {
            type: "number",
            minimum: 20,
            maximum: 999
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "tempo",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.action",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          action: {
            type: "string",
            enum: [
              "jump-in-running-clip"
            ]
          },
          beats: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "action",
          "expectedObjectIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "done"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.create",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          name: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          kind: {
            type: "string",
            enum: [
              "audio",
              "midi"
            ]
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          }
        },
        required: [
          "name",
          "kind",
          "expectedStructureRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 128
          },
          kind: {
            type: "string",
            enum: [
              "audio",
              "midi",
              "group",
              "return",
              "main"
            ]
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "kind",
          "index",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.create-return",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "expectedStructureRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "index",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.delete",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[a-f0-9]{64}$"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          discardChanges: {
            type: "boolean"
          },
          explicitDeletion: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "ref",
          "expectedStructureRevision",
          "expectedObjectIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.delete-return",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          explicitDeletion: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStructureRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          deleted: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "deleted"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.duplicate",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStructureRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStructureRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          objectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            maxLength: 256
          },
          index: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          createdFingerprint: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          ownershipToken: {
            type: "string",
            minLength: 32,
            maxLength: 128,
            pattern: "^[A-Za-z0-9_-]{32,128}$"
          }
        },
        required: [
          "ref",
          "objectIdentity",
          "name",
          "index",
          "createdFingerprint"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.rename",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedName: {
            type: "string",
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedAuthorityRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "name",
          "expectedName",
          "expectedObjectIdentity",
          "expectedAuthorityRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          renamed: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "renamed",
          "name"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.select-instrument",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "done"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          colorIndex: {
            type: "integer",
            minimum: 0,
            maximum: 69
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "track.view.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          collapsed: {
            type: "boolean"
          },
          deviceInsertMode: {
            type: "integer",
            minimum: 0,
            maximum: 8
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          showChains: {
            type: "boolean"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "transaction.group",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          label: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          ops: {
            type: "array",
            items: {
              type: "object",
              properties: {
                operation: {
                  type: "string",
                  minLength: 3,
                  maxLength: 128,
                  pattern: "^[a-z0-9]+(?:[.-][a-z0-9]+)+$"
                },
                args: {
                  type: "object",
                  additionalProperties: true,
                  maxProperties: 64
                }
              },
              required: [
                "operation",
                "args"
              ],
              additionalProperties: false
            },
            minItems: 1
          }
        },
        additionalProperties: false,
        required: [
          "ops"
        ]
      },
      result: {
        type: "object",
        properties: {
          results: {
            type: "array",
            items: {
              type: "object",
              additionalProperties: true,
              maxProperties: 64
            }
          }
        },
        required: [
          "results"
        ],
        additionalProperties: false
      }
    },
    {
      id: "transport.action",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          action: {
            type: "string",
            enum: [
              "start",
              "continue",
              "stop",
              "play-selection",
              "scrub",
              "tap-tempo",
              "nudge-up",
              "nudge-down",
              "re-enable-automation",
              "trigger-session-record",
              "force-link-beat-time",
              "stop-all-clips",
              "back-to-arrangement",
              "jump-by"
            ]
          },
          beatTime: {
            type: "number",
            minimum: -1e6,
            maximum: 1e9
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedRevision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          beats: {
            type: "number",
            minimum: -1e6,
            maximum: 1e6
          }
        },
        required: [
          "setRef",
          "action",
          "expectedObjectIdentity",
          "expectedRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          done: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 1,
            maxLength: 128
          }
        },
        required: [
          "done",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "transport.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          position: {
            type: [
              "number",
              "null"
            ],
            minimum: 0,
            maximum: 1e9
          },
          loopEnabled: {
            type: [
              "boolean",
              "null"
            ]
          },
          loopStart: {
            type: [
              "number",
              "null"
            ],
            minimum: 0,
            maximum: 1e9
          },
          loopLength: {
            type: [
              "number",
              "null"
            ],
            minimum: 0,
            maximum: 1e9
          },
          metronome: {
            type: [
              "boolean",
              "null"
            ]
          },
          punchIn: {
            type: [
              "boolean",
              "null"
            ]
          },
          punchOut: {
            type: [
              "boolean",
              "null"
            ]
          },
          expectedRevision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "expectedRevision",
          "setRef",
          "expectedObjectIdentity"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "tuning.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "setRef"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          tuningSystem: {
            type: "object",
            properties: {
              name: {
                type: "string",
                maxLength: 256
              },
              lowestNote: {
                type: [
                  "object",
                  "null"
                ],
                maxProperties: 8,
                additionalProperties: {
                  type: [
                    "string",
                    "number",
                    "boolean"
                  ],
                  maxLength: 256
                }
              },
              highestNote: {
                type: [
                  "object",
                  "null"
                ],
                maxProperties: 8,
                additionalProperties: {
                  type: [
                    "string",
                    "number",
                    "boolean"
                  ],
                  maxLength: 256
                }
              },
              referencePitch: {
                type: [
                  "object",
                  "null"
                ],
                maxProperties: 8,
                additionalProperties: {
                  type: [
                    "string",
                    "number",
                    "boolean"
                  ],
                  maxLength: 256
                }
              },
              pseudoOctaveInCents: {
                type: [
                  "number",
                  "null"
                ]
              },
              noteTunings: {
                type: "array",
                items: {
                  type: "object",
                  properties: {
                    note: {
                      type: "integer",
                      minimum: 0,
                      maximum: 127
                    },
                    deviation: {
                      type: "number",
                      minimum: -1200,
                      maximum: 1200
                    }
                  },
                  required: [
                    "note",
                    "deviation"
                  ],
                  additionalProperties: false
                },
                maxItems: 128
              }
            },
            required: [
              "name",
              "lowestNote",
              "highestNote",
              "referencePitch",
              "pseudoOctaveInCents",
              "noteTunings"
            ],
            additionalProperties: false
          },
          scale: {
            type: "object",
            properties: {
              rootNote: {
                type: [
                  "integer",
                  "null"
                ],
                minimum: 0,
                maximum: 11
              },
              scaleName: {
                type: [
                  "string",
                  "null"
                ],
                maxLength: 256
              },
              scaleMode: {
                type: [
                  "boolean",
                  "null"
                ]
              },
              scaleIntervals: {
                type: "array",
                items: {
                  type: "integer",
                  minimum: -24,
                  maximum: 24
                },
                maxItems: 32
              }
            },
            required: [
              "rootNote",
              "scaleName",
              "scaleMode",
              "scaleIntervals"
            ],
            additionalProperties: false
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          },
          referencePitch: {
            type: [
              "object",
              "null"
            ],
            properties: {
              frequency: {
                type: "number",
                minimum: 0,
                maximum: 1e5
              },
              indexInOctave: {
                type: "integer",
                minimum: 0,
                maximum: 1024
              },
              octave: {
                type: "integer",
                minimum: -64,
                maximum: 64
              }
            },
            required: [
              "frequency",
              "indexInOctave",
              "octave"
            ],
            additionalProperties: false
          },
          notesInPseudoOctave: {
            type: [
              "integer",
              "null"
            ],
            minimum: 0,
            maximum: 1024
          }
        },
        required: [
          "tuningSystem",
          "scale",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "tuning.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          setRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          name: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          lowestNote: {
            type: "object",
            maxProperties: 8,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean"
              ],
              maxLength: 256
            }
          },
          highestNote: {
            type: "object",
            maxProperties: 8,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean"
              ],
              maxLength: 256
            }
          },
          referencePitch: {
            type: "object",
            maxProperties: 8,
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean"
              ],
              maxLength: 256
            }
          },
          noteTunings: {
            type: "array",
            items: {
              type: "object",
              properties: {
                note: {
                  type: "integer",
                  minimum: 0,
                  maximum: 127
                },
                deviation: {
                  type: "number",
                  minimum: -1200,
                  maximum: 1200
                }
              },
              required: [
                "note",
                "deviation"
              ],
              additionalProperties: false
            },
            minItems: 128,
            maxItems: 128
          },
          rootNote: {
            type: "integer",
            minimum: 0,
            maximum: 11
          },
          scaleName: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          scaleMode: {
            type: "boolean"
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "setRef",
          "expectedObjectIdentity",
          "expectedRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "undo.step.begin",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          label: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          timeoutMs: {
            type: "integer",
            minimum: 1e3,
            maximum: 36e5
          }
        },
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          open: {
            type: "boolean",
            const: true
          },
          stepId: {
            type: "string",
            minLength: 8,
            maxLength: 128
          },
          expiresAt: {
            type: "integer",
            minimum: 0,
            maximum: 9007199254740991
          },
          closedPrevious: {
            type: "boolean"
          }
        },
        required: [
          "open",
          "stepId",
          "expiresAt",
          "closedPrevious"
        ],
        additionalProperties: false
      }
    },
    {
      id: "undo.step.end",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          stepId: {
            type: "string",
            minLength: 8,
            maxLength: 128
          }
        },
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          closed: {
            type: "boolean"
          },
          stepId: {
            type: [
              "string",
              "null"
            ],
            maxLength: 128
          },
          reason: {
            type: "string",
            enum: [
              "ended",
              "not-open",
              "other-step"
            ]
          }
        },
        required: [
          "closed",
          "stepId",
          "reason"
        ],
        additionalProperties: false
      }
    },
    {
      id: "view.control",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          action: {
            type: "string",
            enum: [
              "zoom-in",
              "zoom-out",
              "scroll-left",
              "scroll-right",
              "follow-on",
              "follow-off",
              "collapse-track",
              "expand-track",
              "hide-view",
              "focus-view",
              "browser-toggle"
            ]
          },
          trackRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          view: {
            type: "string",
            minLength: 1,
            maxLength: 64
          }
        },
        required: [
          "action"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          action: {
            type: "string",
            enum: [
              "zoom-in",
              "zoom-out",
              "scroll-left",
              "scroll-right",
              "follow-on",
              "follow-off",
              "collapse-track",
              "expand-track",
              "hide-view",
              "focus-view",
              "browser-toggle"
            ]
          },
          done: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "action",
          "done"
        ],
        additionalProperties: false
      }
    },
    {
      id: "view.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          view: {
            type: "string",
            minLength: 1,
            maxLength: 64
          }
        },
        required: [
          "view"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          view: {
            type: "string",
            minLength: 1,
            maxLength: 64
          },
          visible: {
            type: "boolean",
            const: true
          }
        },
        required: [
          "view",
          "visible"
        ],
        additionalProperties: false
      }
    },
    {
      id: "wavetable.modulation.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          targetIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          parameterRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          source: {
            type: "integer",
            minimum: 0,
            maximum: 1e3
          },
          value: {
            type: "number",
            minimum: -1,
            maximum: 1
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "source",
          "value",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          },
          targetIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          value: {
            type: "number",
            minimum: -1,
            maximum: 1
          },
          prior: {
            type: "number",
            minimum: -1,
            maximum: 1
          }
        },
        required: [
          "changed",
          "revision",
          "targetIndex",
          "value",
          "prior"
        ],
        additionalProperties: false
      }
    },
    {
      id: "wavetable.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          oscillator1WavetableCategory: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          oscillator1WavetableIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          oscillator2WavetableCategory: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          oscillator2WavetableIndex: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          oscillator1EffectMode: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          oscillator2EffectMode: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          filterRouting: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          unisonMode: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          unisonVoiceCount: {
            type: "integer",
            minimum: 0,
            maximum: 1e5
          },
          expectedObjectIdentity: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64,
            pattern: "^[0-9a-f]{64}$"
          }
        },
        required: [
          "ref",
          "expectedObjectIdentity",
          "expectedStateRevision"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          changed: {
            type: "boolean",
            const: true
          },
          revision: {
            type: "integer",
            minimum: 1,
            maximum: 9007199254740991
          }
        },
        required: [
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "willington.device.read",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          kind: {
            type: "string",
            enum: [
              "macro-name",
              "macro-mapping",
              "variation-name",
              "selector-zone",
              "key-zone",
              "velocity-zone"
            ]
          },
          macroIndex: {
            type: "integer",
            minimum: 0,
            maximum: 15
          },
          targetRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          }
        },
        required: [
          "ref",
          "kind"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          state: {
            type: "object",
            properties: {},
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean",
                "null"
              ],
              maxLength: 2048
            },
            maxProperties: 32
          },
          stateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64
          }
        },
        required: [
          "state",
          "stateRevision"
        ],
        additionalProperties: false
      }
    },
    {
      id: "willington.device.set",
      method: "invoke",
      request: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          kind: {
            type: "string",
            enum: [
              "macro-name",
              "macro-mapping",
              "variation-name",
              "selector-zone",
              "key-zone",
              "velocity-zone"
            ]
          },
          macroIndex: {
            type: "integer",
            minimum: 0,
            maximum: 15
          },
          targetRef: {
            type: "string",
            minLength: 1,
            maxLength: 256
          },
          expectedStateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64
          },
          next: {
            type: "object",
            properties: {
              name: {
                type: "string",
                minLength: 1,
                maxLength: 256
              },
              mapping: {
                type: [
                  "object",
                  "null"
                ],
                properties: {
                  index: {
                    type: "integer",
                    minimum: 0,
                    maximum: 15
                  },
                  minimum: {
                    type: "number"
                  },
                  maximum: {
                    type: "number"
                  },
                  kind: {
                    type: "string",
                    enum: [
                      "continuous",
                      "enum",
                      "boolean"
                    ]
                  }
                },
                required: [
                  "index",
                  "minimum",
                  "maximum",
                  "kind"
                ],
                additionalProperties: false
              },
              parameterValue: {
                type: "number"
              },
              minimum: {
                type: "integer",
                minimum: 0,
                maximum: 127
              },
              maximum: {
                type: "integer",
                minimum: 0,
                maximum: 127
              },
              fadeMinimum: {
                type: "integer",
                minimum: 0,
                maximum: 127
              },
              fadeMaximum: {
                type: "integer",
                minimum: 0,
                maximum: 127
              }
            },
            additionalProperties: false
          }
        },
        required: [
          "ref",
          "kind",
          "expectedStateRevision",
          "next"
        ],
        additionalProperties: false
      },
      result: {
        type: "object",
        properties: {
          state: {
            type: "object",
            properties: {},
            additionalProperties: {
              type: [
                "string",
                "number",
                "boolean",
                "null"
              ],
              maxLength: 2048
            },
            maxProperties: 32
          },
          stateRevision: {
            type: "string",
            minLength: 64,
            maxLength: 64
          },
          changed: {
            type: "boolean"
          },
          revision: {
            type: "integer",
            minimum: 0
          }
        },
        required: [
          "state",
          "stateRevision",
          "changed",
          "revision"
        ],
        additionalProperties: false
      }
    }
  ]
};

// apps/live-extension/src/registry.ts
var registry = ableton_live_v1_operations_default;
var byId = new Map(registry.operations.map((operation) => [operation.id, operation]));
function canonicalRegistry(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalRegistry).join(",")}]`;
  const object = value;
  return `{${Object.keys(object).sort().map((key) => `${JSON.stringify(key)}:${canonicalRegistry(object[key])}`).join(",")}}`;
}
var REGISTRY_HASH = (0, import_node_crypto2.createHash)("sha256").update(canonicalRegistry(registry)).digest("hex");
function hasOperation(id) {
  return byId.has(id);
}
function matchesType(value, type) {
  if (type === "null") return value === null;
  if (type === "object") return typeof value === "object" && value !== null && !Array.isArray(value);
  if (type === "array") return Array.isArray(value);
  if (type === "integer") return typeof value === "number" && Number.isSafeInteger(value);
  if (type === "number") return typeof value === "number" && Number.isFinite(value);
  return typeof value === type;
}
function validate(schema, value, path = "$") {
  const declared = Array.isArray(schema.type) ? schema.type : [schema.type];
  if (!declared.some((type) => matchesType(value, type))) throw new Error(`${path} does not match registry type`);
  if (schema.const !== void 0 && value !== schema.const) throw new Error(`${path} does not match registry constant`);
  if (Array.isArray(schema.enum) && !schema.enum.some((item) => item === value)) throw new Error(`${path} is outside registry enum`);
  if (typeof value === "string") {
    if (typeof schema.minLength === "number" && value.length < schema.minLength) throw new Error(`${path} is shorter than registry minimum`);
    if (typeof schema.maxLength === "number" && value.length > schema.maxLength) throw new Error(`${path} exceeds registry maximum`);
    if (typeof schema.pattern === "string" && !new RegExp(schema.pattern).test(value)) throw new Error(`${path} does not match registry pattern`);
  }
  if (typeof value === "number" && (typeof schema.minimum === "number" && value < schema.minimum || typeof schema.maximum === "number" && value > schema.maximum)) throw new Error(`${path} is outside registry numeric bounds`);
  if (Array.isArray(value)) {
    if (typeof schema.minItems === "number" && value.length < schema.minItems) throw new Error(`${path} is below registry item bound`);
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems) throw new Error(`${path} exceeds registry item bound`);
    value.forEach((item, index) => validate(schema.items, item, `${path}[${index}]`));
  }
  if (typeof value === "object" && value !== null && !Array.isArray(value)) {
    const object = value;
    const properties = schema.properties ?? {};
    if (typeof schema.maxProperties === "number" && Object.keys(object).length > schema.maxProperties) throw new Error(`${path} exceeds registry property bound`);
    for (const required of schema.required ?? []) if (!(required in object)) throw new Error(`${path}.${required} is required by registry`);
    for (const key of Object.keys(object)) {
      if (key in properties) validate(properties[key], object[key], `${path}.${key}`);
      else if (schema.additionalProperties === false) throw new Error(`${path}.${key} is not allowed by registry`);
      else if (typeof schema.additionalProperties === "object") validate(schema.additionalProperties, object[key], `${path}.${key}`);
    }
  }
}
function validateRequest(id, value) {
  const operation = byId.get(id);
  if (!operation) throw new Error(`operation is not in the registry: ${id}`);
  validate(operation.request, value, `${id}.request`);
}
function validateResult(id, value) {
  const operation = byId.get(id);
  if (!operation) throw new Error(`operation is not in the registry: ${id}`);
  validate(operation.result, value, `${id}.result`);
}

// apps/live-extension/src/pointing.ts
var import_sdk3 = __toESM(require_dist());
var OBJECT_SCOPES = ["AudioClip", "MidiClip", "AudioTrack", "MidiTrack", "ClipSlot", "Scene", "Simpler", "Sample", "DrumRack"];
var SELECTION_SCOPES = ["ClipSlotSelection", "AudioTrack.ArrangementSelection", "MidiTrack.ArrangementSelection"];
var POINT = "kumi.point";
var POINT_SELECTION = "kumi.point-selection";
function where(located) {
  return { kind: located.kind, path: located.path, name: located.name, trail: located.trail };
}
function locateObject(context, object) {
  const found = locate(context, object);
  if (found) return found;
  const parent = object.parent;
  const owner = parent ? locate(context, parent) : void 0;
  return owner ? { ...owner, kind: owner.kind === "device" ? "sample" : owner.kind } : void 0;
}
function pointedAt(context, argument) {
  const selection = argument;
  if (selection && Array.isArray(selection.selected_lanes)) {
    const lanes = selection.selected_lanes.map((handle) => locateObject(context, context.getObjectFromHandle(handle, import_sdk3.DataModelObject))).filter((item) => !!item);
    const from = Number(selection.time_selection_start);
    const to = Number(selection.time_selection_end);
    return { kind: "arrangement_selection", lanes: lanes.map(where), timeSelection: { fromBeat: from, toBeat: to }, name: lanes.map((lane) => lane.name).join(", "), trail: lanes.map((lane) => lane.name) };
  }
  if (selection && Array.isArray(selection.selected_clip_slots)) {
    const slots = selection.selected_clip_slots.map((handle) => locateObject(context, context.getObjectFromHandle(handle, import_sdk3.DataModelObject))).filter((item) => !!item);
    return { kind: "clip_slot_selection", slots: slots.map(where), name: `${slots.length} clip slots`, trail: [...new Set(slots.map((slot) => slot.trail[0] ?? ""))] };
  }
  const object = context.getObjectFromHandle(argument, import_sdk3.DataModelObject);
  const located = locateObject(context, object);
  return located ? where(located) : void 0;
}
async function registerPointing(context, onPointed, log2 = () => void 0) {
  const handle = (argument) => {
    try {
      const payload = pointedAt(context, argument);
      if (payload) onPointed({ ...payload, at: Date.now() });
      else log2("pointed at something Kumi couldn't find in the Set");
    } catch (error) {
      log2(`pointing failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  };
  context.commands.registerCommand(POINT, handle);
  context.commands.registerCommand(POINT_SELECTION, handle);
  for (const scope of OBJECT_SCOPES) await context.ui.registerContextMenuAction(scope, "Ask Kumi about this", POINT);
  for (const scope of SELECTION_SCOPES) await context.ui.registerContextMenuAction(scope, "Ask Kumi about this selection", POINT_SELECTION);
}

// apps/live-extension/src/server.ts
var import_node_net = require("node:net");
var REQUIRED = ["version", "id", "method", "nonce", "sequence", "bridgeEpoch", "connectionChallenge", "deadlineMs", "mac"];
var OPTIONAL = ["operation", "args", "ref", "transactionId", "idempotencyKey", "stateDigest", "ownershipToken"];
var ID = /^[A-Za-z0-9_-]{1,128}$/;
var ExtensionServer = class {
  constructor(secret, handlers, log2 = () => void 0) {
    this.secret = secret;
    this.handlers = handlers;
    this.log = log2;
    this.server = (0, import_node_net.createServer)((socket) => this.accept(socket));
  }
  secret;
  handlers;
  log;
  bridgeEpoch = token(24);
  /** This process's own epoch for events; the host checks references against the Remote Script's. */
  epoch = 1 + Math.floor(Math.random() * (Number.MAX_SAFE_INTEGER - 1));
  server;
  connections = /* @__PURE__ */ new Set();
  // Changes to the Set go one at a time, in the order they arrive.
  queue = Promise.resolve();
  listen(port = 0) {
    return new Promise((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(port, "127.0.0.1", () => {
        const address = this.server.address();
        resolve(typeof address === "object" && address ? address.port : port);
      });
    });
  }
  close() {
    for (const connection of this.connections) connection.socket.destroy();
    return new Promise((resolve) => this.server.close(() => resolve()));
  }
  get clients() {
    return this.connections.size;
  }
  /** An event for every connected host, each numbered in its connection's own sequence. */
  broadcast(type, payload, ref) {
    for (const connection of this.connections) {
      connection.eventSequence += 1;
      const event = { epoch: this.epoch, sequence: connection.eventSequence, type, ...ref ? { ref } : {}, payload };
      this.send(connection, { version: LOOPBACK_PROTOCOL, id: "event", ok: true, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, result: { event } });
    }
  }
  accept(socket) {
    socket.setNoDelay(true);
    const connection = { socket, challenge: token(24), lastSequence: 0, eventSequence: 0, pieces: [], buffered: 0 };
    this.connections.add(connection);
    socket.on("data", (chunk) => this.onData(connection, chunk));
    socket.on("error", () => void 0);
    socket.on("close", () => this.connections.delete(connection));
    this.send(connection, { version: LOOPBACK_PROTOCOL, id: "hello", ok: true, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, result: { protocol: LIVE_PROTOCOL, registryHash: REGISTRY_HASH, maxDeadlineMs: 6e5 } });
  }
  send(connection, payload) {
    if (connection.socket.destroyed) return;
    connection.socket.write(`${JSON.stringify(signed(this.secret, payload))}
`);
  }
  onData(connection, chunk) {
    connection.pieces.push(chunk);
    connection.buffered += chunk.length;
    if (chunk.indexOf(10) < 0) {
      if (connection.buffered > MAX_FRAME_BYTES) connection.socket.destroy();
      return;
    }
    let buffer = Buffer.concat(connection.pieces);
    connection.pieces = [];
    connection.buffered = 0;
    for (let index = buffer.indexOf(10); index >= 0; index = buffer.indexOf(10)) {
      const line = buffer.subarray(0, index);
      buffer = buffer.subarray(index + 1);
      if (line.length > 0) this.onFrame(connection, line.toString("utf8")).catch((error) => {
        this.log(`a request failed unexpectedly: ${error instanceof Error ? error.message : String(error)}`);
        this.error(connection, "invalid", "request failed");
      });
    }
    if (buffer.length > 0) {
      connection.pieces.push(buffer);
      connection.buffered = buffer.length;
    }
  }
  error(connection, id, message) {
    this.send(connection, { version: LOOPBACK_PROTOCOL, id: typeof id === "string" && ID.test(id) ? id : "invalid", ok: false, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, error: message });
  }
  async onFrame(connection, text) {
    let request;
    try {
      request = JSON.parse(text);
    } catch {
      this.error(connection, "invalid", "malformed request");
      return;
    }
    if (typeof request !== "object" || request === null || Array.isArray(request)) {
      this.error(connection, "invalid", "malformed request");
      return;
    }
    const keys = Object.keys(request);
    const now = Date.now();
    if (!REQUIRED.every((key) => keys.includes(key)) || keys.some((key) => !REQUIRED.includes(key) && !OPTIONAL.includes(key)) || request.version !== LOOPBACK_PROTOCOL || request.bridgeEpoch !== this.bridgeEpoch || request.connectionChallenge !== connection.challenge || typeof request.id !== "string" || !ID.test(request.id) || typeof request.deadlineMs !== "number" || !(request.deadlineMs >= now && request.deadlineMs <= now + 6e5) || typeof request.nonce !== "string" || request.nonce.length < 16 || request.nonce.length > 256 || typeof request.sequence !== "number" || !Number.isSafeInteger(request.sequence) || request.sequence <= connection.lastSequence) {
      this.error(connection, request.id, "invalid request");
      return;
    }
    if (!verify(this.secret, request)) {
      this.error(connection, request.id, "authentication or replay check failed");
      return;
    }
    connection.lastSequence = request.sequence;
    const id = request.id;
    try {
      let result;
      if (request.method === "status") result = this.handlers.status();
      else if (request.method === "invoke" || request.method === "mutate") {
        const operation = String(request.operation ?? "");
        const args = request.args ?? {};
        if (!hasOperation(operation) || !this.handlers.operations.includes(operation)) throw new Error(`operation unavailable on the Extensions channel: ${operation}`);
        validateRequest(operation, args);
        const deadline = request.deadlineMs;
        const run = this.queue.then(() => {
          if (Date.now() > deadline) throw new Error("the request's deadline passed before Live could start it; nothing changed");
          return this.handlers.invoke(operation, args);
        });
        this.queue = run.catch(() => void 0);
        result = await run;
        validateResult(operation, result);
      } else throw new Error(`method unavailable on the Extensions channel: ${String(request.method)}`);
      this.send(connection, { version: LOOPBACK_PROTOCOL, id, ok: true, bridgeEpoch: this.bridgeEpoch, connectionChallenge: connection.challenge, result });
    } catch (error) {
      const message = error instanceof Error ? error.message : error === void 0 || error === null ? "Live refused it without giving a reason" : String(error);
      this.log(`request ${id} failed: ${message}`);
      this.error(connection, id, message.slice(0, 1024));
    }
  }
};

// apps/live-extension/src/extension.ts
var VERSION = true ? "1.0.0" : "0.0.0";
var OPERATION_IDS = ["status", ...Object.keys(OPERATIONS), "transaction.group"];
function log(line) {
  console.log(`[kumi] ${line}`);
}
function secretIn(storage) {
  const path = (0, import_node_path2.join)(storage, "secret");
  if ((0, import_node_fs3.existsSync)(path)) {
    const value2 = (0, import_node_fs3.readFileSync)(path, "utf8").trim();
    if (value2.length >= 32) return value2;
  }
  const value = token(32);
  (0, import_node_fs3.writeFileSync)(path, `${value}
`, { mode: 384 });
  return value;
}
function writeEndpoint(storage, endpoint) {
  const path = (0, import_node_path2.join)(storage, "endpoint.json");
  const partial = `${path}.${process.pid}.tmp`;
  (0, import_node_fs3.writeFileSync)(partial, `${JSON.stringify(endpoint)}
`, { mode: 384 });
  try {
    (0, import_node_fs3.chmodSync)(partial, 384);
  } catch {
  }
  (0, import_node_fs3.renameSync)(partial, path);
}
var running;
async function deactivate() {
  const current = running;
  running = void 0;
  if (!current) return;
  clearInterval(current.watchdog);
  try {
    (0, import_node_fs3.rmSync)((0, import_node_path2.join)(current.storage, "endpoint.json"), { force: true });
  } catch {
  }
  await current.server.close();
}
function activate(activation) {
  const context = (0, import_sdk4.initialize)(activation, "1.0.0");
  const storage = context.environment.storageDirectory ?? (0, import_node_path2.join)((0, import_node_os.tmpdir)(), "kumi-live-extension");
  const temp = context.environment.tempDirectory ?? (0, import_node_path2.join)(storage, "tmp");
  (0, import_node_fs3.mkdirSync)(storage, { recursive: true, mode: 448 });
  const environment = { rendersDir: (0, import_node_path2.join)(temp, "renders") };
  const secret = secretIn(storage);
  const group = transactionGroup(validateRequest);
  const server = new ExtensionServer(secret, {
    operations: OPERATION_IDS,
    status: () => ({
      connected: true,
      adapter: "extension",
      epoch: server.epoch,
      protocol: "ableton-live/v1",
      registryHash: REGISTRY_HASH,
      operations: OPERATION_IDS,
      capabilities: [],
      provenance: "real-live",
      environment: { liveVersion: null, os: process.platform, api: "Live Extensions SDK" },
      extension: { version: VERSION, apiVersion: activation.hostApiVersion, pid: process.pid, storageDirectory: storage, tempDirectory: temp }
    }),
    invoke: async (operation, args) => {
      if (operation === "transaction.group") return group(context, args, environment);
      const run = OPERATIONS[operation];
      if (!run) throw new Error(`operation unavailable on the Extensions channel: ${operation}`);
      return run(context, args, environment);
    }
  }, log);
  void server.listen().then((port) => {
    writeEndpoint(storage, { version: 1, host: "127.0.0.1", port, pid: process.pid, extensionVersion: VERSION, registryHash: REGISTRY_HASH, apiVersion: activation.hostApiVersion, startedAt: Date.now() });
    log(`listening on 127.0.0.1:${port}; storage ${storage}; temp ${temp}`);
  }, (error) => log(`couldn't listen: ${error instanceof Error ? error.message : String(error)}`));
  void registerPointing(context, (payload) => server.broadcast("pointed", payload), log).catch((error) => log(`right-click actions unavailable: ${error instanceof Error ? error.message : String(error)}`));
  let misses = 0;
  const watchdog = process.env.KUMI_LAUNCHED_HOST === "1" ? setInterval(() => {
    try {
      void context.application.song.tempo;
      misses = 0;
    } catch {
      misses += 1;
      if (misses < 24) return;
      clearInterval(watchdog);
      log("Live is gone; stopping");
      try {
        (0, import_node_fs3.rmSync)((0, import_node_path2.join)(storage, "endpoint.json"), { force: true });
      } catch {
      }
      void server.close().finally(() => process.exit(0));
    }
  }, 5e3) : void 0;
  watchdog?.unref();
  running = { server, watchdog, storage };
}
// Annotate the CommonJS export names for ESM import in node:
0 && (module.exports = {
  activate,
  deactivate
});
