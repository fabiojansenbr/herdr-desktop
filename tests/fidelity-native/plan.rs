//! Phases of the single native flow of spec 007 (plus the spec 010 `visual-frame` phase) and the evaluator of their outcomes.
//!
//! The page (`src/features/fidelity/e2e.ts`) reports raw observations; the parent test turns
//! them into named boolean checks ([`PhaseOutcome`]). A phase passes only when it is
//! [`Readiness::Prepared`], has an outcome, and every declared check is present and true with
//! no undeclared check. A [`Readiness::Pending`] phase never passes, whatever its outcome says:
//! placeholders are not implementation.

use std::collections::{BTreeMap, BTreeSet};

/// Where the phase's actions happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// The composed App window drives the UI (DOM/IPC) and reports observations.
    Window,
    /// The parent test acts from outside (native keys via wtype on the private display, engine
    /// reads, server swap) while the window observes.
    ParentAndWindow,
}

/// Whether the flow may execute the phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Page scenario, parent driver and parent evaluator exist (executed on the private display
    /// by `e2e_fidelity_flow`; being prepared is not passing).
    Prepared,
    /// Blocked on a seam or integration not yet available; the text names it.
    Pending(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct PhaseSpec {
    /// Phase name passed to the window (`/^[a-z][a-z0-9-]*$/`).
    pub name: &'static str,
    pub acs: &'static [&'static str],
    pub actor: Actor,
    pub readiness: Readiness,
    /// Checks the parent must compute from observations; all must be true.
    pub checks: &'static [&'static str],
}

