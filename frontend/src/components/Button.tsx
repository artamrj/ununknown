import type { ReactNode } from "react";

export const Button = ({
  children,
  kind = "primary",
  ...props
}: { children: ReactNode; kind?: string } & React.ButtonHTMLAttributes<HTMLButtonElement>) => (
  <button className={`button ${kind}`} {...props}>
    {children}
  </button>
);
