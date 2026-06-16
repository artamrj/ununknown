import {useEffect,useState} from "react";
import {useQueryClient} from "@tanstack/react-query";
import type {TerminalLine,Workflow} from "./api";

type ServerEvent={
  kind:string;
  stage?:Workflow["phase"]|string;
  level?:string;
  file?:string;
  timestamp?:string;
  phase?:Workflow["phase"]|string;
  current_file?:string;
  processed?:number;
  matched?:number;
  unmatched?:number;
  failed?:number;
  current:number;
  total:number;
  message:string;
};

export type EventStatus="connecting"|"connected"|"reconnecting";

const phases=new Set(["idle","scan","fetch","preview","apply","finish","failed"]);
const emptyWorkflow=():Workflow=>({
  phase:"idle",
  message:"Ready to scan",
  current:0,
  total:0,
  processed:0,
  matched:0,
  unmatched:0,
  failed:0,
  terminal_log:[],
});

export function useEvents():EventStatus{
  const q=useQueryClient();
  const [status,setStatus]=useState<EventStatus>("connecting");
  useEffect(()=>{
    const e=new EventSource("/api/events");
    e.onmessage=(message)=>{
      setStatus("connected");
      const event=JSON.parse(message.data) as ServerEvent;
      q.setQueryData<Workflow>(["workspace"],old=>{
        old=old||emptyWorkflow();
        if(event.kind==="terminal"){
          const line:TerminalLine={
            timestamp:event.timestamp||new Date().toISOString(),
            level:event.level||"info",
            stage:event.stage||"fetch",
            file:event.file,
            message:event.message,
          };
          return {
            ...old,
            phase:event.phase&&phases.has(event.phase)?event.phase as Workflow["phase"]:old.phase,
            current_file:event.current_file||event.file||old.current_file,
            current:event.current||old.current,
            total:event.total||old.total,
            processed:event.processed??old.processed,
            matched:event.matched??old.matched,
            unmatched:event.unmatched??old.unmatched,
            failed:event.failed??old.failed,
            terminal_log:[...(old.terminal_log||[]),line].slice(-160),
          };
        }
        const phase=event.stage&&phases.has(event.stage)?event.stage as Workflow["phase"]:old.phase;
        return {
          ...old,
          phase,
          message:event.message||old.message,
          current:event.current||old.current,
          total:event.total||old.total,
          current_file:event.current_file||event.file||(phase==="fetch"&&event.message?event.message:old.current_file),
          processed:event.processed??old.processed,
          matched:event.matched??old.matched,
          unmatched:event.unmatched??old.unmatched,
          failed:event.failed??old.failed,
        };
      });
      if(["preview","finish","failed","done"].includes(event.kind)||event.stage==="preview"||event.stage==="finish"){
        q.invalidateQueries({queryKey:["workspace"]});
        q.invalidateQueries({queryKey:["tracks"]});
      }
    };
    e.onopen=()=>setStatus("connected");
    e.onerror=()=>{setStatus("reconnecting");q.invalidateQueries({queryKey:["workspace"]})};
    return()=>{setStatus("connecting");e.close()};
  },[q]);
  return status;
}
