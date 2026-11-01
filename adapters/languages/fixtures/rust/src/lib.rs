pub mod other;
pub mod util;

use util::helper as h;

#[cfg(feature = "fast")]
pub const MODE: i32 = 2;

#[cfg(not(feature = "fast"))]
pub const MODE: i32 = 1;

pub trait Runner {
    fn run(&self) -> i32;
}

pub struct Job;

impl Runner for Job {
    fn run(&self) -> i32 {
        h(MODE)
    }
}

pub fn call_via_trait(runner: &dyn Runner) -> i32 {
    runner.run()
}

pub fn call_via_fn(f: fn(i32) -> i32, value: i32) -> i32 {
    f(value)
}

pub fn scaled() -> i32 {
    crate::scale!(MODE)
}
