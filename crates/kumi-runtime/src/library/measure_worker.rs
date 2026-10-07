//! The library's measurement worker and its pool.
//! Persistent native child processes retain worker isolation and can be killed on decoder timeouts.
use super::{
    features::MeasureOptions,
    learn::{learn_sound, SoundEntry},
    store::Entry,
};
use kumi_common::js::{json::stringify, string::head};
use serde::{Deserialize, Serialize};
use std::{cell::Cell, io, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasureJob {
    pub id: u64,
    pub path: String,
    pub relative: String,
    pub size: u64,
    pub mtime: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct MeasureReply {
    pub id: u64,
    pub entry: SoundEntry,
}
impl MeasureJob {
    pub fn failed(&self, why: &str) -> SoundEntry {
        SoundEntry {
            file: Entry { path: self.path.clone(), size: self.size, mtime: self.mtime, gone: None },
            error: Some(head(why, 160)),
            ..Default::default()
        }
    }
    pub async fn measure(&self) -> SoundEntry {
        learn_sound(
            &self.path,
            &self.relative,
            self.size,
            self.mtime,
            MeasureOptions { start: self.start, seconds: self.seconds, signal: None },
        )
        .await
        .unwrap_or_else(|error| self.failed(&error.to_string()))
    }
}
/// Workers are installed beside the main binary; the override also supports embedders.
pub fn worker_binary(name: &str, variable: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(variable) {
        return path.into();
    }
    let filename = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent().map(|folder| {
                // Cargo's integration tests are one directory below the binaries they exercise.
                let folder = if folder.file_name().is_some_and(|n| n == "deps") { folder.parent().unwrap_or(folder) } else { folder };
                folder.join(&filename)
            })
        })
        .unwrap_or_else(|| filename.into())
}
/// A Job Object holding a worker, and so what it starts: ended together.
#[cfg(windows)]
struct Job(*mut std::ffi::c_void);
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const std::ffi::c_void, name: *const u16) -> *mut std::ffi::c_void;
    fn SetInformationJobObject(job: *mut std::ffi::c_void, class: u32, information: *const std::ffi::c_void, length: u32) -> i32;
    fn QueryInformationJobObject(
        job: *mut std::ffi::c_void,
        class: u32,
        information: *mut std::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: *mut std::ffi::c_void, process: *mut std::ffi::c_void) -> i32;
    fn TerminateJobObject(job: *mut std::ffi::c_void, code: u32) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}
/// JOBOBJECT_EXTENDED_LIMIT_INFORMATION, for JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct JobLimits {
    per_process_user_time: i64,
    per_job_user_time: i64,
    flags: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}
/// JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, for how many of the job's processes are still running.
#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct JobAccounting {
    times: [i64; 4],
    page_faults: u32,
    total: u32,
    active: u32,
    terminated: u32,
}
#[cfg(windows)]
impl Job {
    fn holding(child: &Child) -> Option<Job> {
        let process = child.raw_handle()?;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            // Closing the job ends what's in it, so a Kumi that crashes leaves no worker or ffmpeg behind either.
            let limits = JobLimits { flags: 0x2000, ..Default::default() };
            SetInformationJobObject(job, 9, &limits as *const JobLimits as *const _, std::mem::size_of::<JobLimits>() as u32);
            if AssignProcessToJobObject(job, process) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Job(job))
        }
    }
    fn end(&self) {
        unsafe { TerminateJobObject(self.0, 1) };
    }
    /// How many of its processes are still running: they end a moment after the job is told to end.
    fn running(&self) -> u32 {
        let mut accounting = JobAccounting::default();
        let read = unsafe {
            QueryInformationJobObject(
                self.0,
                1,
                &mut accounting as *mut JobAccounting as *mut _,
                std::mem::size_of::<JobAccounting>() as u32,
                std::ptr::null_mut(),
            )
        };
        if read == 0 {
            0
        } else {
            accounting.active
        }
    }
}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
// A kernel handle: any thread may use it.
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}
struct Worker {
    child: Child,
    input: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    /// The worker's temporary folder (its converted copies): removed once it has stopped.
    temp: PathBuf,
    #[cfg(windows)]
    job: Option<Job>,
    stopped: bool,
}
impl Worker {
    fn spawn(binary: &std::path::Path) -> io::Result<Self> {
        let temp = std::env::temp_dir().join(format!("kumi-measure-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir(&temp)?;
        let mut command = Command::new(binary);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
        command.env("TMPDIR", &temp).env("TMP", &temp).env("TEMP", &temp);
        // A group of its own, so what it starts (ffmpeg, afconvert) ends with it.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&temp);
                return Err(error);
            }
        };
        #[cfg(windows)]
        let job = Job::holding(&child);
        let input = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Ok(Self {
            child,
            input,
            lines,
            temp,
            #[cfg(windows)]
            job,
            stopped: false,
        })
    }
    /// End the worker and everything it started. Before it's waited for, its group (or job) is still its own.
    fn end_all(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.end();
        }
        let _ = self.child.start_kill();
    }
    async fn run(&mut self, job: &MeasureJob) -> io::Result<SoundEntry> {
        self.input.write_all(format!("{}\n", stringify(&serde_json::to_value(job)?)).as_bytes()).await?;
        self.input.flush().await?;
        while let Some(line) = self.lines.next_line().await? {
            let reply: MeasureReply = serde_json::from_str(&line)?;
            if reply.id == job.id {
                return Ok(reply.entry);
            }
        }
        Err(io::Error::new(io::ErrorKind::UnexpectedEof, "measurement worker exited"))
    }
    async fn stop(&mut self) {
        self.end_all();
        let _ = self.child.wait().await;
        // The worker has ended; what it started may hold its files a moment longer.
        #[cfg(windows)]
        if let Some(job) = &self.job {
            for _ in 0..50 {
                if job.running() == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        self.stopped = true;
        remove_folder(&self.temp).await;
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if !self.stopped {
            self.end_all();
            let _ = std::fs::remove_dir_all(&self.temp);
        }
    }
}
/// A stopped worker's folder, removed. On Windows a file in it can stay held a moment after its process has ended (an
/// antivirus scan, say), so it's tried again for about a second.
async fn remove_folder(folder: &std::path::Path) {
    for attempt in 0..10 {
        match tokio::fs::remove_dir_all(folder).await {
            Err(error) if cfg!(windows) && error.kind() != io::ErrorKind::NotFound && attempt < 9 => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            _ => return,
        }
    }
}
/// Workers' folders another Kumi left (it crashed, or Windows held a file past every try): each is named for the
/// process that made it, and goes once that process has ended, or its pid has gone to a process started after it.
fn sweep_left_folders() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.strip_prefix("kumi-measure-")).and_then(|rest| rest.split('-').next()) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if pid == std::process::id() {
            continue;
        }
        let made = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.created().or_else(|_| meta.modified()).ok())
            .map(|time| time.duration_since(std::time::UNIX_EPOCH).map(|since| since.as_millis() as i64).unwrap_or(0));
        let gone = match kumi_common::process::started_at_ms(pid) {
            // A second's slack for how finely a system keeps start times.
            Some(started) => made.is_some_and(|made| started > made + 1000),
            None => !super::state::alive(pid as f64),
        };
        if gone {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
pub struct MeasurePool {
    slots: Vec<Mutex<Option<Worker>>>,
    timeout: Duration,
    binary: PathBuf,
    next: Cell<u64>,
}
impl MeasurePool {
    pub fn new(size: usize) -> Self {
        Self::with_worker(size, worker_binary("kumi-library-measure", "KUMI_LIBRARY_MEASURE_BIN"), Duration::from_millis(90000))
    }
    pub fn with_worker(size: usize, binary: PathBuf, timeout: Duration) -> Self {
        if size > 0 {
            sweep_left_folders();
        }
        Self { slots: (0..size).map(|_| Mutex::new(None)).collect(), timeout, binary, next: Cell::new(0) }
    }
    pub async fn run(&self, slot: usize, mut job: MeasureJob) -> SoundEntry {
        if self.slots.is_empty() {
            return job.measure().await;
        }
        job.id = self.next.get();
        self.next.set(job.id.wrapping_add(1));
        let mut held = self.slots[slot % self.slots.len()].lock().await;
        if held.is_none() {
            match Worker::spawn(&self.binary) {
                Ok(worker) => *held = Some(worker),
                Err(_) => return job.failed("Kumi couldn't read it"),
            }
        }
        let worker = held.as_mut().unwrap();
        let result = tokio::time::timeout(self.timeout, worker.run(&job)).await;
        match result {
            Ok(Ok(entry)) => entry,
            result => {
                let why = if result.is_err() { "it took too long to read" } else { "Kumi couldn't read it" };
                worker.stop().await;
                *held = None;
                job.failed(why)
            }
        }
    }
    pub async fn close(&self) {
        for slot in &self.slots {
            if let Some(mut worker) = slot.lock().await.take() {
                worker.stop().await;
            }
        }
    }
}
pub async fn main() -> io::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let job: MeasureJob = serde_json::from_str(&line)?;
        let reply = MeasureReply { id: job.id, entry: job.measure().await };
        output.write_all(format!("{}\n", stringify(&serde_json::to_value(reply)?)).as_bytes()).await?;
        output.flush().await?;
    }
    Ok(())
}

/// One reference file, with immediate cancellation and no learner's per-file timeout.
pub async fn measure_reference(
    job: MeasureJob,
    signal: kumi_common::abort::Signal,
) -> Result<SoundEntry, crate::core::errors::RuntimeError> {
    use crate::core::errors::RuntimeError;
    let mut worker = Worker::spawn(&worker_binary("kumi-library-measure", "KUMI_LIBRARY_MEASURE_BIN"))
        .map_err(|e| RuntimeError::plain(e.to_string()))?;
    let result = tokio::select! {biased;_=signal.cancelled()=>Err(RuntimeError::Aborted),result=worker.run(&job)=>result.map_err(|e|RuntimeError::plain(e.to_string()))};
    worker.stop().await;
    result
}
