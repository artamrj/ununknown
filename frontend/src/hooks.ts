import {useEffect} from "react";
import {useQueryClient} from "@tanstack/react-query";
export function useEvents(){const q=useQueryClient();useEffect(()=>{const e=new EventSource("/api/events");e.onmessage=()=>{q.invalidateQueries({queryKey:["workspace"]});q.invalidateQueries({queryKey:["tracks"]})};return()=>e.close()},[q])}
