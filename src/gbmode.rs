#[derive(PartialEq, Copy, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum GbMode {
    Classic,
    Color,
    ColorAsClassic,
}

#[derive(PartialEq, Copy, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum GbSpeed {
    Single = 1,
    Double = 2,
}
