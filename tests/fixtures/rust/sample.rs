mod outer {
    pub struct Widget {
        size: u32,
    }

    impl Widget {
        pub const SCALE: u32 = 2;

        pub fn draw(&self) {}
    }

    pub mod inner {
        pub const DEPTH: u32 = 2;
    }
}

pub trait Render {
    fn render(&self) -> String;
}

pub enum Shade {
    Light,
    Dark,
}

pub union Raw {
    int: u32,
    float: f32,
}

pub type Alias = u32;

pub static LIMIT: u32 = 10;

macro_rules! widget {
    () => {};
}

pub fn free() {}
