use aws_mls_core::crypto::CipherSuite;
use thiserror::Error;

use openssl::{
    bn::{BigNum, BigNumContext},
    derive::Deriver,
    ec::{EcGroup, EcKey, EcPoint, PointConversionForm},
    error::ErrorStack,
    nid::Nid,
    pkey::{Id, PKey, Private, Public},
};

pub type EcPublicKey = PKey<Public>;
pub type EcPrivateKey = PKey<Private>;

#[derive(Debug, Error)]
pub enum EcError {
    #[error(transparent)]
    OpensslError(#[from] openssl::error::ErrorStack),
    /// Attempted to import a secret key that does not contain valid bytes for its curve
    #[error("invalid secret key bytes")]
    InvalidSecretKeyBytes,
}

/// Elliptic curve types
#[derive(Clone, Copy, Debug, Eq, enum_iterator::Sequence, PartialEq)]
#[repr(u8)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub enum Curve {
    /// NIST Curve-P256
    P256,
    /// NIST Curve-P384
    P384,
    /// NIST Curve-P521
    P521,
    /// Elliptic-curve Diffie-Hellman key exchange Curve25519
    X25519,
    /// Edwards-curve Digital Signature Algorithm Curve25519
    Ed25519,
    /// Elliptic-curve Diffie-Hellman key exchange Curve448
    X448,
    /// Edwards-curve Digital Signature Algorithm Curve448
    Ed448,
}

impl Curve {
    /// Returns the amount of bytes of a secret key using this curve
    #[inline(always)]
    pub fn secret_key_size(&self) -> usize {
        match self {
            Curve::P256 => 32,
            Curve::P384 => 48,
            Curve::P521 => 66,
            Curve::X25519 => 32,
            Curve::Ed25519 => 32,
            Curve::X448 => 56,
            Curve::Ed448 => 57,
        }
    }

    pub fn from_ciphersuite(cipher_suite: CipherSuite, for_sig: bool) -> Self {
        match cipher_suite {
            CipherSuite::P256Aes128 => Curve::P256,
            CipherSuite::P384Aes256 => Curve::P384,
            CipherSuite::P521Aes256 => Curve::P521,
            CipherSuite::Curve25519Aes128 | CipherSuite::Curve25519ChaCha20 if for_sig => {
                Curve::Ed25519
            }
            CipherSuite::Curve25519Aes128 | CipherSuite::Curve25519ChaCha20 => Curve::X25519,
            CipherSuite::Curve448Aes256 | CipherSuite::Curve448ChaCha20 if for_sig => Curve::Ed448,
            CipherSuite::Curve448Aes256 | CipherSuite::Curve448ChaCha20 => Curve::X448,
        }
    }

    #[inline(always)]
    pub(crate) fn curve_bitmask(&self) -> Option<u8> {
        match self {
            Curve::P256 => Some(0xFF),
            Curve::P384 => Some(0xFF),
            Curve::P521 => Some(0x01),
            Curve::X25519 => None,
            Curve::Ed25519 => None,
            Curve::X448 => None,
            Curve::Ed448 => None,
        }
    }

    /// Returns an iterator over all curves
    #[inline(always)]
    pub fn all() -> impl Iterator<Item = Curve> {
        enum_iterator::all()
    }
}

#[inline(always)]
fn nist_curve_id(curve: Curve) -> Option<Nid> {
    match curve {
        Curve::P256 => Some(Nid::X9_62_PRIME256V1),
        Curve::P384 => Some(Nid::SECP384R1),
        Curve::P521 => Some(Nid::SECP521R1),
        _ => None,
    }
}

pub fn generate_keypair(curve: Curve) -> Result<KeyPair, EcError> {
    let secret = generate_private_key(curve)?;
    let public = private_key_to_public(&secret)?;
    let secret = private_key_to_bytes(&secret)?;
    let public = pub_key_to_uncompressed(&public)?;
    Ok(KeyPair { public, secret })
}

pub struct KeyPair {
    pub public: Vec<u8>,
    pub secret: Vec<u8>,
}

