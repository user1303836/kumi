//! Fetching a pinned model file: kept only whole and as pinned, a body longer than the file stopped, and two fetches
//! of the same file in one Kumi downloading it once.

use kumi_common::abort::Signal;
use kumi_runtime::models::{fetch, Pinned};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

/// A server on this computer answering every request with `body`; how many it answered.
fn server(body: Vec<u8>) -> (&'static str, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url: &'static str = Box::leak(format!("http://{}/model.onnx", listener.local_addr().unwrap()).into_boxed_str());
    let served = Arc::new(AtomicUsize::new(0));
    let count = served.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            count.fetch_add(1, Ordering::SeqCst);
            let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    (url, served)
}

fn pinned(url: &'static str, file: &[u8]) -> Pinned {
    let sha256: &'static str = Box::leak(format!("{:x}", Sha256::digest(file)).into_boxed_str());
    Pinned { name: "model.onnx", url, sha256, size: file.len() as u64 }
}

fn leftovers(folder: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "model.onnx")
        .collect()
}

#[tokio::test]
async fn a_pinned_file_is_kept_whole_once_and_a_longer_one_is_stopped() {
    let file: Vec<u8> = (0..50_000u32).map(|n| (n % 251) as u8).collect();
    let folder = tempfile::tempdir().unwrap();
    let to = folder.path().join("model.onnx");
    let signal = Signal::new();
    let say = |_: &str| {};

    // Two fetches of one file: the second waits for the first and finds it there.
    let (url, served) = server(file.clone());
    let wanted = pinned(url, &file);
    let (first, second) = tokio::join!(fetch(&wanted, &to, "a model", &say, &signal), fetch(&wanted, &to, "a model", &say, &signal));
    assert_eq!((first, second), (Ok(()), Ok(())));
    assert_eq!(std::fs::read(&to).unwrap(), file);
    assert_eq!(served.load(Ordering::SeqCst), 1);
    assert!(leftovers(folder.path()).is_empty(), "{:?}", leftovers(folder.path()));

    // A body longer than the pinned file isn't it: stopped, and nothing's kept.
    std::fs::remove_file(&to).unwrap();
    let mut longer = file.clone();
    longer.extend(std::iter::repeat_n(0u8, 10_000));
    let (url, _) = server(longer);
    let refused = fetch(&pinned(url, &file), &to, "a model", &say, &signal).await;
    assert!(refused.as_ref().is_err_and(|why| why.contains("larger")), "{refused:?}");
    assert!(!to.exists());
    assert!(leftovers(folder.path()).is_empty(), "{:?}", leftovers(folder.path()));

    // One that isn't what was pinned: not kept either.
    let mut changed = file.clone();
    changed[7] ^= 1;
    let (url, _) = server(changed);
    let refused = fetch(&pinned(url, &file), &to, "a model", &say, &signal).await;
    assert!(refused.as_ref().is_err_and(|why| why.contains("checksum")), "{refused:?}");
    assert!(!to.exists());
    assert!(leftovers(folder.path()).is_empty(), "{:?}", leftovers(folder.path()));
}
