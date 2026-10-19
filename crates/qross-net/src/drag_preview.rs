//! Drag-Vorschau am Cursor dieses Rechners (siehe `FinderOverlay::show_drag`): Der Client zeigt sie,
//! solange Dateien vom Server über ihm gezogen werden, der Server, solange Dateien eines Clients bei
//! ihm aufs Ablegen warten. Ablegen, Abbrechen, der Cursor auf einem anderen Rechner und das Ende der
//! Verbindung blenden sie wieder aus.

use std::sync::Arc;

use qross_core::finder::DragPreview;
use qross_platform::FinderOverlay;
use tracing::debug;

use crate::files::Receiver;

/// Was gerade zu sehen ist. Endet die Sitzung, blendet sie beim Aufräumen aus (auch wenn ihr Task
/// abgebrochen wird); sonst bliebe die Vorschau stehen, denn sie läuft nicht von selbst ab.
pub(crate) struct DragView {
    overlay: Option<Arc<dyn FinderOverlay>>,
    /// Übertragung, deren Vorschau zu sehen ist
    showing: Option<u64>,
}

impl DragView {
    /// `overlay`: Anzeige dieses Rechners; `None`: hier nie etwas zeigen
    pub(crate) fn new(overlay: Option<Arc<dyn FinderOverlay>>) -> Self {
        Self { overlay, showing: None }
    }

    /// Zeigt, was mit dieser Übertragung gezogen wird, sofern sie noch aufs Ablegen wartet
    pub(crate) fn show(&mut self, transfer: u64, receiver: &Receiver) {
        let Some(overlay) = &self.overlay else { return };
        let Some(preview) = receiver.awaiting_drop(transfer).and_then(DragPreview::of) else { return };
        debug!("Drag-Vorschau für Übertragung {transfer}: {} ({})", preview.title, preview.size);
        overlay.show_drag(&preview);
        self.showing = Some(transfer);
    }

    pub(crate) fn hide(&mut self) {
        if let (Some(overlay), Some(_)) = (&self.overlay, self.showing.take()) {
            overlay.hide_drag();
        }
    }

    /// Nach jedem Ereignis: Ist die Übertragung abgebrochen oder fehlgeschlagen, gibt es nichts mehr
    /// abzulegen, und die Vorschau verschwindet.
    pub(crate) fn check(&mut self, receiver: &Receiver) {
        if self.showing.is_some_and(|transfer| receiver.awaiting_drop(transfer).is_none()) {
            debug!("Drag-Vorschau ausgeblendet: Übertragung abgebrochen");
            self.hide();
        }
    }
}

impl Drop for DragView {
    fn drop(&mut self) {
        self.hide();
    }
}
