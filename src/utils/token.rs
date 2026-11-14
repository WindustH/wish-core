//! Compare bearer material without an early exit at the first differing byte.
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub fn matches(expected: &str, supplied: &str) -> bool {
  let mut expected_tag =
    Hmac::<Sha256>::new_from_slice(b"wish token comparison").expect("HMAC accepts any key length");
  let mut supplied_tag = expected_tag.clone();
  expected_tag.update(expected.as_bytes());
  supplied_tag.update(supplied.as_bytes());
  supplied_tag.verify_slice(&expected_tag.finalize().into_bytes()).is_ok()
}
