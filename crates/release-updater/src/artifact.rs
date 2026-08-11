use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest as _, Sha256};

use crate::ArtifactDescriptor;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[derive(Debug)]
pub enum ArtifactError {
    Io(io::Error),
    Size { expected: u64, actual: u64 },
    Sha256,
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "cannot read update artifact: {error}"),
            Self::Size { expected, actual } => {
                write!(formatter, "artifact size is {actual}, expected {expected}")
            }
            Self::Sha256 => write!(formatter, "artifact SHA-256 does not match signed metadata"),
        }
    }
}

impl std::error::Error for ArtifactError {}

impl From<io::Error> for ArtifactError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn sha256_hex(path: &Path) -> Result<String, ArtifactError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(lower_hex(&digest.finalize()))
}

pub fn verify_artifact(path: &Path, descriptor: &ArtifactDescriptor) -> Result<(), ArtifactError> {
    let actual = path.metadata()?.len();
    if actual != descriptor.size {
        return Err(ArtifactError::Size {
            expected: descriptor.size,
            actual,
        });
    }
    if !sha256_hex(path)?.eq_ignore_ascii_case(&descriptor.sha256) {
        return Err(ArtifactError::Sha256);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn artifact_requires_exact_size_and_hash() {
        let path =
            std::env::temp_dir().join(format!("release-updater-artifact-{}", std::process::id()));
        fs::write(&path, b"abc").unwrap();
        let descriptor = ArtifactDescriptor {
            name: "a".to_owned(),
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned(),
        };
        verify_artifact(&path, &descriptor).unwrap();
        fs::write(&path, b"abd").unwrap();
        assert!(matches!(
            verify_artifact(&path, &descriptor),
            Err(ArtifactError::Sha256)
        ));
        let _ = fs::remove_file(path);
    }
}
