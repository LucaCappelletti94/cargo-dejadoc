//! A module only a Windows build compiles.

pub fn width(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > limit {
            total += value * 2;
        } else {
            total -= 1;
        }
    }
    total
}

pub fn height(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > limit {
            total += value * 2;
        } else {
            total -= 1;
        }
    }
    total
}
