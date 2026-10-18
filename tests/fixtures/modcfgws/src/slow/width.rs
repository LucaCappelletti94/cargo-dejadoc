/// The clamped width of a row of cells.
pub fn width(cells: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    for cell in cells {
        if *cell > limit {
            total += cell * 3;
        } else {
            total += 1;
        }
    }
    let clamped = total.min(limit * 8);
    clamped
}
