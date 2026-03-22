#[derive(Debug, Clone)]
pub enum VfsNode {
    File(Vec<u8>),
    Dir,
}
