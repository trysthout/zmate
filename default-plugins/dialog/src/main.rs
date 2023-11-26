use zellij_tile::prelude::*;

#[derive(Default)]
struct State {}

register_plugin!(State);

impl ZellijPlugin for State {
    fn render(&mut self, rows: usize, cols: usize) {
        //println!("{}hello", "".to_string().repeat(30))
        let buf = [
            27, 91, 50, 59, 56, 72, 67, 111, 110, 102, 105, 114, 109, 27, 91, 51, 57, 109, 27, 91,
            52, 57, 109, 27, 91, 53, 57, 109, 27, 91, 48, 109, 27, 91, 63, 50, 53, 108, 27, 91, 63,
            50, 53, 104,
        ];
        print!("{}", String::from_utf8_lossy(&buf));
        print!("{:?} {:?}", rows, cols)
    }
}
