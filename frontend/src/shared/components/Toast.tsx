export const Toast = ({ text }: { text: string }) =>
  text ? <div className="toast">{text}</div> : null;
