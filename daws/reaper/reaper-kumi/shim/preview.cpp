// The preview Kumi plays out loud (M5 listening): REAPER's preview_register_t, made here because it embeds the OS's own
// lock (a CRITICAL_SECTION on Windows, a pthread mutex elsewhere), whose size and layout only the platform's headers
// know. REAPER's audio thread takes that lock while it reads the preview, so Kumi takes it to read the position and to
// set the volume. src/listening/audition.rs calls these functions.
//
// The struct is declared here as reaper_plugin.h declares it (the header needs SWELL outside Windows); build.rs checks
// that its fields are the header's, in the header's order. That declaration is Cockos' (Copyright (C) 2006-2015, Cockos
// Incorporated), under the REAPER SDK's zlib license: ../sdk/LICENSE.

#ifdef _WIN32
#include <windows.h>
#else
#include <pthread.h>
#endif
#include <new>

class PCM_source;

typedef struct _REAPER_preview_register_t
{
#ifdef _WIN32
  CRITICAL_SECTION cs;
#else
  pthread_mutex_t mutex;
#endif
  PCM_source *src;
  int m_out_chan; // &1024 means mono, low 10 bits are index of first channel
  double curpos;
  bool loop;
  double volume;

  double peakvol[2];
  void *preview_track; // used for track previews, but only if m_out_chan == -1
} preview_register_t;

namespace {

void lock(preview_register_t *p)
{
#ifdef _WIN32
  EnterCriticalSection(&p->cs);
#else
  pthread_mutex_lock(&p->mutex);
#endif
}

void unlock(preview_register_t *p)
{
#ifdef _WIN32
  LeaveCriticalSection(&p->cs);
#else
  pthread_mutex_unlock(&p->mutex);
#endif
}

}

// A preview of `src` from `curpos` (seconds) at `volume` (a gain), played once, to the hardware output `out_chan`.
// The lock is recursive, as REAPER's own (WDL's) locks are.
extern "C" preview_register_t *kumi_preview_new(PCM_source *src, int out_chan, double curpos, double volume)
{
  preview_register_t *p = new (std::nothrow) preview_register_t();
  if (!p) return 0;
#ifdef _WIN32
  InitializeCriticalSection(&p->cs);
#else
  pthread_mutexattr_t attr;
  pthread_mutexattr_init(&attr);
  pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_RECURSIVE);
  pthread_mutex_init(&p->mutex, &attr);
  pthread_mutexattr_destroy(&attr);
#endif
  p->src = src;
  p->m_out_chan = out_chan;
  p->curpos = curpos;
  p->loop = false;
  p->volume = volume;
  p->peakvol[0] = p->peakvol[1] = 0.0;
  p->preview_track = 0;
  return p;
}

// Where it is playing now (seconds into the source).
extern "C" double kumi_preview_position(preview_register_t *p)
{
  lock(p);
  double at = p->curpos;
  unlock(p);
  return at;
}

// Moves it (a test's REAPER plays it this way).
extern "C" void kumi_preview_set_position(preview_register_t *p, double at)
{
  lock(p);
  p->curpos = at;
  unlock(p);
}

extern "C" void kumi_preview_set_volume(preview_register_t *p, double volume)
{
  lock(p);
  p->volume = volume;
  unlock(p);
}

extern "C" double kumi_preview_volume(preview_register_t *p)
{
  lock(p);
  double volume = p->volume;
  unlock(p);
  return volume;
}

extern "C" PCM_source *kumi_preview_source(preview_register_t *p)
{
  return p->src;
}

extern "C" int kumi_preview_out_chan(preview_register_t *p)
{
  return p->m_out_chan;
}

// Frees it once REAPER no longer plays it (after StopPreview); the source is the caller's.
extern "C" void kumi_preview_delete(preview_register_t *p)
{
  if (!p) return;
#ifdef _WIN32
  DeleteCriticalSection(&p->cs);
#else
  pthread_mutex_destroy(&p->mutex);
#endif
  delete p;
}
