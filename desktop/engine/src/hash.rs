use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use md5::{Digest, Md5};

pub fn md5_bytes(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn md5_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors_match_commons_codec() {
        // DigestUtils.md5Hex of these inputs.
        assert_eq!(md5_bytes(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_bytes(b"hello"), "5d41402abc4b2a76b9719d911017c592");
    }
}