/// The one flow, in execution order. AC-007-01 is proven by contract tests, not here.
pub const FLOW: &[PhaseSpec] = &[
    PhaseSpec {
        name: "compose-mount",
        acs: &["AC-007-02", "AC-007-03"],
        actor: Actor::Window,
        readiness: Readiness::Prepared,
        checks: &[
            "app_mounted_without_fake_bridge",
            "local_disposable_session_live",
            "terminal_active",
            "status_has_text_not_only_color",
        ],
    },
    PhaseSpec {
        name: "lazy-editor",
        acs: &["AC-007-03"],
        actor: Actor::Window,
        readiness: Readiness::Prepared,
        checks: &[
            "idle_window_at_least_2000_ms",
            "zero_editor_modules_mounted",
            "zero_editor_modules_idle",
            "editor_modules_detected_after_open",
        ],
    },
    PhaseSpec {
        name: "hosts-identity",
        acs: &["AC-007-04"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "local_and_ssh_share_pane_w1_p1",
            "projects_have_distinct_roots",
            "ssh_selected_with_friendly_label",
        ],
    },
    PhaseSpec {
        name: "ssh-agent-actions",
        acs: &["AC-007-04"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "agent_started_once_on_ssh",
            "prompt_delivered_once_on_ssh",
            "split_and_focus_confirmed_geometry_shown",
            "zero_actions_on_local",
        ],
    },
    PhaseSpec {
        name: "ssh-files-readonly",
        acs: &["AC-007-04"],
        actor: Actor::Window,
        readiness: Readiness::Prepared,
        checks: &[
            "explorer_on_selected_host",
            "diff_on_selected_host",
            "remote_marked_read_only",
        ],
    },
    PhaseSpec {
        name: "legacy-server",
        acs: &["AC-007-04"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "only_dependent_actions_unavailable",
            "terminal_preserved",
            "local_preserved",
        ],
    },
    PhaseSpec {
        name: "host-switch-dirty",
        acs: &["AC-007-04"],
        actor: Actor::Window,
        readiness: Readiness::Prepared,
        checks: &["local_dirty_buffer_preserved_after_switch"],
    },
    PhaseSpec {
        name: "native-keys",
        acs: &["AC-007-02"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "trusted_native_events",
            "accents_bytes_once",
            "emoji_bytes_once",
            "return_is_cr",
        ],
    },
    PhaseSpec {
        name: "native-ime",
        acs: &["AC-007-02"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "preedit_zero_pty_bytes",
            "cjk_candidates_commit_once",
            "keyboard_normal_after_ime",
        ],
    },
    PhaseSpec {
        name: "paste-selection",
        acs: &["AC-007-02"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "multiline_paste_once",
            "selection_copied_to_private_clipboard",
        ],
    },
    PhaseSpec {
        name: "mouse-scroll-links",
        acs: &["AC-007-02"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "alt_screen_mouse_reports_once",
            "scrollback_navigates",
            "link_open_effect_recorded_once",
        ],
    },
    PhaseSpec {
        name: "resize-dpi",
        acs: &["AC-007-02"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &["resize_geometry_confirmed", "scale_change_repaints_crisp"],
    },
    PhaseSpec {
        name: "a11y-navigation",
        acs: &["AC-007-02"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "focus_visible_on_keyboard_navigation",
            "controls_have_accessible_names",
            "state_not_color_only",
        ],
    },
    // Spec 010: the design frame, measured on the same composed window (tests/fidelity-native/
    // visual_frame.rs holds the literals and the evaluator).
    PhaseSpec {
        name: "visual-frame",
        acs: &["AC-010-01", "AC-010-02", "AC-010-03"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "frame_dimensions_at_1440x900",
            "guide_tokens_on_root",
            "ui_inter_terminal_jetbrains_mono",
            "top_bar_order_menus_and_host",
            "status_bar_real_session",
            "small_text_contrast_at_least_4_5",
            "palette_opens_within_one_frame_with_engine_lists",
            "escape_closes_and_restores_focus",
            "ctrl_k_in_terminal_reaches_pty_once",
        ],
    },
    // Spec 014: the designed agents panel, measured on the same composed window
    // (tests/fidelity-native/visual_agents.rs holds the literals and the evaluator).
    PhaseSpec {
        name: "visual-agents",
        acs: &["AC-014-01", "AC-014-02", "AC-014-03"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "counters_match_engine_states",
            "attention_card_within_500ms",
            "attention_card_shows_engine_pane_project_tab_and_line",
            "enter_focuses_the_pane_once_without_keys",
            "running_rows_ordered_by_observed_activity",
            "collapse_gives_the_center_256_px",
            "panel_small_text_contrast_at_least_4_5",
        ],
    },
    // Spec 011: projects tree, connections footer and connection dialog (tests/fidelity-native/
    // visual_projects.rs holds the literals and the evaluator).
    PhaseSpec {
        name: "visual-projects",
        acs: &["AC-011-01", "AC-011-02", "AC-011-03"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "project_tree_groups_and_projects",
            "active_project_surface3_and_accent_bar",
            "connections_footer_and_host_selection",
            "connection_dialog_layout_and_progress",
            "small_text_contrast_at_least_4_5",
        ],
    },
    // Spec 015: the review screen, measured on the same composed window (tests/fidelity-native/
    // visual_files.rs holds the literals and the evaluator).
    PhaseSpec {
        name: "visual-files",
        acs: &["AC-015-01", "AC-015-02", "AC-015-03"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "review_title_breadcrumb_and_tabs",
            "editor_lazy_before_open_in_review",
            "side_by_side_headers_prefixes_and_sets",
            "diff_line_numbers_match_their_sides",
            "diff_error_and_working_at_15_percent",
            "small_text_contrast_at_least_4_5",
            "remote_banner_read_only_and_snapshot_diff",
            "dock_height_label_and_close_returns_height",
            "dock_surface_keeps_input",
            "dock_surface_frames_painted",
        ],
    },
    // Spec 013: the center of the window (project header, workspace tabs and the frames of the
    // panes over the single surface); literals and evaluator in visual_center.rs.
    PhaseSpec {
        name: "visual-center",
        acs: &["AC-013-01", "AC-013-02", "AC-013-03"],
        actor: Actor::ParentAndWindow,
        readiness: Readiness::Prepared,
        checks: &[
            "project_header_shows_group_branch_and_path",
            "header_actions_confirmed_by_the_engine",
            "workspace_tabs_match_the_session",
            "pane_frames_match_inner_rect_at_four_stages",
            "frame_identity_state_path_and_edges",
            "frame_click_focuses_and_zoom_reaches_the_engine",
            "hidden_panes_force_no_repaint",
        ],
    },
];

/// Checks the parent computed for one phase.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseOutcome {
    pub checks: BTreeMap<String, bool>,
}

