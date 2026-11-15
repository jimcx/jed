//! Files packed into one zstd stream: how `build.rs` embeds the bundled dictionary and the
//! notices (`assets.rs` unpacks them). `build.rs` includes this file with `#[path]`, so it
//! uses nothing but `std` and `zstd`.
#![allow(dead_code)] // build.rs packs, the app unpacks

use std::io;

const LEVEL: i32 = 19;

/// `files` (name, bytes) as one zstd stream of `u16 name length, name, u32 length, bytes` records.
pub fn pack(files: &[(String, Vec<u8>)]) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    for (name, data) in files {
        raw.extend((name.len() as u16).to_le_bytes());
        raw.extend(name.as_bytes());
        raw.extend((data.len() as u32).to_le_bytes());
        raw.extend(data);
    }
    zstd::encode_all(raw.as_slice(), LEVEL)
}

/// The files `pack` made `blob` from, in order.
pub fn unpack(blob: &[u8]) -> io::Result<Vec<(String, Vec<u8>)>> {
    let raw = zstd::decode_all(blob)?;
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "corrupt bundle");
    let take = |at: &mut usize, n: usize| {
        let bytes = raw.get(*at..*at + n).ok_or_else(bad);
        *at += n;
        bytes
    };
    let (mut at, mut files) = (0, Vec::new());
    while at < raw.len() {
        let n = u16::from_le_bytes(take(&mut at, 2)?.try_into().map_err(|_| bad())?) as usize;
        let name = String::from_utf8(take(&mut at, n)?.to_vec()).map_err(|_| bad())?;
        let n = u32::from_le_bytes(take(&mut at, 4)?.try_into().map_err(|_| bad())?) as usize;
        files.push((name, take(&mut at, n)?.to_vec()));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trips_and_shrinks() {
        let files = vec![
            ("a.txt".to_string(), "colour ".repeat(500).into_bytes()),
            ("empty".to_string(), Vec::new()),
            ("é.bin".to_string(), vec![0, 255, 7]),
        ];
        let blob = pack(&files).unwrap();
        assert!(blob.len() < 200, "{}", blob.len());
        assert_eq!(unpack(&blob).unwrap(), files);
    }

    #[test]
    fn corrupt_input_is_an_error() {
        assert!(unpack(b"not zstd").is_err());
        let truncated = zstd::encode_all(&[5u8, 0, b'a'][..], 3).unwrap();
        assert!(unpack(&truncated).is_err());
    }
}
