pub fn helper(value: i32) -> i32 {
    value * 2
}

pub fn main_value(value: i32) -> i32 {
    helper(value) + 1
}

pub struct Boxed {
    pub value: i32,
}
