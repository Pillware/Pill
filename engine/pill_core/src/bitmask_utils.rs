use std::{ fmt::Binary, ops::{Add, Not, Shl, Sub} };

// Creates bitmasks of any size with bits in range passed as parameter set to 1
// Where range order is: from MSB to LSB
// e.g. let x: u32 = create_bitmask_from_range::<u32>(4..8);
// will return mask: 0000_1111_1000_0000_0000_0000_0000_0000
pub fn create_bitmask_from_range<T>(mask_range: &core::ops::Range<T>) -> T 
where
    T: Copy + Default + Binary + From<u8> + Ord + Shl<Output = T> + Sub<Output = T> + Add<Output = T> + Not<Output = T> 
{
    let mask_size = T::from(std::mem::size_of::<T>() as u8 * 8);

    if mask_range.end >= mask_size {
        panic!("Provided mask range exceeds mask size");
    }

    let range_length: T = mask_range.end - mask_range.start + T::from(1);
    let mask_shift = mask_size - mask_range.end - T::from(1);

    let mask = match range_length == mask_size {
        true => !(T::from(0)) << mask_shift,  // Prevent overflow (when shifting by same value as number of bits in type)
        false => !(!T::from(0) << range_length) << mask_shift,
    };

    mask
}

// From 0 to 15 (16 in total)
// Where range order is: from MSB to LSB
// e.g. create_bitmask_with_one(3);
// will return mask: 0001_0000_0000_0000
pub fn create_bitmask_with_one(index: u16) -> u16 {
    pub const FIRST_BIT: u16 = 0b1000_0000_0000_0000;    
    let mut mask: u16 = 0b0000_0000_0000_0000;
    if (0_u16..=15_u16).contains(&index) {
        mask = mask | FIRST_BIT;
        for _ in 0..index {
            mask = mask >> 1;
        }
    }
    mask
}

// From 0 to 15 (16 in total)
// Where range order is: from MSB to LSB
// e.g. get_indices_of_set_elements(0b0001_0000_0100_0001);
// will return vector: [0, 3, 9, 15]
pub fn get_indices_of_set_elements(bitmask: u16) -> Vec::<usize> {   
    let mut test_mask: u16 =  0b1000_0000_0000_0000;
    let mut indices = Vec::<usize>::new();
    for i in 0..=15 {
        if bitmask & test_mask > 0 {
            indices.push(i);
        }
        test_mask = test_mask >> 1;
    }
    indices
}


