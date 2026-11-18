#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCommand {
    Primary,
    Region,
    EditRegion,
    ActiveWindow,
    RepeatRegion,
    ActiveDisplay,
}

impl CaptureCommand {
    pub(crate) fn label(self) -> &'static str {
        crate::CaptureHotkeys::LABELS[match self {
            Self::Primary => 0,
            Self::Region => 1,
            Self::EditRegion => 2,
            Self::ActiveWindow => 3,
            Self::RepeatRegion => 4,
            Self::ActiveDisplay => 5,
        }]
    }
}