fn pub_key_from_uncompressed_nist(bytes: &[u8], nid: Nid) -> Result<EcPublicKey, ErrorStack> {
    let group = EcGroup::from_curve_name(nid)?;
    let mut ctx = BigNumContext::new_secure()?;
    let point = EcPoint::from_bytes(&group, bytes, &mut ctx)?;
    let key = EcKey::from_public_key(&group, &point)?;

    PKey::from_ec_key(key)
}

fn pub_key_from_uncompressed_non_nist(bytes: &[u8], id: Id) -> Result<EcPublicKey, ErrorStack> {
    PKey::public_key_from_raw_bytes(bytes, id)
}

pub fn pub_key_from_uncompressed(bytes: &[u8], curve: Curve) -> Result<EcPublicKey, ErrorStack> {
    if let Some(nist_id) = nist_curve_id(curve) {
        pub_key_from_uncompressed_nist(bytes, nist_id)
    } else {
        pub_key_from_uncompressed_non_nist(bytes, Id::from(curve))
    }
}

pub fn pub_key_to_uncompressed(key: &EcPublicKey) -> Result<Vec<u8>, ErrorStack> {
    if let Ok(ec_key) = key.ec_key() {
        let mut ctx = BigNumContext::new()?;

        ec_key
            .public_key()
            .to_bytes(ec_key.group(), PointConversionForm::UNCOMPRESSED, &mut ctx)
    } else {
        key.raw_public_key()
    }
}

impl From<Curve> for Id {
    fn from(c: Curve) -> Self {
        match c {
            Curve::P256 => Id::EC,
            Curve::P384 => Id::EC,
            Curve::P521 => Id::EC,
            Curve::X25519 => Id::X25519,
            Curve::Ed25519 => Id::ED25519,
            Curve::X448 => Id::X448,
            Curve::Ed448 => Id::ED448,
        }
    }
}

fn generate_pkey_with_nid(nid: Nid) -> Result<PKey<Private>, ErrorStack> {
    let group = EcGroup::from_curve_name(nid)?;
    let ec_key = EcKey::generate(&group)?;
    PKey::from_ec_key(ec_key)
}

pub fn generate_private_key(curve: Curve) -> Result<EcPrivateKey, ErrorStack> {
    let key = match curve {
        Curve::X25519 => PKey::generate_x25519(),
        Curve::Ed25519 => PKey::generate_ed25519(),
        Curve::X448 => PKey::generate_x448(),
        Curve::Ed448 => PKey::generate_ed448(),
        Curve::P256 => generate_pkey_with_nid(Nid::X9_62_PRIME256V1),
        Curve::P384 => generate_pkey_with_nid(Nid::SECP384R1),
        Curve::P521 => generate_pkey_with_nid(Nid::SECP521R1),
    }?;

    Ok(key)
}

fn private_key_from_bn_nist(
    mut sk_val: BigNum,
    ctx: BigNumContext,
    group: EcGroup,
    order: BigNum,
) -> Result<Option<EcPrivateKey>, ErrorStack> {
    sk_val.set_const_time();

    // The secret can't be greater than or equal to the order of the curve
    if sk_val.ge(&order) || sk_val.lt(&BigNum::from_u32(1)?) {
        return Ok(None);
    }
    // Derive the public key from the private key since this is the only way we can get
    // what we need from the openssl crate
    let mut pk_val = EcPoint::new(&group)?;
    pk_val.mul_generator(&group, &sk_val, &ctx)?;

    let key = EcKey::from_private_components(&group, &sk_val, &pk_val)?;

    // Clear the original sk_val
    sk_val.clear();

    Some(PKey::from_ec_key(key)).transpose()
}

fn private_key_from_bytes_nist(bytes: &[u8], nid: Nid) -> Result<Option<EcPrivateKey>, ErrorStack> {
    // Get the order and verify that the bytes are in range
    let mut ctx = BigNumContext::new_secure()?;

    let group = EcGroup::from_curve_name(nid)?;
    let mut order = BigNum::new_secure()?;
    order.set_const_time();
    group.order(&mut order, &mut ctx)?;

    // Create a BigNum from our sk_val
    let mut sk_val = BigNum::from_slice(bytes)?;
    sk_val.set_const_time();

    private_key_from_bn_nist(sk_val, ctx, group, order)
}

