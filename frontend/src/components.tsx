import type {ReactNode} from "react";
export const Button=({children,kind="primary",...p}:{children:ReactNode;kind?:string}&React.ButtonHTMLAttributes<HTMLButtonElement>)=><button className={`button ${kind}`} {...p}>{children}</button>;
export const Spinner=()=> <span className="spinner"/>;
export const Toast=({text}:{text:string})=>text?<div className="toast">{text}</div>:null;
