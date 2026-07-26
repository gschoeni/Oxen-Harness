// The one Remove-project confirm, shared by every surface that offers the
// action (the project page's corner trash, the quiet rows, the card grid) —
// so the wording and the "removal ≠ deletion" promise can't drift between
// copies. Owns its in-flight state; the caller owns what "confirmed" does.

import { useState } from "react";
import { Button, Modal } from "../../components/ui";

export function RemoveProjectModal({
  name,
  onCancel,
  onConfirm,
}: {
  /** The project name the confirm text calls out. */
  name: string;
  onCancel: () => void;
  /** Perform the removal (and any navigation after it). Errors re-enable the
   *  buttons so the user can retry or cancel. */
  onConfirm: () => Promise<void>;
}) {
  const [removing, setRemoving] = useState(false);

  async function confirm() {
    setRemoving(true);
    try {
      await onConfirm();
    } finally {
      setRemoving(false);
    }
  }

  return (
    <Modal title="Remove project?" onClose={() => !removing && onCancel()}>
      <p className="delete-confirm-text">
        Remove <strong>{name}</strong> from your projects? Its folder and chat history stay on
        disk — it just won’t be listed here anymore.
      </p>
      <div className="delete-confirm-actions">
        <Button variant="ghost" onClick={onCancel} disabled={removing}>
          Cancel
        </Button>
        <Button variant="danger" onClick={() => void confirm()} disabled={removing}>
          {removing ? "Removing…" : "Remove"}
        </Button>
      </div>
    </Modal>
  );
}