fn private_key_from_bytes_non_nist(bytes: &[u8], id: Id) -> Result<EcPrivateKey, ErrorStack> {
    PKey::private_key_from_raw_bytes(bytes, id)
}

pub fn private_key_from_bytes(bytes: &[u8], curve: Curve) -> Result<EcPrivateKey, EcError> {
    let maybe_secret_key = if let Some(nist_id) = nist_curve_id(curve) {
        private_key_from_bytes_nist(bytes, nist_id)
    } else {
        Some(private_key_from_bytes_non_nist(bytes, Id::from(curve))).transpose()
    }?;

    maybe_secret_key.ok_or(EcError::InvalidSecretKeyBytes)
}

pub fn private_key_to_bytes(key: &EcPrivateKey) -> Result<Vec<u8>, ErrorStack> {
    if let Ok(ec_key) = key.ec_key() {
        Ok(ec_key.private_key().to_vec())
    } else {
        key.raw_private_key()
    }
}

pub fn private_key_bytes_to_public(secret_key: &[u8], curve: Curve) -> Result<Vec<u8>, EcError> {
    let secret_key = private_key_from_bytes(secret_key, curve)?;
    let public_key = private_key_to_public(&secret_key)?;
    Ok(pub_key_to_uncompressed(&public_key)?)
}

pub fn private_key_to_public(private_key: &EcPrivateKey) -> Result<EcPublicKey, ErrorStack> {
    if let Ok(ec_key) = private_key.ec_key() {
        let pub_key = EcKey::from_public_key(ec_key.group(), ec_key.public_key())?;
        PKey::from_ec_key(pub_key)
    } else {
        let key_data = private_key.raw_public_key()?;
        pub_key_from_uncompressed_non_nist(&key_data, private_key.id())
    }
}

pub fn private_key_ecdh(
    private_key: &EcPrivateKey,
    remote_public: &EcPublicKey,
) -> Result<Vec<u8>, ErrorStack> {
    let mut ecdh_derive = Deriver::new(private_key)?;
    ecdh_derive.set_peer(remote_public)?;
    ecdh_derive.derive_to_vec().map_err(Into::into)
}

#[cfg(test)]
pub mod test_utils {
    use aws_mls_core::crypto::CipherSuite;
    use serde::Deserialize;

    use super::Curve;

    #[derive(Deserialize)]
    pub(crate) struct TestKeys {
        #[serde(with = "hex::serde")]
        p256: Vec<u8>,
        #[serde(with = "hex::serde")]
        p384: Vec<u8>,
        #[serde(with = "hex::serde")]
        p521: Vec<u8>,
        #[serde(with = "hex::serde")]
        x25519: Vec<u8>,
        #[serde(with = "hex::serde")]
        ed25519: Vec<u8>,
        #[serde(with = "hex::serde")]
        x448: Vec<u8>,
        #[serde(with = "hex::serde")]
        ed448: Vec<u8>,
    }

    impl TestKeys {
        pub(crate) fn get_key(&self, cipher_suite: CipherSuite, for_sig: bool) -> Vec<u8> {
            let curve = Curve::from_ciphersuite(cipher_suite, for_sig);

            match curve {
                Curve::P256 => self.p256.clone(),
                Curve::P384 => self.p384.clone(),
                Curve::P521 => self.p521.clone(),
                Curve::X25519 => self.x25519.clone(),
                Curve::Ed25519 => self.ed25519.clone(),
                Curve::X448 => self.x448.clone(),
                Curve::Ed448 => self.ed448.clone(),
            }
        }
    }

    pub(crate) fn get_test_public_keys() -> TestKeys {
        let test_case_file = include_str!("../test_data/test_public_keys.json");
        serde_json::from_str(test_case_file).unwrap()
    }

    pub(crate) fn get_test_secret_keys() -> TestKeys {
        let test_case_file = include_str!("../test_data/test_private_keys.json");
        serde_json::from_str(test_case_file).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use crate::ec::generate_private_key;

    use super::Curve;

    #[test]
    fn private_key_can_be_generated_for_all_curves() {
        Curve::all().for_each(|curve| {
            assert_matches!(
                generate_private_key(curve),
                Ok(_),
                "Failed to generate private key for {curve:?}"
            );
        });
    }
}