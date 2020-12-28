//! Pure Rust implementation of the [`SHA-crypt` password hash based on SHA-512][1],
//! a legacy password hashing scheme supported by the [POSIX crypt C library][2].
//!
//! Password hashes using this algorithm start with `$6$` when encoded using the
//! [PHC string format][3].
//!
//! # Usage
//!
//! ```
//! use sha_crypt::{Sha512Params, sha512_simple, sha512_check};
//!
//! // First setup the Sha512Params arguments with:
//! // rounds = 10_000
//! let params = Sha512Params::new(10_000).expect("RandomError!");
//! // Hash the password for storage
//! let hashed_password = sha512_simple("Not so secure password", &params)
//!     .expect("Should not fail");
//! // Verifying a stored password
//! assert!(sha512_check("Not so secure password", &hashed_password).is_ok());
//! ```
//!
//! [1]: https://www.akkadia.org/drepper/SHA-crypt.txt
//! [2]: https://en.wikipedia.org/wiki/Crypt_(C)
//! [3]: https://github.com/P-H-C/phc-string-format/blob/master/phc-sf-spec.md

#[cfg(feature = "include_simple")]
use constant_time_eq::constant_time_eq;
#[cfg(feature = "include_simple")]
use rand::distributions::Distribution;
#[cfg(feature = "include_simple")]
use rand::{thread_rng, Rng};
use sha2::{Digest, Sha512};
use std::result::Result;

mod b64;
mod defs;
pub mod errors;
pub mod params;

#[cfg(feature = "include_simple")]
use errors::CheckError;
use errors::CryptError;
pub use params::{Sha512Params, ROUNDS_DEFAULT, ROUNDS_MAX, ROUNDS_MIN};

use defs::BLOCK_SIZE;
#[cfg(feature = "include_simple")]
use defs::{SALT_MAX_LEN, TAB};

#[cfg(feature = "include_simple")]
static SHA512_SALT_PREFIX: &str = "$6$";
#[cfg(feature = "include_simple")]
static SHA512_ROUNDS_PREFIX: &str = "rounds=";

#[cfg(feature = "include_simple")]
#[derive(Debug)]
struct ShaCryptDistribution;

#[cfg(feature = "include_simple")]
impl Distribution<char> for ShaCryptDistribution {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> char {
        const RANGE: u32 = 26 + 26 + 10 + 2; // 2 == "./"
        loop {
            let var = rng.next_u32() >> (32 - 6);
            if var < RANGE {
                return TAB[var as usize] as char;
            }
        }
    }
}

fn produce_byte_seq(len: usize, fill_from: &[u8], bs: usize) -> Vec<u8> {
    let mut seq: Vec<u8> = vec![0; len];
    let mut offset: usize = 0;
    for _ in 0..(len / bs) {
        let from_slice = &fill_from[..offset + bs];
        seq[offset..offset + bs].clone_from_slice(from_slice);
        offset += bs;
    }
    let from_slice = &fill_from[..offset + (len % bs)];
    seq[offset..offset + (len % bs)].clone_from_slice(from_slice);
    seq
}

fn sha512crypt_intermediate(password: &[u8], salt: &[u8]) -> Vec<u8> {
    let pw_len = password.len();

    let mut hasher = Sha512::default();
    hasher.update(password);
    hasher.update(salt);

    // 4.
    let mut hasher_alt = Sha512::default();
    // 5.
    hasher_alt.update(password);
    // 6.
    hasher_alt.update(salt);
    // 7.
    hasher_alt.update(password);
    // 8.
    let digest_b = hasher_alt.finalize();

    // 9.
    for _ in 0..(pw_len / BLOCK_SIZE) {
        hasher.update(&digest_b);
    }
    // 10.
    hasher.update(&digest_b[..(pw_len % BLOCK_SIZE)]);

    // 11
    let mut n = pw_len;
    for _ in 0..pw_len {
        if n == 0 {
            break;
        }
        if (n & 1) != 0 {
            hasher.update(&digest_b);
        } else {
            hasher.update(password);
        }
        n >>= 1;
    }

    // 12.
    let intermediate = hasher.finalize();

    let mut r: Vec<u8> = vec![];
    r.extend_from_slice(intermediate.as_slice());
    r
}

/// The SHA512 crypt function returned as byte vector
///
/// If the provided hash is longer than defs::SALT_MAX_LEN character, it will
/// be stripped down to defs::SALT_MAX_LEN characters.
///
/// # Arguments
/// - `password` - The password to process as a byte vector
/// - `salt` - The salt value to use as a byte vector
/// - `params` - The Sha512Params to use
///   **WARNING: Make sure to compare this value in constant time!**
///
/// # Return
/// `Ok(())` if calculation was successful. Otherwise a errors::CryptError
pub fn sha512_crypt(
    password: &[u8],
    salt: &[u8],
    params: &Sha512Params,
) -> Result<Vec<u8>, CryptError> {
    let pw_len = password.len();

    let salt_len = salt.len();
    let salt = match salt_len {
        0..=15 => &salt[0..salt_len],
        _ => &salt[0..16],
    };
    let salt_len = salt.len();

    if params.rounds < ROUNDS_MIN || params.rounds > ROUNDS_MAX {
        return Err(CryptError::RoundsError);
    }

    let digest_a = sha512crypt_intermediate(password, salt);

    // 13.
    let mut hasher_alt = Sha512::default();

    // 14.
    for _ in 0..pw_len {
        hasher_alt.update(password);
    }

    // 15.
    let dp = hasher_alt.finalize();

    // 16.
    // Create byte sequence P.
    let p_vec = produce_byte_seq(pw_len, &dp, BLOCK_SIZE);

    // 17.
    hasher_alt = Sha512::default();

    // 18.
    // For every character in the password add the entire password.
    for _ in 0..(16 + digest_a[0] as usize) {
        hasher_alt.update(salt);
    }

    // 19.
    // Finish the digest.
    let ds = hasher_alt.finalize();

    // 20.
    // Create byte sequence S.
    let s_vec = produce_byte_seq(salt_len, &ds, BLOCK_SIZE);

    let mut digest_c = digest_a;
    // Repeatedly run the collected hash value through SHA512 to burn
    // CPU cycles
    for i in 0..params.rounds as usize {
        // new hasher
        let mut hasher = Sha512::default();

        // Add key or last result
        if (i & 1) != 0 {
            hasher.update(&p_vec);
        } else {
            hasher.update(&digest_c);
        }

        // Add salt for numbers not divisible by 3
        if i % 3 != 0 {
            hasher.update(&s_vec);
        }

        // Add key for numbers not divisible by 7
        if i % 7 != 0 {
            hasher.update(&p_vec);
        }

        // Add key or last result
        if (i & 1) != 0 {
            hasher.update(&digest_c);
        } else {
            hasher.update(&p_vec);
        }

        digest_c.clone_from_slice(&hasher.finalize());
    }

    Ok(digest_c.as_slice().to_vec())
}

