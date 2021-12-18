use std::{fmt::Binary, ops::{Add, Not, Shl, Sub}};

// Creates bitmasks of any size with bits in range passed as parameter set to 1
// Where range order is: from MSB to LSB
// e.g. let x: u32 = create_bitmask_from_range::<u32>(4..8);
// will return mask: 00001111100000000000000000000000
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

