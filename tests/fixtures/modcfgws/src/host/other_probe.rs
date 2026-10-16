/// Count of the values below a bound, shifted.
pub fn measure(values: &[u64], bound: u64) -> u64 {
    let mut count = 0;
    for value in values {
        if *value < bound {
            count += value >> 1;
        } else {
            count += 3;
        }
    }
    let scaled = count * bound;
    scaled
}
