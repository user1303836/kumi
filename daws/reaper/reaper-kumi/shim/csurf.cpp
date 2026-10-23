// Kumi's hidden control surface: REAPER calls its virtual functions, and each one forwards to a plain C callback
// that Rust implements (src/csurf.rs). It is registered with plugin_register("csurf_inst", ...), so the user sets
// nothing up in Preferences.
//
// IReaperControlSurface is declared here as reaper_plugin.h declares it (the header needs SWELL outside Windows).
// build.rs checks that its virtual functions are the header's, in the header's order. That declaration is Cockos'
// (Copyright (C) 2006-2015, Cockos Incorporated), under the REAPER SDK's zlib license: ../sdk/LICENSE.

#include <new>

class MediaTrack;

class IReaperControlSurface
{
  public:
    IReaperControlSurface() { }
    virtual ~IReaperControlSurface() { }

    virtual const char *GetTypeString()=0; // simple unique string with only A-Z, 0-9, no spaces or other chars
    virtual const char *GetDescString()=0; // human readable description (can include instance specific info)
    virtual const char *GetConfigString()=0; // string of configuration data

    virtual void CloseNoReset() { } // close without sending "reset" messages, prevent "reset" being sent on destructor


    virtual void Run() { } // called 30x/sec or so.


    // these will be called by the host when states change etc
    virtual void SetTrackListChange() { }
    virtual void SetSurfaceVolume(MediaTrack *trackid, double volume) { }
    virtual void SetSurfacePan(MediaTrack *trackid, double pan) { }
    virtual void SetSurfaceMute(MediaTrack *trackid, bool mute) { }
    virtual void SetSurfaceSelected(MediaTrack *trackid, bool selected) { }
    virtual void SetSurfaceSolo(MediaTrack *trackid, bool solo) { } // trackid==master means "any solo"
    virtual void SetSurfaceRecArm(MediaTrack *trackid, bool recarm) { }
    virtual void SetPlayState(bool play, bool pause, bool rec) { }
    virtual void SetRepeatState(bool rep) { }
    virtual void SetTrackTitle(MediaTrack *trackid, const char *title) { }
    virtual bool GetTouchState(MediaTrack *trackid, int isPan) { return false; }
    virtual void SetAutoMode(int mode) { } // automation mode for current track

    virtual void ResetCachedVolPanStates() { } // good to flush your control states here

    virtual void OnTrackSelection(MediaTrack *trackid) { } // track was selected

    virtual bool IsKeyDown(int key) { return false; } // VK_CONTROL, VK_MENU, VK_SHIFT, etc, whatever makes sense for your surface

    virtual int Extended(int call, void *parm1, void *parm2, void *parm3) { return 0; } // return 0 if unsupported
};

extern "C" {

// The same struct as Callbacks in src/csurf.rs. A null entry is skipped.
struct KumiCsurfCallbacks
{
  void *ctx;
  void (*run)(void *ctx);
  void (*set_track_list_change)(void *ctx);
  void (*set_surface_volume)(void *ctx, MediaTrack *track, double volume);
  void (*set_surface_pan)(void *ctx, MediaTrack *track, double pan);
  void (*set_surface_mute)(void *ctx, MediaTrack *track, bool mute);
  void (*set_surface_selected)(void *ctx, MediaTrack *track, bool selected);
  void (*set_surface_solo)(void *ctx, MediaTrack *track, bool solo);
  void (*set_surface_rec_arm)(void *ctx, MediaTrack *track, bool armed);
  void (*set_play_state)(void *ctx, bool play, bool pause, bool rec);
  void (*set_repeat_state)(void *ctx, bool repeat);
  void (*set_track_title)(void *ctx, MediaTrack *track, const char *title);
  void (*set_auto_mode)(void *ctx, int mode);
  void (*reset_cached_vol_pan_states)(void *ctx);
  void (*on_track_selection)(void *ctx, MediaTrack *track);
  int (*extended)(void *ctx, int call, void *parm1, void *parm2, void *parm3);
};

}

namespace {

class KumiSurface : public IReaperControlSurface
{
  public:
    explicit KumiSurface(const KumiCsurfCallbacks &callbacks) : cb(callbacks) { }

    const char *GetTypeString() { return "KUMI"; }
    const char *GetDescString() { return "Kumi"; }
    const char *GetConfigString() { return ""; }

    void Run() { if (cb.run) cb.run(cb.ctx); }
    void SetTrackListChange() { if (cb.set_track_list_change) cb.set_track_list_change(cb.ctx); }
    void SetSurfaceVolume(MediaTrack *t, double v) { if (cb.set_surface_volume) cb.set_surface_volume(cb.ctx, t, v); }
    void SetSurfacePan(MediaTrack *t, double v) { if (cb.set_surface_pan) cb.set_surface_pan(cb.ctx, t, v); }
    void SetSurfaceMute(MediaTrack *t, bool v) { if (cb.set_surface_mute) cb.set_surface_mute(cb.ctx, t, v); }
    void SetSurfaceSelected(MediaTrack *t, bool v) { if (cb.set_surface_selected) cb.set_surface_selected(cb.ctx, t, v); }
    void SetSurfaceSolo(MediaTrack *t, bool v) { if (cb.set_surface_solo) cb.set_surface_solo(cb.ctx, t, v); }
    void SetSurfaceRecArm(MediaTrack *t, bool v) { if (cb.set_surface_rec_arm) cb.set_surface_rec_arm(cb.ctx, t, v); }
    void SetPlayState(bool play, bool pause, bool rec) { if (cb.set_play_state) cb.set_play_state(cb.ctx, play, pause, rec); }
    void SetRepeatState(bool v) { if (cb.set_repeat_state) cb.set_repeat_state(cb.ctx, v); }
    void SetTrackTitle(MediaTrack *t, const char *title) { if (cb.set_track_title) cb.set_track_title(cb.ctx, t, title); }
    void SetAutoMode(int mode) { if (cb.set_auto_mode) cb.set_auto_mode(cb.ctx, mode); }
    void ResetCachedVolPanStates() { if (cb.reset_cached_vol_pan_states) cb.reset_cached_vol_pan_states(cb.ctx); }
    void OnTrackSelection(MediaTrack *t) { if (cb.on_track_selection) cb.on_track_selection(cb.ctx, t); }
    int Extended(int call, void *p1, void *p2, void *p3) { return cb.extended ? cb.extended(cb.ctx, call, p1, p2, p3) : 0; }

  private:
    KumiCsurfCallbacks cb;
};

}

extern "C" IReaperControlSurface *kumi_csurf_new(const KumiCsurfCallbacks *callbacks)
{
  return callbacks ? new (std::nothrow) KumiSurface(*callbacks) : 0;
}

extern "C" void kumi_csurf_delete(IReaperControlSurface *surface)
{
  delete surface;
}

// Lets the tests drive a surface through its vtable as REAPER would.
extern "C" void kumi_csurf_test_drive(IReaperControlSurface *s, MediaTrack *track)
{
  s->Run();
  s->SetTrackListChange();
  s->SetSurfaceVolume(track, 0.5);
  s->SetSurfaceSelected(track, true);
  s->SetPlayState(true, false, true);
  s->SetTrackTitle(track, "Bass");
  s->OnTrackSelection(track);
  s->Extended(0x00010013, track, 0, 0);
  (void)s->GetTypeString();
}
