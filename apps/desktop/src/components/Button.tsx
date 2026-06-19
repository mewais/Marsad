import React from "react";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "success";
export type ButtonSize = "sm" | "md" | "lg";

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  children: React.ReactNode;
}

const variantStyles: Record<ButtonVariant, React.CSSProperties> = {
  primary: {
    background: "var(--accent)",
    color: "#fff",
    border: "none",
  },
  secondary: {
    background: "var(--panel2)",
    color: "var(--txt2)",
    border: "1px solid var(--border2)",
  },
  ghost: {
    background: "transparent",
    color: "var(--txt2)",
    border: "1px solid var(--border2)",
  },
  danger: {
    background: "var(--error-bg)",
    color: "var(--error)",
    border: "1px solid var(--error)",
  },
  success: {
    background: "var(--done)",
    color: "#0e1116",
    border: "none",
  },
};

const sizeStyles: Record<ButtonSize, React.CSSProperties> = {
  sm: { fontSize: 11, padding: "5px 10px", borderRadius: 7 },
  md: { fontSize: 12.5, padding: "8px 14px", borderRadius: 9 },
  lg: { fontSize: 13, padding: "10px 18px", borderRadius: 10 },
};

export function Button({
  variant = "secondary",
  size = "md",
  children,
  style,
  ...rest
}: ButtonProps) {
  return (
    <button
      style={{
        fontFamily: "var(--font-sans, inherit)",
        fontWeight: 600,
        cursor: "pointer",
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        ...variantStyles[variant],
        ...sizeStyles[size],
        ...style,
      }}
      {...rest}
    >
      {children}
    </button>
  );
}