/// Same as sha512_crypt except base64 representation will be returned.
///
/// # Arguments
/// - `password` - The password to process as a byte vector
/// - `salt` - The salt value to use as a byte vector
/// - `params` - The Sha512Params to use
///   **WARNING: Make sure to compare this value in constant time!**
///
/// # Return
/// `Ok(())` if calculation was successful. Otherwise a errors::CryptError
pub fn sha512_crypt_b64(
    password: &[u8],
    salt: &[u8],
    params: &Sha512Params,
) -> Result<String, CryptError> {
    let output = sha512_crypt(password, salt, params)?;
    let r = String::from_utf8(b64::encode(output.as_slice()))?;
    Ok(r)
}

/// Simple interface for generating a SHA512 password hash.
///
/// The salt will be chosen randomly. The output format will conform to [1].
///
///  `$<ID>$<SALT>$<HASH>`
///
/// # Returns
/// - `Ok(String)` containing the full SHA512 password hash format on success
/// - `Err(CryptError)` if something went wrong.
#[cfg(feature = "include_simple")]
pub fn sha512_simple(password: &str, params: &Sha512Params) -> Result<String, CryptError> {
    let rng = thread_rng();

    let salt: String = rng
        .sample_iter(&ShaCryptDistribution)
        .take(SALT_MAX_LEN)
        .collect();

    let out = sha512_crypt(password.as_bytes(), salt.as_bytes(), &params)?;

    let mut result = String::new();
    result.push_str(SHA512_SALT_PREFIX);
    if params.rounds != ROUNDS_DEFAULT {
        result.push_str(&format!("{}{}", SHA512_ROUNDS_PREFIX, params.rounds));
        result.push('$');
    }
    result.push_str(&salt);
    result.push('$');
    let s = String::from_utf8(b64::encode(out.as_slice()))?;
    result.push_str(&s);
    Ok(result)
}

/// Checks that given password matches provided hash.
///
/// # Arguments
/// - `password` - expected password
/// - `hashed_value` - the hashed value which should be used for checking,
/// should be of format mentioned in [1]: `$6$<SALT>$<PWD>`
///
/// # Return
/// `OK(())` if password matches otherwise Err(CheckError) in case of invalid
/// format or password mismatch.
///
/// [1]: https://www.akkadia.org/drepper/SHA-crypt.txt
#[cfg(feature = "include_simple")]
pub fn sha512_check(password: &str, hashed_value: &str) -> Result<(), CheckError> {
    let mut iter = hashed_value.split('$');

    // Check that there are no characters before the first "$"
    if iter.next() != Some("") {
        return Err(CheckError::InvalidFormat(
            "Should start with '$".to_string(),
        ));
    }

    if iter.next() != Some("6") {
        return Err(CheckError::InvalidFormat(format!(
            "does not contain SHA512 identifier: '{}'",
            SHA512_SALT_PREFIX
        )));
    }

    let mut next = iter.next().ok_or_else(|| {
        CheckError::InvalidFormat("Does not contain a rounds or salt nor hash string".to_string())
    })?;
    let rounds = if next.starts_with(SHA512_ROUNDS_PREFIX) {
        let rounds = next;
        next = iter.next().ok_or_else(|| {
            CheckError::InvalidFormat("Does not contain a salt nor hash string".to_string())
        })?;

        rounds[SHA512_ROUNDS_PREFIX.len()..].parse().map_err(|_| {
            CheckError::InvalidFormat(format!(
                "{} specifier need to be a number",
                SHA512_ROUNDS_PREFIX
            ))
        })?
    } else {
        ROUNDS_DEFAULT
    };

    let salt = next;

    let hash = iter
        .next()
        .ok_or_else(|| CheckError::InvalidFormat("Does not contain a hash string".to_string()))?;

    // Make sure there is no trailing data after the final "$"
    if iter.next() != None {
        return Err(CheckError::InvalidFormat(
            "Trailing characters present".to_string(),
        ));
    }

    let params = Sha512Params { rounds };

    let output = match sha512_crypt(password.as_bytes(), salt.as_bytes(), &params) {
        Ok(v) => v,
        Err(e) => return Err(CheckError::Crypt(e)),
    };

    let hash = b64::decode(hash.as_bytes());

    if !constant_time_eq(&output, &hash) {
        return Err(CheckError::HashMismatch);
    }

    Ok(())
}