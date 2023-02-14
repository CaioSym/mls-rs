#![no_main]
use aws_mls::bench_utils::group_functions::{create_group, TestClientConfig};
use aws_mls::Group;
use aws_mls::{CipherSuite, MLSMessage};
use futures::executor::block_on;
use libfuzzer_sys::fuzz_target;
use once_cell::sync::Lazy;
use std::sync::Mutex;

pub const CIPHER_SUITE: aws_mls::CipherSuite = CipherSuite::CURVE25519_AES128;

static GROUP_DATA: Lazy<Mutex<Vec<Group<TestClientConfig>>>> = Lazy::new(|| {
    let container = block_on(create_group(CIPHER_SUITE, 2));
    Mutex::new(container)
});

fuzz_target!(|data: MLSMessage| {
    let _ = block_on(GROUP_DATA.lock().unwrap()[1].process_incoming_message(data));
});
