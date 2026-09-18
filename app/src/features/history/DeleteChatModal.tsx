// The one confirmation before a chat is gone for good — reached from a tab's
// menu and from the history list alike. Deleting the visible chat lands the
// strip on its neighbour (the store handles that); this only asks.

import { useState } from "react";
import { useStore } from "../../lib/store";
import { Button, Modal } from "../../components/ui";
import "./history.css";

export function DeleteChatModal({
  chat,
  onClose,
}: {
  chat: { id: string; title: string | null };
  /** Called after the delete lands, or when the user backs out. */
  onClose: () => void;
}) {
  const removeSession = useStore((s) => s.removeSession);
  const [deleting, setDeleting] = useState(false);

  async function confirm() {
    setDeleting(true);
    try {
      await removeSession(chat.id);
      onClose();
    } finally {
      setDeleting(false);
    }
  }

  return (
    <Modal title="Delete chat?" onClose={() => !deleting && onClose()}>
      <p className="delete-confirm-text">
        Permanently delete <strong>{chat.title?.trim() || "this chat"}</strong> and its messages?
        This can’t be undone.
      </p>
      <div className="delete-confirm-actions">
        <Button variant="ghost" onClick={onClose} disabled={deleting}>
          Cancel
        </Button>
        <Button variant="danger" onClick={confirm} disabled={deleting}>
          {deleting ? "Deleting…" : "Delete"}
        </Button>
      </div>
    </Modal>
  );
}
