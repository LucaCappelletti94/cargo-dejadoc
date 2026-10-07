//! Context blocks for the integration tests.

pub mod imports {
    pub fn first() -> u32 {
        1
    }

    pub fn second() -> u32 {
        2
    }
}

/// Renamed local binders merge.
pub fn count_up(base: u32) -> u32 {
    let acc = base + 1;
    let more = acc + 2;
    acc + more
}

/// The same body under another name.
pub fn count_down(base: u32) -> u32 {
    let sum = base + 1;
    let extra = sum + 2;
    sum + extra
}

/// A captured parameter merges under rename.
pub fn shifted(start: u32) -> u32 {
    let bump = |value: u32| value + start;
    bump(bump(1))
}

/// The same closure shape again.
pub fn moved(start: u32) -> u32 {
    let step = |value: u32| value + start;
    step(step(1))
}

/// One repeated and one distinct reference.
pub fn mixed(start: u32) -> u32 {
    let bump = |value: u32| value + value;
    let step = |value: u32| value + start;
    bump(1) + step(1)
}

/// The same alias to different import targets.
pub fn via_first(value: u32) -> u32 {
    use crate::imports::first as pick;
    pick() + value
}

pub fn via_second(value: u32) -> u32 {
    use crate::imports::second as pick;
    pick() + value
}

/// Larger bodies around a shared small block.
pub fn framed_left(n: u32) -> u32 {
    let acc = n + 1;
    let step = {
        let v = 3;
        v * 2
    };
    acc + step
}

pub fn framed_right(n: u32) -> u32 {
    let acc = n + 1;
    let step = {
        let v = 3;
        v * 2
    };
    acc + step
}

/// The shared small block, on its own.
pub fn bare() -> u32 {
    1 + {
        let v = 3;
        v * 2
    }
}

/// Two closures sharing one source line.
pub fn paired(n: u32) -> (u32, u32) {
    let (f, g) = (|x: u32| x + 1, |y: u32| y + 1);
    (f(n), g(n))
}
