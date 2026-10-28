use window_zones::{
    DisplayGeometry, FocusedWindow, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError,
};

/// Simulated desktop for `--backend dry-run`: one window on two displays that
/// moves when asked, so actions and their sequences can be checked safely.
#[derive(Debug)]
pub(in crate::application) struct DryRunWindowSystem {
    focused_window: FocusedWindow,
    displays: Vec<DisplayGeometry>,
    pub(super) last_move: Option<WindowMove>,
}

impl DryRunWindowSystem {
    pub(in crate::application) fn new() -> Self {
        Self {
            focused_window: FocusedWindow::new(
                WindowId::new("dry-run-window"),
                Rect::new(40, 40, 640, 480),
            ),
            displays: vec![
                DisplayGeometry::new("display-0", Rect::new(0, 0, 1920, 1080)),
                DisplayGeometry::new("display-1", Rect::new(1920, 0, 1280, 1024)),
            ],
            last_move: None,
        }
    }
}

impl WindowSystem for DryRunWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        Ok(Some(self.focused_window.clone()))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        Ok(self.displays.clone())
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        if window_move.window != self.focused_window.id {
            return Err(WindowSystemError::WindowGone(window_move.window.clone()));
        }
        self.focused_window.geometry = window_move.target;
        self.last_move = Some(window_move.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use window_zones::{Rect, WindowId, WindowMove, WindowSystem, WindowSystemError};

    use super::super::{RuntimeBackend, RuntimeWindowSystem};

    #[test]
    fn runtime_dry_run_rejects_a_closed_window_id_without_moving_the_focus() {
        let mut system = RuntimeWindowSystem::with_backend(RuntimeBackend::DryRun);
        let before = system.focused_window().unwrap();
        let window_move =
            WindowMove::new(WindowId::new("closed-window"), Rect::new(0, 0, 100, 100));
        assert_eq!(
            system.move_window(&window_move),
            Err(WindowSystemError::WindowGone(window_move.window.clone()))
        );
        assert_eq!(system.focused_window().unwrap(), before);
        assert!(system.last_move().is_none());
    }
}
