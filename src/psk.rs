use crate::{
    cipher_suite::CipherSuite,
    client_config::PskStore,
    group::{
        epoch::Epoch,
        key_schedule::{KeyScheduleKdf, KeyScheduleKdfError},
    },
};
use ferriscrypt::{
    kdf::KdfError,
    rand::{SecureRng, SecureRngError},
};
use std::borrow::Cow;
use thiserror::Error;
use tls_codec::Serialize;
use tls_codec_derive::{TlsDeserialize, TlsSerialize, TlsSize};

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    PartialEq,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct PreSharedKeyID {
    pub key_id: JustPreSharedKeyID,
    pub psk_nonce: PskNonce,
}

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    PartialEq,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
)]
#[repr(u8)]
pub enum JustPreSharedKeyID {
    #[tls_codec(discriminant = 1)]
    External(ExternalPskId),
    Resumption(ResumptionPsk),
}

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    PartialEq,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct ExternalPskId(#[tls_codec(with = "crate::tls::ByteVec")] pub Vec<u8>);

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    PartialEq,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct PskGroupId(#[tls_codec(with = "crate::tls::ByteVec")] pub Vec<u8>);

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    PartialEq,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct PskNonce(#[tls_codec(with = "crate::tls::ByteVec")] pub Vec<u8>);

impl PskNonce {
    pub fn random(cipher_suite: CipherSuite) -> Result<Self, SecureRngError> {
        Ok(Self(SecureRng::gen(
            KeyScheduleKdf::new(cipher_suite.kdf_type()).extract_size(),
        )?))
    }
}

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    PartialEq,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct ResumptionPsk {
    pub usage: ResumptionPSKUsage,
    pub psk_group_id: PskGroupId,
    pub psk_epoch: u64,
}

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    PartialEq,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
)]
#[repr(u8)]
pub enum ResumptionPSKUsage {
    Application = 1,
    Reinit,
    Branch,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Psk(pub Vec<u8>);

impl From<Vec<u8>> for Psk {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for Psk {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, TlsSerialize, TlsSize)]
struct PSKLabel<'a> {
    id: &'a PreSharedKeyID,
    index: u16,
    count: u16,
}

pub(crate) fn psk_secret<'a, S, F, E>(
    cipher_suite: CipherSuite,
    secret_store: &S,
    mut get_epoch: F,
    psk_ids: &[PreSharedKeyID],
) -> Result<Vec<u8>, PskSecretError>
where
    S: PskStore,
    F: FnMut(u64) -> Result<Option<Cow<'a, Epoch>>, E>,
    E: std::error::Error + Send + Sync + 'static,
{
    let len = psk_ids.len();
    let len = u16::try_from(len).map_err(|_| PskSecretError::TooManyPskIds(len))?;
    let kdf = KeyScheduleKdf::new(cipher_suite.kdf_type());
    psk_ids
        .iter()
        .enumerate()
        .try_fold(vec![0; kdf.extract_size()], |psk_secret, (index, id)| {
            let index = index as u16;
            let psk = match &id.key_id {
                JustPreSharedKeyID::External(id) => secret_store
                    .psk(id)
                    .map_err(|e| PskSecretError::SecretStoreError(Box::new(e)))?
                    .ok_or_else(|| PskSecretError::NoPskForId(id.clone()))?,
                JustPreSharedKeyID::Resumption(ResumptionPsk { psk_epoch, .. }) => {
                    get_epoch(*psk_epoch)
                        .map_err(|e| PskSecretError::EpochRepositoryError(e.into()))?
                        .ok_or(PskSecretError::EpochNotFound(*psk_epoch))?
                        .key_schedule
                        .resumption_secret
                        .clone()
                        .into()
                }
            };
            let label = PSKLabel {
                id,
                index,
                count: len,
            };
            let label_bytes = label.tls_serialize_detached()?;
            let psk_extracted = kdf.extract(&vec![0; kdf.extract_size()], psk.as_ref())?;
            let psk_input = kdf.expand_with_label(
                &psk_extracted,
                "derived psk",
                &label_bytes,
                kdf.extract_size(),
            )?;
            let psk_secret = kdf.extract(&psk_input, &psk_secret)?;
            Ok(psk_secret)
        })
}

