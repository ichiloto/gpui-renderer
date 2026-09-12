import ctypes, json, pathlib, subprocess, sys, time
base=pathlib.Path('/tmp/ichiloto-renderer-burst-20260912/evidence')
name=sys.argv[1]; pid=int(sys.argv[2]); duration=sys.argv[3] if len(sys.argv)>3 else '3'
clock=ctypes.CDLL(None).clock_gettime_nsec_np
clock.argtypes=[ctypes.c_int]; clock.restype=ctypes.c_uint64
# CLOCK_UPTIME_RAW, verified from the active Darwin SDK.
def mark(stage):
    with (base/(name+'-sample-markers.ndjson')).open('a') as f:
        f.write(json.dumps({'stage':stage,'host_ns':clock(8),'wall_ns':time.time_ns(),'pid':pid})+'\n')
mark('armed')
deadline=time.monotonic()+55
while not (base/(name+'-request')).exists():
    if time.monotonic()>deadline: raise SystemExit('No burst request within 55 seconds')
    time.sleep(.005)
mark('sample_command_begin')
cmd=['/usr/bin/sample',str(pid),duration,'1','-file',str(base/(name+'-sample.txt'))]
with (base/(name+'-sample-console.txt')).open('w') as f:
    proc=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    ready=False
    for line in proc.stdout:
        f.write(line);f.flush()
        if 'Sampling completed' in line: mark('sample_collection_completed_banner')
        if 'Sampling process' in line and not ready:
            mark('sample_collecting_banner'); (base/(name+'-ready')).write_text('ready\n');ready=True
    result=proc.wait()
mark('sample_command_end')
print(json.dumps({'command':cmd,'returncode':result,'ready':ready}))
