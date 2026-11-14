// extern crate cpuprofiler;

pub mod configs;
pub mod data_structures;
pub mod fss;
pub mod fuzzy_match;
pub mod garbled_circuits;
pub mod naive;
pub mod randomness;
pub mod data_toolkit;

pub mod bench;
pub mod channel;
pub mod okvs_f2k;
pub mod util;

use scuttlebutt::Block;

// Additive group, such as (Z_n, +)
pub trait Group {
    fn zero() -> Self;
    fn one() -> Self;
    fn negate(&mut self);
    fn reduce(&mut self);
    fn add(&mut self, other: &Self);
    fn add_lazy(&mut self, other: &Self);
    fn mul(&mut self, other: &Self);
    fn mul_lazy(&mut self, other: &Self);
    fn sub(&mut self, other: &Self);
}

pub fn u32_to_bits(nbits: u8, input: u32) -> Vec<bool> {
    assert!(nbits <= 32);

    let mut out: Vec<bool> = Vec::new();
    for i in 0..nbits {
        let bit = (input & (1 << i)) != 0;
        out.push(bit);
    }
    out
}

pub fn msb_u32_to_bits(nbits: u8, input: u32) -> Vec<bool> {
    assert!(nbits <= 32);

    let mut out: Vec<bool> = Vec::new();
    for i in (0..nbits).rev() {
        let bit = (input & (1 << i)) != 0;
        out.push(bit);
    }
    out
}

pub fn bits_to_u32(bits: &[bool]) -> u32 {
    assert!(bits.len() <= 32);

    let mut result: u32 = 0;
    for (i, &bit) in bits.iter().enumerate() {
        if bit {
            result |= 1 << (bits.len() - 1 - i);
        }
    }
    result
}

pub fn string_to_bits(s: &str) -> Vec<bool> {
    let mut bits = vec![];
    let byte_vec = s.to_string().into_bytes();
    for byte in &byte_vec {
        let mut b = crate::u32_to_bits(8, (*byte).into());
        bits.append(&mut b);
    }
    bits
}

pub fn bits_to_u8(bits: &[bool]) -> u8 {
    assert_eq!(bits.len(), 8);
    let mut out = 0u8;
    for i in 0..8 {
        let b8: u8 = bits[i].into();
        out |= b8 << i;
    }

    out
}

pub fn bits_to_string(bits: &[bool]) -> String {
    assert!(bits.len() % 8 == 0);

    let mut out: String = "".to_string();
    let byte_len = bits.len() / 8;
    for b in 0..byte_len {
        let byte = &bits[8 * b..8 * (b + 1)];
        let ubyte = bits_to_u8(&byte);
        out.push_str(std::str::from_utf8(&[ubyte]).unwrap());
    }

    out
}

pub fn add_bitstrings(alpha: &[bool], beta: &[bool]) -> Vec<bool> {
    let max_len = alpha.len().max(beta.len());
    let mut alpha_padded = vec![false; max_len - alpha.len()];
    alpha_padded.extend(alpha);
    let mut beta_padded = vec![false; max_len - beta.len()];
    beta_padded.extend(beta);
    let mut sum = Vec::new();
    let mut carry = false;

    // Iterate from LSB to MSB
    for (a, b) in alpha_padded.iter().rev().zip(beta_padded.iter().rev()) {
        let (s, c) = full_adder(*a, *b, carry);
        sum.push(s);
        carry = c;
    }
    if carry {
        sum.push(true);
    }
    // Reverse to get MSB first ordering
    sum.into_iter().rev().collect()
}

pub fn subtract_bitstrings(alpha: &[bool], beta: &[bool]) -> Vec<bool> {
    let max_len = alpha.len().max(beta.len());
    let mut alpha_padded = vec![false; max_len - alpha.len()];
    alpha_padded.extend(alpha);
    let mut beta_padded = vec![false; max_len - beta.len()];
    beta_padded.extend(beta);

    let mut beta_twos_complement: Vec<bool> = beta_padded.iter().map(|b| !b).collect();

    let mut carry = true;
    for bit in beta_twos_complement.iter_mut().rev() {
        let sum = *bit ^ carry;
        carry = *bit && carry;
        *bit = sum;
        if !carry {
            break;
        }
    }

    let mut result = Vec::new();
    let mut carry = false;

    for (a, b) in alpha_padded
        .iter()
        .rev()
        .zip(beta_twos_complement.iter().rev())
    {
        let (s, c) = full_adder(*a, *b, carry);
        result.push(s);
        carry = c;
    }

    // If there’s a carry-out, ignore it (overflow)

    // Reverse to get MSB-first ordering
    result.into_iter().rev().collect()
}

pub fn block_to_bits(block: Block, lsb_first: bool) -> Vec<bool> {
    let bytes = block.as_ref(); // Get the 16-byte array
    let mut bits = Vec::with_capacity(128);

    if lsb_first {
        // LSB first: process bytes in order, bits within each byte from 0 to 7
        for byte in bytes.iter() {
            for i in 0..8 {
                bits.push((*byte & (1 << i)) != 0);
            }
        }
    } else {
        // MSB first: process bytes in reverse order, bits within each byte from 7 to 0
        for byte in bytes.iter().rev() {
            for i in (0..8).rev() {
                bits.push((*byte & (1 << i)) != 0);
            }
        }
    }

    bits
}

// Helper function for single-bit addition with carry
fn full_adder(a: bool, b: bool, carry_in: bool) -> (bool, bool) {
    let sum = a ^ b ^ carry_in;
    let carry_out = (a & b) | (b & carry_in) | (a & carry_in);
    (sum, carry_out)
}

fn bytes_to_u128(bytes: &[u8]) -> u128 {
    let mut buffer = [0u8; 16]; // Create a 16-byte buffer, initialized to zeros

    // Determine how many bytes to copy (up to 16)
    let bytes_to_copy = bytes.len().min(16);

    // Copy the input bytes into the buffer.
    // If bytes.len() < 16, the remaining bytes in buffer will stay 0.
    // If bytes.len() > 16, only the first 16 bytes will be copied.
    buffer[..bytes_to_copy].copy_from_slice(&bytes[..bytes_to_copy]);

    u128::from_le_bytes(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_bits() {
        let empty: Vec<bool> = vec![];
        assert_eq!(u32_to_bits(0, 7), empty);
        assert_eq!(u32_to_bits(1, 0), vec![false]);
        assert_eq!(u32_to_bits(2, 0), vec![false, false]);
        assert_eq!(u32_to_bits(2, 3), vec![true, true]);
        assert_eq!(u32_to_bits(2, 1), vec![true, false]);
        assert_eq!(u32_to_bits(12, 65535), vec![true; 12]);
    }

    #[test]
    fn to_string() {
        let empty: Vec<bool> = vec![];
        assert_eq!(string_to_bits(""), empty);
        let avec = vec![true, false, false, false, false, true, true, false];
        assert_eq!(string_to_bits("a"), avec);

        let mut aaavec = vec![];
        for _i in 0..3 {
            aaavec.append(&mut avec.clone());
        }
        assert_eq!(string_to_bits("aaa"), aaavec);
    }

    #[test]
    fn to_from_string() {
        let s = "basfsdfwefwf";
        let bitvec = string_to_bits(s);
        let s2 = bits_to_string(&bitvec);

        assert_eq!(bitvec.len(), s.len() * 8);
        assert_eq!(s, s2);
    }
}
