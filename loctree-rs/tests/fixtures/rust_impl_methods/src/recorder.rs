pub struct Recorder;

impl Recorder {
    pub fn start(&self) {
        println!("start");
    }

    fn stop(&self) {
        println!("stop");
    }
}
