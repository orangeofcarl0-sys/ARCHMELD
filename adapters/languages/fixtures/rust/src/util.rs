#[macro_export]
macro_rules! scale {
    ($x:expr) => {
        $x * 2
    };
}

pub fn helper(value: i32) -> i32 {
    value * 2
}
