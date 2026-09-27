fn main() {
    let path = std::env::args().nth(1).expect("give me a file path");
    let mut player = player_core::Player::new().unwrap();
    player.load(std::path::Path::new(&path)).unwrap();
    player.play();
    std::thread::sleep(std::time::Duration::from_secs(30)); // hold the process open
}