//! The CLI's [`FleetSink`]: how a `spawn_agents` fleet (launched by the model
//! from inside any turn) reaches the screen.
//!
//! The sink publishes lane state into the process-wide [`FleetHub`] and picks
//! a display for the context the turn is running in:
//!
//! - **Live composer** (interactive turns): the composer already owns the
//!   terminal, paints the hub's block in its pinned area, and drives lane
//!   focus with alt+digits — the sink only feeds state.
//! - **Cooked mode** (e.g. a fleet during `oxen-harness loop run`): the sink
//!   starts its own [`BlockPainter`], which paints in place and owns the
//!   keyboard (1-9 focus, esc overview, ctrl-c stop).
//! - **Plain terminals** (pipes, `NO_COLOR`): milestone lines only.

use std::sync::{Arc, Mutex as StdMutex};

use harness_agent::fleet::{FleetEvent, FleetSink};
use tokio_util::sync::CancellationToken;

use crate::fleet_ui::{BlockPainter, FleetHub, FleetState};
use crate::theme::Ui;

pub(crate) struct CliFleetSink {
    ui: Ui,
    hub: Arc<FleetHub>,
    /// The cooked-mode painter, when this sink had to start one.
    painter: StdMutex<Option<BlockPainter>>,
}

impl CliFleetSink {
    pub(crate) fn new(ui: Ui) -> Self {
        Self {
            ui,
            hub: FleetHub::global(),
            painter: StdMutex::new(None),
        }
    }

    fn plain(&self) -> bool {
        !self.ui.animates()
    }
}

impl FleetSink for CliFleetSink {
    fn started(&self, labels: &[String], cancel: CancellationToken) {
        self.hub.install(FleetState::new(labels, Some(cancel)));
        if self.plain() {
            println!(
                "  {} {}",
                self.ui.green("🐂"),
                self.ui.dim(&format!(
                    "spawning {} agents: {}",
                    labels.len(),
                    labels.join(", ")
                )),
            );
        } else if !self.hub.is_live() {
            // Cooked-mode context: nobody else paints, so we do.
            *self.painter.lock().expect("fleet painter poisoned") =
                Some(BlockPainter::start(&self.ui, self.hub.clone()));
        }
    }

    fn event(&self, event: &FleetEvent) {
        match event {
            FleetEvent::TaskStarted { index, label } => {
                if let Some(state) = self.hub.lock().as_mut() {
                    state.lane_started(*index);
                }
                if self.plain() {
                    crate::fleet_ui::print_lane_started(&self.ui, label);
                }
            }
            FleetEvent::Agent { index, event } => {
                if let Some(state) = self.hub.lock().as_mut() {
                    state.lane_event(*index, event, &self.ui);
                }
            }
            FleetEvent::TaskCompleted {
                index,
                label,
                ok,
                tokens_used,
                summary,
            } => {
                if let Some(state) = self.hub.lock().as_mut() {
                    state.lane_completed(*index, *ok, *tokens_used, summary);
                }
                if self.plain() {
                    crate::fleet_ui::print_lane_completed(
                        &self.ui,
                        label,
                        *ok,
                        *tokens_used,
                        summary,
                    );
                }
            }
        }
    }

    fn finished(&self) {
        if let Some(painter) = self.painter.lock().expect("fleet painter poisoned").take() {
            painter.finish();
        }
        self.hub.clear();
    }
}
