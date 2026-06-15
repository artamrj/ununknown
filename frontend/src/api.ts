export type Candidate={id:number;title?:string;artist?:string;album?:string;album_artist?:string;track_number?:number;track_total?:number;disc_number?:number;disc_total?:number;year?:string;genre?:string;composer?:string;label?:string;isrc?:string;score:number};
export type Track={id:number;filename:string;format?:string;current_title?:string;current_artist?:string;current_album?:string;selected_candidate_id?:number;status:string;stage:string;stage_message?:string;error?:string;candidates:Candidate[]};
export type TrackPage={items:Track[];total:number;counts:Record<string,number>};
export type PreviewItem={track_id:number;current_path:string;destination_path:string;action:string;warnings:string[];duplicate_group_id?:string;duplicate_action:string;duplicate_reason?:string;kept_track_id?:number};
export type Preview={preview_token:string;items:PreviewItem[];summary?:{write_count:number;duplicate_skipped:number}};
export type Workflow={phase:"idle"|"scan"|"fetch"|"preview"|"apply"|"finish"|"failed";message:string;current_file?:string;current:number;total:number;processed:number;matched:number;unmatched:number;failed:number};
export const api=async<T>(path:string,init?:RequestInit):Promise<T>=>{const r=await fetch(`/api${path}`,{headers:{"Content-Type":"application/json"},...init});const b=await r.json();if(!r.ok)throw new Error(b.error||r.statusText);return b};
