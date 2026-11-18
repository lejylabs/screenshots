#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Capture,
    Region,
    EditRegion,
    ActiveWindow,
    ActiveDisplay,
    RepeatRegion,
    Show,
    Quit,
}
