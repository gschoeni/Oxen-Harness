// Design-system primitives. Token-driven, no feature logic.
import "./ui.css";
import { X } from "lucide-react";
import type { ButtonHTMLAttributes, ReactNode } from "react";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary" | "ghost" | "danger";
  size?: "sm" | "md";
};

export function Button({ variant = "default", size = "md", className = "", ...rest }: ButtonProps) {
  const cls = ["btn", variant !== "default" ? variant : "", size !== "md" ? size : "", className]
    .filter(Boolean)
    .join(" ");
  return <button className={cls} {...rest} />;
}

export function IconButton({
  active,
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { active?: boolean }) {
  return <button className={`icon-btn ${active ? "active" : ""} ${className}`} {...rest} />;
}

export function Spinner() {
  return <div className="spinner" role="status" aria-label="Loading" />;
}

export function Empty({
  icon,
  title,
  children,
}: {
  icon?: ReactNode;
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="empty">
      {icon && <div className="empty-icon">{icon}</div>}
      <h3 className="empty-title">{title}</h3>
      {children && <div className="empty-sub">{children}</div>}
    </div>
  );
}

export function Modal({
  title,
  onClose,
  wide,
  children,
}: {
  title: string;
  onClose: () => void;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="modal-scrim" onClick={onClose}>
      <div
        className={`modal ${wide ? "wide" : ""}`}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <div className="modal-header">
          <h2 className="modal-title">{title}</h2>
          <IconButton onClick={onClose} aria-label="Close">
            <X size={20} />
          </IconButton>
        </div>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}
