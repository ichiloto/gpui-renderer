import json,pathlib,collections
base=pathlib.Path('/tmp/ichiloto-renderer-burst-release-20260912/evidence')
events=sorted([json.loads(x) for x in (base/'renderer.ndjson').read_text().splitlines() if x],key=lambda r:r.get('at_ns',0))
ends={'elements_built':'render_callback','layout_request_end':'layout_request_begin','prepaint_end':'prepaint_begin','paint_end':'paint_begin','prepaint_begin':'layout_request_end','submit_end':'submit_begin','submit_failed':'submit_begin'}
active=collections.defaultdict(collections.deque);spans=[]
for e in events:
 if e.get('diagnostic')!='frame':continue
 seq=e['sequence'];stage=e['stage'];key=(seq,stage)
 if stage in ends:
  begin_key=(seq,ends[stage])
  if active[begin_key]:
   b=active[begin_key].popleft();spans.append({'sequence':seq,'frame':e['frame'],'stage':ends[stage]+'→'+stage,'begin_ns':b['at_ns'],'end_ns':e['at_ns'],'duration_ms':(e['at_ns']-b['at_ns'])/1e6})
 if stage in set(ends.values()):active[key].append(e)
summary=json.loads((base/'live-summary.json').read_text());rows=[]
for row in summary['rows']:
 if row['native_id']<6 or row['frame'] is None:continue
 seq=row['frame'];st=summary['frame_stages'][str(seq)];a=st['accepted']['at_ns'];b=st['dequeued']['at_ns']
 overlapping=[]
 for s in spans:
  if s['stage'].startswith('submit'):continue
  overlap=max(0,min(b,s['end_ns'])-max(a,s['begin_ns']))/1e6
  if overlap:overlapping.append(dict(s,overlap_ms=overlap))
 rows.append({'native_id':row['native_id'],'frame':seq,'accepted_to_dequeued_ms':(b-a)/1e6,'submission_ms':next(s['duration_ms'] for s in spans if s['sequence']==seq and s['stage']=='submit_begin→submit_end'),'overlap':overlapping})
result={'spans':spans,'rows':rows,'unpaired':[{ 'sequence':k[0],'stage':k[1],'count':len(v)} for k,v in active.items() if v and k[1]!='layout_request_end']}
(base/'span-correlation.json').write_text(json.dumps(result,indent=2)+'\n')
for row in rows[-4:]:
 print('key',row['native_id'],'frame',row['frame'],'accepted→dequeue',round(row['accepted_to_dequeued_ms'],3),'submit',round(row['submission_ms'],3),'covered',round(sum(s['overlap_ms'] for s in row['overlap']),3))
 for s in row['overlap']:print(' previous/current',s['sequence'],s['stage'],round(s['overlap_ms'],3))
print('unpaired',result['unpaired'])
