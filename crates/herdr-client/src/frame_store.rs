//! Committed pane surface with patch validation identical to the reference client
//! (`src/client/shell/surface_patch.rs`): boot, projection, base revision, +1 revision,
//! pane geometry and row containment. A rejected patch marks the surface stale; recovery
//! is a new full surface, never a replay of input.

use herdr_protocol::wire::{
    CursorState, PaneSurfaceFrame, PaneSurfacePane, PaneSurfacePatch, PaneSurfacePatchRow,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleReason {
    NoBaseSurface,
    BootChanged,
    ProjectionMismatch,
    RevisionGap,
    InvalidPatch,
    QueueOverflow,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum SurfaceState {
    Empty,
    Live,
    Stale(StaleReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Rejected(StaleReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Ask the endpoint for a complete surface (resize with the current geometry or
    /// re-attach). Buffered input is not replayed.
    RequestFullSurface,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FrameStats {
    pub full_frames: u64,
    pub patches_applied: u64,
    pub patches_rejected: u64,
}

#[derive(Debug)]
pub struct FrameStore {
    surface: Option<PaneSurfaceFrame>,
    state: SurfaceState,
    interest: bool,
    stats: FrameStats,
    /// One full surface must be requested for the current stale episode.
    recovery_requested: bool,
}

impl Default for FrameStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameStore {
    pub fn new() -> Self {
        Self {
            surface: None,
            state: SurfaceState::Empty,
            interest: true,
            stats: FrameStats::default(),
            recovery_requested: false,
        }
    }

    pub fn state(&self) -> SurfaceState {
        self.state
    }

    pub fn stats(&self) -> FrameStats {
        self.stats
    }

    pub fn surface(&self) -> Option<&PaneSurfaceFrame> {
        self.surface.as_ref()
    }

    pub fn revision(&self) -> Option<u64> {
        self.surface.as_ref().map(|s| s.surface_revision)
    }

    pub fn boot_id(&self) -> Option<&str> {
        self.surface.as_ref().map(|s| s.boot_id.as_str())
    }

    /// Surface interest: hidden surfaces keep the last frame but neither repaint nor
    /// accept input.
    pub fn set_interest(&mut self, active: bool) {
        self.interest = active;
    }

    pub fn interest(&self) -> bool {
        self.interest
    }

    /// Input is allowed only on a live, interested surface.
    pub fn input_allowed(&self) -> bool {
        self.interest && matches!(self.state, SurfaceState::Live)
    }

    pub fn recovery(&self) -> Option<RecoveryAction> {
        match self.state {
            SurfaceState::Stale(_) => Some(RecoveryAction::RequestFullSurface),
            _ => None,
        }
    }

    /// Whether one full surface must be requested now. True once per stale episode: further
    /// rejected patches of the same episode do not ask again (no request storm). Buffered or
    /// refused input is never part of the recovery.
    pub fn take_recovery_request(&mut self) -> bool {
        std::mem::take(&mut self.recovery_requested)
    }

    /// Marks the surface stale from outside the frame sequence (queue overflow, disconnect).
    /// Always re-arms the request: the full surface of a running episode may have been lost.
    pub fn mark_stale(&mut self, reason: StaleReason) {
        self.state = SurfaceState::Stale(reason);
        self.recovery_requested = true;
    }

    fn enter_stale(&mut self, reason: StaleReason) {
        if !matches!(self.state, SurfaceState::Stale(_)) {
            self.recovery_requested = true;
        }
        self.state = SurfaceState::Stale(reason);
    }

    /// Commits a complete surface. Returns whether the boot id changed relative to the
    /// previously committed surface (callers must re-qualify targets when it did).
    pub fn apply_full(&mut self, frame: PaneSurfaceFrame) -> Result<bool, StaleReason> {
        if !frame.frame.is_consistent() {
            self.enter_stale(StaleReason::InvalidPatch);
            return Err(StaleReason::InvalidPatch);
        }
        let boot_changed = self
            .surface
            .as_ref()
            .is_some_and(|previous| previous.boot_id != frame.boot_id);
        self.surface = Some(frame);
        self.state = SurfaceState::Live;
        self.recovery_requested = false;
        self.stats.full_frames += 1;
        Ok(boot_changed)
    }

    pub fn apply_patch(&mut self, patch: PaneSurfacePatch) -> ApplyOutcome {
        if let Some(reason) = self.validate_patch(&patch).err() {
            self.stats.patches_rejected += 1;
            self.enter_stale(reason);
            return ApplyOutcome::Rejected(reason);
        }
        let surface = self.surface.as_mut().expect("validated");
        for row in &patch.rows {
            apply_row(row, surface);
        }
        for updated in &patch.panes {
            if let Some(existing) = surface
                .panes
                .iter_mut()
                .find(|p| p.pane_id == updated.pane_id)
            {
                *existing = updated.clone();
            }
        }
        surface.frame.cursor = patch.cursor.clone();
        surface.surface_revision = patch.surface_revision;
        self.stats.patches_applied += 1;
        if matches!(self.state, SurfaceState::Live) {
            ApplyOutcome::Applied
        } else {
            // A patch never resurrects a stale surface; only a full frame does.
            ApplyOutcome::Rejected(match self.state {
                SurfaceState::Stale(reason) => reason,
                _ => StaleReason::NoBaseSurface,
            })
        }
    }

    fn validate_patch(&self, patch: &PaneSurfacePatch) -> Result<(), StaleReason> {
        let Some(current) = self.surface.as_ref() else {
            return Err(StaleReason::NoBaseSurface);
        };
        if let SurfaceState::Stale(reason) = self.state {
            return Err(reason);
        }
        if patch.boot_id != current.boot_id {
            return Err(StaleReason::BootChanged);
        }
        if patch.projection_revision != current.projection_revision {
            return Err(StaleReason::ProjectionMismatch);
        }
        if patch.base_surface_revision != current.surface_revision
            || patch.surface_revision != current.surface_revision.saturating_add(1)
        {
            return Err(StaleReason::RevisionGap);
        }
        if current.popup.is_some()
            || !current.graphics.placements.is_empty()
            || !current.graphics.retained_assets.is_empty()
        {
            return Err(StaleReason::InvalidPatch);
        }
        for updated in &patch.panes {
            let Some(existing) = current.panes.iter().find(|p| p.pane_id == updated.pane_id) else {
                return Err(StaleReason::InvalidPatch);
            };
            if !pane_geometry_matches(existing, updated) {
                return Err(StaleReason::InvalidPatch);
            }
        }
        for row in &patch.rows {
            if !row_fits_frame(row, current) || row.cells.is_empty() {
                return Err(StaleReason::InvalidPatch);
            }
            let inside_some_pane = patch.panes.iter().any(|pane| {
                let row_end = row
                    .x
                    .saturating_add(row.cells.len().min(u16::MAX as usize) as u16);
                let terminal_row = row.x >= pane.inner_rect.x
                    && row.y >= pane.inner_rect.y
                    && row.y < pane.inner_rect.y.saturating_add(pane.inner_rect.height)
                    && row_end <= pane.inner_rect.x.saturating_add(pane.inner_rect.width);
                let scrollbar_rect = pane.scrollbar_rect.or_else(|| {
                    current
                        .panes
                        .iter()
                        .find(|existing| existing.pane_id == pane.pane_id)
                        .and_then(|existing| existing.scrollbar_rect)
                });
                let scrollbar_row = scrollbar_rect.is_some_and(|rect| {
                    row.x == rect.x
                        && row.y >= rect.y
                        && row.y < rect.y.saturating_add(rect.height)
                        && row.cells.len() == usize::from(rect.width)
                });
                terminal_row || scrollbar_row
            });
            if !inside_some_pane {
                return Err(StaleReason::InvalidPatch);
            }
        }
        Ok(())
    }

    /// Row-major text of the committed surface (wide-char continuation cells excluded),
    /// used by tests, traces and the resource report.
    pub fn text_rows(&self) -> Vec<String> {
        let Some(surface) = self.surface.as_ref() else {
            return Vec::new();
        };
        let width = usize::from(surface.frame.width);
        if width == 0 {
            return Vec::new();
        }
        surface
            .frame
            .cells
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol.as_str()).collect::<String>())
            .collect()
    }

    pub fn cursor(&self) -> Option<&CursorState> {
        self.surface.as_ref().and_then(|s| s.frame.cursor.as_ref())
    }
}

fn pane_geometry_matches(left: &PaneSurfacePane, right: &PaneSurfacePane) -> bool {
    left.pane_id == right.pane_id
        && left.rect == right.rect
        && left.inner_rect == right.inner_rect
        && left.focused == right.focused
        && left.pixel_width == right.pixel_width
        && left.pixel_height == right.pixel_height
}

fn row_fits_frame(row: &PaneSurfacePatchRow, surface: &PaneSurfaceFrame) -> bool {
    row.x
        .saturating_add(row.cells.len().min(u16::MAX as usize) as u16)
        <= surface.frame.width
        && row.y < surface.frame.height
}

fn apply_row(row: &PaneSurfacePatchRow, surface: &mut PaneSurfaceFrame) {
    let width = usize::from(surface.frame.width);
    let start = usize::from(row.y) * width + usize::from(row.x);
    let end = start + row.cells.len();
    if end <= surface.frame.cells.len() {
        surface.frame.cells[start..end].clone_from_slice(&row.cells);
    }
}
