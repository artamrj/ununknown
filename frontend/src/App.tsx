import {useEffect,useState} from "react";
import {useMutation,useQuery,useQueryClient} from "@tanstack/react-query";
import {api,Preview,TrackPage,Workflow} from "./api";
import {useEvents} from "./hooks";
import {Button,Spinner,Toast} from "./components";
import {Settings} from "./Settings";

const steps=["Scan","Fetch","Preview","Apply","Finish"];
const phaseIndex=(phase:string)=>phase==="idle"?0:phase==="scan"?0:phase==="fetch"?1:phase==="preview"?2:phase==="apply"?3:4;

export function App(){
  useEvents();
  const q=useQueryClient();
  const [settingsPage,setSettingsPage]=useState(false);
  const [preview,setPreview]=useState<Preview>();
  const [toast,setToast]=useState("");
  const settings=useQuery({queryKey:["settings"],queryFn:()=>api<any>("/settings")});
  const workflow=useQuery({queryKey:["workspace"],queryFn:()=>api<Workflow>("/workspace"),refetchInterval:2000});
  const tracks=useQuery({queryKey:["tracks"],queryFn:()=>api<TrackPage>("/tracks?page=1&page_size=200")});
  const mutate=(path:string,body="{}")=>useMutation({mutationFn:()=>api<any>(path,{method:"POST",body}),onSuccess:()=>q.invalidateQueries(),onError:e=>setToast(e.message)});
  const scan=mutate("/scan/start"),stop=mutate("/scan/stop");
  const makePreview=useMutation({mutationFn:()=>api<Preview>("/apply/preview",{method:"POST",body:"{}"}),onSuccess:setPreview,onError:e=>setToast(e.message)});
  const apply=useMutation({mutationFn:()=>api("/apply/start",{method:"POST",body:JSON.stringify({preview_token:preview?.preview_token})}),onSuccess:()=>{setPreview(undefined);q.invalidateQueries()},onError:e=>setToast(e.message)});
  useEffect(()=>{if(workflow.data?.phase==="preview"&&!preview&&workflow.data.matched>0)makePreview.mutate()},[workflow.data?.phase]);
  if(settingsPage)return <Settings settings={settings.data} back={()=>setSettingsPage(false)}/>;
  const w=workflow.data,active=!!w&&["scan","fetch","apply"].includes(w.phase),items=tracks.data?.items||[];
  return <><header className="topbar"><b><i>U</i> Ununknown <small>0.3.0</small></b><span>{settings.data?.input_dir}</span><Button kind="quiet" onClick={()=>setSettingsPage(true)}>Settings</Button></header>
  <main className="pipeline">
    <nav className="stepper">{steps.map((s,i)=><div className={i<phaseIndex(w?.phase||"idle")?"done":i===phaseIndex(w?.phase||"idle")?"active":""} key={s}><span>{i+1}</span><strong>{s}</strong></div>)}</nav>
    {!w||workflow.isLoading?<section className="state-card"><Spinner/> Loading workspace</section>:
    w.phase==="idle"?<section className="hero state-card"><span className="eyebrow">Local metadata repair</span><h1>Match your music, one track at a time.</h1><p>Ununknown discovers every audio file first, then identifies them sequentially without saving unmatched files.</p><Button onClick={()=>scan.mutate()}>Scan music</Button></section>:
    ["scan","fetch","apply"].includes(w.phase)?<section className="process state-card"><div className="process-icon"><Spinner/></div><span className="eyebrow">{w.phase}</span><h1>{w.message}</h1><p className="current-file">{w.current_file||"Reading mounted music folder..."}</p><div className="counter"><strong>{w.current}</strong><span>of {w.total||"-"}</span></div><div className="summary"><span>{w.matched} matched</span><span>{w.unmatched} unmatched</span><span>{w.failed} failed</span></div><Button kind="danger" onClick={()=>stop.mutate()}>Stop</Button></section>:
    w.phase==="failed"?<section className="state-card hero"><span className="eyebrow error">Workflow error</span><h1>Processing stopped</h1><p>{w.message}</p><Button onClick={()=>scan.mutate()}>Start new scan</Button></section>:
    <section className="results state-card"><header><div><span className="eyebrow">{w.phase==="finish"?"Finished":"Preview"}</span><h1>{w.phase==="finish"?"Metadata apply complete":`${w.matched} matched tracks ready`}</h1><p>{w.unmatched} unmatched · {w.failed} failed{preview?.summary?.duplicate_skipped?` · ${preview.summary.duplicate_skipped} duplicate skipped`:""}</p></div><Button kind="quiet" onClick={()=>confirm("Clear this preview and start a new scan?")&&scan.mutate()}>Start new scan</Button></header>
    {w.phase!=="finish"&&<div className="preview-list">{items.map(t=>{const c=t.candidates.find(x=>x.id===t.selected_candidate_id),p=preview?.items.find(x=>x.track_id===t.id),skip=p?.duplicate_action==="skip_duplicate";return <article className={skip?"duplicate-skip":""} key={t.id}><div><strong>{t.filename}</strong><span>{t.current_artist||"Unknown artist"} · {t.current_title||"Unknown title"}</span></div><b>{skip?"x":"->"}</b><div><strong>{c?.title}</strong><span>{c?.artist} · {c?.album||"Unknown album"} · {Math.round(c?.score||0)}%</span>{p?.duplicate_reason&&<em>{p.duplicate_reason}</em>}</div></article>})}</div>}
    {w.phase!=="finish"&&preview&&<div className="apply-bar"><span>Dry-run ready for {preview.summary?.write_count??preview.items.length} writes{preview.summary?.duplicate_skipped?` · ${preview.summary.duplicate_skipped} duplicates skipped`:""}</span><Button disabled={!(preview.summary?.write_count??preview.items.length)||apply.isPending} onClick={()=>confirm("Apply these metadata changes? Duplicate skips will not be written.")&&apply.mutate()}>{apply.isPending?"Applying...":"Apply changes"}</Button></div>}</section>}
    {active&&<div className="sr-only">Processing</div>}
  </main><Toast text={toast}/></>
}
