//! Kumi Ears records audio with Live's beat phase and position and passes the sound on, holding it back only for a
//! candidate Kumi compares on its own.

use crate::devices::amxd::{encode_amxd, DeviceType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const EARS_VERSION: u32 = 4;
pub const EARS_NAME: &str = "Kumi Ears";
/// In a folder named for its kind: Live's Browser item says nothing else, and an item that might be an instrument
/// doesn't load onto a track that has one (Live would replace it), so on a MIDI track it would never go.
pub const EARS_ITEM: &str = "user_library/Kumi/Audio Effects/Kumi Ears";
pub const KUMI_PORTS: [u16; 5] = [47290, 47291, 47292, 47293, 47294];
pub const DEVICE_PORT_BASE: u16 = 47300;
pub const DEVICE_PORTS: u16 = 600;
pub const EARS_CHANNELS: usize = 4;
/// The device's Max v8 code, held inside its codebox.
pub fn ears_code() -> String {
    ears_patcher()["patcher"]["boxes"]
        .as_array()
        .expect("device boxes")
        .iter()
        .find(|entry| entry["box"]["id"] == "obj-code")
        .and_then(|entry| entry["box"]["code"].as_str())
        .expect("device code")
        .to_string()
}
/// The fixed device patcher, including every inlet, outlet, cord and presentation coordinate.
pub fn ears_patcher() -> Value {
    serde_json::from_str(include_str!("device-patcher.json")).expect("Kumi Ears patcher")
}
pub fn ears_file() -> Vec<u8> {
    encode_amxd(DeviceType::AudioEffect, &ears_patcher())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledEars {
    pub file: String,
    pub written: bool,
}
/// Write the device only when missing or different, so an untouched file keeps its modification time. The copy an
/// older Kumi kept straight in the Kumi folder goes.
pub async fn install_ears(user_library: impl AsRef<Path>) -> std::io::Result<InstalledEars> {
    let older = user_library.as_ref().join("Kumi").join(format!("{EARS_NAME}.amxd"));
    if let Ok(bytes) = tokio::fs::read(&older).await {
        let ours = crate::devices::amxd::decode_amxd(&bytes).is_some_and(|device| device.patcher.to_string().contains("---kumiears"));
        if ours {
            let _ = tokio::fs::remove_file(&older).await;
        }
    }
    let folder = user_library.as_ref().join("Kumi").join("Audio Effects");
    let file = folder.join(format!("{EARS_NAME}.amxd"));
    let bytes = ears_file();
    if tokio::fs::read(&file).await.is_ok_and(|current| current == bytes) {
        return Ok(InstalledEars { file: file.to_string_lossy().into_owned(), written: false });
    }
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o755);
    builder.create(&folder).await?;
    let temporary = folder.join(format!(".{}.amxd", uuid::Uuid::new_v4()));
    let result = async {
        use tokio::io::AsyncWriteExt;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o644);
        let mut output = options.open(&temporary).await?;
        output.write_all(&bytes).await?;
        output.flush().await?;
        drop(output);
        tokio::fs::rename(&temporary, &file).await
    }
    .await;
    let _ = tokio::fs::remove_file(temporary).await;
    result?;
    Ok(InstalledEars { file: file.to_string_lossy().into_owned(), written: true })
}