impl PhaseOutcome {
    pub fn with(checks: &[(&str, bool)]) -> Self {
        Self {
            checks: checks.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowVerdict {
    pub passed: Vec<String>,
    /// Phase → reasons (false, missing or undeclared checks).
    pub failed: BTreeMap<String, Vec<String>>,
    /// Phase → blocking seam.
    pub pending: BTreeMap<String, String>,
    /// Prepared phases without an outcome.
    pub missing: Vec<String>,
    /// Outcomes naming no phase of the plan.
    pub unknown: Vec<String>,
}

impl FlowVerdict {
    pub fn is_pass(&self, plan: &[PhaseSpec]) -> bool {
        self.failed.is_empty()
            && self.pending.is_empty()
            && self.missing.is_empty()
            && self.unknown.is_empty()
            && self.passed.len() == plan.len()
    }
}

pub fn evaluate(plan: &[PhaseSpec], outcomes: &BTreeMap<String, PhaseOutcome>) -> FlowVerdict {
    let mut verdict = FlowVerdict::default();
    let names: BTreeSet<&str> = plan.iter().map(|p| p.name).collect();
    verdict.unknown = outcomes
        .keys()
        .filter(|k| !names.contains(k.as_str()))
        .cloned()
        .collect();
    for phase in plan {
        if let Readiness::Pending(seam) = phase.readiness {
            verdict
                .pending
                .insert(phase.name.to_owned(), seam.to_owned());
            continue;
        }
        let Some(outcome) = outcomes.get(phase.name) else {
            verdict.missing.push(phase.name.to_owned());
            continue;
        };
        let mut reasons = Vec::new();
        for check in phase.checks {
            match outcome.checks.get(*check) {
                Some(true) => {}
                Some(false) => reasons.push(format!("{check}: false")),
                None => reasons.push(format!("{check}: missing")),
            }
        }
        for key in outcome.checks.keys() {
            if !phase.checks.contains(&key.as_str()) {
                reasons.push(format!("{key}: undeclared"));
            }
        }
        if reasons.is_empty() {
            verdict.passed.push(phase.name.to_owned());
        } else {
            verdict.failed.insert(phase.name.to_owned(), reasons);
        }
    }
    verdict
}

/// Structural problems of a plan (empty = well formed).
pub fn plan_problems(plan: &[PhaseSpec]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for phase in plan {
        let valid = phase
            .name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
            && phase
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            problems.push(format!("{}: invalid phase name", phase.name));
        }
        if !seen.insert(phase.name) {
            problems.push(format!("{}: duplicate phase", phase.name));
        }
        if phase.checks.is_empty() {
            problems.push(format!("{}: no checks", phase.name));
        }
        if phase.acs.is_empty() {
            problems.push(format!("{}: no AC", phase.name));
        }
        let mut checks = BTreeSet::new();
        for check in phase.checks {
            if !checks.insert(*check) {
                problems.push(format!("{}: duplicate check {check}", phase.name));
            }
        }
    }
    for ac in ["AC-007-02", "AC-007-03", "AC-007-04"] {
        if !plan.iter().any(|p| p.acs.contains(&ac)) {
            problems.push(format!("{ac}: no phase"));
        }
    }
    problems
}
