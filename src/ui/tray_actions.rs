//! The tray menu's actions. gpui-tray dispatches ordinary GPUI actions, one per menu
//! item, which is how the WinForms `ContextMenuStrip` click handlers are modelled.

gpui_kit::actions!(orbital_tray, [TraySettings, TrayRestart, TrayExit]);

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Action as _;

    #[test]
    fn actions_have_distinct_names() {
        // Registering two actions under the same name panics during `App` creation,
        // so each menu item needs its own type.
        assert_ne!(TraySettings.name(), TrayRestart.name());
        assert_ne!(TraySettings.name(), TrayExit.name());
        assert_ne!(TrayRestart.name(), TrayExit.name());
    }
}
