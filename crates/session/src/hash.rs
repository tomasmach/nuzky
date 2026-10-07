//! Stable FNV-1a for local cache keys and project endpoint names, not authentication.
pub const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash = (*hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_vector_and_chunking() {
        let mut whole = FNV_OFFSET;
        hash_bytes(&mut whole, b"hello");
        assert_eq!(whole, 0xa430d84680aabd0b);
        let mut chunks = FNV_OFFSET;
        hash_bytes(&mut chunks, b"he");
        hash_bytes(&mut chunks, b"llo");
        assert_eq!(whole, chunks);
    }
}