#[derive(Debug, Error)]
pub enum PskSecretError {
    #[error("Too many PSK IDs ({0}) to compute PSK secret")]
    TooManyPskIds(usize),
    #[error("No PSK for ID {0:?}")]
    NoPskForId(ExternalPskId),
    #[error(transparent)]
    SecretStoreError(Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    KdfError(#[from] KeyScheduleKdfError),
    #[error(transparent)]
    SerializationError(#[from] tls_codec::Error),
    #[error(transparent)]
    EpochRepositoryError(Box<dyn std::error::Error + Send + Sync>),
    #[error("Epoch {0} not found")]
    EpochNotFound(u64),
}

impl From<KdfError> for PskSecretError {
    fn from(e: KdfError) -> Self {
        PskSecretError::KdfError(e.into())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        cipher_suite::CipherSuite,
        client_config::InMemoryPskStore,
        psk::{
            psk_secret, ExternalPskId, JustPreSharedKeyID, PreSharedKeyID, Psk, PskNonce,
            PskSecretError,
        },
    };
    use assert_matches::assert_matches;
    use ferriscrypt::{kdf::hkdf::Hkdf, rand::SecureRng};
    use serde::{Deserialize, Serialize};
    use std::{convert::Infallible, iter};

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    const TEST_CIPHER_SUITE: CipherSuite = CipherSuite::Curve25519Aes128V1;

    fn digest_size(cipher_suite: CipherSuite) -> usize {
        Hkdf::from(cipher_suite.kdf_type()).extract_size()
    }

    fn make_external_psk_id(cipher_suite: CipherSuite) -> ExternalPskId {
        ExternalPskId(SecureRng::gen(digest_size(cipher_suite)).unwrap())
    }

    fn make_nonce(cipher_suite: CipherSuite) -> PskNonce {
        PskNonce::random(cipher_suite).unwrap()
    }

    fn wrap_external_psk_id(cipher_suite: CipherSuite, id: ExternalPskId) -> PreSharedKeyID {
        PreSharedKeyID {
            key_id: JustPreSharedKeyID::External(id),
            psk_nonce: make_nonce(cipher_suite),
        }
    }

    #[test]
    fn unknown_id_leads_to_error() {
        let expected_id = make_external_psk_id(TEST_CIPHER_SUITE);
        let res = psk_secret(
            TEST_CIPHER_SUITE,
            &InMemoryPskStore::default(),
            |_| Ok::<_, Infallible>(None),
            &[wrap_external_psk_id(TEST_CIPHER_SUITE, expected_id.clone())],
        );
        assert_matches!(res, Err(PskSecretError::NoPskForId(actual_id)) if actual_id == expected_id);
    }

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct PskInfo {
        #[serde(with = "hex::serde")]
        id: Vec<u8>,
        #[serde(with = "hex::serde")]
        psk: Vec<u8>,
        #[serde(with = "hex::serde")]
        nonce: Vec<u8>,
    }

    impl From<PskInfo> for PreSharedKeyID {
        fn from(id: PskInfo) -> Self {
            PreSharedKeyID {
                key_id: JustPreSharedKeyID::External(ExternalPskId(id.id)),
                psk_nonce: PskNonce(id.nonce),
            }
        }
    }

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestScenario {
        cipher_suite: u16,
        psks: Vec<PskInfo>,
        #[serde(with = "hex::serde")]
        psk_secret: Vec<u8>,
    }

    impl TestScenario {
        fn generate() -> Vec<TestScenario> {
            let make_psk_list = |cs, n| {
                iter::repeat_with(|| PskInfo {
                    id: make_external_psk_id(cs).0,
                    psk: Psk(SecureRng::gen(digest_size(cs)).unwrap()).0,
                    nonce: make_nonce(cs).0,
                })
                .take(n)
                .collect::<Vec<_>>()
            };

            CipherSuite::all()
                .flat_map(|cs| (1..=10).map(move |n| (cs, n)))
                .map(|(cs, n)| {
                    let psks = make_psk_list(cs, n);
                    let psk_secret = Self::compute_psk_secret(cs, &psks);
                    TestScenario {
                        cipher_suite: cs as u16,
                        psks,
                        psk_secret,
                    }
                })
                .collect()
        }

        fn load() -> Vec<TestScenario> {
            load_test_cases!(psk_secret, TestScenario::generate)
        }

        fn compute_psk_secret(cipher_suite: CipherSuite, psks: &[PskInfo]) -> Vec<u8> {
            let secret_store = psks
                .iter()
                .fold(InMemoryPskStore::default(), |mut store, psk| {
                    store.insert(ExternalPskId(psk.id.clone()), psk.psk.clone().into());
                    store
                });
            let ids = psks
                .iter()
                .cloned()
                .map(PreSharedKeyID::from)
                .collect::<Vec<_>>();
            psk_secret(
                cipher_suite,
                &secret_store,
                |_| Ok::<_, Infallible>(None),
                &ids,
            )
            .unwrap()
        }
    }

    #[test]
    fn expected_psk_secret_is_produced() {
        assert_eq!(
            TestScenario::load()
                .into_iter()
                .enumerate()
                .map(|(i, scenario)| (format!("Scenario #{i}"), scenario))
                .find(|(_, scenario)| {
                    if let Some(cipher_suite) = CipherSuite::from_raw(scenario.cipher_suite) {
                        scenario.psk_secret
                            != TestScenario::compute_psk_secret(cipher_suite, &scenario.psks)
                    } else {
                        false
                    }
                }),
            None
        );
    }

    #[test]
    fn random_generation_of_nonces_is_random() {
        let good = CipherSuite::all().all(|cipher_suite| {
            let nonce = make_nonce(cipher_suite);
            iter::repeat_with(|| make_nonce(cipher_suite))
                .take(1000)
                .all(|other| other != nonce)
        });
        assert!(good);
    }
}
