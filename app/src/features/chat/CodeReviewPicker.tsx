// A compact dropdown in the composer for running the code-review pipeline
// (find → verify → report) on this chat's workspace: the working diff with one
// click, or PR-style against a typed base branch. The findings land in the
// thread as a settled exchange, so "fix 1 and 3" works as the next message.
// Disabled mid-turn — a review holds the same agent lock a turn does.

import { useState, type FormEvent } from "react";
import { ArrowRight, ChevronDown, SearchCode, Settings2 } from "lucide-react";
import { Menu, MenuHead, MenuItem, MenuSep, useMenuState } from "../../components/ui/Menu";
import { useStore } from "../../lib/store";

export function CodeReviewPicker({ disabled }: { disabled: boolean }) {
  const startCodeReview = useStore((s) => s.startCodeReview);
  const openSettings = useStore((s) => s.openSettings);
  const { open, setOpen, ref } = useMenuState();
  const [branch, setBranch] = useState("");

  function run(baseBranch?: string) {
    setOpen(false);
    setBranch("");
    startCodeReview(baseBranch);
  }

  function submitBranch(e: FormEvent) {
    e.preventDefault();
    const b = branch.trim();
    if (b) run(b);
  }

  return (
    <div className="picker" ref={ref}>
      <button
        type="button"
        className="picker-btn"
        onClick={() => setOpen((o) => !o)}
        disabled={disabled}
        title={
          disabled
            ? "Finish the current turn to run a code review"
            : "Code review: find → verify → report on your changes"
        }
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <SearchCode size={13} />
        <span className="picker-label">Review</span>
        <ChevronDown size={13} className="picker-caret" />
      </button>

      {open && (
        <Menu className="picker-menu">
          <MenuHead>Code review</MenuHead>
          <MenuItem
            name="Uncommitted changes"
            hint="staged, unstaged & untracked"
            onSelect={() => run()}
          />
          <form className="review-branch-row" onSubmit={submitBranch}>
            <input
              className="review-branch-input"
              value={branch}
              placeholder="Against a base branch, e.g. main"
              onChange={(e) => setBranch(e.target.value)}
              aria-label="Base branch to review against"
            />
            <button
              type="submit"
              className="review-branch-go"
              disabled={!branch.trim()}
              aria-label="Review against this branch"
            >
              <ArrowRight size={14} />
            </button>
          </form>
          <MenuSep />
          <MenuItem
            manage
            checkSlot={<Settings2 size={15} className="menu-check" />}
            name="Review settings…"
            onSelect={() => {
              setOpen(false);
              openSettings("code-review");
            }}
          />
        </Menu>
      )}
    </div>
  );
}
