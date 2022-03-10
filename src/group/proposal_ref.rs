use super::*;
use crate::hash_reference::HashReference;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, TlsDeserialize, TlsSerialize, TlsSize,
)]
pub struct ProposalRef(HashReference);

impl Deref for ProposalRef {
    type Target = HashReference;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ProposalRef {
    pub fn from_plaintext(
        cipher_suite: CipherSuite,
        plaintext: &MLSPlaintext,
        encrypted: bool,
    ) -> Result<Self, ProposalCacheError> {
        let message_content_auth = MLSMessageContentAuth {
            wire_format: if encrypted {
                WireFormat::Cipher
            } else {
                WireFormat::Plain
            },
            content: &plaintext.content,
            auth: &plaintext.auth,
        };
        Ok(ProposalRef(HashReference::from_value(
            &message_content_auth.tls_serialize_detached()?,
            b"MLS 1.0 Proposal Reference",
            cipher_suite,
        )?))
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    pub fn plaintext_from_proposal(proposal: Proposal, sender: KeyPackageRef) -> MLSPlaintext {
        MLSPlaintext {
            auth: MLSMessageAuth {
                signature: MessageSignature::from(SecureRng::gen(128).unwrap()),
                confirmation_tag: None,
            },
            membership_tag: Some(Tag::from(Vec::new()).into()),
            ..MLSPlaintext::new(
                b"test_group".to_vec(),
                0,
                Sender::Member(sender),
                Content::Proposal(proposal),
            )
        }
    }
}

#[cfg(test)]
mod test {
    use super::test_util::plaintext_from_proposal;
    use super::*;
    use crate::tree_kem::test::get_test_key_package;

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::{wasm_bindgen_test as test, wasm_bindgen_test_configure};

    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test_configure!(run_in_browser);

    fn get_test_extension_list() -> ExtensionList {
        let test_extension = RequiredCapabilitiesExt {
            extensions: vec![42],
            proposals: Default::default(),
        };

        let mut extension_list = ExtensionList::new();
        extension_list.set_extension(test_extension).unwrap();

        extension_list
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct TestCase {
        cipher_suite: u16,
        #[serde(with = "hex::serde")]
        input: Vec<u8>,
        #[serde(with = "hex::serde")]
        output: Vec<u8>,
    }

    #[allow(dead_code)]
    fn generate_proposal_test_cases() -> Vec<TestCase> {
        let mut test_cases = Vec::new();

        for (protocol_version, cipher_suite) in
            ProtocolVersion::all().flat_map(|p| CipherSuite::all().map(move |cs| (p, cs)))
        {
            let mut sender = [0u8; 16];
            SecureRng::fill(&mut sender).unwrap();

            let add = plaintext_from_proposal(
                Proposal::Add(AddProposal {
                    key_package: get_test_key_package(
                        protocol_version,
                        cipher_suite,
                        SecureRng::gen(16).unwrap(),
                    )
                    .key_package
                    .into(),
                }),
                sender.into(),
            );

            let update = plaintext_from_proposal(
                Proposal::Update(UpdateProposal {
                    key_package: get_test_key_package(
                        protocol_version,
                        cipher_suite,
                        SecureRng::gen(16).unwrap(),
                    )
                    .key_package
                    .into(),
                }),
                sender.into(),
            );

            let mut key_package_ref = [0u8; 16];
            SecureRng::fill(&mut key_package_ref).unwrap();

            let remove = plaintext_from_proposal(
                Proposal::Remove(RemoveProposal {
                    to_remove: key_package_ref.into(),
                }),
                sender.into(),
            );

            let group_context_ext = plaintext_from_proposal(
                Proposal::GroupContextExtensions(get_test_extension_list()),
                sender.into(),
            );

            test_cases.push(TestCase {
                cipher_suite: cipher_suite as u16,
                input: add.tls_serialize_detached().unwrap(),
                output: ProposalRef::from_plaintext(cipher_suite, &add, false)
                    .unwrap()
                    .to_vec(),
            });

            test_cases.push(TestCase {
                cipher_suite: cipher_suite as u16,
                input: update.tls_serialize_detached().unwrap(),
                output: ProposalRef::from_plaintext(cipher_suite, &update, false)
                    .unwrap()
                    .to_vec(),
            });

            test_cases.push(TestCase {
                cipher_suite: cipher_suite as u16,
                input: remove.tls_serialize_detached().unwrap(),
                output: ProposalRef::from_plaintext(cipher_suite, &remove, false)
                    .unwrap()
                    .to_vec(),
            });

            test_cases.push(TestCase {
                cipher_suite: cipher_suite as u16,
                input: group_context_ext.tls_serialize_detached().unwrap(),
                output: ProposalRef::from_plaintext(cipher_suite, &group_context_ext, false)
                    .unwrap()
                    .to_vec(),
            });
        }

        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/test_data/proposal_ref.json"),
            serde_json::to_vec_pretty(&test_cases).unwrap(),
        )
        .unwrap();

        test_cases
    }

    #[test]
    fn test_proposal_ref() {
        let test_cases: Vec<TestCase> =
            serde_json::from_slice(include_bytes!("../../test_data/proposal_ref.json")).unwrap();

        for one_case in test_cases {
            let cipher_suite = CipherSuite::from_raw(one_case.cipher_suite);

            if cipher_suite.is_none() {
                println!("Skipping test case due to unsupported cipher suite");
                continue;
            }

            let proposal = MLSPlaintext::tls_deserialize(&mut one_case.input.as_slice()).unwrap();

            let proposal_ref =
                ProposalRef::from_plaintext(cipher_suite.unwrap(), &proposal, false).unwrap();

            let expected_out = ProposalRef(HashReference::from(
                <[u8; 16]>::try_from(one_case.output).unwrap(),
            ));

            assert_eq!(expected_out, proposal_ref);
        }
    }
}