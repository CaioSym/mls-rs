use crate::provider::crypto::{CipherSuiteProvider, HpkePublicKey};
use crate::serde_utils::vec_u8_as_base64::VecAsBase64;
use crate::tree_kem::math as tree_math;
use crate::tree_kem::math::TreeMathError;
use crate::tree_kem::node::{LeafIndex, Node, NodeIndex, NodeVecError};
use crate::tree_kem::RatchetTreeError;
use crate::tree_kem::TreeKemPublic;
use serde_with::serde_as;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use thiserror::Error;
use tls_codec::Serialize;
use tls_codec_derive::{TlsDeserialize, TlsSerialize, TlsSize};

use super::leaf_node::LeafNodeSource;
use super::ValidatedUpdatePath;

#[derive(Error, Debug)]
pub enum ParentHashError {
    #[error(transparent)]
    SerializationError(#[from] tls_codec::Error),
    #[error(transparent)]
    NodeVecError(#[from] NodeVecError),
    #[error(transparent)]
    TreeMathError(#[from] TreeMathError),
    #[error("original tree hash not initialized for node index {0}")]
    TreeHashNotInitialized(u32),
    #[error(transparent)]
    CipherSuiteProviderError(Box<dyn std::error::Error + Send + Sync + 'static>),
}

#[derive(Clone, Debug, TlsSerialize, TlsSize)]
struct ParentHashInput<'a> {
    #[tls_codec(with = "crate::tls::ByteVec")]
    public_key: &'a HpkePublicKey,
    #[tls_codec(with = "crate::tls::ByteVec")]
    parent_hash: &'a [u8],
    #[tls_codec(with = "crate::tls::ByteVec")]
    original_sibling_tree_hash: &'a [u8],
}

#[serde_as]
#[derive(
    Clone,
    Debug,
    TlsDeserialize,
    TlsSerialize,
    TlsSize,
    serde::Deserialize,
    serde::Serialize,
    PartialEq,
    Eq,
)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct ParentHash(
    #[tls_codec(with = "crate::tls::ByteVec")]
    #[serde_as(as = "VecAsBase64")]
    Vec<u8>,
);

impl From<Vec<u8>> for ParentHash {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl Deref for ParentHash {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ParentHash {
    pub fn new<P: CipherSuiteProvider>(
        cipher_suite_provider: &P,
        public_key: &HpkePublicKey,
        parent_hash: &ParentHash,
        original_sibling_tree_hash: &[u8],
    ) -> Result<Self, ParentHashError> {
        let input = ParentHashInput {
            public_key,
            parent_hash,
            original_sibling_tree_hash,
        };

        let input_bytes = input.tls_serialize_detached()?;

        let hash = cipher_suite_provider
            .hash(&input_bytes)
            .map_err(|e| ParentHashError::CipherSuiteProviderError(e.into()))?;

        Ok(Self(hash))
    }

    pub fn empty() -> Self {
        ParentHash(Vec::new())
    }

    pub fn matches(&self, hash: &ParentHash) -> bool {
        //TODO: Constant time equals
        hash == self
    }
}

impl Node {
    fn get_parent_hash(&self) -> Option<ParentHash> {
        match self {
            Node::Parent(p) => Some(p.parent_hash.clone()),
            Node::Leaf(l) => match &l.leaf_node_source {
                LeafNodeSource::Commit(parent_hash) => Some(parent_hash.clone()),
                _ => None,
            },
        }
    }
}

impl TreeKemPublic {
    fn parent_hash<P: CipherSuiteProvider>(
        &self,
        parent_parent_hash: &ParentHash,
        node_index: NodeIndex,
        co_path_child_index: NodeIndex,
        cipher_suite_provider: &P,
    ) -> Result<ParentHash, RatchetTreeError> {
        let node = self.nodes.borrow_as_parent(node_index)?;

        ParentHash::new(
            cipher_suite_provider,
            &node.public_key,
            parent_parent_hash,
            &self.tree_hashes.original[co_path_child_index as usize],
        )
        .map_err(RatchetTreeError::from)
    }

    fn parent_hash_for_leaf<P, T>(
        &self,
        cipher_suite_provider: &P,
        index: LeafIndex,
        mut on_node_calculation: T,
    ) -> Result<ParentHash, RatchetTreeError>
    where
        P: CipherSuiteProvider,
        T: FnMut(NodeIndex, &ParentHash),
    {
        if self.total_leaf_count() <= 1 {
            return Ok(ParentHash::empty());
        }

        let mut filtered_direct_co_path = self
            .nodes
            .filtered_direct_path_co_path(index)?
            .into_iter()
            .rev();

        // Calculate all the parent hash values along the direct path from root to leaf
        filtered_direct_co_path.try_fold(
            ParentHash::empty(),
            |last_hash, (index, sibling_index)| {
                if !self.nodes.is_leaf(index) {
                    on_node_calculation(index, &last_hash);
                }

                let calculated =
                    self.parent_hash(&last_hash, index, sibling_index, cipher_suite_provider)?;

                Ok(calculated)
            },
        )
    }

    // Updates all of the required parent hash values, and returns the calculated parent hash value for the leaf node
    // If an update path is provided, additionally verify that the calculated parent hash matches
    pub(crate) fn update_parent_hashes<P: CipherSuiteProvider>(
        &mut self,
        index: LeafIndex,
        update_path: Option<&ValidatedUpdatePath>,
        cipher_suite_provider: &P,
    ) -> Result<ParentHash, RatchetTreeError> {
        // First update the relevant original hashes used for parent hash computation.
        self.update_hashes(&mut vec![index], &[], cipher_suite_provider)?;

        let mut changes = HashMap::new();

        // Since we can't mut borrow self here we will just collect the list of changes
        // and apply them later
        let leaf_hash =
            self.parent_hash_for_leaf(cipher_suite_provider, index, |index, hash| {
                changes.insert(index, hash.clone());
            })?;

        changes.drain().try_for_each(|(index, hash)| {
            self.nodes
                .borrow_as_parent_mut(index)
                .map(|p| {
                    p.parent_hash = hash;
                })
                .map_err(RatchetTreeError::from)
        })?;

        if let Some(update_path) = update_path {
            // Verify the parent hash of the new sender leaf node and update the parent hash values
            // in the local tree
            if let LeafNodeSource::Commit(parent_hash) = &update_path.leaf_node.leaf_node_source {
                if !leaf_hash.matches(parent_hash) {
                    return Err(RatchetTreeError::ParentHashMismatch);
                }
            } else {
                return Err(RatchetTreeError::ParentHashNotFound);
            }
        }

        // Update hashes after changes to the tree.
        self.update_hashes(&mut vec![index], &[], cipher_suite_provider)?;

        Ok(leaf_hash)
    }

    pub(super) fn validate_parent_hashes<P: CipherSuiteProvider>(
        &self,
        cipher_suite_provider: &P,
    ) -> Result<(), RatchetTreeError> {
        let mut nodes_to_validate: HashSet<u32> = self
            .nodes
            .non_empty_parents()
            .map(|(node_index, _)| node_index)
            .collect();
        let num_leaves = self.total_leaf_count();
        let root = tree_math::root(num_leaves);

        // For each leaf l, validate all non-blank nodes on the chain from l up the tree.
        self.nodes
            .non_empty_leaves()
            .try_for_each(|(leaf_index, _)| {
                let mut n = NodeIndex::from(leaf_index);
                while n != root {
                    // Find the first non-blank ancestor p of n and p's co-path child s.
                    let mut p = tree_math::parent(n, num_leaves)?;
                    let mut s = tree_math::sibling(n, num_leaves)?;
                    while self.nodes.is_blank(p)? {
                        match tree_math::parent(p, num_leaves) {
                            Ok(p_parent) => {
                                s = tree_math::sibling(p, num_leaves)?;
                                p = p_parent;
                            }
                            // If we reached the root, we're done with this chain.
                            Err(_) => return Ok(()),
                        }
                    }

                    // Check is n's parent_hash field matches the parent hash of p with co-path child s.
                    let p_parent_hash = self
                        .nodes
                        .borrow_node(p)?
                        .as_ref()
                        .and_then(|p_node| p_node.get_parent_hash());
                    if let Some((p_parent_hash, n_node)) =
                        p_parent_hash.zip(self.nodes.borrow_node(n)?.as_ref())
                    {
                        if n_node.get_parent_hash()
                            == Some(self.parent_hash(
                                &p_parent_hash,
                                p,
                                s,
                                cipher_suite_provider,
                            )?)
                        {
                            // Check that "n is in the resolution of c, and the intersection of p's unmerged_leaves with the subtree
                            // under c is equal to the resolution of c with n removed".
                            let c = tree_math::sibling(s, num_leaves)?;

                            let mut c_resolution = self
                                .nodes
                                .get_resolution_index(c)?
                                .into_iter()
                                .collect::<HashSet<_>>();

                            let p_unmerged_in_c_subtree = self
                                .unmerged_in_subtree(p, c)?
                                .iter()
                                .copied()
                                .map(|x| *x * 2)
                                .collect::<HashSet<_>>();

                            if c_resolution.remove(&n)
                                && c_resolution == p_unmerged_in_c_subtree
                                && nodes_to_validate.remove(&p)
                            {
                                // If n's parent_hash field matches and p has not been validated yet, mark p as validated and continue.
                                n = p;
                            } else {
                                // If p is validated for the second time, the check fails ("all non-blank parent nodes are covered by exactly one such chain").
                                return Err(RatchetTreeError::ParentHashMismatch);
                            }
                        } else {
                            // If n's parent_hash field doesn't match, we're done with this chain.
                            return Ok(());
                        }
                    }
                }

                Ok(())
            })?;

        // The check passes iff all non-blank nodes are validated.
        if nodes_to_validate.is_empty() {
            Ok(())
        } else {
            Err(RatchetTreeError::ParentHashMismatch)
        }
    }
}

#[cfg(test)]
pub(crate) mod test_utils {

    use crate::{
        cipher_suite::CipherSuite,
        provider::{
            crypto::test_utils::test_cipher_suite_provider, identity::BasicIdentityProvider,
        },
        tree_kem::{leaf_node::test_utils::get_basic_test_node, node::Parent},
    };

    use super::*;

    pub(crate) fn test_parent(
        cipher_suite: CipherSuite,
        unmerged_leaves: Vec<LeafIndex>,
    ) -> Parent {
        let (_, public_key) = test_cipher_suite_provider(cipher_suite)
            .kem_generate()
            .unwrap();

        Parent {
            public_key,
            parent_hash: ParentHash::empty(),
            unmerged_leaves,
        }
    }

    pub(crate) fn test_parent_node(
        cipher_suite: CipherSuite,
        unmerged_leaves: Vec<LeafIndex>,
    ) -> Node {
        Node::Parent(test_parent(cipher_suite, unmerged_leaves))
    }

    // Create figure 12 from MLS RFC
    pub(crate) fn get_test_tree_fig_12(cipher_suite: CipherSuite) -> TreeKemPublic {
        let cipher_suite_provider = test_cipher_suite_provider(cipher_suite);

        let mut tree = TreeKemPublic::new();

        let leaves = ["A", "B", "C", "D", "E", "F", "G"]
            .map(|l| get_basic_test_node(cipher_suite, l))
            .to_vec();

        tree.add_leaves(leaves, BasicIdentityProvider, &cipher_suite_provider)
            .unwrap();

        tree.nodes[1] = Some(test_parent_node(cipher_suite, vec![]));
        tree.nodes[3] = Some(test_parent_node(cipher_suite, vec![LeafIndex(3)]));

        tree.nodes[7] = Some(test_parent_node(
            cipher_suite,
            vec![LeafIndex(3), LeafIndex(6)],
        ));

        tree.nodes[9] = Some(test_parent_node(cipher_suite, vec![LeafIndex(5)]));

        tree.nodes[11] = Some(test_parent_node(
            cipher_suite,
            vec![LeafIndex(5), LeafIndex(6)],
        ));

        tree.update_parent_hashes(LeafIndex(0), None, &cipher_suite_provider)
            .unwrap();

        tree.update_parent_hashes(LeafIndex(4), None, &cipher_suite_provider)
            .unwrap();

        tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cipher_suite::CipherSuite;
    use crate::client::test_utils::TEST_CIPHER_SUITE;
    use crate::provider::crypto::test_utils::{
        test_cipher_suite_provider, try_test_cipher_suite_provider,
    };
    use crate::provider::identity::BasicIdentityProvider;
    use crate::tree_kem::leaf_node::test_utils::get_basic_test_node;
    use crate::tree_kem::leaf_node::LeafNodeSource;
    use crate::tree_kem::node::{NodeTypeResolver, NodeVec};
    use crate::tree_kem::parent_hash::test_utils::{get_test_tree_fig_12, test_parent_node};
    use crate::tree_kem::RatchetTreeError;
    use assert_matches::assert_matches;
    use tls_codec::Deserialize;

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn test_missing_parent_hash() {
        let mut test_tree = get_test_tree_fig_12(TEST_CIPHER_SUITE);

        let test_key_package = get_basic_test_node(TEST_CIPHER_SUITE, "foo");

        let test_update_path = ValidatedUpdatePath {
            leaf_node: test_key_package,
            nodes: vec![],
        };

        let missing_parent_hash_res = test_tree.update_parent_hashes(
            LeafIndex(0),
            Some(&test_update_path),
            &test_cipher_suite_provider(TEST_CIPHER_SUITE),
        );

        assert_matches!(
            missing_parent_hash_res,
            Err(RatchetTreeError::ParentHashNotFound)
        );
    }

    #[test]
    fn test_parent_hash_mismatch() {
        let mut test_tree = get_test_tree_fig_12(TEST_CIPHER_SUITE);

        let test_key_package = get_basic_test_node(TEST_CIPHER_SUITE, "foo");

        let mut test_update_path = ValidatedUpdatePath {
            leaf_node: test_key_package,
            nodes: vec![],
        };

        let unexpected_parent_hash = ParentHash::from(hex!("f00d"));

        test_update_path.leaf_node.leaf_node_source =
            LeafNodeSource::Commit(unexpected_parent_hash);

        let invalid_parent_hash_res = test_tree.update_parent_hashes(
            LeafIndex(0),
            Some(&test_update_path),
            &test_cipher_suite_provider(TEST_CIPHER_SUITE),
        );

        assert_matches!(
            invalid_parent_hash_res,
            Err(RatchetTreeError::ParentHashMismatch)
        );
    }

    #[test]
    fn test_parent_hash_invalid() {
        let mut test_tree = get_test_tree_fig_12(TEST_CIPHER_SUITE);
        test_tree.nodes[2] = None;

        let res = test_tree.validate_parent_hashes(&test_cipher_suite_provider(TEST_CIPHER_SUITE));
        assert_matches!(res, Err(RatchetTreeError::ParentHashMismatch));
    }

    #[test]
    fn test_parent_hash_with_blanks() {
        let cipher_suite_provider = test_cipher_suite_provider(TEST_CIPHER_SUITE);

        // Create a tree with 4 blanks: leaves C and D, and their 2 ancestors.
        let mut tree = TreeKemPublic::new();

        let leaves = ["A", "B", "C", "D", "E", "F"]
            .map(|l| get_basic_test_node(TEST_CIPHER_SUITE, l))
            .to_vec();

        tree.add_leaves(leaves, BasicIdentityProvider, &cipher_suite_provider)
            .unwrap();

        tree.nodes[1] = Some(test_parent_node(TEST_CIPHER_SUITE, vec![]));
        tree.nodes[7] = Some(test_parent_node(TEST_CIPHER_SUITE, vec![]));
        tree.nodes[9] = Some(test_parent_node(TEST_CIPHER_SUITE, vec![]));
        tree.nodes[4] = None;
        tree.nodes[6] = None;

        // Compute parent hashes after E commits and then A commits.
        for i in [4, 0] {
            tree.nodes
                .borrow_as_leaf_mut(LeafIndex(i))
                .unwrap()
                .leaf_node_source = LeafNodeSource::Commit(
                tree.update_parent_hashes(LeafIndex(i), None, &cipher_suite_provider)
                    .unwrap(),
            );
        }

        assert!(tree.validate_parent_hashes(&cipher_suite_provider).is_ok());
    }

    #[test]
    fn test_parent_hash_edge() {
        let cipher_suite_provider = test_cipher_suite_provider(TEST_CIPHER_SUITE);

        let mut tree = TreeKemPublic::new();

        let leaves = [
            "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13",
        ]
        .map(|l| get_basic_test_node(TEST_CIPHER_SUITE, l))
        .to_vec();

        tree.add_leaves(leaves, BasicIdentityProvider, &cipher_suite_provider)
            .unwrap();

        for i in [19, 23, 1, 3, 5, 9, 11, 13, 7, 15] {
            tree.nodes[i] = Some(test_parent_node(TEST_CIPHER_SUITE, vec![]));
        }

        for i in [16, 24] {
            tree.nodes[i] = None;
        }

        for i in [0, 2, 4, 6, 9] {
            tree.nodes
                .borrow_as_leaf_mut(LeafIndex(i))
                .unwrap()
                .leaf_node_source = LeafNodeSource::Commit(
                tree.update_parent_hashes(LeafIndex(i), None, &cipher_suite_provider)
                    .unwrap(),
            );
        }

        for leaf_name in ["A", "B", "C"] {
            tree.add_leaves(
                vec![get_basic_test_node(TEST_CIPHER_SUITE, leaf_name)],
                BasicIdentityProvider,
                &cipher_suite_provider,
            )
            .unwrap();
        }

        assert!(tree.validate_parent_hashes(&cipher_suite_provider).is_ok());
    }

    #[derive(serde::Deserialize, serde::Serialize)]
    struct TestCase {
        cipher_suite: u16,
        #[serde(with = "hex::serde")]
        tree_data: Vec<u8>,
    }

    impl TestCase {
        fn generate() -> Vec<TestCase> {
            CipherSuite::all()
                .map(|cipher_suite| {
                    let tree = get_test_tree_fig_12(cipher_suite);

                    TestCase {
                        cipher_suite: cipher_suite as u16,
                        tree_data: tree.export_node_data().tls_serialize_detached().unwrap(),
                    }
                })
                .collect()
        }
    }

    fn load_test_cases() -> Vec<TestCase> {
        load_test_cases!(parent_hash, TestCase::generate)
    }

    #[test]
    fn test_parent_hash_test_vectors() {
        let cases = load_test_cases();

        for one_case in cases {
            let Some(cs_provider) = try_test_cipher_suite_provider(one_case.cipher_suite) else {
                continue;
            };

            let tree = TreeKemPublic::import_node_data(
                NodeVec::tls_deserialize(&mut &*one_case.tree_data).unwrap(),
                BasicIdentityProvider,
            )
            .unwrap();

            for (index, leaf) in tree.non_empty_leaves() {
                if let LeafNodeSource::Commit(parent_hash) = &leaf.leaf_node_source {
                    let calculated_parent_hash = tree
                        .parent_hash_for_leaf(&cs_provider, index, |node_index, parent_hash| {
                            let expected_parent = &tree
                                .nodes
                                .borrow_node(node_index)
                                .unwrap()
                                .as_parent()
                                .unwrap()
                                .parent_hash;

                            assert_eq!(parent_hash, expected_parent);
                        })
                        .unwrap();

                    assert_eq!(&calculated_parent_hash, parent_hash);
                }
            }
        }
    }
}