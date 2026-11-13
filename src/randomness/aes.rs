// Only the unused SIMD helpers below need x86_64; the PRG itself uses the portable `aes` crate.
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    __m128i, _mm_add_epi64, _mm_set_epi64x, _mm_storeu_si128,
};

use aes::cipher::{Block, BlockCipherEncrypt, Key, KeyInit};
use aes::Aes128;
use core::convert::Infallible;
use rand_core::TryRng;
use std::cell::RefCell;

pub const AES_KEY_SIZE: usize = 16;
pub const AES_BLOCK_SIZE: usize = 16;

pub struct FixedKeyPrgStream {
    aes: Aes128,
    ctr_generic_array: Vec<Block<Aes128>>,
    buf_blocks: Vec<Block<Aes128>>,
    count: usize,
    buf: [u8; 101 * AES_BLOCK_SIZE],
    buf_ptr: usize,
    have: usize,
}

thread_local!(static FIXED_KEY_STREAM: RefCell<FixedKeyPrgStream> = RefCell::new(FixedKeyPrgStream::new()));

impl FixedKeyPrgStream {
    pub fn new() -> Self {
        let key = Key::<Aes128>::default();
        let mut ctr_generic_array = vec![Block::<Aes128>::default(); 101];
        for i in 0..=100 {
            let mut ctr_bytes = [0u8; AES_BLOCK_SIZE];
            ctr_bytes[8..].copy_from_slice(&(i as u64).to_be_bytes());
            ctr_generic_array[i].copy_from_slice(&ctr_bytes);
        }
        FixedKeyPrgStream {
            aes: Aes128::new(&key),
            ctr_generic_array: ctr_generic_array.clone(),
            buf_blocks: ctr_generic_array,
            count: 0,
            buf: [0; 101 * AES_BLOCK_SIZE],
            buf_ptr: 0,
            have: 0,
        }
    }

    pub fn set_key(&mut self, key: &[u8; 16]) {
        for i in 0..self.count {
            self.buf_blocks[i].copy_from_slice(&self.ctr_generic_array[i]);
        }
        let mut aes_key = Key::<Aes128>::default();
        aes_key.copy_from_slice(key);
        self.aes = Aes128::new(&aes_key);
        self.buf_ptr = 0;
        self.have = 0;
        self.count = 0;
    }

    pub fn skip_block(&mut self) {
        self.buf_ptr += AES_BLOCK_SIZE;
    }

    pub fn refill(&mut self) {
        self.have += AES_BLOCK_SIZE;

        let mut to_encrypt = self.ctr_generic_array[self.count].clone();
        self.aes.encrypt_block(&mut to_encrypt);
        to_encrypt
            .iter_mut()
            .zip(self.ctr_generic_array[self.count].iter())
            .for_each(|(x1, x2)| *x1 ^= *x2);
        self.buf[self.count * AES_BLOCK_SIZE..(self.count + 1) * AES_BLOCK_SIZE]
            .copy_from_slice(&to_encrypt);
        self.count += 1;
    }

    pub fn refill8(&mut self) {
        self.have += 8 * AES_BLOCK_SIZE;

        let mut blocks_to_encrypt = self.ctr_generic_array[self.count..self.count + 8].to_vec();
        self.aes.encrypt_blocks(&mut blocks_to_encrypt);

        for i in 0..8 {
            blocks_to_encrypt[i]
                .iter_mut()
                .zip(self.ctr_generic_array[self.count + i].iter())
                .for_each(|(x1, x2)| *x1 ^= *x2);
            self.buf[(self.count + i) * AES_BLOCK_SIZE..(self.count + i + 1) * AES_BLOCK_SIZE]
                .copy_from_slice(&blocks_to_encrypt[i]);
        }
        self.count += 8;
    }

    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    #[allow(unused)]
    fn inc_be(v: __m128i) -> __m128i {
        unsafe { _mm_add_epi64(v, _mm_set_epi64x(1, 0)) }
    }

    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    #[allow(unused)]
    fn store(val: __m128i, at: &mut [u8]) {
        debug_assert_eq!(at.len(), AES_BLOCK_SIZE);

        #[allow(clippy::cast_ptr_alignment)]
        unsafe {
            _mm_storeu_si128(at.as_mut_ptr() as *mut __m128i, val)
        }
    }

}

impl TryRng for FixedKeyPrgStream {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut buf = [0u8; 4];
        self.try_fill_bytes(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut buf = [0u8; 8];
        self.try_fill_bytes(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
        let mut dest_ptr = 0;
        while dest_ptr < dest.len() {
            if self.buf_ptr == self.have {
                if dest.len() > 4 * AES_BLOCK_SIZE {
                    self.refill8();
                } else {
                    self.refill();
                }
            }

            let to_copy = std::cmp::min(self.have - self.buf_ptr, dest.len() - dest_ptr);
            dest[dest_ptr..dest_ptr + to_copy]
                .copy_from_slice(&self.buf[self.buf_ptr..self.buf_ptr + to_copy]);

            self.buf_ptr += to_copy;
            dest_ptr += to_copy;
        }
        Ok(())
    }
}
