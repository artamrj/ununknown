import {useEffect} from "react";
import {useQueryClient} from "@tanstack/react-query";
export function useEvents(){const q=useQueryClient();useEffect(()=>{let timer:number|undefined;const e=new EventSource("/api/events");e.onmessage=()=>{window.clearTimeout(timer);timer=window.setTimeout(()=>q.invalidateQueries({queryKey:["tracks"]}),180)};return()=>{window.clearTimeout(timer);e.close()}},[q])}
