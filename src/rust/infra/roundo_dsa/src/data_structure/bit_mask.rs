/// L is the number of bytes of BitMask. \
/// 0th bit start from the smallest bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BitMask<const L: usize>(pub [u8; L]);
impl<const L: usize> Default for BitMask<L> {
    fn default() -> Self {
        Self([0u8; L])
    }
}

impl<const L: usize> BitMask<L> {
    fn check_index(index: i32) -> usize {
        let max_bits = L * 8;
        if index < 0 || (index as usize) >= max_bits {
            panic!("BitMask Overflow");
        }
        index as usize
    }
    pub fn bit(&self, index: i32) -> bool {
        let index = Self::check_index(index);
        let byte_index = index >> 3;
        let bit_index = index & 7;
        let mask = 1u8 << bit_index;
        (self.0[byte_index] & mask) != 0
    }

    pub fn set(&mut self, index: i32, state: bool) {
        let index = Self::check_index(index);
        let byte_index = index >> 3;
        let bit_index = index & 7;
        let mask = 1u8 << bit_index;
        if state {
            self.0[byte_index] |= mask;
        } else {
            self.0[byte_index] &= !mask;
        }
    }
}
