import {useEffect,useRef,useState} from "react";
import {useMutation,useQuery,useQueryClient} from "@tanstack/react-query";
import {api,MetadataSummary,Preview,PreviewItem,TerminalLine,Workflow} from "./api";
import {EventStatus,useEvents} from "./hooks";
import {Button,Spinner,Toast} from "./components";
import {Settings} from "./Settings";

const steps=["Scan","Fetch","Preview","Apply","Finish"];
const phaseIndex=(phase:string)=>phase==="idle"?0:phase==="scan"?0:phase==="fetch"?1:phase==="preview"?2:phase==="apply"?3:4;

export function App(){
  const eventStatus=useEvents();
  const q=useQueryClient();
  const [settingsPage,setSettingsPage]=useState(false);
  const [preview,setPreview]=useState<Preview>();
  const [toast,setToast]=useState("");
  const settings=useQuery({queryKey:["settings"],queryFn:()=>api<any>("/settings")});
  const workflow=useQuery({queryKey:["workspace"],queryFn:()=>api<Workflow>("/workspace"),refetchInterval:1500});
  const mutate=(path:string,body="{}")=>useMutation({mutationFn:()=>api<any>(path,{method:"POST",body}),onSuccess:()=>q.invalidateQueries(),onError:e=>setToast(e.message)});
  const scan=mutate("/scan/start"),stop=mutate("/scan/stop");
  const makePreview=useMutation({mutationFn:()=>api<Preview>("/apply/preview",{method:"POST",body:"{}"}),onSuccess:setPreview,onError:e=>setToast(e.message)});
  const apply=useMutation({mutationFn:()=>api("/apply/start",{method:"POST",body:JSON.stringify({preview_token:preview?.preview_token})}),onSuccess:()=>{setPreview(undefined);q.invalidateQueries()},onError:e=>setToast(e.message)});
  useEffect(()=>{if(workflow.data?.phase==="preview"&&!preview&&workflow.data.matched>0)makePreview.mutate()},[workflow.data?.phase]);
  if(settingsPage)return <Settings settings={settings.data} back={()=>setSettingsPage(false)}/>;
  const w=workflow.data,active=!!w&&["scan","fetch","apply"].includes(w.phase);
  return <><header className="topbar"><b><i>U</i> Ununknown <small>0.4.0</small></b><span>{settings.data?.input_dir}</span><Button kind="quiet" onClick={()=>setSettingsPage(true)}>Settings</Button></header>
  <main className="pipeline v4">
    <Flow phase={w?.phase||"idle"}/>
    {!w||workflow.isLoading?<section className="state-card"><Spinner/> Loading workspace</section>:
    w.phase==="idle"?<section className="hero state-card"><span className="eyebrow">Local metadata repair</span><h1>Start a clean metadata run.</h1><p>Ununknown scans first, then fetches one track at a time with visible provider diagnostics.</p><Button onClick={()=>scan.mutate()}>Scan music</Button></section>:
    ["scan","fetch","apply"].includes(w.phase)?<section className="run-grid"><div className="process state-card"><div className="process-icon"><Spinner/></div><span className="eyebrow">{w.phase}</span><h1>{w.message}</h1><p className="current-file">{w.current_file||"Reading mounted music folder..."}</p><div className="counter"><strong>{w.current}</strong><span>of {w.total||"-"}</span></div><Progress current={w.current} total={w.total}/><div className="summary"><span>{w.matched} matched</span><span>{w.unmatched} unmatched</span><span>{w.failed} failed</span></div><Button kind="danger" onClick={()=>stop.mutate()}>Stop</Button></div><Terminal lines={w.terminal_log||[]} status={eventStatus}/></section>:
    w.phase==="failed"?<section className="run-grid"><div className="state-card hero"><span className="eyebrow error">Workflow error</span><h1>Processing stopped</h1><p>{w.message}</p><Button onClick={()=>scan.mutate()}>Start new scan</Button></div><Terminal lines={w.terminal_log||[]} status={eventStatus}/></section>:
    <PreviewPanel workflow={w} preview={preview} applyPending={apply.isPending} eventStatus={eventStatus} onScan={()=>scan.mutate()} onApply={()=>confirm("Apply these metadata changes? Duplicate skips will not be written.")&&apply.mutate()}/>}
    {active&&<div className="sr-only">Processing</div>}
  </main><Toast text={toast}/></>
}

