import json,sys,pathlib
base=pathlib.Path('/tmp/ichiloto-renderer-burst-20260912'); pid=7644
keys={};frames={};native=[];selected=[];buffer=''
with (base/'runtime/logs/latency.ndjson').open() as f:
 for line in f:
  try:r=json.loads(line)
  except json.JSONDecodeError:continue
  if r.get('pid')!=pid:continue
  stage=r['stage']; iid=r.get('input_id')
  if iid is not None: keys.setdefault(iid,{}) .setdefault(stage,r)
  if stage=='transport.queued' and r.get('frame') is not None:frames[r['frame']]=r
  if stage=='renderer.stderr':
   buffer+=r['text']
   while '\n' in buffer:
    text,buffer=buffer.split('\n',1)
    try:native.append(json.loads(text))
    except json.JSONDecodeError:native.append({'ordinary_error':text})
  elif iid is not None or stage=='transport.queued':selected.append(r)
anchors=[r for r in native if r.get('diagnostic')=='clock_anchor']
low=max(a['host_ns']-a['elapsed_after_ns'] for a in anchors);high=min(a['host_ns']-a['elapsed_before_ns'] for a in anchors);offset=(low+high)//2
nk={};nf={}
for r in native:
 if r.get('diagnostic')=='key':nk.setdefault(r['id'],{}).setdefault(r['stage'],r)
 if r.get('diagnostic')=='frame':nf.setdefault(r['sequence'],{}).setdefault(r['stage'],r)
written=[k for k in nk.values() if 'written' in k];parsed=[k for k in keys.values() if 'transport.key.parsed' in k];assert len(written)==len(parsed),(len(written),len(parsed))
rows=[]
for n,k in zip(written,parsed):
 p=k['transport.key.parsed']; assert p['key']==n['written']['key']
 frame=k.get('presentation.frame.queued',{}).get('frame'); matches=[f for f in nf.values() if f['received']['frame']==frame];assert len(matches)<=1
 f=matches[0] if matches else {};ts={s:offset+v['at_ns'] for s,v in f.items()}; native_ns=offset+n['native']['at_ns']
 d=lambda a,b:(b-a)/1e6 if a is not None and b is not None else None
 rows.append({'native_id':n['native']['id'],'input_id':p['input_id'],'key':p['key'],'frame':frame,'native_ns':native_ns,'before':k.get('game.update.begin',{}).get('player'),'after':k.get('game.update.end',{}).get('player'),'event_owns_input':k.get('game.update.end',{}).get('event_owns_input'),'state':k.get('game.update.end',{}).get('scene_state'),'times_host_ns':ts,'native_to_received_ms':d(native_ns,ts.get('received')),'received_to_accepted_ms':d(ts.get('received'),ts.get('accepted')),'accepted_to_replaced_ms':d(ts.get('accepted'),ts.get('replaced')),'replaced_to_callback_ms':d(ts.get('replaced'),ts.get('render_callback')),'native_to_callback_ms':d(native_ns,ts.get('render_callback'))})
summary={'pid':pid,'anchors':anchors,'offset_bounds':[low,high],'incomplete_buffer':len(buffer),'keys':len(rows),'frames':len(nf),'rows':rows,'frame_stages':nf,'errors':[r for r in native if 'ordinary_error'in r],'closing_summary':[r for r in native if r.get('diagnostic')=='summary']}
(base/'evidence/live-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
(base/'evidence/renderer.ndjson').write_text(''.join(json.dumps(r)+'\n' for r in native))
(base/'evidence/engine-selected.ndjson').write_text(''.join(json.dumps(r)+'\n' for r in selected))
print(json.dumps({'keys':len(rows),'frames':len(nf),'offset_bounds':[low,high],'last_rows':rows[-4:]},indent=2))
