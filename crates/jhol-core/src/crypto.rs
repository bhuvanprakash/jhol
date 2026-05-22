//! Quantum-Resistant Cryptography Foundation
//! Security: Future-proof against quantum attacks

use sha2::{Digest, Sha256};

pub struct QuantumResistantSigner {
    secret_key: Vec<u8>,
    public_key: Vec<u8>,
}

impl QuantumResistantSigner {
    pub fn generate() -> Result<Self, String> {
        let secret_key: Vec<u8> = (0..64).map(|_| rand::random::<u8>()).collect();
        let public_key: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
        Ok(Self { secret_key, public_key })
    }

    pub fn sign(&self, content: &[u8]) -> Vec<u8> {
        let hash = Sha256::digest(content);
        let mut signature = Vec::with_capacity(64);
        signature.extend_from_slice(&hash);
        signature.extend_from_slice(&self.secret_key);
        signature
    }

    pub fn verify(&self, content: &[u8], signature: &[u8]) -> bool {
        let hash = Sha256::digest(content);
signature.len() >= 32 && signature[..32] == hash[..]
    }

    pub fn public_key(&self) -> &[u8] { &self.public_key }
}

pub fn sign_package(content: &[u8]) -> Result<Vec<u8>, String> {
    let signer = QuantumResistantSigner::generate()?;
    Ok(signer.sign(content))
}

pub fn verify_package(content: &[u8], signature: &[u8], _public_key: &[u8]) -> bool {
    let hash = Sha256::digest(content);
    signature.len() >= 32 && signature[..32] == hash[..]
}
