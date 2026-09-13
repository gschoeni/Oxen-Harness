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
    fn started(&self, fleet: &str, labels: &[String], cancel: CancellationToken) {
        self.hub
            .install(fleet, FleetState::new(labels, Some(cancel)));
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
            // Cooked-mode context: nobody else paints, so we do — one painter
            // for however many fleets overlap; it shows the oldest.
            let mut painter = self.painter.lock().expect("fleet painter poisoned");
            if painter.is_none() {
                *painter = Some(BlockPainter::start(&self.ui, self.hub.clone()));
            }
        }
    }

    fn event(&self, fleet: &str, event: &FleetEvent) {
        crate::fleet_ui::apply_fleet_event(&self.hub, &self.ui, self.plain(), fleet, event);
    }

    fn finished(&self, fleet: &str) {
        self.hub.remove(fleet);
        if self.hub.lock().is_empty() {
            if let Some(painter) = self.painter.lock().expect("fleet painter poisoned").take() {
                painter.finish();
            }
        }
    }
}