function Flow({phase}:{phase:string}){const active=phaseIndex(phase);return <nav className="flowline">{steps.map((s,i)=><div className={`flow-step ${i<active?"done":i===active?"active":"wait"}`} key={s}><span>{i+1}</span><strong>{s}</strong>{i<steps.length-1&&<i/>}</div>)}</nav>}
function Progress({current,total}:{current:number;total:number}){const value=total?Math.min(100,Math.round(current/total*100)):0;return <div className="live-progress" aria-label={`Progress ${value}%`}><span style={{width:`${value}%`}}/></div>}
function Terminal({lines,status="connected"}:{lines:TerminalLine[];status?:EventStatus}){
  const body=useRef<HTMLDivElement>(null);
  useEffect(()=>{if(body.current)body.current.scrollTop=body.current.scrollHeight},[lines.length]);
  const recent=lines.slice(-120);
  return <aside className={`terminal-card ${status}`}><header><span/> Fetch terminal <small>{status} · {recent.length} lines</small></header><div ref={body}>{recent.length?recent.map((l,i)=><p className={l.level} key={`${l.timestamp}-${i}`}><time>{new Date(l.timestamp).toLocaleTimeString()}</time><b>{l.stage}</b><span>{l.file?<strong>{l.file}</strong>:null}{l.file?" · ":""}{l.message}</span></p>):<p className="muted"><time>--:--:--</time><b>idle</b><span>{status==="reconnecting"?"Reconnecting to live events...":"Waiting for scan output..."}</span></p>}</div></aside>
}
function PreviewPanel({workflow,preview,applyPending,eventStatus,onScan,onApply}:{workflow:Workflow;preview?:Preview;applyPending:boolean;eventStatus:EventStatus;onScan:()=>void;onApply:()=>void}){
  const items=preview?.items||[];
  const writeCount=preview?.summary?.write_count??items.length;
  const empty=workflow.phase!=="finish"&&!items.length;
  return <section className="results state-card preview-v4"><header><div><span className="eyebrow">{workflow.phase==="finish"?"Finished":"Preview"}</span><h1>{workflow.phase==="finish"?"Metadata apply complete":`${workflow.matched} matched tracks ready`}</h1><p>{workflow.unmatched} unmatched · {workflow.failed} failed{preview?.summary?.duplicate_skipped?` · ${preview.summary.duplicate_skipped} duplicate skipped`:""}</p></div><Button kind="quiet" onClick={()=>confirm("Clear this preview and start a new scan?")&&onScan()}>Start new scan</Button></header>{empty&&<div className="empty-preview"><div><h2>No writable matches yet</h2><p>The fetch terminal explains provider errors, missing AcoustID configuration, low confidence scores, and unmatched decisions.</p></div><Terminal lines={workflow.terminal_log||[]} status={eventStatus}/></div>}{workflow.phase!=="finish"&&!empty&&<VirtualPreview items={items}/>} {workflow.phase!=="finish"&&preview&&<div className="apply-bar"><span>Dry-run ready for {writeCount} writes{preview.summary?.duplicate_skipped?` · ${preview.summary.duplicate_skipped} duplicates skipped`:""}</span><Button disabled={!writeCount||applyPending} onClick={onApply}>{applyPending?"Applying...":"Apply changes"}</Button></div>}</section>
}
function VirtualPreview({items}:{items:PreviewItem[]}){
  const rowHeight=92,overscan=8,viewport=620;
  const [scrollTop,setScrollTop]=useState(0);
  const start=Math.max(0,Math.floor(scrollTop/rowHeight)-overscan);
  const count=Math.ceil(viewport/rowHeight)+overscan*2;
  const visible=items.slice(start,start+count);
  return <><div className="compact-preview-head"><span>Current metadata</span><span>Proposed metadata</span><span>Output</span></div><div className="virtual-preview" onScroll={e=>setScrollTop(e.currentTarget.scrollTop)}><div style={{height:items.length*rowHeight,position:"relative"}}>{visible.map((item,i)=><div className="virtual-row" style={{height:rowHeight,transform:`translateY(${(start+i)*rowHeight}px)`}} key={item.track_id}><Compare item={item}/></div>)}</div></div><div className="preview-count">{items.length} matched preview rows · rendering {visible.length} visible rows</div></>
}
function Compare({item}:{item:PreviewItem}){
  const oldData=item.old||{};
  const newData=item.new||{};
  const warnings=item.warnings||[];
  const skip=item.duplicate_action==="skip_duplicate";
  return <article className={`compare-row ${skip?"duplicate-skip":""}`}><MusicCard label="Current" data={oldData}/><div className="change-arrow"><span>{skip?"skip":"→"}</span><small>{skip?item.duplicate_reason:"changes"}</small></div><MusicCard label="Proposed" data={newData} cover={item.cover_url} confidence={item.confidence} path={item.destination_path} artwork={item.artwork_action} changedFrom={oldData}/>{warnings.length>0&&<footer>{warnings.map(w=><small key={w}>{w}</small>)}</footer>}</article>
}
function MusicCard({label,data={},cover,confidence,path,artwork,changedFrom}:{label:string;data?:MetadataSummary;cover?:string;confidence?:number;path?:string;artwork?:string;changedFrom?:MetadataSummary}){const row=(k:keyof MetadataSummary,name:string)=>{const changed=changedFrom&&String(changedFrom[k]||"")!==String(data[k]||"");return <span className={changed?"changed":""}><b>{name}</b>{String(data[k]??"—")}</span>};return <section className="music-card"><div className="cover">{cover?<img src={cover}/>:<span>{(data.title||"?").slice(0,1)}</span>}</div><div className="music-meta"><em>{label}{confidence!==undefined?` · ${Math.round(confidence)}%`:""}</em><h3>{data.title||"Unknown title"}</h3><strong>{data.artist||"Unknown artist"}</strong><div className="meta-grid">{row("album","Album")}{row("album_artist","Album artist")}{row("year","Year")}{row("track_number","Track")}{row("disc_number","Disc")}{row("genre","Genre")}{row("label","Label")}{row("isrc","ISRC")}</div>{path&&<p className="path-line"><span>Output</span><b>{path}</b></p>}{artwork&&<p className="path-line"><span>Artwork</span><b>{artwork}</b></p>}</div></section>}
