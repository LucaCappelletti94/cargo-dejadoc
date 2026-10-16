#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    Literal(u32),
    Parameter(usize),
    Operation(char, Vec<Self>),
}

pub fn operation(op: char, operands: impl IntoIterator<Item = Value>) -> Value {
    let mut leaves = Vec::new();
    for operand in operands {
        match operand {
            Value::Operation(inner, children) if inner == op => leaves.extend(children),
            other => leaves.push(other),
        }
    }
    leaves.sort();
    if op != '^' {
        leaves.dedup();
    }
    let mut literal = None;
    leaves.retain(|leaf| {
        let Value::Literal(value) = leaf else {
            return true;
        };
        literal = Some(match literal {
            None => *value,
            Some(left) => match op {
                '&' => left & value,
                '|' => left | value,
                '^' => left ^ value,
                _ => panic!("unsupported oracle operation"),
            },
        });
        false
    });
    if let Some(value) = literal {
        leaves.push(Value::Literal(value));
    }
    leaves.sort();
    if leaves.len() == 1 {
        leaves.pop().unwrap()
    } else {
        Value::Operation(op, leaves)
    }
}
