pub type ClientId = Vec<u8>;
#[derive(Debug, Clone)]
pub enum ServerEvent {
    ClientConnected(String),
    ClientMessage(String, String),
    ClientList(Vec<String>),
    Log(String),
}
